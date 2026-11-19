package com.urdevice.app.host

import android.content.Context
import android.service.notification.NotificationListenerService
import androidx.core.app.NotificationManagerCompat

/**
 * The notification listener that lets the app read the phone's active media sessions
 * (R11.4, design §13). The system lists an app under "Notification access" because of this service;
 * once the user allows it, `MediaSessionManager.getActiveSessions` accepts this component.
 *
 * It reads no notifications for the media feature. Other features (notifications, #26) may handle
 * `onNotificationPosted` here later.
 */
class MediaListenerService : NotificationListenerService() {
    companion object {
        /** True when the user allowed notification access for this app. */
        fun isEnabled(context: Context): Boolean =
            NotificationManagerCompat.getEnabledListenerPackages(context).contains(context.packageName)
    }
}
