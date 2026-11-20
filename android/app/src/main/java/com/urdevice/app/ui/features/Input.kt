package com.urdevice.app.ui.features

import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.core.FfiInputEvent
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.InputListener
import com.urdevice.core.UrDeviceNode
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/**
 * Remote control from this phone: Mousepad, Presenter, and Keyboard (#18, R9, R9a, R9b, R10).
 *
 * The three tiles open one session each. Opening a screen asks the desktop for a remote-control
 * activity (the desktop's user approves it under Always Ask and sees the indicator with Stop);
 * leaving the screen ends it. The pointer, key, and text events go through the core, which groups
 * them every 8 ms. Android hosts and desktop-to-desktop control are #19.
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

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        node.inputSetListener(this)
    }

    override fun onEvent(event: FfiInputEvent) {
        _sessions.update { all ->
            val cur = all[event.deviceId] ?: Session()
            val next = when (event.kind) {
                "session" ->
                    if (event.detail == "start") cur.copy(connecting = false, active = true, refused = "", status = "", missing = "")
                    else cur.copy(connecting = false, active = false, status = "")
                "status" -> cur.copy(status = event.detail, missing = event.missing)
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
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            SessionStatus(peer, session, onStart = { connect(node, peer.id) }, onStop = { end(node, peer.id) })
            when (tile) {
                "presenter" -> PresenterPad(node, peer.id, session.active)
                "keyboard" -> KeyboardPad(node, peer.id, session.active)
                else -> MousepadPad(node, peer.id, session.active)
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
