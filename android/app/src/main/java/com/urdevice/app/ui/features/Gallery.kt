package com.urdevice.app.ui.features

import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import com.urdevice.app.R
import com.urdevice.app.host.GalleryHost
import com.urdevice.core.FfiGalleryPage
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiOpened
import com.urdevice.core.GalleryHostFfi
import com.urdevice.core.UrDeviceNode

/**
 * Android gallery (#17, R8.7). The phone is the host: paired desktops browse and download its
 * photos and videos ([GalleryHost]). When a desktop asks before the user gave UrDevice access to
 * photos and videos, a notification asks for it (R15.2: asked when first needed).
 */
object GalleryFeature : Feature {
    override val id = "gallery"

    // Shown only when the host lists the capability.
    override fun tiles(modes: Map<String, FfiMode>) = listed(modes, FeatureTile("gallery.v1", "gallery.v1"))

    private const val CHANNEL = "gallery"
    private const val NOTIF = 49_999

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        val app = context.applicationContext
        app.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, app.getString(R.string.gallery_channel), NotificationManager.IMPORTANCE_DEFAULT),
        )
        val host = GalleryHost(app)
        node.gallerySetHost(object : GalleryHostFfi {
            override fun page(cursor: ULong, limit: UInt): FfiGalleryPage {
                val p = host.page(cursor, limit)
                if (p.noPermission) askForAccess(app)
                return p
            }
            override fun open(id: ULong): FfiOpened = host.open(id)
        })
    }

    private fun askForAccess(ctx: Context) {
        val tap = PendingIntent.getActivity(
            ctx, 0, Intent(ctx, GalleryAccessActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK), PendingIntent.FLAG_IMMUTABLE,
        )
        val n = android.app.Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(ctx.getString(R.string.gallery_access_title))
            .setContentText(ctx.getString(R.string.gallery_access_body))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .build()
        runCatching { ctx.getSystemService(NotificationManager::class.java).notify(NOTIF, n) }
    }
}

/** Asks for the photo and video permission, then closes. */
class GalleryAccessActivity : ComponentActivity() {
    private val ask = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) { finish() }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (GalleryHost.hasMediaPermission(this)) finish() else ask.launch(GalleryHost.mediaPermissions())
    }
}
