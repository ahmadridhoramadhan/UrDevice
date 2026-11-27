package com.urdevice.app.ui

import android.text.format.DateUtils
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
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
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.HorizontalDivider
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
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.material.icons.outlined.Close
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.semantics.toggleableState
import androidx.compose.ui.state.ToggleableState
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.DpOffset
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

/** The id the Home picker uses for this device. */
const val PICK_ME = "me"

/**
 * The Home page (owner decision 2026-11-24): one picker at the top lists this device and every
 * paired device, and the page below it shows what belongs to the picked one.
 */
@Composable
fun HomeScreen(
    snap: FfiSnapshot,
    selected: String,
    onSelect: (String) -> Unit,
    onAdd: () -> Unit,
    content: @Composable (picked: String) -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        DevicePicker(snap, selected, onSelect, onAdd)
        // Picking another device slides its page in: from the right for a device further down the list, else from the left.
        AnimatedContent(
            targetState = selected,
            transitionSpec = {
                val order = listOf(PICK_ME) + snap.peers.map { it.id }
                val forward = order.indexOf(targetState) >= order.indexOf(initialState)
                val slide = { width: Int -> if (forward) width / 8 else -width / 8 }
                val enter = fadeIn(tween(220, delayMillis = 60)) +
                    slideInHorizontally(tween(280, easing = FastOutSlowInEasing)) { slide(it) }
                val exit = fadeOut(tween(110)) +
                    slideOutHorizontally(tween(280, easing = FastOutSlowInEasing)) { -slide(it) }
                (enter togetherWith exit).using(null)
            },
            label = "home-content",
        ) { picked -> content(picked) }
        if (snap.peers.isEmpty() && selected == PICK_ME) {
            Text(stringResource(R.string.no_devices), color = MaterialTheme.colorScheme.onSurfaceVariant)
            OutlinedButton(onClick = onAdd, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Icon(Icons.Outlined.Add, null)
                Spacer(Modifier.width(6.dp))
                Text(stringResource(R.string.add_device), fontWeight = FontWeight.SemiBold)
            }
        }
    }
}

/**
 * The dropdown that picks a device. The closed form is a card with the picked device (icon with
 * its online dot, name, connection line, battery) and a chevron that turns when it is open. The
 * open form is a rounded list as wide as the card, with the picked row tinted.
 */
@Composable
private fun DevicePicker(snap: FfiSnapshot, selected: String, onSelect: (String) -> Unit, onAdd: () -> Unit) {
    val c = LocalModes.current
    var open by remember { mutableStateOf(false) }
    var width by remember { mutableStateOf(0) }
    val density = LocalDensity.current
    val turn by animateFloatAsState(if (open) 180f else 0f, tween(200), label = "picker-chevron")
    val peer = snap.peers.firstOrNull { it.id == selected }
    Box(Modifier.fillMaxWidth().onSizeChanged { width = it.width }) {
        Card(
            shape = RoundedCornerShape(20.dp),
            colors = CardDefaults.cardColors(containerColor = c.header),
            modifier = Modifier.fillMaxWidth().clickable { open = true },
        ) {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 14.dp).heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
                Box {
                    Box(Modifier.size(46.dp).background(Lime, RoundedCornerShape(14.dp)), contentAlignment = Alignment.Center) {
                        Icon(platformIcon(peer?.platform ?: "android"), null, tint = Forest)
                    }
                    if (peer != null) {
                        Box(
                            Modifier.align(Alignment.BottomEnd).size(14.dp).background(c.header, CircleShape).padding(2.dp)
                                .background(if (peer.status == "connected") c.online else c.offline, CircleShape),
                        )
                    }
                }
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Text(peer?.name ?: snap.me.name, color = c.onHeader, fontWeight = FontWeight.Bold, fontSize = 18.sp)
                    Text(
                        if (peer != null) peerLine(peer) else stringResource(R.string.this_device),
                        color = c.onHeader.copy(alpha = 0.8f),
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
                val battery = if (peer != null) peer.batteryPct.takeIf { peer.status == "connected" } else snap.me.batteryPct
                battery?.let {
                    val charging = peer?.charging == true
                    Box(Modifier.background(Lime, CircleShape).padding(horizontal = 10.dp, vertical = 4.dp)) {
                        Text(
                            if (charging) stringResource(R.string.battery_charging, it.toInt()) else stringResource(R.string.battery_pct, it.toInt()),
                            color = Forest,
                            fontWeight = FontWeight.Bold,
                            style = MaterialTheme.typography.labelMedium,
                        )
                    }
                    Spacer(Modifier.width(8.dp))
                }
                Box(
                    Modifier.size(32.dp).background(c.onHeader.copy(alpha = 0.14f), CircleShape),
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(Icons.Outlined.KeyboardArrowDown, stringResource(R.string.devices), tint = c.onHeader, modifier = Modifier.rotate(turn))
                }
            }
        }
        DropdownMenu(
            expanded = open,
            onDismissRequest = { open = false },
            modifier = Modifier.width(with(density) { width.toDp() }),
            offset = DpOffset(0.dp, 8.dp),
            shape = RoundedCornerShape(20.dp),
            containerColor = MaterialTheme.colorScheme.surface,
            tonalElevation = 0.dp,
            shadowElevation = 10.dp,
        ) {
            Column(Modifier.padding(horizontal = 8.dp, vertical = 4.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                PickerRow(
                    icon = platformIcon("android"),
                    title = snap.me.name,
                    subtitle = stringResource(R.string.this_device),
                    selected = selected == PICK_ME,
                    dot = null,
                    onClick = { open = false; onSelect(PICK_ME) },
                )
                if (snap.peers.isNotEmpty()) {
                    HorizontalDivider(Modifier.padding(vertical = 4.dp, horizontal = 8.dp), color = MaterialTheme.colorScheme.outline.copy(alpha = 0.5f))
                    Text(
                        stringResource(R.string.devices),
                        style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 10.dp, vertical = 2.dp),
                    )
                }
                snap.peers.forEach { p ->
                    PickerRow(
                        icon = platformIcon(p.platform),
                        title = p.name,
                        subtitle = peerShort(p),
                        selected = selected == p.id,
                        dot = if (p.status == "connected") c.online else c.offline,
                        onClick = { open = false; onSelect(p.id) },
                    )
                }
                HorizontalDivider(Modifier.padding(vertical = 4.dp, horizontal = 8.dp), color = MaterialTheme.colorScheme.outline.copy(alpha = 0.5f))
                Row(
                    Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).clickable { open = false; onAdd() }
                        .padding(10.dp).heightIn(min = 44.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.size(40.dp).background(Lime.copy(alpha = 0.4f), RoundedCornerShape(12.dp)), contentAlignment = Alignment.Center) {
                        Icon(Icons.Outlined.Add, null, tint = MaterialTheme.colorScheme.onSurface)
                    }
                    Spacer(Modifier.width(12.dp))
                    Text(stringResource(R.string.add_device), fontWeight = FontWeight.SemiBold)
                }
            }
        }
    }
}

/** One device in the open picker. The picked one has a tinted background and a check mark. */
@Composable
private fun PickerRow(icon: ImageVector, title: String, subtitle: String, selected: Boolean, dot: Color?, onClick: () -> Unit) {
    val tint = if (selected) MaterialTheme.colorScheme.primary.copy(alpha = 0.16f) else Color.Transparent
    Row(
        Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(tint).clickable(onClick = onClick)
            .padding(10.dp).heightIn(min = 44.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(
            Modifier.size(40.dp).background(if (selected) Lime else MaterialTheme.colorScheme.secondaryContainer, RoundedCornerShape(12.dp)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(icon, null, tint = if (selected) Forest else MaterialTheme.colorScheme.primary)
        }
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, fontWeight = FontWeight.SemiBold, maxLines = 1)
            Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
        }
        if (selected) {
            Icon(Icons.Outlined.Check, null, tint = MaterialTheme.colorScheme.onSurface)
        } else if (dot != null) {
            Box(Modifier.size(9.dp).background(dot, CircleShape))
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
        // Feature cards, for example the Bluetooth LE banner (R6.4).
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
