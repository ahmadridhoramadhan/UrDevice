package com.urdevice.app

import android.app.Activity
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import com.urdevice.app.host.ClipboardSender

/**
 * Sends the current clipboard to the paired devices (R7.5). Android lets only a focused app read
 * the clipboard, so the notification action and the Quick Settings tile open this invisible
 * activity, which reads once it has focus and closes.
 */
class ClipboardSendActivity : Activity() {
    private var done = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        UrDeviceService.start(this)
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (!hasFocus || done) return
        done = true
        ClipboardSender.toast(this, ClipboardSender.sendCurrent(this))
        finish()
    }

    companion object {
        fun intent(context: Context) = Intent(context, ClipboardSendActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION or Intent.FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS)

        /** The "Send clipboard" button on the service notification. */
        fun pending(context: Context): PendingIntent =
            PendingIntent.getActivity(context, 21, intent(context), PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
    }
}

/** The share sheet target: "UrDevice clipboard" sends shared text or an image (R7.5). */
class ClipboardShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        UrDeviceService.start(this)
        val result = when {
            intent?.action != Intent.ACTION_SEND -> null
            intent.type?.startsWith("image/") == true -> {
                val uri: Uri? = if (Build.VERSION.SDK_INT >= 33) {
                    intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    @Suppress("DEPRECATION") intent.getParcelableExtra(Intent.EXTRA_STREAM)
                }
                uri?.let { ClipboardSender.sendImage(this, it) }
            }
            else -> {
                val text = intent.getStringExtra(Intent.EXTRA_TEXT)
                if (text.isNullOrEmpty()) null else ClipboardSender.sendText(text, intent.getStringExtra(Intent.EXTRA_HTML_TEXT))
            }
        }
        ClipboardSender.toast(this, result)
        finish()
    }
}

/** The Quick Settings tile "Send clipboard" (R7.5). */
class ClipboardTileService : TileService() {
    override fun onStartListening() {
        qsTile?.apply {
            state = Tile.STATE_INACTIVE
            updateTile()
        }
    }

    override fun onClick() {
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(ClipboardSendActivity.pending(this))
        } else {
            @Suppress("DEPRECATION") startActivityAndCollapse(ClipboardSendActivity.intent(this))
        }
    }
}
