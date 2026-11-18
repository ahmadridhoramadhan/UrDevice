package com.urdevice.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Allow / Deny on prompt notifications, and Stop on the service notification. */
class ActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val node = Core.node() ?: return
        val id = intent.getLongExtra(EXTRA_ID, -1)
        if (id < 0) return
        when (intent.action) {
            ALLOW -> node.answerPrompt(id.toULong(), true)
            DENY -> node.answerPrompt(id.toULong(), false)
            STOP -> node.stopActivity(id.toULong())
        }
    }

    companion object {
        const val EXTRA_ID = "id"
        const val ALLOW = "com.urdevice.app.ALLOW"
        const val DENY = "com.urdevice.app.DENY"
        const val STOP = "com.urdevice.app.STOP"
    }
}
