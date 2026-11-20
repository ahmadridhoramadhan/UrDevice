package com.urdevice.app.ui.features

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.text.format.DateFormat
import android.text.format.Formatter
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.Checkbox
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.urdevice.app.Core
import com.urdevice.app.MainActivity
import com.urdevice.app.R
import com.urdevice.core.FfiLanMode
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.FfiSharedPort
import com.urdevice.core.FfiTunnel
import com.urdevice.core.UrDeviceNode
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

/**
 * Localhost sharing and Expose on LAN (#22, R13, design §11).
 *
 * - Viewer: the paired device's shared ports, each opened on this phone's loopback (the core's
 *   listener lives in the foreground service's process), with "Open in browser".
 * - Host: this phone's shared ports on the "This device" screen, with Expose on LAN ("1 hour" or
 *   "Always on" after an extra confirmation), and an ongoing notification while any port is open
 *   on the LAN (the persistent indicator).
 */
object TunnelFeature : Feature {
    override val id = "tunnel"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("tunnel.v1", "tunnel.v1"))

    override val hasScreen get() = true

    private const val CHANNEL = "tunnel_lan"
    private const val LAN_NOTIFICATION = 2201

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        val app = context.applicationContext
        app.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, app.getString(R.string.tunnel_channel), NotificationManager.IMPORTANCE_LOW),
        )
        // The LAN indicator follows the core state, also with no screen open. Exposures that
        // were "Always on" come back after a restart, so this runs from the start.
        CoroutineScope(SupervisorJob() + Dispatchers.Main).launch {
            var last: List<UShort>? = null
            Core.snapshot.collect {
                val ports = runCatching { node.tunnelLanExposed() }.getOrDefault(emptyList())
                if (ports != last) {
                    last = ports
                    showLanIndicator(app, ports)
                }
            }
        }
    }

    private fun showLanIndicator(ctx: Context, ports: List<UShort>) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        if (ports.isEmpty()) {
            nm.cancel(LAN_NOTIFICATION)
            return
        }
        if (Build.VERSION.SDK_INT >= 33 &&
            ctx.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            return
        }
        val open = PendingIntent.getActivity(
            ctx, 2201, Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val n = Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setOngoing(true)
            .setContentTitle(ctx.getString(R.string.tunnel_lan_banner, ports.joinToString(", ")))
            .setContentText(ctx.getString(R.string.tunnel_lan_notif_body))
            .setContentIntent(open)
            .build()
        runCatching { nm.notify(LAN_NOTIFICATION, n) }
    }

    @Composable
    override fun Screen(peer: FfiPeer, tile: String) {
        ViewerScreen(peer)
    }

    @Composable
    override fun ThisDevice() {
        HostSection()
    }
}

private fun openBrowser(ctx: Context, localPort: UShort) {
    // Only ever built from a port number.
    val i = Intent(Intent.ACTION_VIEW, Uri.parse("http://localhost:$localPort/")).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    runCatching { ctx.startActivity(i) }
}

private fun stats(ctx: Context, t: FfiTunnel): String =
    ctx.resources.getQuantityString(R.plurals.tunnel_connections, t.connections.toInt(), t.connections.toInt()) +
        " · " + Formatter.formatShortFileSize(ctx, (t.bytesUp + t.bytesDown).toLong())

private fun reasonText(ctx: Context, reason: String, host: String): String = when (reason) {
    "denied" -> ctx.getString(R.string.tunnel_denied, host)
    "declined" -> ctx.getString(R.string.tunnel_declined, host)
    "timeout" -> ctx.getString(R.string.tunnel_timeout, host)
    "offline" -> ctx.getString(R.string.tunnel_offline, host)
    "bind" -> ctx.getString(R.string.tunnel_bind)
    "refused_port" -> ctx.getString(R.string.tunnel_refused_port, host)
    else -> ctx.getString(R.string.tunnel_failed, reason)
}

@Composable
private fun ViewerScreen(peer: FfiPeer) {
    val ctx = LocalContext.current
    val node = Core.node()
    val snap by Core.snapshot.collectAsState()
    var tunnels by remember { mutableStateOf<List<FfiTunnel>>(emptyList()) }
    var openWhenReady by remember(peer.id) { mutableStateOf(setOf<UShort>()) }
    var error by remember(peer.id) { mutableStateOf("") }
    val ports = remember(snap, peer.id) { runCatching { node?.tunnelRemotePorts(peer.id) }.getOrNull() ?: emptyList() }
    val denied = peer.remotePermissions.any { it.capability == "tunnel.v1" && it.mode == FfiMode.DENY }

    // Traffic counters change without a snapshot; read them every second.
    LaunchedEffect(peer.id, snap) {
        while (true) {
            tunnels = node?.tunnelList()?.filter { !it.incoming && it.deviceId == peer.id } ?: emptyList()
            val ready = tunnels.filter { it.status == "open" && it.remotePort in openWhenReady }
            ready.forEach { openBrowser(ctx, it.localPort) }
            val failed = tunnels.filter { it.status == "failed" }.map { it.remotePort }.toSet()
            if (ready.isNotEmpty() || failed.any { it in openWhenReady }) {
                openWhenReady = openWhenReady - ready.map { it.remotePort }.toSet() - failed
            }
            delay(1000)
        }
    }

    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(stringResource(R.string.tunnel_title, peer.name), style = MaterialTheme.typography.titleMedium)
        when {
            denied -> Text(stringResource(R.string.tunnel_denied, peer.name), style = MaterialTheme.typography.bodySmall)
            ports.isEmpty() -> Text(stringResource(R.string.tunnel_none, peer.name), style = MaterialTheme.typography.bodySmall)
        }
        ports.forEach { p ->
            val t = tunnels.firstOrNull { it.remotePort == p.port }
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    val local = if (t != null && t.status == "open") t.localPort else p.port
                    Text("localhost:$local", fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold)
                    val detail = if (t == null) "" else when (t.status) {
                        "open" -> " · " + stats(ctx, t)
                        "opening" -> " · " + stringResource(R.string.tunnel_waiting)
                        "failed" -> " · " + reasonText(ctx, t.reason, peer.name)
                        else -> ""
                    }
                    Text(
                        (p.label.ifEmpty { stringResource(R.string.tunnel_no_label) }) +
                            stringResource(R.string.tunnel_from, peer.name, p.port.toInt()) + detail,
                        style = MaterialTheme.typography.bodySmall,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        Button(
                            enabled = t?.status != "opening",
                            onClick = {
                                error = ""
                                if (t != null && t.status == "open") {
                                    openBrowser(ctx, t.localPort)
                                } else {
                                    openWhenReady = openWhenReady + p.port
                                    runCatching { node?.tunnelOpen(peer.id, p.port) }.onFailure { error = it.message ?: "" }
                                }
                            },
                            modifier = Modifier.heightIn(min = 44.dp),
                        ) { Text(stringResource(R.string.tunnel_open_browser)) }
                        if (t != null && (t.status == "open" || t.status == "opening")) {
                            OutlinedButton(onClick = { node?.tunnelStop(t.id) }, modifier = Modifier.heightIn(min = 44.dp)) {
                                Text(stringResource(R.string.stop))
                            }
                        }
                    }
                }
            }
        }
        Text(stringResource(R.string.tunnel_hint, peer.name), style = MaterialTheme.typography.bodySmall)
        if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
    }
}

@Composable
private fun HostSection() {
    val ctx = LocalContext.current
    val node = Core.node() ?: return
    val snap by Core.snapshot.collectAsState()
    var shared by remember { mutableStateOf<List<FfiSharedPort>>(emptyList()) }
    var incoming by remember { mutableStateOf<List<FfiTunnel>>(emptyList()) }
    var portText by remember { mutableStateOf("") }
    var label by remember { mutableStateOf("") }
    var error by remember { mutableStateOf("") }
    // Port and mode waiting for the LAN warning to be confirmed.
    var pending by remember { mutableStateOf<Pair<UShort, FfiLanMode>?>(null) }
    val deny = snap?.permissions?.firstOrNull { it.capability == "tunnel.v1" }?.mode == FfiMode.DENY

    LaunchedEffect(snap) {
        while (true) {
            shared = node.tunnelShared()
            incoming = node.tunnelList().filter { it.incoming }
            delay(1000)
        }
    }

    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(stringResource(R.string.tunnel_shared_title), style = MaterialTheme.typography.titleMedium)
        val lan = shared.filter { it.lan != FfiLanMode.OFF }
        if (lan.isNotEmpty()) {
            Card(Modifier.fillMaxWidth()) {
                Text(
                    stringResource(R.string.tunnel_lan_banner, lan.joinToString(", ") { it.port.toString() }),
                    modifier = Modifier.padding(12.dp),
                    fontWeight = FontWeight.SemiBold,
                    color = MaterialTheme.colorScheme.error,
                )
            }
        }
        Text(
            stringResource(if (deny) R.string.tunnel_shared_sub_deny else R.string.tunnel_shared_sub),
            style = MaterialTheme.typography.bodySmall,
        )
        if (shared.isEmpty()) Text(stringResource(R.string.tunnel_no_ports), style = MaterialTheme.typography.bodySmall)
        shared.forEach { p ->
            Card(Modifier.fillMaxWidth()) {
                Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    Text("localhost:${p.port}", fontFamily = FontFamily.Monospace, fontWeight = FontWeight.SemiBold)
                    val lanText = when {
                        p.lan == FfiLanMode.OFF -> ""
                        !p.lanListening -> " · " + stringResource(R.string.tunnel_lan_waiting)
                        p.lan == FfiLanMode.ONE_HOUR -> " · " + stringResource(
                            R.string.tunnel_lan_until,
                            DateFormat.getTimeFormat(ctx).format(java.util.Date(p.lanUntil.toLong() * 1000)),
                        )
                        else -> " · " + stringResource(R.string.tunnel_lan_always)
                    }
                    Text(p.label.ifEmpty { stringResource(R.string.tunnel_no_label) } + lanText, style = MaterialTheme.typography.bodySmall)
                    Text(stringResource(R.string.tunnel_expose), style = MaterialTheme.typography.labelMedium)
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalAlignment = Alignment.CenterVertically) {
                        listOf(
                            FfiLanMode.OFF to R.string.tunnel_lan_off,
                            FfiLanMode.ONE_HOUR to R.string.tunnel_lan_hour,
                            FfiLanMode.ALWAYS to R.string.tunnel_lan_always_on,
                        ).forEach { (m, res) ->
                            FilterChip(
                                selected = p.lan == m,
                                onClick = {
                                    error = ""
                                    if (m == FfiLanMode.OFF) {
                                        runCatching { node.tunnelSetLan(p.port, m, false) }.onFailure { error = it.message ?: "" }
                                    } else if (m != p.lan) {
                                        pending = p.port to m
                                    }
                                },
                                label = { Text(stringResource(res)) },
                                modifier = Modifier.heightIn(min = 44.dp),
                            )
                        }
                    }
                    OutlinedButton(
                        onClick = { runCatching { node.tunnelUnshare(p.port) }.onFailure { error = it.message ?: "" } },
                        modifier = Modifier.heightIn(min = 44.dp),
                    ) { Text(stringResource(R.string.tunnel_unshare), color = MaterialTheme.colorScheme.error) }
                }
            }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                value = portText,
                onValueChange = { portText = it.filter(Char::isDigit).take(5) },
                label = { Text(stringResource(R.string.tunnel_port)) },
                singleLine = true,
                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number),
                modifier = Modifier.width(110.dp),
            )
            OutlinedTextField(
                value = label,
                onValueChange = { label = it.take(64) },
                label = { Text(stringResource(R.string.tunnel_label)) },
                singleLine = true,
                modifier = Modifier.weight(1f),
            )
        }
        Button(
            onClick = {
                val port = portText.toIntOrNull()
                if (port == null || port !in 1..65535) {
                    error = ctx.getString(R.string.tunnel_bad_port)
                } else {
                    runCatching { node.tunnelShare(port.toUShort(), label) }
                        .onSuccess { portText = ""; label = ""; error = "" }
                        .onFailure { error = it.message ?: "" }
                }
            },
            modifier = Modifier.heightIn(min = 44.dp),
        ) { Text(stringResource(R.string.tunnel_share)) }
        if (error.isNotEmpty()) Text(error, color = MaterialTheme.colorScheme.error)
        if (incoming.isNotEmpty()) Text(stringResource(R.string.tunnel_in_use), style = MaterialTheme.typography.titleSmall)
        incoming.forEach { t ->
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    stringResource(R.string.tunnel_incoming, t.deviceName, t.remotePort.toInt(), stats(ctx, t)),
                    style = MaterialTheme.typography.bodySmall,
                    modifier = Modifier.weight(1f),
                )
                OutlinedButton(onClick = { node.tunnelStop(t.id) }, modifier = Modifier.heightIn(min = 44.dp)) {
                    Text(stringResource(R.string.stop))
                }
            }
        }
    }

    pending?.let { (port, mode) ->
        val always = mode == FfiLanMode.ALWAYS
        var ack by remember(port, mode) { mutableStateOf(false) }
        AlertDialog(
            onDismissRequest = { pending = null },
            title = { Text(stringResource(R.string.tunnel_lan_warn_title, port.toInt())) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(stringResource(R.string.tunnel_lan_warn_body, port.toInt()))
                    if (always) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Checkbox(checked = ack, onCheckedChange = { ack = it })
                            Text(stringResource(R.string.tunnel_lan_ack))
                        }
                    } else {
                        Text(stringResource(R.string.tunnel_lan_hour_note))
                    }
                }
            },
            confirmButton = {
                TextButton(
                    enabled = !always || ack,
                    onClick = {
                        runCatching { node.tunnelSetLan(port, mode, always && ack) }.onFailure { error = it.message ?: "" }
                        pending = null
                    },
                ) { Text(stringResource(if (always) R.string.tunnel_keep_open else R.string.tunnel_open_hour)) }
            },
            dismissButton = { TextButton(onClick = { pending = null }) { Text(stringResource(R.string.cancel)) } },
        )
    }
}
