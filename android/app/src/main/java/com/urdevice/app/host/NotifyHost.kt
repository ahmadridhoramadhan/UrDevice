package com.urdevice.app.host

import android.app.Notification
import android.content.Context
import android.content.pm.ApplicationInfo
import android.os.Build
import android.service.notification.StatusBarNotification
import com.urdevice.app.R
import com.urdevice.core.FfiNotifApp
import com.urdevice.core.FfiNotifItem
import com.urdevice.core.FfiNotifState
import com.urdevice.core.NotifyHostFfi

/**
 * This phone's notifications, for desktops that turned mirroring on (R18.1, design §18.2).
 *
 * They come from [MediaListenerService], so they need the same notification access as media. The core
 * calls [state] from a worker thread and only filters it by the apps the desktop picked; secret and
 * private content is hidden here, on the phone, before anything is handed over (R18.3):
 *  - `VISIBILITY_SECRET`: the notification is left out.
 *  - `VISIBILITY_PRIVATE`: the full content, as on by default. When the user turns [FULL_CONTENT] off,
 *    the app's own public version if it made one, otherwise only "New notification from <app>".
 * Ongoing notifications (media players, foreground services) and group summaries are left out.
 */
class NotifyHost(private val context: Context) : NotifyHostFfi {
    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    override fun state(): FfiNotifState {
        val list = try {
            MediaListenerService.instance?.activeNotifications
        } catch (_: SecurityException) {
            null
        }
        if (list == null) {
            val off = !MediaListenerService.isEnabled(context)
            return FfiNotifState(emptyList(), emptyList(), if (off) "listener_permission" else "")
        }
        val full = prefs.getBoolean(FULL_CONTENT, true)
        val items = ArrayList<FfiNotifItem>()
        val apps = LinkedHashMap<String, String>()
        for (sbn in list) {
            val item = item(sbn, full) ?: continue
            items.add(item)
            apps[item.appId] = item.appName
        }
        items.sortByDescending { it.postedMs }
        return FfiNotifState(items, apps.map { (id, name) -> FfiNotifApp(id, name) }, "")
    }

    private fun item(sbn: StatusBarNotification, full: Boolean): FfiNotifItem? {
        val n = sbn.notification ?: return null
        if (sbn.packageName == context.packageName) return null
        val skip = Notification.FLAG_GROUP_SUMMARY or Notification.FLAG_ONGOING_EVENT or Notification.FLAG_FOREGROUND_SERVICE
        if (n.flags and skip != 0) return null
        if (n.visibility == Notification.VISIBILITY_SECRET) return null
        val app = appName(n, sbn.packageName)
        var title: String
        var text: String
        var hidden = false
        if (n.visibility == Notification.VISIBILITY_PRIVATE && !full) {
            val public = n.publicVersion
            if (public != null) {
                title = clean(public.extras.getCharSequence(Notification.EXTRA_TITLE))
                text = clean(public.extras.getCharSequence(Notification.EXTRA_TEXT))
            } else {
                title = context.getString(R.string.notify_private_title, app)
                text = ""
                hidden = true
            }
        } else {
            val x = n.extras
            title = clean(x.getCharSequence(Notification.EXTRA_TITLE))
            text = clean(x.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: x.getCharSequence(Notification.EXTRA_TEXT))
        }
        if (title.isEmpty() && text.isEmpty()) return null
        return FfiNotifItem(
            key = sbn.key,
            appId = sbn.packageName,
            appName = app,
            title = title,
            text = text,
            postedMs = sbn.postTime.coerceAtLeast(0).toULong(),
            hidden = hidden,
        )
    }

    /** The app's label. The notification carries its application info, so no package visibility is needed. */
    private fun appName(n: Notification, pkg: String): String {
        val info = if (Build.VERSION.SDK_INT >= 33) {
            n.extras.getParcelable(EXTRA_APP_INFO, ApplicationInfo::class.java)
        } else {
            @Suppress("DEPRECATION")
            n.extras.getParcelable<ApplicationInfo>(EXTRA_APP_INFO)
        }
        return info?.loadLabel(context.packageManager)?.toString()?.takeIf { it.isNotBlank() } ?: pkg
    }

    private fun clean(s: CharSequence?): String = s?.toString()?.trim() ?: ""

    companion object {
        const val PREFS = "urdevice-notify"

        /** The user allows the full content of private notifications (R18.3). On by default. */
        const val FULL_CONTENT = "full_content"

        /** `Notification.EXTRA_BUILDER_APPLICATION_INFO`, which is hidden from the SDK. */
        private const val EXTRA_APP_INFO = "android.appInfo"
    }
}
