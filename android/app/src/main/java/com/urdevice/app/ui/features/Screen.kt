package com.urdevice.app.ui.features

import android.content.Context
import android.graphics.SurfaceTexture
import android.media.MediaCodec
import android.media.MediaFormat
import android.os.Build
import android.view.Surface
import android.view.TextureView
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.app.host.ScreenCaptures
import com.urdevice.app.host.ScreenHost
import com.urdevice.core.FfiDisplay
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.FfiVideoFrame
import com.urdevice.core.ScreenListener
import com.urdevice.core.UrDeviceNode
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.MutableStateFlow

/**
 * Screen sharing (#21, R12, design §10).
 *
 * As a host, other devices watch this phone's screen ([ScreenHost]: MediaProjection and MediaCodec).
 * As a viewer, the screen below shows another device's screen: MediaCodec decodes straight to a
 * TextureView surface, pinch zooms and pans the picture, and a tap maps to a position on the
 * host's display for remote control (R12.5).
 */
object ScreenFeature : Feature {
    override val id = "screen"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("screen.h264.v1", "screen.h264.v1"))

    override val hasScreen get() = true

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        val app = context.applicationContext
        ScreenCaptures.channel(app)
        runCatching { node.screenSetHost(ScreenHost(app)) }
        runCatching { node.screenSetListener(ScreenViewer) }
    }

    @Composable
    override fun Screen(peer: FfiPeer, tile: String) {
        ScreenScreen(peer)
    }
}

/** Remote control from the picture needs the input service (#18), not on `main` yet. */
private const val INPUT_FORWARDING = false

/**
 * A tap at [x], [y] (fractions of the host's display). Hook for remote control (R12.5): once the
 * input service (#18) is on `main`, request an `input.v1` activity and send an `Abs` pointer event.
 */
@Suppress("UNUSED_PARAMETER")
private fun pointer(peer: String, x: Float, y: Float) {
    // TODO(#18): forward to the input service once it exists.
}

/** Viewer state and events from the core. Callbacks run on core threads; StateFlow is thread-safe. */
object ScreenViewer : ScreenListener {
    const val IDLE = 0
    const val ASKING = 1
    const val STARTING = 2
    const val SHOWING = 3

    val state = MutableStateFlow(IDLE)
    /** Why the last session stopped: a core reason (`declined`, `denied`, ...) or `ended`. */
    val reason = MutableStateFlow<String?>(null)
    val displays = MutableStateFlow<List<FfiDisplay>>(emptyList())
    val selected = MutableStateFlow(0)
    /** The picture size, from the last keyframe. */
    val size = MutableStateFlow(0 to 0)

    @Volatile var peer: String? = null
        private set
    @Volatile private var activity: ULong? = null
    @Volatile private var started = false
    val decoder = VideoDecoder { requestKeyframe() }

    fun start(peerId: String) {
        stop(null)
        peer = peerId
        reason.value = null
        state.value = ASKING
        if (runCatching { Core.node()?.screenRequestView(peerId) }.isFailure) finish("offline")
    }

    fun stop(why: String?) {
        val p = peer
        val a = activity
        if (p != null && a != null) runCatching { Core.node()?.screenStop(p, a) }
        finish(why)
    }

    private fun finish(why: String?) {
        activity = null
        started = false
        decoder.reset()
        displays.value = emptyList()
        size.value = 0 to 0
        reason.value = why
        state.value = IDLE
    }

    fun pick(index: Int) {
        val p = peer ?: return
        val a = activity ?: return
        val d = displays.value.getOrNull(index) ?: return
        selected.value = index
        state.value = STARTING
        runCatching { Core.node()?.screenStart(p, a, d.id, 0u, 0u, 0u) }
    }

    private fun requestKeyframe() {
        val p = peer ?: return
        val a = activity ?: return
        runCatching { Core.node()?.screenRequestKeyframe(p, a) }
    }

    override fun onOpened(deviceId: String, activity: ULong) {
        if (deviceId != peer || state.value != ASKING) {
            runCatching { Core.node()?.screenStop(deviceId, activity) }
            return
        }
        this.activity = activity
        state.value = STARTING
        runCatching { Core.node()?.screenListDisplays(deviceId, activity) }
    }

    override fun onDisplays(deviceId: String, activity: ULong, displays: List<FfiDisplay>) {
        if (deviceId != peer || activity != this.activity) return
        this.displays.value = displays
        if (displays.isEmpty()) {
            stop("no_host")
            return
        }
        if (!started) {
            started = true
            pick(displays.indexOfFirst { it.primary }.coerceAtLeast(0))
        }
    }

    override fun onStreamStarted(deviceId: String, activity: ULong, displayId: UInt) {
        if (deviceId == peer && activity == this.activity) decoder.reset()
    }

    override fun onFrame(deviceId: String, activity: ULong, frame: FfiVideoFrame) {
        if (deviceId != peer || activity != this.activity) return
        if (frame.keyframe && frame.width > 0u && frame.height > 0u) {
            size.value = frame.width.toInt() to frame.height.toInt()
            if (state.value == STARTING) state.value = SHOWING
        }
        decoder.offer(frame)
    }

    override fun onError(deviceId: String, activity: ULong, reason: String) {
        if (deviceId != peer) return
        val ours = (activity == 0uL && state.value == ASKING) || activity == this.activity
        if (ours) stop(reason)
    }

    override fun onEnded(deviceId: String, activity: ULong) {
        if (deviceId == peer && activity == this.activity) finish("ended")
    }
}

/**
 * MediaCodec H.264 decoder on its own thread, rendering to [surface]. The queue is small: when it
 * is full, frames up to the next keyframe are dropped and [wantKeyframe] asks the host for one.
 */
class VideoDecoder(private val wantKeyframe: () -> Unit) {
    private val queue = ArrayBlockingQueue<FfiVideoFrame>(6)
    @Volatile private var waitKey = true
    @Volatile private var resetCodec = false
    @Volatile var surface: Surface? = null
        set(value) {
            field = value
            resetCodec = true
            waitKey = true
            if (value != null) wantKeyframe()
        }

    init {
        Thread(::loop, "screen-decoder").apply { isDaemon = true }.start()
    }

    fun reset() {
        queue.clear()
        waitKey = true
        resetCodec = true
    }

    fun offer(f: FfiVideoFrame) {
        if (waitKey && !f.keyframe) return
        if (f.keyframe) waitKey = false
        if (!queue.offer(f)) {
            queue.clear()
            waitKey = true
            wantKeyframe()
        }
    }

    private fun loop() {
        var codec: MediaCodec? = null
        var codecSize = 0 to 0
        var needKey = true
        val info = MediaCodec.BufferInfo()
        while (true) {
            val f = queue.poll(100, TimeUnit.MILLISECONDS)
            if (resetCodec) {
                resetCodec = false
                codec?.let { runCatching { it.stop() }; runCatching { it.release() } }
                codec = null
                needKey = true
            }
            if (f == null) continue
            if (needKey && !f.keyframe) continue
            val s = surface ?: continue
            val w = f.width.toInt()
            val h = f.height.toInt()
            if (f.keyframe && w > 0 && h > 0 && (codec == null || codecSize != w to h)) {
                codec?.let { runCatching { it.stop() }; runCatching { it.release() } }
                codec = runCatching {
                    val fmt = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, w, h)
                    if (Build.VERSION.SDK_INT >= 30) fmt.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                    MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_VIDEO_AVC).apply {
                        configure(fmt, s, null, 0)
                        start()
                    }
                }.getOrNull()
                codecSize = w to h
            }
            val c = codec ?: continue
            needKey = false
            try {
                val idx = c.dequeueInputBuffer(50_000)
                if (idx < 0) {
                    needKey = true
                    wantKeyframe()
                    continue
                }
                val buf = c.getInputBuffer(idx) ?: continue
                buf.clear()
                buf.put(f.data)
                val flags = if (f.keyframe) MediaCodec.BUFFER_FLAG_KEY_FRAME else 0
                c.queueInputBuffer(idx, 0, f.data.size, f.ptsUs.toLong(), flags)
                while (true) {
                    val o = c.dequeueOutputBuffer(info, 0)
                    if (o >= 0) c.releaseOutputBuffer(o, true) else if (o == MediaCodec.INFO_TRY_AGAIN_LATER) break
                }
            } catch (e: Exception) {
                // A broken frame or a lost surface: start again at a keyframe.
                codec?.let { runCatching { it.release() } }
                codec = null
                needKey = true
                wantKeyframe()
            }
        }
    }
}

/** Map a tap on the zoomed picture back to fractions of the host's display, or null outside it. */
private fun toHost(tap: Offset, box: IntSize, scale: Float, offset: Offset): Offset? {
    if (box.width == 0 || box.height == 0) return null
    val cx = box.width / 2f
    val cy = box.height / 2f
    val x = (tap.x - cx - offset.x) / scale + cx
    val y = (tap.y - cy - offset.y) / scale + cy
    val fx = x / box.width
    val fy = y / box.height
    return if (fx in 0f..1f && fy in 0f..1f) Offset(fx, fy) else null
}

@Composable
private fun reasonText(reason: String, name: String): String = when (reason) {
    "declined" -> stringResource(R.string.screen_err_declined, name)
    "denied", "ask", "no_activity" -> stringResource(R.string.screen_err_denied, name)
    "offline" -> stringResource(R.string.screen_err_offline, name)
    "no_host", "unsupported", "no_display" -> stringResource(R.string.screen_err_no_host, name)
    "consent_declined" -> stringResource(R.string.screen_err_consent, name)
    "timeout" -> stringResource(R.string.screen_err_timeout, name)
    "ended", "stopped" -> stringResource(R.string.screen_ended)
    else -> stringResource(R.string.screen_err_failed, name)
}

@Composable
private fun ScreenScreen(peer: FfiPeer) {
    val state by ScreenViewer.state.collectAsState()
    val reason by ScreenViewer.reason.collectAsState()
    val displays by ScreenViewer.displays.collectAsState()
    val selected by ScreenViewer.selected.collectAsState()
    val size by ScreenViewer.size.collectAsState()
    val ours = ScreenViewer.peer == peer.id
    val shown = if (ours) state else ScreenViewer.IDLE

    // Leaving the screen ends the session.
    DisposableEffect(peer.id) {
        onDispose { if (ScreenViewer.peer == peer.id) ScreenViewer.stop(null) }
    }

    Column(Modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            if (shown == ScreenViewer.IDLE) {
                Button(onClick = { ScreenViewer.start(peer.id) }, enabled = peer.status == "connected") {
                    Text(stringResource(R.string.screen_view))
                }
            } else {
                OutlinedButton(onClick = { ScreenViewer.stop(null) }) { Text(stringResource(R.string.screen_stop)) }
            }
        }
        when {
            shown == ScreenViewer.ASKING -> Text(stringResource(R.string.screen_asking, peer.name))
            shown == ScreenViewer.STARTING -> Text(stringResource(R.string.screen_starting, peer.name))
            shown == ScreenViewer.IDLE && ours && reason != null ->
                Text(reasonText(reason!!, peer.name), color = MaterialTheme.colorScheme.error)
        }
        if (shown >= ScreenViewer.STARTING && displays.size > 1) {
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                displays.forEachIndexed { i, d ->
                    FilterChip(selected = i == selected, onClick = { ScreenViewer.pick(i) }, label = { Text(d.name) })
                }
            }
        }
        if (shown >= ScreenViewer.STARTING) {
            if (!INPUT_FORWARDING) {
                Text(stringResource(R.string.screen_view_only), style = MaterialTheme.typography.bodySmall)
            }
            Picture(peer.id, size)
        }
    }
}

@Composable
private fun Picture(peerId: String, size: Pair<Int, Int>) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    var box by remember { mutableStateOf(IntSize.Zero) }
    val ratio = if (size.first > 0 && size.second > 0) size.first.toFloat() / size.second else 16f / 9f
    Box(
        Modifier
            .fillMaxWidth()
            .aspectRatio(ratio)
            .clipToBounds()
            .background(Color.Black)
            .onSizeChanged { box = it }
            .pointerInput(Unit) {
                detectTransformGestures { _, pan, zoom, _ ->
                    scale = (scale * zoom).coerceIn(1f, 5f)
                    val maxX = (box.width * (scale - 1f)) / 2f
                    val maxY = (box.height * (scale - 1f)) / 2f
                    offset = Offset(
                        (offset.x + pan.x).coerceIn(-maxX, maxX),
                        (offset.y + pan.y).coerceIn(-maxY, maxY),
                    )
                }
            }
            .pointerInput(Unit) {
                detectTapGestures(
                    onDoubleTap = {
                        scale = 1f
                        offset = Offset.Zero
                    },
                    onTap = { pos ->
                        if (INPUT_FORWARDING) toHost(pos, box, scale, offset)?.let { pointer(peerId, it.x, it.y) }
                    },
                )
            },
    ) {
        AndroidView(
            factory = { ctx ->
                TextureView(ctx).apply {
                    surfaceTextureListener = object : TextureView.SurfaceTextureListener {
                        override fun onSurfaceTextureAvailable(st: SurfaceTexture, w: Int, h: Int) {
                            ScreenViewer.decoder.surface = Surface(st)
                        }

                        override fun onSurfaceTextureSizeChanged(st: SurfaceTexture, w: Int, h: Int) {}

                        override fun onSurfaceTextureDestroyed(st: SurfaceTexture): Boolean {
                            val s = ScreenViewer.decoder.surface
                            ScreenViewer.decoder.surface = null
                            s?.release()
                            return true
                        }

                        override fun onSurfaceTextureUpdated(st: SurfaceTexture) {}
                    }
                }
            },
            modifier = Modifier
                .fillMaxSize()
                .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
        )
    }
}
