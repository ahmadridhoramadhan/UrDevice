package com.urdevice.app.ui.features

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
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
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerInputScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.urdevice.app.R
import com.urdevice.core.UrDeviceNode
import kotlin.math.abs
import kotlin.math.max
import kotlinx.coroutines.withTimeoutOrNull

/** Mouse buttons on the wire. */
private const val LEFT = 0u
private const val RIGHT = 1u
private const val MIDDLE = 2u

/** A tap is shorter than this and moves less than the touch slop (R9a.2). */
private const val TAP_MS = 250L

/** A second touch this soon after a tap starts a drag (R9a.4). */
private const val DRAG_WINDOW_MS = 300L

/** A press that stays put this long is a right click (R9a.2). */
private const val LONG_PRESS_MS = 500L

/** Wire scroll units (120 per notch) for each pixel two fingers travel. */
private const val SCROLL_UNITS_PER_PX = 3f

private const val PREFS = "input"

/** Keeps the screen on while a screen that controls another device is open (R9a.5). */
@Composable
fun KeepScreenOn() {
    val view = LocalView.current
    DisposableEffect(view) {
        view.keepScreenOn = true
        onDispose { view.keepScreenOn = false }
    }
}

/**
 * The Mousepad (R9a): one finger moves the pointer (with sensitivity and acceleration), a tap
 * clicks, a two-finger tap or a long press right-clicks, two fingers scroll, and a tap followed by
 * a press-and-drag drags.
 */
@Composable
fun MousepadPad(node: UrDeviceNode, device: String, enabled: Boolean) {
    KeepScreenOn()
    val ctx = LocalContext.current
    val prefs = remember { ctx.getSharedPreferences(PREFS, Context.MODE_PRIVATE) }
    var sensitivity by remember { mutableFloatStateOf(prefs.getFloat("sensitivity", 1.5f)) }
    var accel by remember { mutableFloatStateOf(prefs.getFloat("accel", 0.04f)) }
    var natural by remember { mutableStateOf(prefs.getBoolean("natural", true)) }
    val haptic = LocalHapticFeedback.current

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        Box(
            Modifier
                .fillMaxWidth()
                .height(320.dp)
                .clip(RoundedCornerShape(16.dp))
                .background(MaterialTheme.colorScheme.surfaceVariant)
                .pointerInput(device, enabled) {
                    if (enabled) {
                        padGestures(
                            node, device,
                            sensitivity = { sensitivity },
                            accel = { accel },
                            natural = { natural },
                            onLongPress = { haptic.performHapticFeedback(HapticFeedbackType.LongPress) },
                        )
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            Text(stringResource(R.string.input_pad_hint), style = MaterialTheme.typography.bodySmall)
        }
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.fillMaxWidth()) {
            Button(
                onClick = { clickOnce(node, device, LEFT) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_left_click)) }
            Button(
                onClick = { clickOnce(node, device, MIDDLE) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_middle_click)) }
            Button(
                onClick = { clickOnce(node, device, RIGHT) },
                enabled = enabled,
                modifier = Modifier.weight(1f).heightIn(min = 48.dp),
            ) { Text(stringResource(R.string.input_right_click)) }
        }
        Text(stringResource(R.string.input_sensitivity), style = MaterialTheme.typography.bodyMedium)
        Slider(
            value = sensitivity,
            onValueChange = { sensitivity = it },
            onValueChangeFinished = { prefs.edit().putFloat("sensitivity", sensitivity).apply() },
            valueRange = 0.5f..4f,
        )
        Text(stringResource(R.string.input_acceleration), style = MaterialTheme.typography.bodyMedium)
        Slider(
            value = accel,
            onValueChange = { accel = it },
            onValueChangeFinished = { prefs.edit().putFloat("accel", accel).apply() },
            valueRange = 0f..0.15f,
        )
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(stringResource(R.string.input_natural_scroll), modifier = Modifier.weight(1f))
            Switch(checked = natural, onCheckedChange = {
                natural = it
                prefs.edit().putBoolean("natural", it).apply()
            })
        }
    }
}

private fun clickOnce(node: UrDeviceNode, device: String, button: UInt) {
    runCatching {
        node.inputButton(device, button, true)
        node.inputButton(device, button, false)
    }
}

/**
 * The touch state machine. One gesture runs from the first finger down until every finger is up.
 * Pointer events are consumed, so the screen's own scrolling never competes with the pad.
 */
private suspend fun PointerInputScope.padGestures(
    node: UrDeviceNode,
    device: String,
    sensitivity: () -> Float,
    accel: () -> Float,
    natural: () -> Boolean,
    onLongPress: () -> Unit,
) {
    val slop = viewConfiguration.touchSlop
    // When the last single-finger tap ended, for tap-then-drag.
    var lastTapUp = 0L
    // Fractions of a pixel not sent yet.
    var carryX = 0f
    var carryY = 0f
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        down.consume()
        val t0 = down.uptimeMillis
        val startNs = System.nanoTime()
        val dragMode = lastTapUp != 0L && t0 - lastTapUp < DRAG_WINDOW_MS
        if (dragMode) runCatching { node.inputButton(device, LEFT, true) }
        var maxPointers = 1
        var travelled = 0f
        var longPressed = false
        var prevCount = 1
        var prevX = down.position.x
        var prevY = down.position.y
        var lastUp = t0
        var scrollX = 0f
        var scrollY = 0f
        while (true) {
            val canLongPress = !longPressed && !dragMode && maxPointers == 1 && travelled < slop
            val remaining = LONG_PRESS_MS - (System.nanoTime() - startNs) / 1_000_000
            val event = when {
                !canLongPress -> awaitPointerEvent()
                remaining <= 0 -> null
                else -> withTimeoutOrNull(remaining) { awaitPointerEvent() }
            }
            if (event == null) {
                // The finger stayed put for the long-press time: right click.
                longPressed = true
                onLongPress()
                clickOnce(node, device, RIGHT)
                continue
            }
            val pressed = event.changes.filter { it.pressed }
            lastUp = event.changes.maxOf { it.uptimeMillis }
            if (pressed.isEmpty()) {
                event.changes.forEach { it.consume() }
                break
            }
            maxPointers = max(maxPointers, pressed.size)
            val cx = pressed.map { it.position.x }.average().toFloat()
            val cy = pressed.map { it.position.y }.average().toFloat()
            if (pressed.size != prevCount) {
                // A finger came or went: the centre jumps, so start from here.
                prevCount = pressed.size
                prevX = cx
                prevY = cy
                event.changes.forEach { it.consume() }
                continue
            }
            val dx = cx - prevX
            val dy = cy - prevY
            prevX = cx
            prevY = cy
            travelled += abs(dx) + abs(dy)
            if (pressed.size >= 2) {
                // Two fingers scroll, vertically and horizontally (R9a.3).
                val sign = if (natural()) -1f else 1f
                scrollX += sign * dx * SCROLL_UNITS_PER_PX
                scrollY += sign * dy * SCROLL_UNITS_PER_PX
                val ix = scrollX.toInt()
                val iy = scrollY.toInt()
                if (ix != 0 || iy != 0) {
                    scrollX -= ix
                    scrollY -= iy
                    runCatching { node.inputScroll(device, ix, iy) }
                }
            } else if (!longPressed && (travelled >= slop || dragMode)) {
                // One finger moves the pointer, faster when the finger is fast (R9a.1).
                val speed = abs(dx) + abs(dy)
                val factor = sensitivity() * (1f + accel() * speed)
                carryX += dx * factor
                carryY += dy * factor
                val ix = carryX.toInt()
                val iy = carryY.toInt()
                if (ix != 0 || iy != 0) {
                    carryX -= ix
                    carryY -= iy
                    runCatching { node.inputMove(device, ix, iy) }
                }
            }
            event.changes.forEach { it.consume() }
        }
        // Every finger is up: decide what the gesture was.
        val short = lastUp - t0 < TAP_MS
        val still = travelled < slop * 2
        when {
            dragMode -> {
                runCatching { node.inputButton(device, LEFT, false) }
                lastTapUp = 0L
            }
            longPressed -> lastTapUp = 0L
            short && still && maxPointers == 1 -> {
                clickOnce(node, device, LEFT)
                lastTapUp = lastUp
            }
            short && still && maxPointers == 2 -> {
                clickOnce(node, device, RIGHT)
                lastTapUp = 0L
            }
            else -> lastTapUp = 0L
        }
    }
}
