package com.urdevice.app.host

import android.inputmethodservice.InputMethodService
import android.os.SystemClock
import android.view.Gravity
import android.view.KeyEvent
import android.view.View
import android.view.inputmethod.InputMethodManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import com.urdevice.app.R

/**
 * The UrDevice keyboard (#19, R10.3): types what a paired desktop sends into the focused field.
 * It has no keys of its own; its small panel says so and switches back to the user's keyboard.
 * Text and keys are never logged.
 */
class UrDeviceIme : InputMethodService() {
    override fun onCreate() {
        super.onCreate()
        instance = this
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        super.onDestroy()
    }

    override fun onCreateInputView(): View {
        val pad = (16 * resources.displayMetrics.density).toInt()
        return LinearLayout(this).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(pad, pad, pad, pad)
            addView(
                TextView(context).apply { setText(R.string.input_ime_panel) },
                LinearLayout.LayoutParams(0, LinearLayout.LayoutParams.WRAP_CONTENT, 1f),
            )
            addView(
                Button(context).apply {
                    setText(R.string.input_ime_switch)
                    setOnClickListener {
                        if (!switchToPreviousInputMethod()) {
                            getSystemService(InputMethodManager::class.java).showInputMethodPicker()
                        }
                    }
                },
            )
        }
    }

    /** Commit [text] at the cursor. False when no field is focused. */
    fun commit(text: String): Boolean = currentInputConnection?.commitText(text, 1) ?: false

    /** Send one key event to the focused field with [meta] state. */
    fun key(code: Int, down: Boolean, meta: Int): Boolean {
        val ic = currentInputConnection ?: return false
        val now = SystemClock.uptimeMillis()
        return ic.sendKeyEvent(
            KeyEvent(now, now, if (down) KeyEvent.ACTION_DOWN else KeyEvent.ACTION_UP, code, 0, meta),
        )
    }

    companion object {
        /** The running keyboard, or null while it is not in use. */
        @Volatile
        var instance: UrDeviceIme? = null
            private set
    }
}
