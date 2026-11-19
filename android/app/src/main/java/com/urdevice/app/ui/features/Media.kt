package com.urdevice.app.ui.features

import android.content.Context
import android.content.Intent
import android.graphics.BitmapFactory
import android.provider.Settings
import androidx.compose.foundation.Image
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
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material.icons.filled.VolumeOff
import androidx.compose.material.icons.filled.VolumeUp
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.app.host.MediaHost
import com.urdevice.app.host.MediaListenerService
import com.urdevice.core.FfiMediaCmd
import com.urdevice.core.FfiMediaSession
import com.urdevice.core.FfiMediaState
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.FfiPlayState
import com.urdevice.core.FfiVolumeChange
import com.urdevice.core.MediaListener
import com.urdevice.core.UrDeviceNode
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update

/**
 * Media control (#20, R11, design §9.4, §13): the media remote for another device, and the host
 * adapter that lets other devices control this phone.
 *
 * The screen shows one tab per media app on the host, a seek bar, only the controls the session
 * supports, the host's volume and mute, and an on/off switch. Turning it off tells the host to stop
 * sending updates (R11.6).
 */
object MediaFeature : Feature {
    override val id = "media"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("media.v1", "media.v1"))

    override val hasScreen get() = true

    /** Bumped when the core has news, so the screen reads the state again now. */
    private val updates = MutableStateFlow(0)

    /** The last refusal from a host (`denied`, `declined`, ...), until the next state arrives. */
    private val refused = MutableStateFlow<String?>(null)

    /** Hosts the user turned the player off for, until the app closes. */
    private val off = HashSet<String>()

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        // This phone's sessions and volume, for other devices.
        node.mediaSetHost(MediaHost(context))
        // State from hosts this phone controls. Called on core threads; StateFlow is thread-safe.
        node.mediaSetListener(object : MediaListener {
            override fun onUpdated(deviceId: String) {
                refused.value = null
                updates.update { it + 1 }
            }

            override fun onRefused(deviceId: String, reason: String) {
                refused.value = reason
                updates.update { it + 1 }
            }
        })
    }

    @Composable
    override fun Screen(peer: FfiPeer, tile: String) {
        MediaScreen(peer)
    }

    internal fun isOff(peer: String) = peer in off

    internal fun setOff(peer: String, value: Boolean) {
        if (value) off.add(peer) else off.remove(peer)
    }

    internal val updateFlow get() = updates
    internal val refusedFlow get() = refused
}

@Composable
private fun MediaScreen(peer: FfiPeer) {
    val context = LocalContext.current
    val node = Core.node()
    var on by remember(peer.id) { mutableStateOf(!MediaFeature.isOff(peer.id)) }
    var state by remember(peer.id) { mutableStateOf<FfiMediaState?>(null) }
    var selected by remember(peer.id) { mutableStateOf("") }
    val tick by MediaFeature.updateFlow.collectAsStateCompat()
    val refusal by MediaFeature.refusedFlow.collectAsStateCompat()
    val connected = peer.status == "connected"
    val denied = peer.remotePermissions.any { it.capability == "media.v1" && it.mode == FfiMode.DENY }

    // Turn the player on for this host while the screen is open and the switch is on.
    LaunchedEffect(peer.id, on, connected) {
        if (on && connected) runCatching { node?.mediaSubscribe(peer.id, true) }
    }
    // Read the newest state twice a second, and at once when the core says there is news. The core
    // moves the position of a playing session forward, so the seek bar keeps running.
    LaunchedEffect(peer.id, on, tick) {
        while (true) {
            state = if (on && connected && !denied) node?.mediaState(peer.id) else null
            delay(500)
        }
    }

    Column(
        Modifier.verticalScroll(rememberScrollState()).padding(top = 4.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        ListenerAccessCard(context)
        Row(verticalAlignment = Alignment.CenterVertically) {
            Switch(
                checked = on,
                onCheckedChange = {
                    on = it
                    MediaFeature.setOff(peer.id, !it)
                    runCatching { node?.mediaSubscribe(peer.id, it) }
                    if (!it) state = null
                },
            )
            Spacer(Modifier.width(12.dp))
            Text(stringResource(R.string.media_show, peer.name), Modifier.weight(1f))
        }
        val st = state
        val message = when {
            !connected -> stringResource(R.string.media_offline, peer.name)
            denied -> stringResource(R.string.media_denied, peer.name)
            !on -> stringResource(R.string.media_off)
            st == null && refusal != null -> stringResource(R.string.media_refused, peer.name)
            st == null -> stringResource(R.string.media_waiting, peer.name)
            st.unavailable == "listener_permission" -> stringResource(R.string.media_listener_off, peer.name)
            st.sessions.isEmpty() && st.unavailable.isNotEmpty() -> stringResource(R.string.media_unsupported, peer.name)
            st.sessions.isEmpty() -> stringResource(R.string.media_empty, peer.name)
            else -> null
        }
        if (message != null) {
            Text(message, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (st != null && on && connected && !denied) {
            if (st.sessions.isNotEmpty()) {
                val current = st.sessions.firstOrNull { it.id == selected } ?: st.sessions.first()
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
                    st.sessions.forEach { s ->
                        FilterChip(
                            selected = s.id == current.id,
                            onClick = { selected = s.id },
                            label = { Text(s.appName, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                            modifier = Modifier.heightIn(min = 44.dp),
                        )
                    }
                }
                SessionCard(peer, node, current)
            }
            if (st.canVolume) VolumeCard(peer, node, st)
        }
    }
}

/** The tick flow as Compose state without pulling in the lifecycle artifact's helpers. */
@Composable
private fun <T> MutableStateFlow<T>.collectAsStateCompat(): androidx.compose.runtime.State<T> {
    val s = remember(this) { mutableStateOf(value) }
    LaunchedEffect(this) { collect { s.value = it } }
    return s
}

@Composable
private fun SessionCard(peer: FfiPeer, node: UrDeviceNode?, s: FfiMediaSession) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Row(horizontalArrangement = Arrangement.spacedBy(16.dp), verticalAlignment = Alignment.CenterVertically) {
                Artwork(s.artworkPng)
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                    Text(
                        s.title.ifEmpty { stringResource(R.string.media_unknown_title) },
                        fontWeight = FontWeight.Bold,
                        style = MaterialTheme.typography.titleMedium,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                    if (s.artist.isNotEmpty()) {
                        Text(s.artist, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    }
                    Text(s.appName, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            val duration = s.durationMs?.toLong()
            if (s.canSeek && duration != null && duration > 0) {
                SeekBar(s, duration) { ms ->
                    runCatching { node?.mediaCommand(peer.id, s.id, FfiMediaCmd.SEEK, ms.toULong()) }
                }
            }
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                if (s.canPrev) {
                    IconButton(
                        onClick = { runCatching { node?.mediaCommand(peer.id, s.id, FfiMediaCmd.PREV, 0uL) } },
                        modifier = Modifier.size(48.dp),
                    ) { Icon(Icons.Filled.SkipPrevious, stringResource(R.string.media_previous)) }
                }
                if (s.canPlayPause) {
                    Spacer(Modifier.width(8.dp))
                    val playing = s.state == FfiPlayState.PLAYING
                    FilledIconButton(
                        onClick = { runCatching { node?.mediaCommand(peer.id, s.id, FfiMediaCmd.PLAY_PAUSE, 0uL) } },
                        modifier = Modifier.size(56.dp),
                    ) {
                        Icon(
                            if (playing) Icons.Filled.Pause else Icons.Filled.PlayArrow,
                            stringResource(if (playing) R.string.media_pause else R.string.media_play),
                        )
                    }
                    Spacer(Modifier.width(8.dp))
                }
                if (s.canNext) {
                    IconButton(
                        onClick = { runCatching { node?.mediaCommand(peer.id, s.id, FfiMediaCmd.NEXT, 0uL) } },
                        modifier = Modifier.size(48.dp),
                    ) { Icon(Icons.Filled.SkipNext, stringResource(R.string.media_next)) }
                }
            }
        }
    }
}

@Composable
private fun Artwork(png: ByteArray) {
    val bitmap = remember(png) {
        if (png.isEmpty()) null else runCatching { BitmapFactory.decodeByteArray(png, 0, png.size) }.getOrNull()
    }
    Box(
        Modifier.size(96.dp),
        contentAlignment = Alignment.Center,
    ) {
        if (bitmap != null) {
            Image(
                bitmap.asImageBitmap(), null,
                modifier = Modifier.size(96.dp),
                contentScale = ContentScale.Fit,
            )
        } else {
            Icon(Icons.Outlined.PlayCircle, null, modifier = Modifier.size(48.dp), tint = MaterialTheme.colorScheme.primary)
        }
    }
}

@Composable
private fun SeekBar(s: FfiMediaSession, durationMs: Long, onSeek: (Long) -> Unit) {
    var dragging by remember { mutableStateOf(false) }
    var drag by remember { mutableFloatStateOf(0f) }
    val position = (s.positionMs?.toLong() ?: 0L).coerceIn(0, durationMs)
    val shown = if (dragging) drag else position.toFloat() / durationMs
    Column {
        Slider(
            value = shown.coerceIn(0f, 1f),
            onValueChange = { dragging = true; drag = it },
            onValueChangeFinished = {
                dragging = false
                onSeek((drag * durationMs).toLong())
            },
            modifier = Modifier.heightIn(min = 44.dp),
        )
        Row(Modifier.fillMaxWidth()) {
            Text(formatMs((shown * durationMs).toLong()), style = MaterialTheme.typography.bodySmall)
            Spacer(Modifier.weight(1f))
            Text(formatMs(durationMs), style = MaterialTheme.typography.bodySmall)
        }
    }
}

@Composable
private fun VolumeCard(peer: FfiPeer, node: UrDeviceNode?, st: FfiMediaState) {
    var dragging by remember { mutableStateOf(false) }
    var drag by remember { mutableFloatStateOf(0f) }
    var lastSent by remember { mutableStateOf(0L) }
    Card(Modifier.fillMaxWidth()) {
        Row(Modifier.padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(
                onClick = { runCatching { node?.mediaVolume(peer.id, FfiVolumeChange.Mute(!st.muted)) } },
                modifier = Modifier.size(48.dp),
            ) {
                Icon(
                    if (st.muted) Icons.Filled.VolumeOff else Icons.Filled.VolumeUp,
                    stringResource(if (st.muted) R.string.media_unmute else R.string.media_mute),
                )
            }
            val shown = if (dragging) drag else if (st.muted) 0f else st.volume
            Slider(
                value = shown.coerceIn(0f, 1f),
                onValueChange = {
                    dragging = true
                    drag = it
                    // At most one message every 150 ms while dragging.
                    val now = System.currentTimeMillis()
                    if (now - lastSent >= 150) {
                        lastSent = now
                        runCatching { node?.mediaVolume(peer.id, FfiVolumeChange.Set(it)) }
                    }
                },
                onValueChangeFinished = {
                    dragging = false
                    runCatching { node?.mediaVolume(peer.id, FfiVolumeChange.Set(drag)) }
                },
                modifier = Modifier.weight(1f).heightIn(min = 44.dp),
            )
            Text(
                "${(shown * 100).toInt()}%",
                style = MaterialTheme.typography.bodySmall,
                modifier = Modifier.width(44.dp).padding(end = 8.dp),
            )
        }
    }
}

/** R11.4: say why this phone needs notification access, and open the settings page. */
@Composable
private fun ListenerAccessCard(context: Context) {
    var enabled by remember { mutableStateOf(MediaListenerService.isEnabled(context)) }
    LaunchedEffect(Unit) {
        while (true) {
            enabled = MediaListenerService.isEnabled(context)
            delay(1500)
        }
    }
    if (enabled) return
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.media_why_title), fontWeight = FontWeight.SemiBold)
            Text(stringResource(R.string.media_why_body), style = MaterialTheme.typography.bodySmall)
            Button(
                onClick = {
                    context.startActivity(
                        Intent(Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    )
                },
                modifier = Modifier.heightIn(min = 44.dp),
            ) { Text(stringResource(R.string.media_grant)) }
        }
    }
}

private fun formatMs(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    val h = s / 3600
    val m = s / 60 % 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}
