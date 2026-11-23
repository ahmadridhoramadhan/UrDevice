package com.urdevice.app.host

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.MimeTypeMap
import com.urdevice.core.FileSourceFfi
import com.urdevice.core.FilesHostFfi
import java.io.File

/**
 * Receiving files (#17, R8.6, R15.3). Files arrive in app-private storage, where the core cleans
 * the names, writes `.urdevice-part` files, and checks SHA-256. A verified file is then published to
 * `Download/UrDevice/<folder>` through MediaStore, which needs no storage permission and never
 * replaces an existing file (it picks `name (1).ext`).
 */
class FilesHost(context: Context) : FilesHostFfi {
    private val app = context.applicationContext

    override fun incomingDir(): String = File(app.filesDir, "incoming").apply { mkdirs() }.absolutePath

    override fun saved(path: String, relDir: String): String? {
        val file = File(path)
        val name = file.name
        val ext = name.substringAfterLast('.', "").lowercase()
        val mime = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
        // `relDir` is the core's cleaned folder: plain names joined by '/'.
        val rel = listOf(Environment.DIRECTORY_DOWNLOADS, "UrDevice") + relDir.split('/').filter { it.isNotEmpty() }
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime)
            put(MediaStore.MediaColumns.RELATIVE_PATH, rel.joinToString("/") + "/")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val resolver = app.contentResolver
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: return null
        return try {
            resolver.openOutputStream(uri)?.use { out -> file.inputStream().use { it.copyTo(out, 1 shl 20) } }
                ?: error("no stream")
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            file.delete()
            val shown = resolver.query(uri, arrayOf(MediaStore.MediaColumns.DISPLAY_NAME), null, null, null)?.use {
                if (it.moveToFirst()) it.getString(0) else null
            } ?: name
            rel.joinToString("/") + "/" + shown
        } catch (e: Exception) {
            Log.w("UrDevice", "publish to Downloads failed: ${e.javaClass.simpleName}")
            resolver.delete(uri, null, null)
            null
        }
    }
}

/** A file picked in the share sheet or the picker. */
data class Picked(val uri: Uri, val name: String, val size: Long)

/** Name and size of each content URI; ones that cannot be read are left out. */
fun describe(context: Context, uris: List<Uri>): List<Picked> = uris.mapNotNull { uri ->
    runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
            ?.use { c ->
                if (!c.moveToFirst()) return@use null
                val name = c.getString(0)?.takeIf { it.isNotBlank() } ?: "file"
                val size = if (c.isNull(1)) -1L else c.getLong(1)
                val real = if (size >= 0) size else context.contentResolver.openAssetFileDescriptor(uri, "r")?.use { it.length } ?: -1L
                if (real < 0) null else Picked(uri, name, real)
            }
    }.getOrNull()
}

/** Most files taken from one folder, so a huge folder cannot stall the app. */
private const val MAX_TREE_FILES = 10_000

/**
 * Every file under the folder [tree] (from the folder picker), named `Folder/sub/file` so the
 * other device rebuilds the structure. Files whose size is unknown are left out.
 */
fun describeTree(context: Context, tree: Uri): List<Picked> {
    val resolver = context.contentResolver
    val out = ArrayList<Picked>()
    val rootId = runCatching { DocumentsContract.getTreeDocumentId(tree) }.getOrNull() ?: return out
    val rootName = runCatching {
        resolver.query(
            DocumentsContract.buildDocumentUriUsingTree(tree, rootId),
            arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME), null, null, null,
        )?.use { if (it.moveToFirst()) it.getString(0) else null }
    }.getOrNull()?.takeIf { it.isNotBlank() } ?: "folder"

    fun walk(docId: String, prefix: String) {
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, docId)
        resolver.query(
            children,
            arrayOf(
                DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                DocumentsContract.Document.COLUMN_MIME_TYPE,
                DocumentsContract.Document.COLUMN_SIZE,
            ),
            null, null, null,
        )?.use { c ->
            while (c.moveToNext() && out.size < MAX_TREE_FILES) {
                val id = c.getString(0) ?: continue
                val name = c.getString(1)?.replace('/', '_')?.takeIf { it.isNotBlank() } ?: continue
                val size = if (c.isNull(3)) -1L else c.getLong(3)
                if (c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    walk(id, "$prefix$name/")
                } else if (size >= 0) {
                    out += Picked(DocumentsContract.buildDocumentUriUsingTree(tree, id), "$prefix$name", size)
                }
            }
        }
    }
    runCatching { walk(rootId, "${rootName.replace('/', '_')}/") }
    return out
}

/** Opens the picked URIs for the core, which owns the descriptor it gets. */
class UriSource(context: Context, private val uris: List<Uri>) : FileSourceFfi {
    private val app = context.applicationContext

    override fun open(index: UInt): Int = runCatching {
        app.contentResolver.openFileDescriptor(uris[index.toInt()], "r")!!.detachFd()
    }.getOrDefault(-1)
}
