package com.urdevice.app.ui.features

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.LaunchedEffect
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
import com.urdevice.app.Core
import com.urdevice.app.MainActivity
import com.urdevice.app.R
import com.urdevice.app.host.InputHost
import com.urdevice.core.FfiInputEvent
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.InputListener
import com.urdevice.core.UrDeviceNode
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/**
 * Remote control from this phone: Mousepad, Presenter, and Keyboard (#18, R9, R9a, R9b, R10).
 *
 * The three tiles open one session each. Opening a screen asks the desktop for a remote-control
 * activity (the desktop's user approves it under Always Ask and sees the indicator with Stop);
 * leaving the screen ends it. The pointer, key, and text events go through the core, which groups
 * them every 8 ms.
 *
 * This phone is also a host (#19): [InputHost] runs taps, swipes, Back, Home and Recents through the
 * UrDevice accessibility service and types through the UrDevice keyboard. The "This device" row
 * checks both are on, shows the Play disclosure before opening Android's settings, and guides the
 * user. The indicator with Stop while a desktop controls the phone is the service notification.
 */
object InputFeature : Feature, InputListener {
    override val id = "input"

    /** What the screens know about the session with one desktop. */
    data class Session(
        val connecting: Boolean = false,
        val active: Boolean = false,
        /** The reason the desktop refused, or empty. */
        val refused: String = "",
        /** `ready`, `unavailable`, `missing`, or empty before the desktop answered. */
        val status: String = "",
        /** For `missing`: `accessibility`, `uinput`, or `portal`. */
        val missing: String = "",
        /** `input_method` when the host cannot type, otherwise empty. */
        val textMissing: String = "",
        /** The host's display in pixels, 0 when unknown. */
        val width: Int = 0,
        val height: Int = 0,
    )

    private val _sessions = MutableStateFlow<Map<String, Session>>(emptyMap())

    /** Session state by device id. */
    val sessions: StateFlow<Map<String, Session>> = _sessions

    override fun tiles(modes: Map<String, FfiMode>) = listOf(
        FeatureTile("input.v1", "input.v1", R.string.tile_mousepad),
        FeatureTile("input.v1", "presenter", R.string.tile_presenter),
        FeatureTile("input.v1", "keyboard", R.string.tile_keyboard),
    )

    override val hasScreen get() = true

    /** The Mousepad fills the page; the other screens scroll. */
    override fun fullScreen(tile: String) = tile != "presenter" && tile != "keyboard" && tile != MOUSEPAD_SETTINGS

    /** The settings icon of the Mousepad tile opens the Mousepad settings page. */
    override fun settingsPage(tile: String): String? = if (tile == "input.v1") MOUSEPAD_SETTINGS else null

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        node.inputSetListener(this)
        appContext = context.applicationContext
        runCatching { node.inputSetHost(InputHost(context.applicationContext)) }
    }

    private var appContext: Context? = null

    private const val HOST_CHANNEL = "input_host"
    private const val HOST_NOTIFICATION = 4190

    /** This phone is controlled but a setting is off: tell the user, who opens the app from the notification. */
    private fun guideHost(missing: String) {
        val ctx = appContext ?: return
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(HOST_CHANNEL, ctx.getString(R.string.input_host_title), NotificationManager.IMPORTANCE_HIGH),
        )
        val text = ctx.getString(
            if (missing == "input_method") R.string.input_host_notif_keyboard else R.string.input_host_notif_service,
        )
        val open = PendingIntent.getActivity(
            ctx, 0,
            Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE,
        )
        val n = Notification.Builder(ctx, HOST_CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(ctx.getString(R.string.input_host_notif_title))
            .setContentText(text)
            .setAutoCancel(true)
            .setContentIntent(open)
            .build()
        runCatching { nm.notify(HOST_NOTIFICATION, n) }
    }

    @Composable
    override fun ThisDevice() {
        HostRow()
    }

    override fun onEvent(event: FfiInputEvent) {
        _sessions.update { all ->
            val cur = all[event.deviceId] ?: Session()
            val next = when (event.kind) {
                "session" ->
                    if (event.detail == "start") cur.copy(connecting = false, active = true, refused = "", status = "", missing = "")
                    else cur.copy(connecting = false, active = false, status = "")
                "status" -> cur.copy(
                    status = event.detail,
                    missing = event.missing,
                    textMissing = event.textMissing,
                    width = event.width.toInt(),
                    height = event.height.toInt(),
                )
                "host_missing" -> {
                    guideHost(event.missing)
                    cur
                }
                "refused" -> cur.copy(connecting = false, active = false, refused = event.detail)
                else -> cur
            }
            all + (event.deviceId to next)
        }
    }

    /** Ask [device] for a session. */
    fun connect(node: UrDeviceNode, device: String) {
        _sessions.update { all -> all + (device to (all[device] ?: Session()).copy(connecting = true, refused = "")) }
        runCatching { node.inputConnect(device) }.onFailure {
            _sessions.update { all -> all + (device to (all[device] ?: Session()).copy(connecting = false, refused = "unsupported")) }
        }
    }

    /** End the session with [device]. */
    fun end(node: UrDeviceNode, device: String) {
        runCatching { node.inputEnd(device) }
        _sessions.update { all -> all + (device to (all[device] ?: Session()).copy(connecting = false, active = false)) }
    }

    @Composable
    override fun Screen(peer: FfiPeer, tile: String) {
        val node = Core.node() ?: return
        val all by sessions.collectAsState()
        val session = all[peer.id] ?: Session()
        // Open the session with the screen and end it when the screen closes.
        DisposableEffect(peer.id) {
            connect(node, peer.id)
            onDispose { end(node, peer.id) }
        }
        // The settings page keeps the session, so opening it does not ask the other device again.
        if (tile == MOUSEPAD_SETTINGS) {
            MousepadSettings()
            return
        }
        Column(
            if (fullScreen(tile)) Modifier.fillMaxSize() else Modifier,
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            SessionStatus(peer, session, onStart = { connect(node, peer.id) }, onStop = { end(node, peer.id) })
            when (tile) {
                "presenter" -> PresenterPad(node, peer.id, session.active)
                "keyboard" -> KeyboardPad(node, peer.id, session.active)
                else -> MousepadPad(node, peer.id, session.active, Modifier.weight(1f))
            }
        }
    }
}

/** One line about the session, with Start or Stop. Explains a missing permission on the desktop (R9.4). */
@Composable
private fun SessionStatus(peer: FfiPeer, s: InputFeature.Session, onStart: () -> Unit, onStop: () -> Unit) {
    val text = when {
        s.connecting -> stringResource(R.string.input_waiting, peer.name)
        s.refused.isNotEmpty() -> when (s.refused) {
            "denied" -> stringResource(R.string.input_denied, peer.name)
            "declined" -> stringResource(R.string.input_declined, peer.name)
            "timeout" -> stringResource(R.string.input_timeout, peer.name)
            "unsupported" -> stringResource(R.string.input_unsupported, peer.name)
            else -> stringResource(R.string.input_offline, peer.name)
        }
        s.active && s.status == "missing" -> when (s.missing) {
            "accessibility" -> stringResource(R.string.input_missing_accessibility, peer.name)
            "uinput" -> stringResource(R.string.input_missing_uinput, peer.name)
            "accessibility_service" -> stringResource(R.string.input_missing_accessibility_service, peer.name)
            "input_method" -> stringResource(R.string.input_missing_input_method, peer.name)
            else -> stringResource(R.string.input_missing_portal, peer.name)
        }
        s.active && s.status == "unavailable" -> stringResource(R.string.input_unavailable, peer.name)
        s.active -> stringResource(R.string.input_controlling, peer.name)
        else -> stringResource(R.string.input_stopped)
    }
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(text, style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
        if (s.active) {
            OutlinedButton(onClick = onStop, modifier = Modifier.heightIn(min = 44.dp)) { Text(stringResource(R.string.input_stop)) }
        } else if (!s.connecting) {
            Button(onClick = onStart, modifier = Modifier.heightIn(min = 44.dp)) { Text(stringResource(R.string.input_start)) }
        }
    }
}

/**
 * The host settings for this phone (#19): whether the accessibility service and the UrDevice
 * keyboard are on, with buttons that guide the user. Android only lets the user turn the service on,
 * so the first tap shows the Play disclosure (docs/spikes/t0.4-accessibility-play.md) and "Agree"
 * opens Android's Accessibility settings.
 */
@Composable
private fun HostRow() {
    val context = LocalContext.current
    var service by remember { mutableStateOf(InputHost.serviceEnabled(context)) }
    var keyboardOn by remember { mutableStateOf(InputHost.keyboardEnabled(context)) }
    var keyboardUsed by remember { mutableStateOf(InputHost.isDefaultKeyboard(context)) }
    var disclosure by remember { mutableStateOf(false) }
    // Android has no change callback for these settings; look again while the screen is open.
    LaunchedEffect(Unit) {
        while (true) {
            service = InputHost.serviceEnabled(context)
            keyboardOn = InputHost.keyboardEnabled(context)
            keyboardUsed = InputHost.isDefaultKeyboard(context)
            delay(1000)
        }
    }
    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.input_host_title), style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
        Text(
            stringResource(if (service) R.string.input_host_service_on else R.string.input_host_service_off),
            style = MaterialTheme.typography.bodyMedium,
        )
        if (!service) {
            Button(onClick = { disclosure = true }, modifier = Modifier.heightIn(min = 44.dp)) {
                Text(stringResource(R.string.input_host_enable_service))
            }
            Text(stringResource(R.string.input_host_restricted), style = MaterialTheme.typography.bodySmall)
            OutlinedButton(onClick = { InputHost.openAppInfo(context) }, modifier = Modifier.heightIn(min = 44.dp)) {
                Text(stringResource(R.string.input_host_app_info))
            }
            Text(stringResource(R.string.input_host_advanced_protection), style = MaterialTheme.typography.bodySmall)
        } else {
            Text(stringResource(R.string.input_host_sensitive), style = MaterialTheme.typography.bodySmall)
        }
        Text(
            stringResource(if (keyboardUsed) R.string.input_host_ime_on else R.string.input_host_ime_off),
            style = MaterialTheme.typography.bodyMedium,
        )
        if (!keyboardOn) {
            OutlinedButton(onClick = { InputHost.openKeyboardSettings(context) }, modifier = Modifier.heightIn(min = 44.dp)) {
                Text(stringResource(R.string.input_host_enable_keyboard))
            }
        } else if (!keyboardUsed) {
            OutlinedButton(onClick = { InputHost.showKeyboardPicker(context) }, modifier = Modifier.heightIn(min = 44.dp)) {
                Text(stringResource(R.string.input_host_pick_keyboard))
            }
        }
    }
    if (disclosure) {
        AlertDialog(
            onDismissRequest = { disclosure = false },
            title = { Text(stringResource(R.string.input_disclosure_title)) },
            text = { Text(stringResource(R.string.input_disclosure_body)) },
            confirmButton = {
                TextButton(onClick = {
                    disclosure = false
                    InputHost.openAccessibilitySettings(context)
                }) { Text(stringResource(R.string.input_disclosure_agree)) }
            },
            dismissButton = {
                TextButton(onClick = { disclosure = false }) { Text(stringResource(R.string.input_disclosure_not_now)) }
            },
        )
    }
}
