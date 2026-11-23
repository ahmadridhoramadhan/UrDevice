package com.urdevice.app.ui

import android.text.format.DateUtils
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.material.icons.outlined.Close
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.urdevice.app.Labels
import com.urdevice.app.R
import com.urdevice.app.ui.features.BluetoothFeature
import com.urdevice.app.ui.features.allFeatures
import com.urdevice.core.FfiActivity
import com.urdevice.core.FfiLogEntry
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.FfiSnapshot

private fun lastSeen(ts: ULong): String =
    DateUtils.getRelativeTimeSpanString(ts.toLong() * 1000).toString()

@Composable
fun peerLine(p: FfiPeer): String = when (p.status) {
    "connected" -> stringResource(R.string.status_connected_line, platformName(p.platform), BluetoothFeature.pathLabel(p.path), (p.rttMs ?: 0u).toInt())
    "disconnected" -> stringResource(R.string.status_disconnected_by, p.disconnectedBy)
    else -> if (p.lastSeen > 0u) stringResource(R.string.status_last_seen, lastSeen(p.lastSeen)) else stringResource(R.string.status_offline)
}

/** The one-line status in the device list. */
@Composable
fun peerShort(p: FfiPeer): String =
    if (p.status == "connected") stringResource(R.string.status_connected) else peerLine(p)

/** "X is using Y on this device" with Stop (R5.4). */
@Composable
fun ActiveBanner(a: FfiActivity, onStop: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().background(LocalModes.current.warnBg).padding(horizontal = 16.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            stringResource(R.string.active_banner, a.deviceName, stringResource(Labels.cap(a.capability))),
            fontWeight = FontWeight.SemiBold,
            modifier = Modifier.weight(1f),
        )
        OutlinedButton(onClick = onStop, modifier = Modifier.heightIn(min = 44.dp)) {
            Text(stringResource(R.string.stop), color = LocalModes.current.denyFg)
        }
    }
}

@Composable
private fun SectionLabel(t: String) =
    Text(t, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)

@Composable
fun HomeScreen(snap: FfiSnapshot, onThisDevice: () -> Unit, onPeer: (String) -> Unit, onAdd: () -> Unit) {
    val c = LocalModes.current
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        Card(
            colors = CardDefaults.cardColors(containerColor = c.header),
            modifier = Modifier.fillMaxWidth().clickable(onClick = onThisDevice),
        ) {
            Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(44.dp).background(Lime, RoundedCornerShape(12.dp)), contentAlignment = Alignment.Center) {
                    Icon(platformIcon("android"), null, tint = Forest)
                }
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(snap.me.name, color = c.onHeader, fontWeight = FontWeight.Bold, fontSize = 18.sp)
                    Text(stringResource(R.string.this_device), color = c.onHeader, style = MaterialTheme.typography.bodySmall)
                }
                snap.me.batteryPct?.let {
                    Box(Modifier.background(Lime, CircleShape).padding(horizontal = 10.dp, vertical = 4.dp)) {
                        Text(stringResource(R.string.battery_pct, it.toInt()), color = Forest, fontWeight = FontWeight.Bold)
                    }
                }
            }
        }
        SectionLabel(stringResource(R.string.devices))
        if (snap.peers.isEmpty()) {
            Text(stringResource(R.string.no_devices), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        snap.peers.forEach { p ->
            Card(
                colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
                modifier = Modifier.fillMaxWidth().clickable { onPeer(p.id) },
            ) {
                Row(Modifier.padding(14.dp).heightIn(min = 44.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconBox(platformIcon(p.platform))
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(p.name, fontWeight = FontWeight.SemiBold)
                        Text(peerShort(p), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (p.status == "connected") {
                        p.batteryPct?.let {
                            Text(stringResource(R.string.battery_pct, it.toInt()), style = MaterialTheme.typography.bodySmall)
                            Spacer(Modifier.width(8.dp))
                        }
                    }
                    Box(Modifier.size(9.dp).background(if (p.status == "connected") c.online else c.offline, CircleShape))
                }
            }
        }
        OutlinedButton(onClick = onAdd, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
            Icon(Icons.Outlined.Add, null)
            Spacer(Modifier.width(6.dp))
            Text(stringResource(R.string.add_device), fontWeight = FontWeight.SemiBold)
        }
    }
}

/** "This device": sections from features, for example shared localhost ports. Permissions are in Settings. */
@Composable
fun ThisDeviceScreen() {
    Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
        allFeatures.forEach { it.ThisDevice() }
    }
}

/** A paired device (R4.7–R4.9, R5.7). */
@Composable
fun DeviceScreen(
    snap: FfiSnapshot,
    peer: FfiPeer,
    onDisconnect: () -> Unit,
    onReconnect: () -> Unit,
    onForget: () -> Unit,
    onRequest: (String) -> Unit,
    onOpenFeature: (feature: String, tile: String) -> Unit,
    onSettings: () -> Unit,
    onStop: (ULong) -> Unit,
) {
    val c = LocalModes.current
    val ctx = androidx.compose.ui.platform.LocalContext.current
    var forget by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (peer.status == "connected") {
                OutlinedButton(onClick = onDisconnect, modifier = Modifier.heightIn(min = 44.dp)) { Text(stringResource(R.string.disconnect)) }
            } else {
                Button(onClick = onReconnect, enabled = peer.status == "disconnected", modifier = Modifier.heightIn(min = 44.dp)) {
                    Text(stringResource(R.string.reconnect))
                }
            }
            Spacer(Modifier.weight(1f))
            OutlinedButton(onClick = { forget = true }, modifier = Modifier.heightIn(min = 44.dp)) {
                Text(stringResource(R.string.forget), color = c.denyFg)
            }
        }
        Card(colors = CardDefaults.cardColors(containerColor = c.header), modifier = Modifier.fillMaxWidth()) {
            Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Icon(platformIcon(peer.platform), null, tint = c.onHeader, modifier = Modifier.size(32.dp))
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(peer.name, color = c.onHeader, fontWeight = FontWeight.Bold, fontSize = 20.sp)
                    Text(
                        peerLine(peer),
                        color = c.onHeader, style = MaterialTheme.typography.bodySmall,
                    )
                }
                peer.batteryPct?.let {
                    Text(
                        if (peer.charging) stringResource(R.string.battery_charging, it.toInt()) else stringResource(R.string.battery_pct, it.toInt()),
                        color = c.onHeader, fontWeight = FontWeight.Bold,
                    )
                }
            }
        }
        // Feature cards under the header, for example the Bluetooth LE banner (R6.4).
        allFeatures.forEach { it.DeviceBanner(peer) }
        if (peer.status != "connected") {
            Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
                Column(Modifier.padding(14.dp)) {
                    Text(
                        if (peer.status == "disconnected") stringResource(R.string.status_disconnected_by, peer.disconnectedBy)
                        else stringResource(R.string.peer_offline_title, peer.name),
                        fontWeight = FontWeight.SemiBold,
                    )
                    Text(
                        if (peer.status == "disconnected") stringResource(R.string.peer_disconnected_body)
                        else stringResource(R.string.peer_offline_body, peer.name),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        } else {
            Text(stringResource(R.string.features), fontWeight = FontWeight.SemiBold)
            val modes = peer.remotePermissions.associate { it.capability to it.mode }
            // Each feature's tiles (ui/features/), in the order of allFeatures.
            val tiles = allFeatures.flatMap { f -> f.tiles(modes).map { f to it } }
            tiles.chunked(2).forEach { row ->
                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                    row.forEach { (f, t) ->
                        val mode = modes[t.cap] ?: FfiMode.DENY
                        var menuOpen by remember(peer.id, t.key) { mutableStateOf(false) }
                        var flipped by remember(peer.id, t.key) { mutableStateOf<Boolean?>(null) }
                        val switchOn = f.switch(peer)?.let { flipped ?: it }
                        FeatureTile(
                            label = stringResource(t.label ?: Labels.cap(t.cap)),
                            icon = t.key,
                            mode = mode,
                            available = BluetoothFeature.available(peer, t.cap),
                            host = peer.name,
                            onClick = {
                                if (f.tileMenu) menuOpen = true
                                else if (f.hasScreen) onOpenFeature(f.id, t.key)
                                else onRequest(t.cap)
                            },
                            menu = { if (f.tileMenu) f.TileMenu(peer, t.key, menuOpen) { menuOpen = false } },
                            onGear = {
                                val page = f.settingsPage(t.key)
                                if (page != null) onOpenFeature(f.id, page)
                                else if (!f.openSettings(ctx)) onSettings()
                            },
                            switchOn = switchOn,
                            onSwitch = { flipped = it; f.setSwitch(peer, it) },
                            modifier = Modifier.weight(1f),
                        )
                    }
                    if (row.size == 1) Spacer(Modifier.weight(1f))
                }
            }
            snap.activities.filter { !it.incoming && it.deviceId == peer.id }.forEach { a ->
                Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
                    Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(stringResource(R.string.you_are_using, stringResource(Labels.cap(a.capability)), peer.name), modifier = Modifier.weight(1f))
                        OutlinedButton(onClick = { onStop(a.id) }, modifier = Modifier.heightIn(min = 44.dp)) { Text(stringResource(R.string.end)) }
                    }
                }
            }
        }
    }
    if (forget) {
        ConfirmDialog(
            stringResource(R.string.forget_title), stringResource(R.string.forget_body), stringResource(R.string.forget),
            onConfirm = { forget = false; onForget() }, onDismiss = { forget = false },
        )
    }
}

/**
 * A feature tile: normal for Allow, "Needs approval" for Always Ask, grey for Deny, and grey with
 * "Not available over Bluetooth" when the session runs on Bluetooth LE (R6.4).
 */
@Composable
private fun FeatureTile(
    label: String,
    icon: String,
    mode: FfiMode,
    available: Boolean,
    host: String,
    onClick: () -> Unit,
    onGear: () -> Unit,
    /** Non-null for a switch tile: the state; the tile has a switch instead of the settings icon. */
    switchOn: Boolean?,
    onSwitch: (Boolean) -> Unit,
    modifier: Modifier,
    /** Extra content anchored to the tile, for example a menu. */
    menu: @Composable () -> Unit = {},
) {
    val c = LocalModes.current
    val off = mode == FfiMode.DENY || !available
    // Screen readers still get the state in words.
    val state = if (!available) stringResource(R.string.bt_tile_unavailable) else when (mode) {
        FfiMode.ALLOW -> stringResource(R.string.tile_ready)
        FfiMode.ASK -> stringResource(R.string.tile_needs_approval)
        FfiMode.DENY -> stringResource(R.string.tile_turned_off_by, host)
    }
    // A switch tile lights up when it is on.
    val lit = switchOn == true && !off
    val fg = if (lit) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface
    Card(
        colors = CardDefaults.cardColors(
            containerColor = when {
                lit -> MaterialTheme.colorScheme.primary
                off -> MaterialTheme.colorScheme.surfaceVariant
                else -> MaterialTheme.colorScheme.surface
            },
        ),
        modifier = modifier.heightIn(min = 88.dp)
            .alpha(if (off) 0.45f else 1f)
            .semantics(mergeDescendants = true) {
                stateDescription = state
                if (switchOn != null) toggleableState = ToggleableState(switchOn)
            }
            .clickable(enabled = !off, onClick = if (switchOn != null) ({ onSwitch(!switchOn) }) else onClick),
    ) {
        Column(Modifier.padding(12.dp).then(if (off) Modifier.padding(0.dp) else Modifier)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (lit) {
                    Box(
                        Modifier.size(34.dp).background(fg.copy(alpha = 0.16f), RoundedCornerShape(10.dp)),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(capIcon(icon), contentDescription = null, tint = fg, modifier = Modifier.size(17.dp))
                    }
                } else if (off) {
                    Box(
                        Modifier.size(34.dp).background(MaterialTheme.colorScheme.onSurfaceVariant.copy(alpha = 0.18f), RoundedCornerShape(10.dp)),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(capIcon(icon), contentDescription = null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(17.dp))
                    }
                } else {
                    IconBox(capIcon(icon), 34)
                }
                Spacer(Modifier.weight(1f))
                if (off) {
                    Icon(Icons.Outlined.Close, null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(16.dp))
                } else if (switchOn == null) {
                    IconButton(onClick = onGear, modifier = Modifier.size(44.dp)) {
                        Icon(Icons.Outlined.Settings, null, tint = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.size(18.dp))
                    }
                }
            }
            // No status words: an amber dot means "you approve first"; a dimmed tile is off.
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(label, fontWeight = FontWeight.SemiBold, color = if (off) MaterialTheme.colorScheme.onSurfaceVariant else fg)
                if (!off && mode == FfiMode.ASK) {
                    Spacer(Modifier.width(6.dp))
                    Box(Modifier.size(8.dp).background(c.askFg, CircleShape))
                }
            }
            menu()
        }
    }
}

@Composable
fun SettingsScreen(
    snap: FfiSnapshot,
    onRename: (String) -> Unit,
    onHidden: (Boolean) -> Unit,
    onLog: () -> Unit,
    onExport: () -> Unit,
    onReset: () -> Unit,
    onDeleteAll: () -> Unit,
    onSetPermission: (String, FfiMode, Boolean) -> Unit,
    version: String,
) {
    var name by remember(snap.me.name) { mutableStateOf(snap.me.name) }
    var confirm by remember { mutableStateOf<String?>(null) }
    var permConfirm by remember { mutableStateOf<String?>(null) }
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SettingsCard {
            Text(stringResource(R.string.set_name), fontWeight = FontWeight.SemiBold)
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { name = it.take(64) }, singleLine = true, modifier = Modifier.weight(1f))
                Button(onClick = { onRename(name) }, enabled = name.isNotBlank() && name != snap.me.name) { Text(stringResource(R.string.save)) }
            }
            Text(stringResource(R.string.device_id, snap.me.fingerprint), style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace)
        }
        // Permissions for other devices (R5.1), with the extra confirmation for terminal and file changes.
        SectionLabel(stringResource(R.string.perms_title))
        Text(stringResource(R.string.perms_apply_all), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        snap.permissions.forEach { p ->
            PermissionRow(p) { m ->
                if (p.localOnly && m == FfiMode.ALLOW && p.mode != FfiMode.ALLOW) permConfirm = p.capability
                else onSetPermission(p.capability, m, false)
            }
        }
        SettingsCard {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.set_hidden), fontWeight = FontWeight.SemiBold)
                    Text(stringResource(R.string.set_hidden_body), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Switch(checked = snap.me.hidden, onCheckedChange = onHidden)
            }
            // Feature settings, such as "Pause clipboard on all devices" (ui/features/).
            allFeatures.forEach { it.Settings() }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.set_language), fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                Text(stringResource(R.string.set_language_body), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        SettingsCard {
            OutlinedButton(onClick = onLog, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.set_log)) }
            OutlinedButton(onClick = onExport, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) { Text(stringResource(R.string.set_export)) }
            OutlinedButton(onClick = { confirm = "reset" }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Text(stringResource(R.string.set_reset), color = LocalModes.current.denyFg)
            }
            OutlinedButton(onClick = { confirm = "delete" }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Text(stringResource(R.string.set_delete), color = LocalModes.current.denyFg)
            }
        }
        Text(stringResource(R.string.core_version, version), style = MaterialTheme.typography.bodySmall)
    }
    permConfirm?.let { cap ->
        val term = cap == "term.v1"
        ConfirmDialog(
            title = stringResource(if (term) R.string.confirm_term_title else R.string.confirm_fs_title),
            body = stringResource(if (term) R.string.confirm_term_body else R.string.confirm_fs_body),
            confirm = stringResource(R.string.allow),
            onConfirm = { onSetPermission(cap, FfiMode.ALLOW, true); permConfirm = null },
            onDismiss = { permConfirm = null },
        )
    }
    when (confirm) {
        "reset" -> ConfirmDialog(
            stringResource(R.string.set_reset), stringResource(R.string.set_reset_body), stringResource(R.string.set_reset),
            onConfirm = { confirm = null; onReset() }, onDismiss = { confirm = null },
        )
        "delete" -> ConfirmDialog(
            stringResource(R.string.set_delete), stringResource(R.string.set_delete_body), stringResource(R.string.set_delete),
            onConfirm = { confirm = null; onDeleteAll() }, onDismiss = { confirm = null },
        )
    }
}

@Composable
private fun SettingsCard(content: @Composable () -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface), modifier = Modifier.fillMaxWidth()) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) { content() }
    }
}

@Composable
fun LogScreen(entries: List<FfiLogEntry>) {
    val ctx = androidx.compose.ui.platform.LocalContext.current
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.set_log_body), style = MaterialTheme.typography.bodySmall)
        if (entries.isEmpty()) Text(stringResource(R.string.log_empty))
        entries.forEach { e ->
            Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface), modifier = Modifier.fillMaxWidth()) {
                Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("${e.deviceName} · ${stringResource(Labels.cap(e.capability))}", fontWeight = FontWeight.SemiBold)
                        Text(
                            DateUtils.formatDateTime(ctx, e.ts.toLong() * 1000, DateUtils.FORMAT_SHOW_DATE or DateUtils.FORMAT_SHOW_TIME),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    Text(stringResource(Labels.decision(e.decision)))
                }
            }
        }
    }
}
