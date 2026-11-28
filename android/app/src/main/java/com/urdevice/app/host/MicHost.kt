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
import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import android.os.Bundle
import android.os.IBinder
import android.util.Log
import androidx.activity.ComponentActivity
import androidx.activity.result.contract.ActivityResultContracts
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.core.MicHostFfi
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * This phone's microphone for other devices (#28, R17.4, design §18.1).
 *
 * The core calls [start] on a worker thread after its permission engine allowed the listener
 * (the microphone is watched: there is always an activity, so the service notification shows who
 * is listening and a Stop button, R5.4, R17.6). [start] opens [MicConsentActivity], which asks
 * Android for the RECORD_AUDIO permission when it is missing and starts [MicStreamService] from a
 * visible screen (Android 14 only lets a microphone foreground service start from one). The
 * service records 48 kHz mono 16-bit PCM and hands each 20 ms chunk to the core. Android shows its
 * own microphone indicator too. Audio is never stored or logged.
 */
class MicHost(private val context: Context) : MicHostFfi {
    override fun start(session: ULong): String {
        val pending = MicStreams.Pending(session)
        MicStreams.pending[session] = pending
        MicStreams.askConsent(context, session)
        val answered = pending.done.await(CONSENT_WAIT_S, TimeUnit.SECONDS)
        MicStreams.pending.remove(session)
        MicStreams.cancelConsentNotification(context)
        if (!answered) return "consent_declined"
        return pending.result
    }

    override fun stop(session: ULong) {
        val c = MicStreams.current?.takeIf { it.session == session } ?: return
        c.stop()
        context.stopService(Intent(context, MicStreamService::class.java))
    }

    companion object {
        /** How long the permission dialog may stay open. */
        const val CONSENT_WAIT_S = 60L
    }
}

/** Recordings waiting for the microphone to open, and the one running. */
object MicStreams {
    private const val TAG = "UrDeviceMic"
    const val CHANNEL = "mic"
    const val NOTIF_CONSENT = 49_230
    const val NOTIF_SERVICE = 49_231

    class Pending(val session: ULong) {
        val done = CountDownLatch(1)
        @Volatile var result: String = "capture_failed"

        fun finish(r: String) {
            result = r
            done.countDown()
        }
    }

    val pending = ConcurrentHashMap<ULong, Pending>()

    @Volatile var current: MicRecording? = null

    fun channel(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.mic_channel), NotificationManager.IMPORTANCE_HIGH),
        )
    }

    /** Open the permission activity; a notification that opens it is posted too, in case Android blocks the start. */
    fun askConsent(ctx: Context, session: ULong) {
        val intent = Intent(ctx, MicConsentActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            .putExtra(MicConsentActivity.EXTRA_SESSION, session.toLong())
        runCatching { ctx.startActivity(intent) }
        val tap = PendingIntent.getActivity(ctx, 2, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(ctx.getString(R.string.mic_consent_title))
            .setContentText(ctx.getString(R.string.mic_consent_body))
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

/** Asks for the RECORD_AUDIO permission when needed, then starts the microphone service from a visible screen. */
class MicConsentActivity : ComponentActivity() {
    private var session: Long = 0

    private val ask = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted) startStream() else MicStreams.pending[session.toULong()]?.finish("consent_declined")
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        MicStreams.cancelConsentNotification(this)
        session = intent.getLongExtra(EXTRA_SESSION, 0)
        if (MicStreams.pending[session.toULong()] == null) {
            finish()
            return
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED) {
            startStream()
            finish()
        } else {
            ask.launch(Manifest.permission.RECORD_AUDIO)
        }
    }

    private fun startStream() {
        val svc = Intent(this, MicStreamService::class.java).putExtra(EXTRA_SESSION, session)
        runCatching { startForegroundService(svc) }.onFailure {
            MicStreams.log("microphone service did not start", it)
            MicStreams.pending[session.toULong()]?.finish("capture_failed")
        }
    }

    companion object {
        const val EXTRA_SESSION = "session"
    }
}

/** The `microphone` foreground service that owns the running recording. */
class MicStreamService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val session = intent?.getLongExtra(MicConsentActivity.EXTRA_SESSION, 0)?.toULong() ?: 0uL
        val pending = MicStreams.pending[session]
        MicStreams.channel(this)
        val n = Notification.Builder(this, MicStreams.CHANNEL)
            .setSmallIcon(R.drawable.ic_urdevice_symbol)
            .setContentTitle(getString(R.string.mic_streaming_title))
            .setOngoing(true)
            .build()
        startForeground(MicStreams.NOTIF_SERVICE, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE)
        if (pending == null) {
            stopSelf()
            return START_NOT_STICKY
        }
        MicStreams.current?.stop()
        val rec = MicRecording(session) { stopSelf() }
        val ok = runCatching { rec.start() }.onFailure { MicStreams.log("microphone did not start", it) }.getOrDefault(false)
        if (ok) {
            MicStreams.current = rec
            pending.finish("")
        } else {
            rec.stop()
            pending.finish("capture_failed")
            stopSelf()
        }
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        MicStreams.current?.stop()
        MicStreams.current = null
        super.onDestroy()
    }
}

/**
 * One recording: `AudioRecord` at 48 kHz, mono, 16-bit, read in 20 ms chunks on a thread of its
 * own and handed to the core. The permission is checked by the consent screen before this starts.
 */
class MicRecording(val session: ULong, private val onEnded: () -> Unit) {
    @Volatile private var running = false
    private var record: AudioRecord? = null
    private var thread: Thread? = null

    @Suppress("MissingPermission")
    fun start(): Boolean {
        val min = AudioRecord.getMinBufferSize(RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_16BIT)
        if (min <= 0) return false
        val r = AudioRecord(
            MediaRecorder.AudioSource.MIC,
            RATE,
            AudioFormat.CHANNEL_IN_MONO,
            AudioFormat.ENCODING_PCM_16BIT,
            maxOf(min, CHUNK_BYTES * 4),
        )
        if (r.state != AudioRecord.STATE_INITIALIZED) {
            r.release()
            return false
        }
        record = r
        running = true
        r.startRecording()
        thread = Thread(::pump, "mic-recorder").also { it.start() }
        return true
    }

    private fun pump() {
        val node = Core.node()
        val r = record ?: return
        val buf = ByteArray(CHUNK_BYTES)
        while (running) {
            // Fill a whole 20 ms chunk: a read may return less.
            var got = 0
            while (running && got < CHUNK_BYTES) {
                val n = r.read(buf, got, CHUNK_BYTES - got)
                if (n < 0) {
                    // The permission was taken away, or another app took the microphone.
                    if (running) {
                        node?.micCaptureEnded(session, "capture_failed")
                        stop()
                        onEnded()
                    }
                    return
                }
                got += n
            }
            if (running && got == CHUNK_BYTES) node?.micPushChunk(session, buf.copyOf())
        }
    }

    fun stop() {
        if (!running) return
        running = false
        record?.let { runCatching { it.stop() }; runCatching { it.release() } }
        record = null
        if (MicStreams.current === this) MicStreams.current = null
    }

    companion object {
        const val RATE = 48_000

        /** 20 ms of 16-bit mono samples. */
        const val CHUNK_BYTES = RATE / 50 * 2
    }
}
