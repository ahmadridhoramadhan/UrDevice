package com.urdevice.app.ui.features

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.urdevice.app.R
import com.urdevice.app.host.NotifyHost
import com.urdevice.core.UrDeviceNode

/**
 * Notification mirroring (#26, R18). This phone is the host: a desktop that turned mirroring on and
 * picked apps sees their notifications. The phone does not receive notifications from other devices
 * yet, so it has no tile. The only setting here is whether private notifications are shown in full
 * (R18.3); it needs the same notification access as media (see Media.kt).
 */
object NotifyFeature : Feature {
    override val id = "notify"

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        node.notifySetHost(NotifyHost(context))
    }

    @Composable
    override fun ThisDevice() {
        val context = LocalContext.current
        val prefs = remember { context.getSharedPreferences(NotifyHost.PREFS, Context.MODE_PRIVATE) }
        var full by remember { mutableStateOf(prefs.getBoolean(NotifyHost.FULL_CONTENT, true)) }
        Card(Modifier.fillMaxWidth()) {
            Row(
                Modifier.padding(14.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(12.dp),
            ) {
                Column(Modifier.weight(1f).heightIn(min = 44.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(stringResource(R.string.notify_full_title), fontWeight = FontWeight.SemiBold)
                    Text(stringResource(R.string.notify_full_body), style = MaterialTheme.typography.bodySmall)
                }
                Switch(
                    checked = full,
                    onCheckedChange = {
                        full = it
                        prefs.edit().putBoolean(NotifyHost.FULL_CONTENT, it).apply()
                    },
                )
            }
        }
    }
}
