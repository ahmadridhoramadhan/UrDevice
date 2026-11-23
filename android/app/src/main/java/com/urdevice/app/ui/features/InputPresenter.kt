package com.urdevice.app.ui.features

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.os.Build
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.view.KeyEvent
import android.view.Window
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Pause
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.PointerEventType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.urdevice.app.R
import com.urdevice.core.UrDeviceNode
import kotlin.math.PI
import kotlin.math.abs
import kotlinx.coroutines.delay

// HID keyboard usages the presenter sends (R9b.1, R9b.2).
private const val KEY_B = 0x05u
private const val KEY_ESCAPE = 0x29u
private const val KEY_F5 = 0x3Eu
private const val KEY_RIGHT = 0x4Fu
private const val KEY_LEFT = 0x50u

/** Laser updates are sent at most this often: 60 per second (R9b.3, design §9.3). */
private const val LASER_EVERY_MS = 16L

/** Turning the phone this far (radians) from where the laser button went down moves the dot across the screen. */
private const val GYRO_RANGE = 0.35f

private fun tap(node: UrDeviceNode, device: String, usage: UInt) {
    runCatching { node.inputTapKey(device, usage, 0u) }
}

/** The tile key of the Presenter settings page. */
const val PRESENTER_SETTINGS = "presenter.settings"

/**
 * The Presenter (R9b), top to bottom: the timer, the laser pointer (touch, or turn the phone),
 * Start, Blank and End, and Previous and Next under the thumb. The volume keys also go to the
 * next and previous slide while this screen is open. Timer length lives on its own page.
 */
@Composable
fun PresenterPad(node: UrDeviceNode, device: String, enabled: Boolean, modifier: Modifier = Modifier) {
    KeepScreenOn()
    val next = rememberUpdatedState { tap(node, device, KEY_RIGHT) }
    val previous = rememberUpdatedState { tap(node, device, KEY_LEFT) }
    VolumeKeys(enabled = enabled, onPrevious = { previous.value() }, onNext = { next.value() })
    val openPage = LocalOpenFeaturePage.current

    Column(modifier, verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text(stringResource(R.string.tile_presenter), style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            IconButton(onClick = { openPage("input", PRESENTER_SETTINGS) }) {
                Icon(Icons.Outlined.Settings, stringResource(R.string.input_presenter_settings))
            }
        }
        SlideTimer()
        LaserTouch(node, device, enabled, Modifier.weight(1f))
        LaserGyro(node, device, enabled)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            OutlinedButton(
                onClick = { tap(node, device, KEY_F5) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_start_show)) }
            OutlinedButton(
                onClick = { tap(node, device, KEY_B) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_blank)) }
            OutlinedButton(
                onClick = { tap(node, device, KEY_ESCAPE) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_end_show)) }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            Button(
                onClick = { previous.value() },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 72.dp),
            ) { Text(stringResource(R.string.input_previous), fontSize = 18.sp) }
            Button(
                onClick = { next.value() },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 72.dp),
            ) { Text(stringResource(R.string.input_next), fontSize = 18.sp) }
        }
    }
}

/**
 * Volume keys as Next and Previous while the screen is open (R9b.1). The activity's window
 * callback is wrapped for the time the screen is shown, so the keys never change the volume.
 * Volume down is Next and volume up is Previous.
 */
@Composable
private fun VolumeKeys(enabled: Boolean, onPrevious: () -> Unit, onNext: () -> Unit) {
    val ctx = LocalContext.current
    val previous = rememberUpdatedState(onPrevious)
    val next = rememberUpdatedState(onNext)
    DisposableEffect(enabled) {
        val window = ctx.findActivity()?.window
        val original = window?.callback
        if (window != null && original != null && enabled) {
            window.callback = object : Window.Callback by original {
                override fun dispatchKeyEvent(event: KeyEvent): Boolean {
                    val code = event.keyCode
                    if (code != KeyEvent.KEYCODE_VOLUME_UP && code != KeyEvent.KEYCODE_VOLUME_DOWN) {
                        return original.dispatchKeyEvent(event)
                    }
                    if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) {
                        if (code == KeyEvent.KEYCODE_VOLUME_DOWN) next.value() else previous.value()
                    }
                    return true
                }
            }
        }
        onDispose {
            if (window != null && original != null && enabled) window.callback = original
        }
    }
}

private tailrec fun Context.findActivity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.findActivity()
    else -> null
}

/** The laser by touch: the dot follows the finger on this area and goes away when it lifts (R9b.3). */
@Composable
private fun LaserTouch(node: UrDeviceNode, device: String, enabled: Boolean, modifier: Modifier = Modifier) {
    Box(
        modifier
            .fillMaxWidth()
            .heightIn(min = 140.dp)
            .clip(RoundedCornerShape(16.dp))
            .background(MaterialTheme.colorScheme.surfaceVariant)
            .pointerInput(device, enabled) {
                if (!enabled) return@pointerInput
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    var last = 0L
                    fun send(x: Float, y: Float) {
                        val w = size.width.coerceAtLeast(1)
                        val h = size.height.coerceAtLeast(1)
                        runCatching { node.inputLaser(device, x / w, y / h, true) }
                    }
                    send(down.position.x, down.position.y)
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull() ?: break
                        change.consume()
                        if (!change.pressed) break
                        if (event.type == PointerEventType.Move && change.uptimeMillis - last >= LASER_EVERY_MS) {
                            last = change.uptimeMillis
                            send(change.position.x, change.position.y)
                        }
                    }
                    runCatching { node.inputLaser(device, 0f, 0f, false) }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Text(stringResource(R.string.input_laser_touch_hint), style = MaterialTheme.typography.bodySmall)
    }
}

/**
 * The laser by gyroscope: hold the button and turn the phone; the dot starts in the middle of the
 * screen and follows the rotation (R9b.3). It goes away when the button is released.
 */
@Composable
private fun LaserGyro(node: UrDeviceNode, device: String, enabled: Boolean) {
    val ctx = LocalContext.current
    val sensors = remember { ctx.getSystemService(Context.SENSOR_SERVICE) as SensorManager }
    val sensor = remember { sensors.getDefaultSensor(Sensor.TYPE_GAME_ROTATION_VECTOR) }
    var held by remember { mutableStateOf(false) }

    DisposableEffect(held, enabled) {
        if (!held || !enabled || sensor == null) return@DisposableEffect onDispose {}
        val listener = object : SensorEventListener {
            private val matrix = FloatArray(9)
            private val angles = FloatArray(3)
            private var yaw0 = 0f
            private var pitch0 = 0f
            private var started = false
            private var last = 0L

            override fun onSensorChanged(e: SensorEvent) {
                SensorManager.getRotationMatrixFromVector(matrix, e.values)
                SensorManager.getOrientation(matrix, angles)
                val yaw = angles[0]
                val pitch = angles[1]
                if (!started) {
                    started = true
                    yaw0 = yaw
                    pitch0 = pitch
                }
                val now = e.timestamp / 1_000_000
                if (now - last < LASER_EVERY_MS) return
                last = now
                var dYaw = yaw - yaw0
                // The compass heading wraps around at half a turn.
                if (dYaw > PI.toFloat()) dYaw -= 2 * PI.toFloat()
                if (dYaw < -PI.toFloat()) dYaw += 2 * PI.toFloat()
                val x = 0.5f + dYaw / (2 * GYRO_RANGE)
                val y = 0.5f + (pitch - pitch0) / (2 * GYRO_RANGE)
                runCatching { node.inputLaser(device, x, y, true) }
            }

            override fun onAccuracyChanged(s: Sensor?, accuracy: Int) {}
        }
        sensors.registerListener(listener, sensor, SensorManager.SENSOR_DELAY_GAME)
        onDispose {
            sensors.unregisterListener(listener)
            runCatching { node.inputLaser(device, 0f, 0f, false) }
        }
    }

    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 56.dp)
            .clip(RoundedCornerShape(16.dp))
            .background(if (held) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceVariant)
            .pointerInput(enabled) {
                detectTapGestures(onPress = {
                    if (enabled && sensor != null) {
                        held = true
                        tryAwaitRelease()
                        held = false
                    }
                })
            },
        contentAlignment = Alignment.Center,
    ) {
        Text(
            if (sensor == null) stringResource(R.string.input_no_gyro) else stringResource(R.string.input_laser_gyro_hint),
            color = if (held) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** The slide timer's state, kept here so it survives a visit to the settings page (R9b.4). */
private object PresenterTimer {
    var totalMin by mutableIntStateOf(20)
    var warnMin by mutableIntStateOf(5)
    var running by mutableStateOf(false)
        private set
    var warned = false
    var ended = false
    private var accumulated by mutableLongStateOf(0L)
    private var startedAt = 0L

    fun elapsed(now: Long): Long = accumulated + if (running) now - startedAt else 0L

    fun toggle() {
        val now = System.currentTimeMillis()
        if (running) accumulated += now - startedAt else startedAt = now
        running = !running
    }

    fun reset() {
        running = false
        accumulated = 0
        warned = false
        ended = false
    }
}

/**
 * The slide timer (R9b.4), one line: the time left, then Start or Pause and Reset. It runs on the
 * phone, with a short vibration at the warning time and a long one when the time is up.
 */
@Composable
private fun SlideTimer() {
    val ctx = LocalContext.current
    val t = PresenterTimer
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(t.running) {
        while (t.running) {
            delay(250)
            now = System.currentTimeMillis()
            val leftMs = t.totalMin * 60_000L - t.elapsed(now)
            if (!t.warned && t.warnMin > 0 && leftMs <= t.warnMin * 60_000L && leftMs > 0) {
                t.warned = true
                vibrate(ctx, 250)
            }
            if (!t.ended && leftMs <= 0) {
                t.ended = true
                vibrate(ctx, 900)
            }
        }
    }
    val leftMs = t.totalMin * 60_000L - t.elapsed(now)
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(
            (if (leftMs < 0) "-" else "") + clock(abs(leftMs)),
            fontSize = 32.sp,
            fontFamily = FontFamily.Monospace,
            color = if (leftMs < 0) Color(0xFFD32F2F) else MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.weight(1f),
        )
        IconButton(onClick = { t.toggle(); now = System.currentTimeMillis() }) {
            Icon(
                if (t.running) Icons.Outlined.Pause else Icons.Outlined.PlayArrow,
                stringResource(if (t.running) R.string.input_timer_pause else R.string.input_timer_start),
            )
        }
        IconButton(onClick = { t.reset(); now = System.currentTimeMillis() }) {
            Icon(Icons.Outlined.Refresh, stringResource(R.string.input_timer_reset))
        }
    }
}

/** The timer length and the warning time, on their own page. */
@Composable
fun PresenterSettings() {
    val t = PresenterTimer
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Text(stringResource(R.string.input_presenter_settings), style = MaterialTheme.typography.titleMedium)
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.input_timer_length, t.totalMin), modifier = Modifier.weight(1f))
            OutlinedButton(onClick = { t.totalMin = (t.totalMin - 5).coerceAtLeast(5) }) { Text("-5") }
            OutlinedButton(onClick = { t.totalMin = (t.totalMin + 5).coerceAtMost(240) }) { Text("+5") }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.input_timer_warn, t.warnMin), modifier = Modifier.weight(1f))
            OutlinedButton(onClick = { t.warnMin = (t.warnMin - 1).coerceAtLeast(0) }) { Text("-1") }
            OutlinedButton(onClick = { t.warnMin = (t.warnMin + 1).coerceAtMost(30) }) { Text("+1") }
        }
    }
}

/** `m:ss`, or `h:mm:ss` from an hour on. */
private fun clock(ms: Long): String {
    val s = ms / 1000
    val h = s / 3600
    val m = (s % 3600) / 60
    val sec = s % 60
    return if (h > 0) "%d:%02d:%02d".format(h, m, sec) else "%d:%02d".format(m, sec)
}

private fun vibrate(ctx: Context, ms: Long) {
    val vibrator: Vibrator? = if (Build.VERSION.SDK_INT >= 31) {
        ctx.getSystemService(VibratorManager::class.java)?.defaultVibrator
    } else {
        @Suppress("DEPRECATION")
        ctx.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
    }
    vibrator?.vibrate(VibrationEffect.createOneShot(ms, VibrationEffect.DEFAULT_AMPLITUDE))
}
