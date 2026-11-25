package com.urdevice.app.host

import android.Manifest
import android.content.ContentResolver
import android.content.ContentUris
import android.content.Context
import android.database.Cursor
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.os.Build
import android.os.Bundle
import android.provider.MediaStore
import android.util.Size
import com.urdevice.core.FfiGalleryItem
import com.urdevice.core.FfiGalleryPage
import com.urdevice.core.FfiOpened
import com.urdevice.core.GalleryHostFfi
import java.io.ByteArrayOutputStream

/**
 * The phone's photos and videos for paired desktops (#17, R8.7, design §9.2): MediaStore pages,
 * newest first, with 256-px JPEG thumbnails. The core checks the Gallery permission before any of
 * this is called.
 */
class GalleryHost(context: Context) : GalleryHostFfi {
    private val app = context.applicationContext
    private val collection = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
    private val selection = "${MediaStore.Files.FileColumns.MEDIA_TYPE} IN (" +
        "${MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE}, ${MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO})"

    override fun page(cursor: ULong, limit: UInt): FfiGalleryPage {
        if (!hasMediaPermission(app)) return FfiGalleryPage(emptyList(), false, true)
        val args = Bundle().apply {
            putString(ContentResolver.QUERY_ARG_SQL_SELECTION, selection)
            // One more than asked, to know whether there is a next page.
            putInt(ContentResolver.QUERY_ARG_LIMIT, limit.toInt() + 1)
            putInt(ContentResolver.QUERY_ARG_OFFSET, cursor.toInt())
        }
        val cols = arrayOf(
            MediaStore.MediaColumns._ID,
            MediaStore.MediaColumns.DISPLAY_NAME,
            MediaStore.MediaColumns.MIME_TYPE,
            MediaStore.MediaColumns.SIZE,
            MediaStore.MediaColumns.DATE_TAKEN,
            MediaStore.MediaColumns.DATE_ADDED,
            MediaStore.MediaColumns.DURATION,
            MediaStore.Files.FileColumns.MEDIA_TYPE,
        )
        val items = ArrayList<FfiGalleryItem>()
        var more = false
        newestFirst(cols, args)?.use { c ->
            while (c.moveToNext()) {
                if (items.size >= limit.toInt()) {
                    more = true
                    break
                }
                val id = c.getLong(0)
                val video = c.getInt(7) == MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO
                val taken = if (c.isNull(4)) c.getLong(5) * 1000 else c.getLong(4)
                items += FfiGalleryItem(
                    id = id.toULong(),
                    name = c.getString(1) ?: "item_$id",
                    mime = c.getString(2) ?: if (video) "video/mp4" else "image/jpeg",
                    size = c.getLong(3).coerceAtLeast(0).toULong(),
                    takenMs = taken,
                    durationMs = if (c.isNull(6)) 0UL else c.getLong(6).coerceAtLeast(0).toULong(),
                    thumb = thumb(id, video),
                )
            }
        }
        return FfiGalleryPage(items, more, false)
    }

    /**
     * Newest first by the day the picture was taken (the day it was added when the phone does not
     * say). Sorting by the day it was added put restored or synced old pictures on top. Some Android
     * versions refuse a sort expression; then the plain taken-date columns are used.
     */
    private fun newestFirst(cols: Array<String>, args: Bundle): Cursor? {
        val taken = MediaStore.MediaColumns.DATE_TAKEN
        val added = MediaStore.MediaColumns.DATE_ADDED
        val expression = Bundle(args).apply {
            putString(ContentResolver.QUERY_ARG_SQL_SORT_ORDER, "COALESCE($taken, $added * 1000) DESC, ${MediaStore.MediaColumns._ID} DESC")
        }
        runCatching { app.contentResolver.query(collection, cols, expression, null) }.getOrNull()?.let { return it }
        val columns = Bundle(args).apply {
            putStringArray(ContentResolver.QUERY_ARG_SORT_COLUMNS, arrayOf(taken, added, MediaStore.MediaColumns._ID))
            putInt(ContentResolver.QUERY_ARG_SORT_DIRECTION, ContentResolver.QUERY_SORT_DIRECTION_DESCENDING)
        }
        return app.contentResolver.query(collection, cols, columns, null)
    }

    private fun uri(id: Long, video: Boolean) = ContentUris.withAppendedId(
        if (video) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI,
        id,
    )

    private fun thumb(id: Long, video: Boolean): ByteArray = runCatching {
        val bmp = app.contentResolver.loadThumbnail(uri(id, video), Size(256, 256), null)
        val out = ByteArrayOutputStream()
        bmp.compress(Bitmap.CompressFormat.JPEG, 70, out)
        bmp.recycle()
        out.toByteArray()
    }.getOrDefault(ByteArray(0))

    override fun open(id: ULong): FfiOpened = runCatching {
        val fileUri = ContentUris.withAppendedId(collection, id.toLong())
        val type = app.contentResolver.query(fileUri, arrayOf(MediaStore.Files.FileColumns.MEDIA_TYPE), null, null, null)
            ?.use { if (it.moveToFirst()) it.getInt(0) else null } ?: return FfiOpened(-1, 0UL)
        val pfd = app.contentResolver.openFileDescriptor(uri(id.toLong(), type == MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO), "r")!!
        val size = pfd.statSize.coerceAtLeast(0).toULong()
        FfiOpened(pfd.detachFd(), size)
    }.getOrDefault(FfiOpened(-1, 0UL))

    companion object {
        /** The runtime permissions for reading photos and videos on this Android version. */
        fun mediaPermissions(): Array<String> = if (Build.VERSION.SDK_INT >= 33) {
            arrayOf(Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VIDEO)
        } else {
            arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)
        }

        fun hasMediaPermission(ctx: Context): Boolean {
            val granted = mediaPermissions().any { ctx.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }
            // Android 14+: "Allow limited access" grants only the selected items.
            val partial = Build.VERSION.SDK_INT >= 34 &&
                ctx.checkSelfPermission(Manifest.permission.READ_MEDIA_VISUAL_USER_SELECTED) == PackageManager.PERMISSION_GRANTED
            return granted || partial
        }
    }
}
