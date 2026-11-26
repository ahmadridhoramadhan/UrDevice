package com.urdevice.app.host

import android.accessibilityservice.AccessibilityService
import android.accessibilityservice.GestureDescription
import android.content.Intent
import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.PixelFormat
import android.os.Handler
import android.os.Looper
import android.util.Log
import android.view.Gravity
import android.view.View
import android.view.WindowManager
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
        Log.i(TAG, "accessibility service connected")
    }

    override fun onDestroy() {
        removeCursor()
        super.onDestroy()
    }

    override fun onUnbind(intent: Intent?): Boolean {
        // Android calls this when the user or the system turns the service off while the app lives.
        Log.i(TAG, "accessibility service unbound (turned off by the system or the user)")
        instance = null
        main.post { removeCursor() }
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

    // The pointer the controller's mouse moves. An accessibility overlay needs no extra permission,
    // does not take touches, and sits above other apps.
    private var cursorView: PointerView? = null
    @Volatile private var cursorAt: Pair<Int, Int>? = null
    @Volatile private var cursorPosted = false

    /** Draw the pointer at [x], [y] pixels, or remove it. Thread-safe; moves are joined per frame. */
    fun cursor(x: Int, y: Int, shown: Boolean) {
        cursorAt = if (shown) x to y else null
        if (cursorPosted) return
        cursorPosted = true
        main.post {
            cursorPosted = false
            applyCursor()
        }
    }

    private fun applyCursor() {
        val at = cursorAt
        if (at == null) {
            removeCursor()
            return
        }
        val wm = getSystemService(WindowManager::class.java)
        val view = cursorView
        if (view == null) {
            val v = PointerView(this)
            cursorView = v
            runCatching { wm.addView(v, cursorParams(at)) }.onFailure { cursorView = null }
        } else {
            runCatching { wm.updateViewLayout(view, cursorParams(at)) }
        }
    }

    private fun pointerSize(): Int = (POINTER_DP * resources.displayMetrics.density).toInt()

    private fun removeCursor() {
        cursorAt = null
        val v = cursorView ?: return
        cursorView = null
        runCatching { getSystemService(WindowManager::class.java).removeView(v) }
    }

    private fun cursorParams(at: Pair<Int, Int>): WindowManager.LayoutParams {
        val size = pointerSize()
        // The arrow hangs right and down from its tip. Near the right and bottom edges it is
        // drawn a little inside the screen so most of it stays visible. Taps still land on the
        // real point.
        val m = resources.displayMetrics
        val maxX = m.widthPixels - (EDGE_KEEP_X_DP * m.density).toInt()
        val maxY = m.heightPixels - (EDGE_KEEP_Y_DP * m.density).toInt()
        return WindowManager.LayoutParams(
            size,
            size,
            WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
            WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
                WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE or
                WindowManager.LayoutParams.FLAG_LAYOUT_NO_LIMITS or
                WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
            PixelFormat.TRANSLUCENT,
        ).apply {
            gravity = Gravity.TOP or Gravity.START
            x = minOf(at.first, maxX)
            y = minOf(at.second, maxY)
        }
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
        private const val TAG = "UrDeviceA11y"
        private const val POINTER_DP = 28

        /** How much of the arrow, right and down of its tip, must stay on screen. */
        private const val EDGE_KEEP_X_DP = 14
        private const val EDGE_KEEP_Y_DP = 18

        /** The connected service, or null while it is off. */
        @Volatile
        var instance: UrDeviceAccessibilityService? = null
            private set
    }
}

/** A mouse pointer: a white arrow with a dark edge. Its tip is the top left corner of the view. */
internal class PointerView(context: Context) : View(context) {
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.WHITE
        style = Paint.Style.FILL
    }
    private val edge = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        color = Color.parseColor("#0F1F10")
        style = Paint.Style.STROKE
        strokeWidth = 2f * context.resources.displayMetrics.density
        strokeJoin = Paint.Join.ROUND
    }

    init {
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_NO
    }


    override fun onDraw(canvas: Canvas) {
        val k = width / 28f
        val arrow = Path().apply {
            moveTo(2f * k, 2f * k)
            lineTo(2f * k, 22f * k)
            lineTo(7f * k, 17f * k)
            lineTo(11f * k, 26f * k)
            lineTo(15f * k, 24f * k)
            lineTo(11f * k, 15.5f * k)
            lineTo(18f * k, 15.5f * k)
            close()
        }
        canvas.drawPath(arrow, fill)
        canvas.drawPath(arrow, edge)
    }
}
