package com.urdevice.app.host

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.util.Log
import androidx.core.content.FileProvider
import com.urdevice.core.ClipboardHostFfi
import java.io.File

/** A provider of our own, so another feature can declare its own without a manifest clash. */
class ClipboardFileProvider : FileProvider()

/**
 * Writes items from other devices to the system clipboard (R7.5). Android lets a foreground
 * service write the clipboard; reading is what it blocks in the background (see [ClipboardSender]).
 * Never logs content.
 */
class ClipboardHost(private val context: Context) : ClipboardHostFfi {
    override fun write(mime: String, bytes: ByteArray, altText: String?): Boolean = try {
        val cm = context.getSystemService(ClipboardManager::class.java)
        val clip = when (mime) {
            "text/html" -> ClipData.newHtmlText("UrDevice", altText ?: "", String(bytes, Charsets.UTF_8))
            "image/png" -> {
                val dir = File(context.cacheDir, "clipboard").apply { mkdirs() }
                // One file at a time; the previous image is replaced.
                dir.listFiles()?.forEach { it.delete() }
                val file = File(dir, "clip-${System.nanoTime()}.png").apply { writeBytes(bytes) }
                val uri = FileProvider.getUriForFile(context, AUTHORITY, file)
                ClipData(ClipDescription("UrDevice", arrayOf("image/png")), ClipData.Item(uri))
            }
            else -> ClipData.newPlainText("UrDevice", String(bytes, Charsets.UTF_8))
        }
        cm.setPrimaryClip(clip)
        true
    } catch (e: Exception) {
        Log.w("UrDevice", "clipboard write failed: ${e.javaClass.simpleName}")
        false
    }

    companion object {
        const val AUTHORITY = "com.urdevice.app.clipboard"
    }
}
