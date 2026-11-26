package com.urdevice.app.host

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.SystemClock
import android.os.IBinder
import android.os.Looper
import android.util.DisplayMetrics
import android.util.Log
import android.view.Display
import android.view.Surface
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.core.FfiDisplay
import com.urdevice.core.FfiVideoFrame
import com.urdevice.core.FfiVideoTarget
import com.urdevice.core.ScreenHostFfi
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * This phone's screen for other devices (#21, R12.2, R12.3, design §10).
 *
 * The core calls [start] on a worker thread after its permission engine allowed the viewer (screen
 * is watched: there is always an activity, so the service notification shows who is watching and
 * a Stop button, R5.4). [start] opens [ScreenConsentActivity], which shows Android's own
 * MediaProjection consent dialog every session, then [ScreenCaptureService] starts as a
 * `mediaProjection` foreground service, mirrors the screen into a MediaCodec input Surface, and
 * hands each encoded frame to the core. Android shows its own capture indicator too. Frames are
 * never stored or logged.
 */
class ScreenHost(private val context: Context) : ScreenHostFfi {
    override fun displays(): List<FfiDisplay> {
        val (w, h) = screenSize(context)
        return listOf(FfiDisplay(0u, Build.MODEL ?: "Phone", w.toUInt(), h.toUInt(), true))
    }

    override fun start(session: ULong, displayId: UInt, target: FfiVideoTarget): String {
        val pending = ScreenCaptures.Pending(session, target)
        ScreenCaptures.pending[session] = pending
        ScreenCaptures.askConsent(context, session)
        val answered = pending.done.await(CONSENT_WAIT_S, TimeUnit.SECONDS)
        ScreenCaptures.pending.remove(session)
        ScreenCaptures.cancelConsentNotification(context)
        if (!answered) return "consent_declined"
        return pending.result
    }

    override fun requestKeyframe(session: ULong) {
        ScreenCaptures.current?.takeIf { it.session == session }?.requestKeyframe()
    }

    override fun setTarget(session: ULong, target: FfiVideoTarget) {
        ScreenCaptures.current?.takeIf { it.session == session }?.setTarget(target)
    }

    override fun stop(session: ULong) {
        val c = ScreenCaptures.current?.takeIf { it.session == session } ?: return
        c.stop()
        context.stopService(Intent(context, ScreenCaptureService::class.java))
    }

    companion object {
        /** How long the consent dialog may stay open. */
        const val CONSENT_WAIT_S = 60L
    }
}

/** The physical screen size in pixels. */
internal fun screenSize(context: Context): Pair<Int, Int> {
    // The real size of the default display in its current rotation: it swaps when the phone turns.
    val display = context.getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
    val m = DisplayMetrics()
    @Suppress("DEPRECATION")
    display.getRealMetrics(m)
    return m.widthPixels to m.heightPixels
}

/** Scale `w` x `h` down to fit the target box, keeping the aspect ratio, in even numbers. */
internal fun fit(w: Int, h: Int, t: FfiVideoTarget): Pair<Int, Int> {
    val mw = t.maxWidth.toInt().coerceAtLeast(16)
    val mh = t.maxHeight.toInt().coerceAtLeast(16)
    // A portrait phone: fit the box turned the same way.
    val (bw, bh) = if (h > w) mh to mw else mw to mh
    val s = minOf(1.0, minOf(bw.toDouble() / w, bh.toDouble() / h))
    val sw = ((w * s).toInt().coerceAtLeast(16)) and 1.inv()
    val sh = ((h * s).toInt().coerceAtLeast(16)) and 1.inv()
    return sw to sh
}

/** Captures waiting for consent, and the one running. */
object ScreenCaptures {
    private const val TAG = "UrDeviceScreen"
    const val CHANNEL = "screen"
    const val NOTIF_CONSENT = 49_210
    const val NOTIF_SERVICE = 49_211

    class Pending(val session: ULong, val target: FfiVideoTarget) {
        val done = CountDownLatch(1)
        @Volatile var result: String = "capture_failed"

        fun finish(r: String) {
            result = r
            done.countDown()
        }
    }

    val pending = ConcurrentHashMap<ULong, Pending>()

    @Volatile var current: CaptureSession? = null

    fun channel(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.screen_channel), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /**
     * Open the consent activity. Android may not let a background app start an activity, so a
     * notification that opens it is posted as well; the activity removes it.
     */
    fun askConsent(ctx: Context, session: ULong) {
        val intent = Intent(ctx, ScreenConsentActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            .putExtra(ScreenConsentActivity.EXTRA_SESSION, session.toLong())
        runCatching { ctx.startActivity(intent) }
        val tap = PendingIntent.getActivity(ctx, 0, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(ctx.getString(R.string.screen_consent_title))
            .setContentText(ctx.getString(R.string.screen_consent_body))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .build()
        runCatching { ctx.getSystemService(NotificationManager::class.java).notify(NOTIF_CONSENT, n) }
    }

    fun cancelConsentNotification(ctx: Context) {
        runCatching { ctx.getSystemService(NotificationManager::class.java).cancel(NOTIF_CONSENT) }
    }

    fun log(msg: String, e: Throwable? = null) {
        Log.i(TAG, msg, e)
    }
}

/** Shows Android's MediaProjection consent dialog for one session, then closes. */
class ScreenConsentActivity : ComponentActivity() {
    private var session: Long = 0

    private val ask = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { r ->
        val pending = ScreenCaptures.pending[session.toULong()]
        if (r.resultCode == Activity.RESULT_OK && r.data != null && pending != null) {
            val svc = Intent(this, ScreenCaptureService::class.java)
                .putExtra(EXTRA_SESSION, session)
                .putExtra(ScreenCaptureService.EXTRA_CODE, r.resultCode)
                .putExtra(ScreenCaptureService.EXTRA_DATA, r.data)
            runCatching { startForegroundService(svc) }.onFailure {
                ScreenCaptures.log("capture service did not start", it)
                pending.finish("capture_failed")
            }
        } else {
            pending?.finish("consent_declined")
        }
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        ScreenCaptures.cancelConsentNotification(this)
        session = intent.getLongExtra(EXTRA_SESSION, 0)
        if (ScreenCaptures.pending[session.toULong()] == null) {
            finish()
            return
        }
        val mpm = getSystemService(MediaProjectionManager::class.java)
        ask.launch(mpm.createScreenCaptureIntent())
    }

    companion object {
        const val EXTRA_SESSION = "session"
    }
}

/** The `mediaProjection` foreground service that owns the running capture. */
class ScreenCaptureService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val session = intent?.getLongExtra(ScreenConsentActivity.EXTRA_SESSION, 0)?.toULong() ?: 0uL
        val pending = ScreenCaptures.pending[session]
        val n = Notification.Builder(this, ScreenCaptures.CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(getString(R.string.screen_sharing_title))
            .setOngoing(true)
            .build()
        // Android 14+: the foreground service must run before the projection is created.
        startForeground(ScreenCaptures.NOTIF_SERVICE, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        if (pending == null || intent == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        val code = intent.getIntExtra(EXTRA_CODE, 0)
        val data: Intent? = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(EXTRA_DATA, Intent::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra(EXTRA_DATA)
        }
        val mpm = getSystemService(MediaProjectionManager::class.java)
        val projection = data?.let { runCatching { mpm.getMediaProjection(code, it) }.getOrNull() }
        if (projection == null) {
            pending.finish("capture_failed")
            stopSelf()
            return START_NOT_STICKY
        }
        ScreenCaptures.current?.stop()
        val capture = CaptureSession(this, session, projection, pending.target) {
            // Ended by the system (Stop in the status bar) or by an error.
            stopSelf()
        }
        val ok = runCatching { capture.start() }.onFailure { ScreenCaptures.log("capture did not start", it) }.isSuccess
        if (ok) {
            ScreenCaptures.current = capture
            pending.finish("")
        } else {
            capture.stop()
            pending.finish("capture_failed")
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        ScreenCaptures.current?.stop()
        ScreenCaptures.current = null
        super.onDestroy()
    }

    companion object {
        const val EXTRA_CODE = "code"
        const val EXTRA_DATA = "data"
    }
}

/**
 * One capture: MediaProjection mirrors the screen into a MediaCodec H.264 encoder's input Surface
 * (low latency, no B-frames, CBR, a keyframe every 2 s or on request). A thread drains the encoder
 * and hands frames to the core; SPS/PPS go in front of each keyframe.
 */
class CaptureSession(
    private val context: Context,
    val session: ULong,
    private val projection: MediaProjection,
    target: FfiVideoTarget,
    private val onEnded: () -> Unit,
) {
    private val lock = Any()
    private val thread = HandlerThread("screen-encoder").apply { start() }
    private val handler = Handler(thread.looper)
    @Volatile private var running = false
    @Volatile private var target = target
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var display: VirtualDisplay? = null
    private var csd: ByteArray? = null
    private var size = 0 to 0

    private val callback = object : MediaProjection.Callback() {
        override fun onStop() {
            if (!running) return
            Core.node()?.screenCaptureEnded(session, "stopped")
            stop()
            onEnded()
        }
    }

    /** The phone turned: the virtual display and the encoder take the new shape (design §10). */
    private val rotation = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) {}

        override fun onDisplayRemoved(displayId: Int) {}

        override fun onDisplayChanged(displayId: Int) {
            if (displayId == Display.DEFAULT_DISPLAY && running) setTarget(target)
        }
    }

    fun start() {
        running = true
        // On the main thread: the encoder thread is busy draining.
        context.getSystemService(DisplayManager::class.java)
            .registerDisplayListener(rotation, Handler(Looper.getMainLooper()))
        // Android 14+: register the callback before creating the virtual display.
        projection.registerCallback(callback, Handler(Looper.getMainLooper()))
        val (w, h) = screenSize(context)
        size = fit(w, h, target)
        val c = newCodec(size, target)
        val s = c.createInputSurface()
        c.start()
        synchronized(lock) {
            codec = c
            surface = s
        }
        val dpi = context.resources.displayMetrics.densityDpi
        display = projection.createVirtualDisplay(
            "UrDevice", size.first, size.second, dpi,
            DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR, s, null, null,
        )
        handler.post(::drain)
    }

    private fun newCodec(sz: Pair<Int, Int>, t: FfiVideoTarget): MediaCodec {
        val f = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, sz.first, sz.second)
        f.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
        f.setInteger(MediaFormat.KEY_BIT_RATE, t.bitrateBps.toInt())
        f.setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
        f.setInteger(MediaFormat.KEY_FRAME_RATE, t.fps.toInt())
        f.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
        f.setInteger(MediaFormat.KEY_PRIORITY, 0)
        f.setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
        // A still screen still sends a frame now and then, so a late viewer gets a picture.
        f.setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 100_000L)
        if (Build.VERSION.SDK_INT >= 30) f.setInteger(MediaFormat.KEY_LATENCY, 1)
        val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        c.configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        return c
    }

    private var pushed = 0L
    private var lastPushAt = 0L
    private var maxGapMs = 0L
    private var windowStart = 0L
    private var windowFrames = 0
    private var windowBytes = 0L

    private fun drain() {
        val info = MediaCodec.BufferInfo()
        val node = Core.node()
        while (running) {
            val c = synchronized(lock) { codec } ?: break
            val idx = try {
                c.dequeueOutputBuffer(info, 20_000)
            } catch (e: IllegalStateException) {
                // The codec is being replaced (the ladder moved).
                Thread.sleep(5)
                continue
            }
            if (idx < 0) continue
            try {
                val buf = c.getOutputBuffer(idx)
                if (buf != null && info.size > 0) {
                    val bytes = ByteArray(info.size)
                    buf.position(info.offset)
                    buf.get(bytes, 0, info.size)
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        csd = bytes
                    } else {
                        val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                        val data = if (key) (csd ?: ByteArray(0)) + bytes else bytes
                        val (w, h) = if (key) size else 0 to 0
                        pushed++
                        if (pushed == 1L || pushed % 600L == 0L) Log.i("UrDeviceScreen", "frames pushed=$pushed key=$key size=$w x $h")
                        // Timing for the log: frames and bytes in two seconds, and the longest wait.
                        val now = SystemClock.elapsedRealtime()
                        if (lastPushAt != 0L) maxGapMs = maxOf(maxGapMs, now - lastPushAt)
                        lastPushAt = now
                        windowFrames++
                        windowBytes += bytes.size
                        if (now - windowStart >= 2_000) {
                            Log.i("UrDeviceScreen", "encoded frames=$windowFrames bytes=$windowBytes longest_gap_ms=$maxGapMs")
                            windowStart = now
                            windowFrames = 0
                            windowBytes = 0
                            maxGapMs = 0
                        }
                        node?.screenPushFrame(
                            session,
                            FfiVideoFrame(info.presentationTimeUs.toULong(), key, data, w.toUInt(), h.toUInt()),
                        )
                    }
                }
                c.releaseOutputBuffer(idx, false)
            } catch (e: IllegalStateException) {
                // Released while draining.
            }
        }
    }

    fun requestKeyframe() {
        val b = Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) }
        synchronized(lock) { runCatching { codec?.setParameters(b) } }
    }

    fun setTarget(t: FfiVideoTarget) {
        if (!running) return
        target = t
        val (w, h) = screenSize(context)
        val next = fit(w, h, t)
        if (next == size) {
            val b = Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, t.bitrateBps.toInt()) }
            synchronized(lock) { runCatching { codec?.setParameters(b) } }
            return
        }
        // A new size needs a new encoder and Surface; the virtual display moves to it.
        Log.i("UrDeviceScreen", "size ${size.first} x ${size.second} -> ${next.first} x ${next.second}")
        runCatching {
            val c = newCodec(next, t)
            val s = c.createInputSurface()
            c.start()
            val old = synchronized(lock) {
                val o = codec to surface
                codec = c
                surface = s
                csd = null
                size = next
                o
            }
            val dpi = context.resources.displayMetrics.densityDpi
            display?.resize(next.first, next.second, dpi)
            display?.surface = s
            old.first?.let { runCatching { it.stop() }; runCatching { it.release() } }
            old.second?.release()
        }.onFailure { ScreenCaptures.log("encoder resize failed", it) }
    }

    fun stop() {
        if (!running) return
        running = false
        runCatching { context.getSystemService(DisplayManager::class.java).unregisterDisplayListener(rotation) }
        runCatching { display?.release() }
        display = null
        synchronized(lock) {
            codec?.let { runCatching { it.stop() }; runCatching { it.release() } }
            codec = null
            surface?.release()
            surface = null
        }
        runCatching { projection.unregisterCallback(callback) }
        runCatching { projection.stop() }
        thread.quitSafely()
        if (ScreenCaptures.current === this) ScreenCaptures.current = null
    }
}
