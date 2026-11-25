package com.urdevice.app.host

import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.Settings
import android.view.inputmethod.InputMethodManager
import com.urdevice.core.InputHostFfi

/**
 * The phone as a remote-control host (#19, R9.3, R10.3): gestures through the accessibility
 * service, text and keys through the UrDevice keyboard. The core decides what to run and checks the
 * permission first; this only runs it. Never logs text or positions.
 */
class InputHost(private val context: Context) : InputHostFfi {
    override fun gesturesReady() = UrDeviceAccessibilityService.instance != null

    override fun textReady() = UrDeviceIme.instance != null && isDefaultKeyboard(context)

    override fun displayWidth(): UInt = context.resources.displayMetrics.widthPixels.toUInt()

    override fun displayHeight(): UInt = context.resources.displayMetrics.heightPixels.toUInt()

    override fun tap(x: UInt, y: UInt, durationMs: UInt): Boolean =
        UrDeviceAccessibilityService.instance
            ?.stroke(intArrayOf(x.toInt()), intArrayOf(y.toInt()), durationMs.toLong()) ?: false

    override fun swipe(xs: List<UInt>, ys: List<UInt>, durationMs: UInt): Boolean =
        UrDeviceAccessibilityService.instance?.stroke(
            xs.map { it.toInt() }.toIntArray(),
            ys.map { it.toInt() }.toIntArray(),
            durationMs.toLong(),
        ) ?: false

    override fun nav(action: UInt): Boolean =
        UrDeviceAccessibilityService.instance?.global(action.toInt()) ?: false

    override fun text(text: String): Boolean = UrDeviceIme.instance?.commit(text) ?: false

    override fun key(code: UInt, down: Boolean, meta: UInt): Boolean =
        UrDeviceIme.instance?.key(code.toInt(), down, meta.toInt()) ?: false

    override fun cursor(x: UInt, y: UInt, shown: Boolean) {
        UrDeviceAccessibilityService.instance?.cursor(x.toInt(), y.toInt(), shown)
    }

    companion object {
        private val SERVICE = "com.urdevice.app.host.UrDeviceAccessibilityService"

        /** True when the UrDevice accessibility service is turned on in Android settings. */
        fun serviceEnabled(context: Context): Boolean {
            val on = Settings.Secure.getString(context.contentResolver, Settings.Secure.ENABLED_ACCESSIBILITY_SERVICES)
                ?: return false
            return on.split(':').any {
                ComponentName.unflattenFromString(it)?.let { c ->
                    c.packageName == context.packageName && c.className == SERVICE
                } == true
            }
        }

        private fun imeId(context: Context) =
            ComponentName(context, UrDeviceIme::class.java).flattenToShortString()

        /** True when the UrDevice keyboard is turned on in Android settings. */
        fun keyboardEnabled(context: Context): Boolean =
            context.getSystemService(InputMethodManager::class.java)
                .enabledInputMethodList.any { it.id == imeId(context) }

        /** True when the UrDevice keyboard is the one in use now. */
        fun isDefaultKeyboard(context: Context): Boolean =
            Settings.Secure.getString(context.contentResolver, Settings.Secure.DEFAULT_INPUT_METHOD) == imeId(context)

        /** Android's Accessibility settings. */
        fun openAccessibilitySettings(context: Context) = start(context, Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))

        /** Android's keyboard list, where the user turns the UrDevice keyboard on. */
        fun openKeyboardSettings(context: Context) = start(context, Intent(Settings.ACTION_INPUT_METHOD_SETTINGS))

        /** The picker that makes the UrDevice keyboard the one in use. */
        fun showKeyboardPicker(context: Context) {
            context.getSystemService(InputMethodManager::class.java).showInputMethodPicker()
        }

        /** UrDevice's app info page, where Android 13+ allows restricted settings for a sideloaded app. */
        fun openAppInfo(context: Context) = start(
            context,
            Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}")),
        )

        private fun start(context: Context, intent: Intent) {
            runCatching { context.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
        }
    }
}
