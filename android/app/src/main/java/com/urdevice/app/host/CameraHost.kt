package com.urdevice.app.host

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.content.pm.ServiceInfo
import android.graphics.Rect
import android.graphics.SurfaceTexture
import android.hardware.camera2.CameraAccessException
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CaptureRequest
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.Log
import android.util.Range
import android.util.Size
import android.view.Surface
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.core.CameraHostFfi
import com.urdevice.core.FfiDisplay
import com.urdevice.core.FfiVideoFrame
import com.urdevice.core.FfiVideoTarget
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * This phone's cameras for other devices (#28, R17.1, design §18.1).
 *
 * The core calls [start] on a worker thread after its permission engine allowed the viewer (camera
 * is watched: there is always an activity, so the service notification shows who is watching and a
 * Stop button, R5.4, R17.6). [start] opens [CameraConsentActivity], which asks Android for the
 * CAMERA permission when it is missing and starts [CameraStreamService] from a visible screen
 * (Android 14 only lets a camera foreground service start from one). The service owns a Camera2
 * session that draws into a MediaCodec H.264 input Surface, and hands each encoded frame to the
 * core. Android shows its own camera indicator too. Frames are never stored or logged.
 */
class CameraHost(private val context: Context) : CameraHostFfi {
    override fun cameras(): List<FfiDisplay> = CameraStreams.cameras(context).mapIndexed { i, c ->
        FfiDisplay(
            i.toUInt(),
            context.getString(if (c.back) R.string.camera_back else R.string.camera_front),
            0u,
            0u,
            c.back,
        )
    }

    override fun start(session: ULong, cameraId: UInt, target: FfiVideoTarget): String {
        if (CameraStreams.cameras(context).getOrNull(cameraId.toInt()) == null) return "no_display"
        val pending = CameraStreams.Pending(session, cameraId.toInt(), target)
        CameraStreams.pending[session] = pending
        CameraStreams.askConsent(context, session)
        val answered = pending.done.await(CONSENT_WAIT_S, TimeUnit.SECONDS)
        CameraStreams.pending.remove(session)
        CameraStreams.cancelConsentNotification(context)
        if (!answered) return "consent_declined"
        return pending.result
    }

    override fun requestKeyframe(session: ULong) {
        CameraStreams.current?.takeIf { it.session == session }?.requestKeyframe()
    }

    override fun setTarget(session: ULong, target: FfiVideoTarget) {
        CameraStreams.current?.takeIf { it.session == session }?.setTarget(target)
    }

    override fun control(session: ULong, flash: Boolean, zoomX100: UInt) {
        CameraStreams.current?.takeIf { it.session == session }?.control(flash, zoomX100.toInt())
    }

    override fun stop(session: ULong) {
        val c = CameraStreams.current?.takeIf { it.session == session } ?: return
        c.stop()
        context.stopService(Intent(context, CameraStreamService::class.java))
    }

    companion object {
        /** How long the permission dialog may stay open. */
        const val CONSENT_WAIT_S = 60L
    }
}

/** One camera of this phone. */
internal class CameraInfo(val id: String, val back: Boolean)

/** Streams waiting for the camera to open, and the one running. */
object CameraStreams {
    private const val TAG = "UrDeviceCamera"
    const val CHANNEL = "camera"
    const val NOTIF_CONSENT = 49_220
    const val NOTIF_SERVICE = 49_221

    class Pending(val session: ULong, val camera: Int, val target: FfiVideoTarget) {
        val done = CountDownLatch(1)
        @Volatile var result: String = "capture_failed"

        fun finish(r: String) {
            result = r
            done.countDown()
        }
    }

    val pending = ConcurrentHashMap<ULong, Pending>()

    @Volatile var current: CameraStream? = null

    /** The back camera first (it is the primary one), then the front camera, if the phone has them. */
    internal fun cameras(ctx: Context): List<CameraInfo> {
        val m = ctx.getSystemService(CameraManager::class.java)
        val out = ArrayList<CameraInfo>()
        val ids = runCatching { m.cameraIdList.toList() }.getOrDefault(emptyList())
        for (facing in listOf(CameraCharacteristics.LENS_FACING_BACK, CameraCharacteristics.LENS_FACING_FRONT)) {
            val id = ids.firstOrNull { runCatching { m.getCameraCharacteristics(it).get(CameraCharacteristics.LENS_FACING) }.getOrNull() == facing }
            if (id != null) out.add(CameraInfo(id, facing == CameraCharacteristics.LENS_FACING_BACK))
        }
        return out
    }

    fun channel(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.camera_channel), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /** Open the permission activity; a notification that opens it is posted too, in case Android blocks the start. */
    fun askConsent(ctx: Context, session: ULong) {
        val intent = Intent(ctx, CameraConsentActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            .putExtra(CameraConsentActivity.EXTRA_SESSION, session.toLong())
        runCatching { ctx.startActivity(intent) }
        val tap = PendingIntent.getActivity(ctx, 1, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(ctx.getString(R.string.camera_consent_title))
            .setContentText(ctx.getString(R.string.camera_consent_body))
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

/** Asks for the CAMERA permission when needed, then starts the camera service from a visible screen. */
class CameraConsentActivity : ComponentActivity() {
    private var session: Long = 0

    private val ask = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) startStream() else CameraStreams.pending[session.toULong()]?.finish("consent_declined")
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        CameraStreams.cancelConsentNotification(this)
        session = intent.getLongExtra(EXTRA_SESSION, 0)
        if (CameraStreams.pending[session.toULong()] == null) {
            finish()
            return
        }
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            startStream()
            finish()
        } else {
            ask.launch(Manifest.permission.CAMERA)
        }
    }

    private fun startStream() {
        val svc = Intent(this, CameraStreamService::class.java).putExtra(EXTRA_SESSION, session)
        runCatching { startForegroundService(svc) }.onFailure {
            CameraStreams.log("camera service did not start", it)
            CameraStreams.pending[session.toULong()]?.finish("capture_failed")
        }
    }

    companion object {
        const val EXTRA_SESSION = "session"
    }
}

/** The `camera` foreground service that owns the running stream. */
class CameraStreamService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val session = intent?.getLongExtra(CameraConsentActivity.EXTRA_SESSION, 0)?.toULong() ?: 0uL
        val pending = CameraStreams.pending[session]
        CameraStreams.channel(this)
        val n = Notification.Builder(this, CameraStreams.CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(getString(R.string.camera_streaming_title))
            .setOngoing(true)
            .build()
        startForeground(CameraStreams.NOTIF_SERVICE, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA)
        if (pending == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        val info = CameraStreams.cameras(this).getOrNull(pending.camera)
        if (info == null) {
            pending.finish("no_display")
            stopSelf()
            return START_NOT_STICKY
        }
        CameraStreams.current?.stop()
        val stream = CameraStream(this, session, info, pending.target) { stopSelf() }
        // Opening the camera waits for callbacks, so it must not run on the main thread.
        Thread {
            val ok = runCatching { stream.start() }.onFailure { CameraStreams.log("camera did not start", it) }.getOrDefault(false)
            if (ok) {
                CameraStreams.current = stream
                pending.finish("")
            } else {
                stream.stop()
                pending.finish("capture_failed")
                stopSelf()
            }
        }.start()
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        CameraStreams.current?.stop()
        CameraStreams.current = null
        super.onDestroy()
    }
}

/**
 * One camera stream: a Camera2 session draws into a MediaCodec H.264 encoder's input Surface (low
 * latency, no B-frames, CBR, a keyframe every 2 s or on request). A thread drains the encoder and
 * hands frames to the core; SPS/PPS go in front of each keyframe. The picture is as the sensor
 * delivers it (no rotation): hold the phone sideways for an upright picture.
 */
class CameraStream internal constructor(
    private val context: Context,
    val session: ULong,
    private val info: CameraInfo,
    target: FfiVideoTarget,
    private val onEnded: () -> Unit,
) {
    private val lock = Any()
    private val manager = context.getSystemService(CameraManager::class.java)
    private val chars: CameraCharacteristics = manager.getCameraCharacteristics(info.id)
    private val cameraThread = HandlerThread("camera-session").apply { start() }
    private val cameraHandler = Handler(cameraThread.looper)
    private val encoderThread = HandlerThread("camera-encoder").apply { start() }
    private val encoderHandler = Handler(encoderThread.looper)

    @Volatile private var running = false
    @Volatile private var target = target
    private var device: CameraDevice? = null
    private var captureSession: CameraCaptureSession? = null
    private var codec: MediaCodec? = null
    private var surface: Surface? = null
    private var csd: ByteArray? = null
    private var size = Size(0, 0)
    private var flash = false
    private var zoom = 100

    /** Open the camera and start streaming. Returns false when it could not start. */
    fun start(): Boolean {
        running = true
        size = pickSize(target)
        buildEncoder(size, target)
        val opened = CountDownLatch(1)
        var failed = false
        manager.openCamera(
            info.id,
            object : CameraDevice.StateCallback() {
                override fun onOpened(camera: CameraDevice) {
                    device = camera
                    opened.countDown()
                }

                override fun onDisconnected(camera: CameraDevice) {
                    camera.close()
                    if (!running) return
                    Core.node()?.screenCaptureEnded(session, "capture_failed")
                    stop()
                    onEnded()
                }

                override fun onError(camera: CameraDevice, error: Int) {
                    failed = true
                    camera.close()
                    opened.countDown()
                    if (running && device != null) {
                        Core.node()?.screenCaptureEnded(session, "capture_failed")
                        stop()
                        onEnded()
                    }
                }
            },
            cameraHandler,
        )
        if (!opened.await(5, TimeUnit.SECONDS) || failed || device == null) return false
        if (!createSession()) return false
        encoderHandler.post(::drain)
        return true
    }

    /** The largest size the camera offers inside the target box, preferring 16:9. */
    private fun pickSize(t: FfiVideoTarget): Size {
        val map = chars.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
        val sizes = map?.getOutputSizes(SurfaceTexture::class.java)?.toList().orEmpty()
        val mw = t.maxWidth.toInt().coerceAtLeast(16)
        val mh = t.maxHeight.toInt().coerceAtLeast(16)
        val (bw, bh) = if (mw >= mh) mw to mh else mh to mw
        val inside = sizes.filter { it.width <= bw && it.height <= bh && it.width % 2 == 0 && it.height % 2 == 0 }
        val pool = inside.ifEmpty { sizes.sortedBy { it.width * it.height }.take(1) }
        return pool.maxWithOrNull(
            compareBy<Size>({ if (it.width * 9 == it.height * 16) 1 else 0 }, { it.width * it.height }),
        ) ?: Size(1280, 720)
    }

    private fun buildEncoder(sz: Size, t: FfiVideoTarget) {
        val f = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, sz.width, sz.height)
        f.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
        f.setInteger(MediaFormat.KEY_BIT_RATE, t.bitrateBps.toInt())
        f.setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
        f.setInteger(MediaFormat.KEY_FRAME_RATE, t.fps.toInt().coerceAtLeast(1))
        f.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 2)
        f.setInteger(MediaFormat.KEY_PRIORITY, 0)
        f.setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
        if (Build.VERSION.SDK_INT >= 30) f.setInteger(MediaFormat.KEY_LATENCY, 1)
        val c = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
        c.configure(f, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        val s = c.createInputSurface()
        c.start()
        synchronized(lock) {
            codec = c
            surface = s
            csd = null
            size = sz
        }
    }

    /** Point the camera at the encoder's Surface. Returns false when the session did not open. */
    @Suppress("DEPRECATION")
    private fun createSession(): Boolean {
        val dev = device ?: return false
        val s = synchronized(lock) { surface } ?: return false
        val ready = CountDownLatch(1)
        var ok = false
        try {
            dev.createCaptureSession(
                listOf(s),
                object : CameraCaptureSession.StateCallback() {
                    override fun onConfigured(session: CameraCaptureSession) {
                        captureSession = session
                        ok = true
                        ready.countDown()
                    }

                    override fun onConfigureFailed(session: CameraCaptureSession) {
                        ready.countDown()
                    }
                },
                cameraHandler,
            )
        } catch (e: CameraAccessException) {
            return false
        }
        if (!ready.await(5, TimeUnit.SECONDS) || !ok) return false
        return applyRequest()
    }

    /** Send the repeating request with the current fps, flash, and zoom. */
    private fun applyRequest(): Boolean {
        val dev = device ?: return false
        val cs = captureSession ?: return false
        val s = synchronized(lock) { surface } ?: return false
        return try {
            val b = dev.createCaptureRequest(CameraDevice.TEMPLATE_RECORD)
            b.addTarget(s)
            fpsRange()?.let { b.set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, it) }
            if (flash && chars.get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true) {
                b.set(CaptureRequest.FLASH_MODE, CaptureRequest.FLASH_MODE_TORCH)
            }
            applyZoom(b)
            cs.setRepeatingRequest(b.build(), null, cameraHandler)
            true
        } catch (e: Exception) {
            CameraStreams.log("camera request failed", e)
            false
        }
    }

    private fun fpsRange(): Range<Int>? {
        val ranges = chars.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES) ?: return null
        val want = target.fps.toInt().coerceAtLeast(1)
        val fits = ranges.filter { it.upper <= want }
        // A steady rate (lower = upper) keeps the frame times even.
        return fits.filter { it.lower == it.upper }.maxByOrNull { it.upper }
            ?: fits.maxWithOrNull(compareBy({ it.upper }, { it.lower }))
            ?: ranges.minByOrNull { it.upper }
    }

    private fun applyZoom(b: CaptureRequest.Builder) {
        val ratio = zoom / 100f
        if (Build.VERSION.SDK_INT >= 30) {
            val range = chars.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE)
            val r = if (range != null) ratio.coerceIn(range.lower, range.upper) else 1f
            b.set(CaptureRequest.CONTROL_ZOOM_RATIO, r)
        } else {
            val active = chars.get(CameraCharacteristics.SENSOR_INFO_ACTIVE_ARRAY_SIZE) ?: return
            val max = chars.get(CameraCharacteristics.SCALER_AVAILABLE_MAX_DIGITAL_ZOOM) ?: 1f
            val r = ratio.coerceIn(1f, max)
            val w = (active.width() / r).toInt()
            val h = (active.height() / r).toInt()
            val x = active.centerX() - w / 2
            val y = active.centerY() - h / 2
            b.set(CaptureRequest.SCALER_CROP_REGION, Rect(x, y, x + w, y + h))
        }
    }

    private var pushed = 0L

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
                        val sz = synchronized(lock) { size }
                        val (w, h) = if (key) sz.width to sz.height else 0 to 0
                        pushed++
                        if (pushed == 1L) CameraStreams.log("first frame ${sz.width} x ${sz.height} key=$key")
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

    /** The ladder moved: a new bitrate, or a new size (a new encoder and a new camera session). */
    fun setTarget(t: FfiVideoTarget) {
        if (!running) return
        target = t
        val next = pickSize(t)
        if (next == synchronized(lock) { size }) {
            val b = Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_VIDEO_BITRATE, t.bitrateBps.toInt()) }
            synchronized(lock) { runCatching { codec?.setParameters(b) } }
            return
        }
        CameraStreams.log("size ${size.width} x ${size.height} -> ${next.width} x ${next.height}")
        runCatching {
            val old = synchronized(lock) { codec to surface }
            runCatching { captureSession?.stopRepeating() }
            runCatching { captureSession?.close() }
            captureSession = null
            buildEncoder(next, t)
            createSession()
            old.first?.let { runCatching { it.stop() }; runCatching { it.release() } }
            old.second?.release()
        }.onFailure { CameraStreams.log("encoder resize failed", it) }
    }

    /** Flash and zoom (ratio times 100) from the viewer. */
    fun control(flash: Boolean, zoomX100: Int) {
        if (!running) return
        this.flash = flash
        this.zoom = zoomX100.coerceIn(100, 1000)
        applyRequest()
    }

    fun stop() {
        if (!running) return
        running = false
        runCatching { captureSession?.stopRepeating() }
        runCatching { captureSession?.close() }
        captureSession = null
        runCatching { device?.close() }
        device = null
        synchronized(lock) {
            codec?.let { runCatching { it.stop() }; runCatching { it.release() } }
            codec = null
            surface?.release()
            surface = null
        }
        cameraThread.quitSafely()
        encoderThread.quitSafely()
        if (CameraStreams.current === this) CameraStreams.current = null
    }
}
