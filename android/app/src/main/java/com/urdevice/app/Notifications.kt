package com.urdevice.app

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import com.urdevice.core.FfiEvent
import com.urdevice.core.FfiSnapshot

/**
 * The ongoing service notification, with Stop while another device acts on
 * this phone (R5.4), and Always Ask prompts with Allow and Deny (R5.3).
 */
object Notifications {
    const val SERVICE_ID = 1
    private const val CH_SERVICE = "service"
    private const val CH_PROMPTS = "prompts"

    fun createChannels(ctx: Context) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CH_SERVICE, ctx.getString(R.string.channel_service), NotificationManager.IMPORTANCE_LOW),
        )
        nm.createNotificationChannel(
            NotificationChannel(CH_PROMPTS, ctx.getString(R.string.channel_prompts), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    private fun canPost(ctx: Context) = Build.VERSION.SDK_INT < 33 ||
        ctx.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun openApp(ctx: Context): PendingIntent = PendingIntent.getActivity(
        ctx, 0,
        Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE,
    )

    private fun action(ctx: Context, code: Int, action: String, id: Long): PendingIntent = PendingIntent.getBroadcast(
        ctx, code,
        Intent(ctx, ActionReceiver::class.java).setAction(action).putExtra(ActionReceiver.EXTRA_ID, id),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    fun service(ctx: Context, snap: FfiSnapshot?): Notification {
        val b = Notification.Builder(ctx, CH_SERVICE)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setOngoing(true)
            .setContentIntent(openApp(ctx))
        val active = snap?.activities?.firstOrNull { it.incoming }
        if (active != null) {
            b.setContentTitle(ctx.getString(R.string.notif_active, active.deviceName, Labels.capName(ctx, active.capability)))
            b.addAction(
                Notification.Action.Builder(
                    null, ctx.getString(R.string.stop),
                    action(ctx, 7, ActionReceiver.STOP, active.id.toLong()),
                ).build(),
            )
        } else {
            val n = snap?.peers?.count { it.status == "connected" } ?: 0
            b.setContentTitle(
                if (n > 0) ctx.getString(R.string.notif_running, n) else ctx.getString(R.string.notif_waiting),
            )
        }
        // Send the clipboard now (R7.5). Android only lets a focused app read it, so this opens
        // a transparent activity (ClipboardSendActivity).
        b.addAction(
            Notification.Action.Builder(null, ctx.getString(R.string.clip_send_action), ClipboardSendActivity.pending(ctx)).build(),
        )
        return b.build()
    }

    fun updateService(ctx: Context, snap: FfiSnapshot?) {
        if (!canPost(ctx)) return
        ctx.getSystemService(NotificationManager::class.java).notify(SERVICE_ID, service(ctx, snap))
    }

    private fun promptId(id: ULong) = 1000 + (id % 100_000UL).toInt()

    fun showPrompt(ctx: Context, p: FfiEvent.Prompt) {
        if (!canPost(ctx)) return
        val feature = Labels.capName(ctx, p.capability)
        val id = p.id.toLong()
        val n = Notification.Builder(ctx, CH_PROMPTS)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(ctx.getString(R.string.prompt_title, p.deviceName, feature))
            .setContentText(ctx.getString(R.string.prompt_body, p.timeoutSecs.toInt()))
            .setCategory(Notification.CATEGORY_CALL)
            .setTimeoutAfter(p.timeoutSecs.toLong() * 1000)
            .setAutoCancel(true)
            .setContentIntent(openApp(ctx))
            .addAction(Notification.Action.Builder(null, ctx.getString(R.string.deny), action(ctx, promptId(p.id) * 2, ActionReceiver.DENY, id)).build())
            .addAction(Notification.Action.Builder(null, ctx.getString(R.string.allow), action(ctx, promptId(p.id) * 2 + 1, ActionReceiver.ALLOW, id)).build())
            .build()
        ctx.getSystemService(NotificationManager::class.java).notify(promptId(p.id), n)
    }

    fun cancelPrompt(ctx: Context, id: ULong) {
        ctx.getSystemService(NotificationManager::class.java).cancel(promptId(id))
    }
}
