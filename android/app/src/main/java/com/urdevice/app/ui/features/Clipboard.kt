package com.urdevice.app.ui.features

import android.content.ClipboardManager
import android.content.Context
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
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
import com.urdevice.app.R
import com.urdevice.app.host.ClipboardHost
import com.urdevice.app.host.ClipboardSender
import com.urdevice.core.ClipboardListener
import com.urdevice.core.FfiClipEvent
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.UrDeviceNode
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Clipboard sharing (#16, R7).
 *
 * Incoming items are written by [ClipboardHost]. Outgoing: while the app is open, a change of the
 * system clipboard is sent on its own; in the background Android blocks reading, so the user sends
 * with the notification action, the Quick Settings tile, the share sheet, or the button on this
 * screen. Per-device and global switches are kept in the core.
 */
object ClipboardFeature : Feature, ClipboardListener {
    override val id = "clipboard"

    private val _status = MutableStateFlow("")

    /** The last thing that happened, without any content. */
    val status: StateFlow<String> = _status

    /** Bumped when a setting changes, so the screens redraw. */
    private val _settings = MutableStateFlow(0)

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("clipboard.v1", "clipboard.v1"))

    override val hasScreen get() = true

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        node.clipboardSetHost(ClipboardHost(context))
        node.clipboardSetListener(this)
        // While the app is open Android lets it read the clipboard (R7.1, design §9.1).
        val cm = context.getSystemService(ClipboardManager::class.java)
        cm.addPrimaryClipChangedListener {
            if (Core.foreground.value && !node.clipboardPaused()) {
                ClipboardSender.sendCurrent(context, automatic = true)
            }
        }
    }

    override fun onEvent(event: FfiClipEvent) {
        val name = Core.snapshot.value?.peers?.firstOrNull { it.id == event.deviceId }?.name ?: ""
        val app = Core.appContext()
        val text = when (event.kind) {
            "received" -> app.getString(R.string.clip_status_received, event.len.toLong(), name)
            "sent" -> app.getString(R.string.clip_status_sent, event.len.toLong(), name)
            "skipped" -> when (event.reason) {
                "sensitive" -> app.getString(R.string.clip_sensitive)
                "too_large" -> app.getString(R.string.clip_too_large)
                else -> null
            }
            "refused" -> app.getString(R.string.clip_status_refused, name, event.reason)
            "settings" -> {
                _settings.value += 1
                null
            }
            else -> null
        }
        if (text != null) _status.value = text
    }

    @Composable
    override fun Screen(peer: FfiPeer, tile: String) {
        val node = Core.node() ?: return
        val ctx = LocalContext.current
        val tick by _settings.collectAsState()
        val status by status.collectAsState()
        var enabled by remember(peer.id, tick) { mutableStateOf(runCatching { node.clipboardPeerEnabled(peer.id) }.getOrDefault(true)) }
        val paused = remember(tick) { node.clipboardPaused() }
        Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(stringResource(R.string.clip_share_with, peer.name), fontWeight = FontWeight.SemiBold)
                    Text(stringResource(R.string.clip_share_body), style = MaterialTheme.typography.bodySmall)
                }
                Switch(checked = enabled, onCheckedChange = {
                    enabled = it
                    runCatching { node.clipboardSetPeerEnabled(peer.id, it) }
                })
            }
            if (paused) Text(stringResource(R.string.clip_paused_hint), style = MaterialTheme.typography.bodySmall)
            Button(
                onClick = { ClipboardSender.toast(ctx, ClipboardSender.sendCurrent(ctx, peer.id)) },
                enabled = enabled && !paused,
                modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.clip_send_now)) }
            Text(stringResource(R.string.clip_background_hint), style = MaterialTheme.typography.bodySmall)
            if (status.isNotEmpty()) Text(status, style = MaterialTheme.typography.bodySmall)
        }
    }

    @Composable
    override fun Settings() {
        val node = Core.node() ?: return
        val tick by _settings.collectAsState()
        var paused by remember(tick) { mutableStateOf(node.clipboardPaused()) }
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(stringResource(R.string.clip_pause), fontWeight = FontWeight.SemiBold)
                Text(stringResource(R.string.clip_pause_body), style = MaterialTheme.typography.bodySmall)
            }
            Switch(checked = paused, onCheckedChange = {
                paused = it
                runCatching { node.clipboardSetPaused(it) }
            })
        }
    }
}
