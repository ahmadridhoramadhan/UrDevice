package com.urdevice.app.host

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Intent
import android.graphics.Path
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.accessibility.AccessibilityEvent
import java.util.ArrayDeque

/**
 * Taps, swipes, and Back, Home, Recents for a paired device that controls this phone (#19, R9.3).
 *
 * Gesture only: it never reads window content or events (see the Play spike), and it acts only on
 * what the core hands it after the user approved the session on this phone. Gestures run one at
 * a time, because Android cancels a gesture that another one interrupts.
 */
class UrDeviceAccessibilityService : AccessibilityService() {
    private val main = Handler(Looper.getMainLooper())
    private val queue = ArrayDeque<GestureDescription>()
    private var running = false

    override fun onServiceConnected() {
        instance = this
    }

    override fun onUnbind(intent: Intent?): Boolean {
        instance = null
        main.post {
            queue.clear()
            running = false
        }
        return super.onUnbind(intent)
    }

    override fun onAccessibilityEvent(event: AccessibilityEvent?) = Unit

    override fun onInterrupt() = Unit

    /** Queue a stroke through [xs] and [ys] (pixels) taking [durationMs]. Thread-safe. */
    fun stroke(xs: IntArray, ys: IntArray, durationMs: Long): Boolean {
        if (xs.isEmpty() || xs.size != ys.size) return false
        val path = Path().apply {
            moveTo(xs[0].toFloat(), ys[0].toFloat())
            if (xs.size == 1) lineTo(xs[0].toFloat(), ys[0].toFloat())
            for (i in 1 until xs.size) lineTo(xs[i].toFloat(), ys[i].toFloat())
        }
        val max = GestureDescription.getMaxGestureDuration()
        val gesture = try {
            GestureDescription.Builder()
                .addStroke(GestureDescription.StrokeDescription(path, 0, durationMs.coerceIn(1, max)))
                .build()
        } catch (e: IllegalArgumentException) {
            return false
        }
        main.post {
            if (queue.size < MAX_QUEUE) queue.add(gesture)
            next()
        }
        return true
    }

    /** Run a global action: 0 Back, 1 Home, 2 Recents. */
    fun global(action: Int): Boolean = when (action) {
        0 -> performGlobalAction(GLOBAL_ACTION_BACK)
        1 -> performGlobalAction(GLOBAL_ACTION_HOME)
        2 -> performGlobalAction(GLOBAL_ACTION_RECENTS)
        else -> false
    }

    private fun next() {
        if (running) return
        val g = queue.poll() ?: return
        running = true
        val done = object : GestureResultCallback() {
            override fun onCompleted(gestureDescription: GestureDescription?) = finished()
            override fun onCancelled(gestureDescription: GestureDescription?) = finished()
        }
        if (!dispatchGesture(g, done, main)) {
            Log.w("UrDevice", "gesture not dispatched")
            finished()
        }
    }

    private fun finished() {
        running = false
        next()
    }

    companion object {
        private const val MAX_QUEUE = 16

        /** The connected service, or null while it is off. */
        @Volatile
        var instance: UrDeviceAccessibilityService? = null
            private set
    }
}
