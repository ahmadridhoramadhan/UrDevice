package com.urdevice.app.ui.features

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.dp
import com.urdevice.app.R
import com.urdevice.core.UrDeviceNode

// Modifier bits on the wire.
private const val CTRL = 1u
private const val SHIFT = 2u
private const val ALT = 4u
private const val META = 8u

// HID keyboard usages (page 0x07).
private const val K_ENTER = 0x28u
private const val K_ESCAPE = 0x29u
private const val K_BACKSPACE = 0x2Au
private const val K_TAB = 0x2Bu
private const val K_F1 = 0x3Au
private const val K_HOME = 0x4Au
private const val K_PAGE_UP = 0x4Bu
private const val K_DELETE = 0x4Cu
private const val K_END = 0x4Du
private const val K_PAGE_DOWN = 0x4Eu
private const val K_RIGHT = 0x4Fu
private const val K_LEFT = 0x50u
private const val K_DOWN = 0x51u
private const val K_UP = 0x52u

/**
 * A character the text field keeps in front of what the user types. Deleting it is how a
 * Backspace on an empty field shows up, since the on-screen keyboard sends nothing for it.
 */
private const val SENTINEL = "\u200B"

/** The HID usage of a character on a US layout, for shortcuts like Ctrl+C. */
internal fun charUsage(c: Char): UInt? {
    val l = c.lowercaseChar()
    return when (l) {
        in 'a'..'z' -> 0x04u + (l - 'a').toUInt()
        in '1'..'9' -> 0x1Eu + (l - '1').toUInt()
        '0' -> 0x27u
        '\n' -> K_ENTER
        '\t' -> K_TAB
        ' ' -> 0x2Cu
        '-' -> 0x2Du
        '=' -> 0x2Eu
        '[' -> 0x2Fu
        ']' -> 0x30u
        '\\' -> 0x31u
        ';' -> 0x33u
        '\'' -> 0x34u
        '`' -> 0x35u
        ',' -> 0x36u
        '.' -> 0x37u
        '/' -> 0x38u
        else -> null
    }
}

/**
 * The Keyboard (R10.1, R10.4): typed text goes to the desktop as Unicode text, so it works with
 * any layout. Special keys, function keys, and shortcuts are buttons. Ctrl, Alt, Shift, and Meta
 * stay on for the next key or character.
 */
@Composable
fun KeyboardPad(node: UrDeviceNode, device: String, enabled: Boolean) {
    var field by remember { mutableStateOf(TextFieldValue(SENTINEL, TextRange(1))) }
    var ctrl by remember { mutableStateOf(false) }
    var shift by remember { mutableStateOf(false) }
    var alt by remember { mutableStateOf(false) }
    var meta by remember { mutableStateOf(false) }
    // Bumped to redraw the chips after the modifiers were cleared.
    var tick by remember { mutableIntStateOf(0) }

    fun mods(): UInt =
        (if (ctrl) CTRL else 0u) or (if (shift) SHIFT else 0u) or (if (alt) ALT else 0u) or (if (meta) META else 0u)

    fun clearMods() {
        ctrl = false
        shift = false
        alt = false
        meta = false
        tick += 1
    }

    fun key(usage: UInt, modifiers: UInt = mods()) {
        runCatching { node.inputTapKey(device, usage, modifiers) }
        clearMods()
    }

    fun typed(text: String) {
        val m = mods()
        when {
            m == 0u -> runCatching { node.inputText(device, text) }
            // Only Shift: the capital letters, as text.
            m == SHIFT -> runCatching { node.inputText(device, text.uppercase()) }
            // A shortcut: each character is a key with the modifiers held.
            else -> text.forEach { c ->
                val usage = charUsage(c)
                if (usage != null) {
                    val extra = if (c.isUpperCase()) SHIFT else 0u
                    runCatching { node.inputTapKey(device, usage, m or extra) }
                }
            }
        }
        clearMods()
    }

    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        OutlinedTextField(
            value = field,
            onValueChange = { new ->
                if (!enabled) return@OutlinedTextField
                if (new.composition != null) {
                    // The keyboard is still composing a word; send it when it is done.
                    field = new
                    return@OutlinedTextField
                }
                val t = new.text
                if (!t.startsWith(SENTINEL)) {
                    key(K_BACKSPACE)
                } else if (t.length > 1) {
                    typed(t.substring(1))
                }
                field = TextFieldValue(SENTINEL, TextRange(1))
            },
            label = { Text(stringResource(R.string.input_type_here)) },
            keyboardOptions = KeyboardOptions(
                keyboardType = KeyboardType.Text,
                autoCorrectEnabled = false,
                imeAction = ImeAction.Send,
            ),
            keyboardActions = KeyboardActions(onSend = { key(K_ENTER) }),
            enabled = enabled,
            modifier = Modifier.fillMaxWidth(),
        )
        // The chips read `tick` so they redraw when the modifiers are cleared.
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
            FilterChip(selected = ctrl && tick >= 0, onClick = { ctrl = !ctrl }, label = { Text("Ctrl") })
            FilterChip(selected = alt && tick >= 0, onClick = { alt = !alt }, label = { Text("Alt") })
            FilterChip(selected = shift && tick >= 0, onClick = { shift = !shift }, label = { Text("Shift") })
            FilterChip(selected = meta && tick >= 0, onClick = { meta = !meta }, label = { Text(stringResource(R.string.input_meta)) })
        }
        KeyRow(
            enabled,
            "Esc" to { key(K_ESCAPE) }, "Tab" to { key(K_TAB) }, "Enter" to { key(K_ENTER) },
            "⌫" to { key(K_BACKSPACE) }, "Del" to { key(K_DELETE) },
        )
        KeyRow(
            enabled,
            "←" to { key(K_LEFT) }, "↑" to { key(K_UP) }, "↓" to { key(K_DOWN) }, "→" to { key(K_RIGHT) },
            "Home" to { key(K_HOME) }, "End" to { key(K_END) },
            "PgUp" to { key(K_PAGE_UP) }, "PgDn" to { key(K_PAGE_DOWN) },
        )
        KeyRow(enabled, *(0 until 12).map { i -> "F${i + 1}" to { key(K_F1 + i.toUInt()) } }.toTypedArray())
        Text(stringResource(R.string.input_shortcuts), style = MaterialTheme.typography.bodyMedium)
        KeyRow(
            enabled,
            stringResource(R.string.input_copy) to { key(charUsage('c') ?: 0x06u, CTRL) },
            stringResource(R.string.input_paste) to { key(charUsage('v') ?: 0x19u, CTRL) },
            stringResource(R.string.input_cut) to { key(charUsage('x') ?: 0x1Bu, CTRL) },
            stringResource(R.string.input_undo) to { key(charUsage('z') ?: 0x1Du, CTRL) },
            stringResource(R.string.input_select_all) to { key(charUsage('a') ?: 0x04u, CTRL) },
            "Alt+Tab" to { key(K_TAB, ALT) },
        )
    }
}

/** A scrolling row of key buttons. */
@Composable
private fun KeyRow(enabled: Boolean, vararg keys: Pair<String, () -> Unit>) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
        keys.forEach { (label, action) ->
            OutlinedButton(onClick = action, enabled = enabled, modifier = Modifier.heightIn(min = 44.dp)) { Text(label) }
        }
    }
}
