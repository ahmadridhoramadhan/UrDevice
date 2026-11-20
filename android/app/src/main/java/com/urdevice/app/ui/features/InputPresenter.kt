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
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
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
import androidx.compose.ui.text.font.FontWeight
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

/**
 * The Presenter (R9b): Previous and Next (also the volume keys while this screen is open), Blank,
 * Start, End, a laser pointer that follows the gyroscope or a finger, and a timer that runs on the
 * phone.
 */
@Composable
fun PresenterPad(node: UrDeviceNode, device: String, enabled: Boolean) {
    KeepScreenOn()
    val next = rememberUpdatedState { tap(node, device, KEY_RIGHT) }
    val previous = rememberUpdatedState { tap(node, device, KEY_LEFT) }
    VolumeKeys(enabled = enabled, onPrevious = { previous.value() }, onNext = { next.value() })

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
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
        Text(stringResource(R.string.input_volume_hint), style = MaterialTheme.typography.bodySmall)
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            OutlinedButton(
                onClick = { tap(node, device, KEY_B) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_blank)) }
            OutlinedButton(
                onClick = { tap(node, device, KEY_F5) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_start_show)) }
            OutlinedButton(
                onClick = { tap(node, device, KEY_ESCAPE) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_end_show)) }
        }
        LaserTouch(node, device, enabled)
        LaserGyro(node, device, enabled)
        SlideTimer()
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
private fun LaserTouch(node: UrDeviceNode, device: String, enabled: Boolean) {
    Text(stringResource(R.string.input_laser_touch), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
    Box(
        Modifier
            .fillMaxWidth()
            .height(180.dp)
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

    Text(stringResource(R.string.input_laser_gyro), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
    Box(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 64.dp)
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

/**
 * The slide timer (R9b.4). It runs on the phone: elapsed and remaining time, with a short
 * vibration when the warning time is reached and a long one when the time is up.
 */
@Composable
private fun SlideTimer() {
    val ctx = LocalContext.current
    var totalMin by remember { mutableIntStateOf(20) }
    var warnMin by remember { mutableIntStateOf(5) }
    var running by remember { mutableStateOf(false) }
    var elapsedMs by remember { mutableLongStateOf(0L) }
    var warned by remember { mutableStateOf(false) }
    var ended by remember { mutableStateOf(false) }

    LaunchedEffect(running) {
        if (!running) return@LaunchedEffect
        var last = System.currentTimeMillis()
        while (true) {
            delay(250)
            val now = System.currentTimeMillis()
            elapsedMs += now - last
            last = now
            val leftMs = totalMin * 60_000L - elapsedMs
            if (!warned && warnMin > 0 && leftMs <= warnMin * 60_000L && leftMs > 0) {
                warned = true
                vibrate(ctx, 250)
            }
            if (!ended && leftMs <= 0) {
                ended = true
                vibrate(ctx, 900)
            }
        }
    }

    val leftMs = totalMin * 60_000L - elapsedMs
    Text(stringResource(R.string.input_timer), style = MaterialTheme.typography.bodyMedium, fontWeight = FontWeight.SemiBold)
    Text(
        (if (leftMs < 0) "-" else "") + clock(abs(leftMs)),
        fontSize = 44.sp,
        fontFamily = FontFamily.Monospace,
        color = if (leftMs < 0) Color(0xFFD32F2F) else MaterialTheme.colorScheme.onSurface,
    )
    Text(
        stringResource(R.string.input_timer_elapsed, clock(elapsedMs)),
        style = MaterialTheme.typography.bodySmall,
    )
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
        Button(onClick = { running = !running }, modifier = Modifier.weight(1f).heightIn(min = 48.dp)) {
            Text(stringResource(if (running) R.string.input_timer_pause else R.string.input_timer_start))
        }
        OutlinedButton(
            onClick = {
                running = false
                elapsedMs = 0
                warned = false
                ended = false
            },
            modifier = Modifier.weight(1f).heightIn(min = 48.dp),
        ) { Text(stringResource(R.string.input_timer_reset)) }
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.input_timer_length, totalMin), modifier = Modifier.weight(1f))
        OutlinedButton(onClick = { totalMin = (totalMin - 5).coerceAtLeast(5) }) { Text("-5") }
        OutlinedButton(onClick = { totalMin = (totalMin + 5).coerceAtMost(240) }) { Text("+5") }
    }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(stringResource(R.string.input_timer_warn, warnMin), modifier = Modifier.weight(1f))
        OutlinedButton(onClick = { warnMin = (warnMin - 1).coerceAtLeast(0) }) { Text("-1") }
        OutlinedButton(onClick = { warnMin = (warnMin + 1).coerceAtMost(30) }) { Text("+1") }
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
