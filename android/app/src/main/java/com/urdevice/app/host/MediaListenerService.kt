package com.urdevice.app.host

import android.content.Context
import android.service.notification.NotificationListenerService
import androidx.core.app.NotificationManagerCompat

/**
 * The notification listener that lets the app read the phone's active media sessions
 * (R11.4, design §13) and its notifications (R18, [NotifyHost]). The system lists an app under
 * "Notification access" because of this service; once the user allows it,
 * `MediaSessionManager.getActiveSessions` accepts this component and [activeNotifications] works.
 */
class MediaListenerService : NotificationListenerService() {
    override fun onListenerConnected() {
        instance = this
    }

    override fun onListenerDisconnected() {
        if (instance === this) instance = null
    }

    companion object {
        /** The connected service, or null while the system has not bound it. */
        @Volatile
        var instance: MediaListenerService? = null
            private set

        /** True when the user allowed notification access for this app. */
        fun isEnabled(context: Context): Boolean =
            NotificationManagerCompat.getEnabledListenerPackages(context).contains(context.packageName)
    }
}
