package com.urdevice.app.host

import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.os.Build
import android.util.Log
import android.widget.Toast
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.core.FfiClipSubmit
import java.io.ByteArrayOutputStream

/**
 * Reads the system clipboard and hands it to the core (R7.1, R7.3, R7.5).
 *
 * Android 10+ lets an app read the clipboard only while it has focus (or is the keyboard), so
 * every path here runs while an activity of ours is focused: the notification action and the
 * Quick Settings tile open [com.urdevice.app.ClipboardSendActivity], the share sheet opens
 * [com.urdevice.app.ClipboardShareActivity], and the clipboard screen is in the app itself.
 * Never logs content.
 */
object ClipboardSender {
    /** Largest item the protocol carries (R7.2). */
    private const val MAX_BYTES = 10 * 1024 * 1024

    /** The platform's own mark for content that must not be shared (R7.3). */
    private const val EXTRA_IS_SENSITIVE = "android.content.extra.IS_SENSITIVE"

    fun isSensitive(desc: ClipDescription?): Boolean {
        val extras = desc?.extras ?: return false
        return extras.getBoolean(EXTRA_IS_SENSITIVE, false) ||
            (Build.VERSION.SDK_INT >= 33 && extras.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE, false))
    }

    /**
     * Send what is on the clipboard now to [device] (hex), or to every connected device. Call
     * only while one of our windows has focus. [automatic] marks a change noticed without a user
     * action, so content just received is not echoed.
     */
    fun sendCurrent(context: Context, device: String? = null, automatic: Boolean = false): FfiClipSubmit? {
        val node = Core.node() ?: return null
        val cm = context.getSystemService(ClipboardManager::class.java)
        val clip = try {
            cm.primaryClip
        } catch (e: SecurityException) {
            return null
        } ?: return null
        if (clip.itemCount == 0) return null
        val sensitive = isSensitive(clip.description)
        val item = clip.getItemAt(0)
        val isImage = (0 until clip.description.mimeTypeCount).any { clip.description.getMimeType(it).startsWith("image/") }
        return try {
            val uri = item.uri
            if (isImage && uri != null) {
                val png = readPng(context, uri) ?: return FfiClipSubmit.Skipped("too_large")
                node.clipboardSendPng(png, sensitive, device, automatic)
            } else {
                val text = item.text?.toString() ?: item.coerceToText(context)?.toString() ?: ""
                if (text.isEmpty()) return null
                node.clipboardSendText(text, item.htmlText, sensitive, device, automatic)
            }
        } catch (e: Exception) {
            Log.w("UrDevice", "clipboard send failed: ${e.javaClass.simpleName}")
            null
        }
    }

    /** Send an image the share sheet handed over. */
    fun sendImage(context: Context, uri: Uri): FfiClipSubmit? {
        val node = Core.node() ?: return null
        val png = readPng(context, uri) ?: return FfiClipSubmit.Skipped("too_large")
        return node.clipboardSendPng(png, false, null, false)
    }

    /** Send text the share sheet handed over. */
    fun sendText(text: String, html: String?): FfiClipSubmit? {
        val node = Core.node() ?: return null
        return node.clipboardSendText(text, html, false, null, false)
    }

    /** The image at [uri] as PNG bytes, or null when it is over the limit or unreadable. */
    private fun readPng(context: Context, uri: Uri): ByteArray? {
        val type = context.contentResolver.getType(uri)
        val raw = context.contentResolver.openInputStream(uri)?.use { it.readNBytes(MAX_BYTES + 1) } ?: return null
        if (type == "image/png" || raw.startsWithPng()) return raw.takeIf { it.size <= MAX_BYTES }
        // Photos and screenshots in other formats become PNG.
        val bmp = BitmapFactory.decodeByteArray(raw, 0, raw.size) ?: return null
        val out = ByteArrayOutputStream()
        bmp.compress(Bitmap.CompressFormat.PNG, 100, out)
        return out.toByteArray().takeIf { it.size <= MAX_BYTES }
    }

    private fun ByteArray.startsWithPng() =
        size > 8 && this[0] == 0x89.toByte() && this[1] == 'P'.code.toByte() && this[2] == 'N'.code.toByte() && this[3] == 'G'.code.toByte()

    /** One short message for the user: what happened to what they asked to send. */
    fun message(context: Context, r: FfiClipSubmit?): String = when (r) {
        null -> context.getString(R.string.clip_nothing)
        is FfiClipSubmit.Sending -> context.getString(R.string.clip_sending, r.count.toInt())
        is FfiClipSubmit.Echo -> context.getString(R.string.clip_echo)
        is FfiClipSubmit.Skipped -> context.getString(
            when (r.reason) {
                "paused" -> R.string.clip_paused
                "sensitive" -> R.string.clip_sensitive
                "too_large" -> R.string.clip_too_large
                "no_device" -> R.string.clip_no_device
                else -> R.string.clip_unsupported
            },
        )
    }

    fun toast(context: Context, r: FfiClipSubmit?) =
        Toast.makeText(context, message(context, r), Toast.LENGTH_SHORT).show()
}
