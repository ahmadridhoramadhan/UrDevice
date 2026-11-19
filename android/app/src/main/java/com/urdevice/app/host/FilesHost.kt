package com.urdevice.app.host

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
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

/** Opens the picked URIs for the core, which owns the descriptor it gets. */
class UriSource(context: Context, private val uris: List<Uri>) : FileSourceFfi {
    private val app = context.applicationContext

    override fun open(index: UInt): Int = runCatching {
        app.contentResolver.openFileDescriptor(uris[index.toInt()], "r")!!.detachFd()
    }.getOrDefault(-1)
}
