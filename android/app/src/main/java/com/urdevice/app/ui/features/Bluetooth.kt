package com.urdevice.app.ui.features

import android.content.ActivityNotFoundException
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.provider.Settings
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.urdevice.app.Labels
import com.urdevice.app.R
import com.urdevice.app.host.BluetoothHost
import com.urdevice.app.ui.LocalModes
import com.urdevice.core.FfiPeer
import com.urdevice.core.UrDeviceNode
import com.urdevice.core.bluetoothAllows

/**
 * Bluetooth fallback (#23, design §8). Bluetooth is an automatic fallback, not a setting: the core
 * moves a session to Bluetooth PAN or to the Bluetooth LE channel on its own. This feature starts
 * the GATT server ([BluetoothHost]), asks for the Nearby devices permission it needs, shows the
 * banner while on Bluetooth LE (R6.4), and opens the tethering settings, because Android does not
 * let an app turn on Bluetooth tethering (R6.6).
 */
object BluetoothFeature : Feature {
    override val id = "bluetooth"

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        BluetoothHost.start(context, node)
    }

    /** True when the tile for [cap] works on [peer]'s current path. */
    fun available(peer: FfiPeer, cap: String): Boolean = peer.path != "ble" || bluetoothAllows(cap)

    /** The connection path for the device line (R4.8). */
    @Composable
    fun pathLabel(path: String): String = stringResource(
        when (path) {
            "bt-pan" -> R.string.bt_path_pan
            "ble" -> R.string.bt_path_ble
            else -> R.string.bt_path_wifi
        },
    )

    @Composable
    override fun DeviceBanner(peer: FfiPeer) {
        if (peer.platform == "android") return
        val ctx = LocalContext.current
        val c = LocalModes.current
        val onBle = peer.status == "connected" && peer.path == "ble"
        if (onBle) {
            val off = peer.remotePermissions
                .map { it.capability }
                .filterNot { bluetoothAllows(it) }
                .map { stringResource(Labels.cap(it)) }
                .distinct()
                .joinToString(", ")
            Card(colors = CardDefaults.cardColors(containerColor = c.warnBg), modifier = Modifier.fillMaxWidth()) {
                Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    Text(stringResource(R.string.bt_banner_title, off), fontWeight = FontWeight.SemiBold)
                    Text(stringResource(R.string.bt_banner_body), style = MaterialTheme.typography.bodySmall)
                }
            }
        }
        var granted by remember { mutableStateOf(BluetoothHost.hasPermission(ctx)) }
        val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
            granted = BluetoothHost.hasPermission(ctx)
            BluetoothHost.instance?.permissionChanged()
        }
        if (!granted) {
            GuideCard(
                title = stringResource(R.string.bt_permission_title),
                body = stringResource(R.string.bt_permission_body, peer.name),
                button = stringResource(R.string.bt_permission_allow),
                onClick = { ask.launch(BluetoothHost.permissions()) },
            )
        }
        if (peer.status != "connected" || onBle) {
            GuideCard(
                title = stringResource(R.string.bt_tether_title),
                body = stringResource(R.string.bt_tether_body, peer.name),
                button = stringResource(R.string.bt_tether_open),
                onClick = { openTetherSettings(ctx) },
            )
        }
    }

    @Composable
    private fun GuideCard(title: String, body: String, button: String, onClick: () -> Unit) {
        Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface), modifier = Modifier.fillMaxWidth()) {
            Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Text(title, fontWeight = FontWeight.SemiBold)
                Text(body, style = MaterialTheme.typography.bodySmall)
                OutlinedButton(onClick = onClick, modifier = Modifier.heightIn(min = 44.dp)) { Text(button) }
            }
        }
    }

    /** The hotspot and tethering screen; on some phones it sits under "Other hotspot settings". */
    private fun openTetherSettings(ctx: Context) {
        val tether = Intent().setComponent(ComponentName("com.android.settings", "com.android.settings.TetherSettings"))
        val fallback = Intent(Settings.ACTION_WIRELESS_SETTINGS)
        for (i in listOf(tether, fallback)) {
            try {
                ctx.startActivity(i.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                return
            } catch (e: ActivityNotFoundException) {
                continue
            } catch (e: SecurityException) {
                continue
            }
        }
    }
}
