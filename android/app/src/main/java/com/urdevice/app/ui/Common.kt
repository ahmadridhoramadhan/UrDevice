package com.urdevice.app.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Adb
import androidx.compose.material.icons.outlined.Cast
import androidx.compose.material.icons.outlined.Code
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.DesktopWindows
import androidx.compose.material.icons.outlined.Edit
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.FolderOpen
import androidx.compose.material.icons.outlined.History
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Keyboard
import androidx.compose.material.icons.outlined.Laptop
import androidx.compose.material.icons.outlined.Lock
import androidx.compose.material.icons.outlined.Mic
import androidx.compose.material.icons.outlined.Mouse
import androidx.compose.material.icons.outlined.Notifications
import androidx.compose.material.icons.outlined.NotificationsActive
import androidx.compose.material.icons.outlined.PhoneAndroid
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Slideshow
import androidx.compose.material.icons.outlined.Sms
import androidx.compose.material.icons.outlined.Speaker
import androidx.compose.material.icons.outlined.SwapHoriz
import androidx.compose.material.icons.outlined.Terminal
import androidx.compose.material.icons.outlined.Videocam
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedButton
import androidx.compose.material3.SegmentedButtonDefaults
import androidx.compose.material3.SingleChoiceSegmentedButtonRow
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.urdevice.app.Labels
import com.urdevice.app.R
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPermission
import com.urdevice.core.qrMatrix

fun capIcon(cap: String): ImageVector = when (cap) {
    "clipboard.v1" -> Icons.Outlined.ContentPaste
    "files.v1" -> Icons.Outlined.Folder
    "gallery.v1" -> Icons.Outlined.Image
    "input.v1" -> Icons.Outlined.Mouse
    "keyboard" -> Icons.Outlined.Keyboard
    "presenter" -> Icons.Outlined.Slideshow
    "media.v1" -> Icons.Outlined.PlayCircle
    "screen.h264.v1" -> Icons.Outlined.DesktopWindows
    "tunnel.v1" -> Icons.Outlined.Code
    "term.v1" -> Icons.Outlined.Terminal
    "notify.v1" -> Icons.Outlined.Notifications
    "ring.v1" -> Icons.Outlined.NotificationsActive
    "camera.v1" -> Icons.Outlined.Videocam
    "mic.v1" -> Icons.Outlined.Mic
    "telephony.v1" -> Icons.Outlined.Sms
    "kvm.v1" -> Icons.Outlined.SwapHoriz
    "display.v1" -> Icons.Outlined.Cast
    "audio.out.v1" -> Icons.Outlined.Speaker
    "cliphist.v1" -> Icons.Outlined.History
    "lock.v1" -> Icons.Outlined.Lock
    "adb.v1" -> Icons.Outlined.Adb
    "fs.browse.v1" -> Icons.Outlined.FolderOpen
    else -> Icons.Outlined.Edit
}

fun platformIcon(platform: String): ImageVector =
    if (platform == "android") Icons.Outlined.PhoneAndroid else Icons.Outlined.Laptop

fun platformName(p: String) = when (p) {
    "linux" -> "Linux"
    "windows" -> "Windows"
    "macos" -> "macOS"
    "android" -> "Android"
    else -> p
}

@Composable
fun IconBox(icon: ImageVector, size: Int = 40) {
    Box(
        Modifier.size(size.dp).background(MaterialTheme.colorScheme.secondaryContainer, RoundedCornerShape(10.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = MaterialTheme.colorScheme.primary, modifier = Modifier.size((size / 2).dp))
    }
}

/** Allow / Always Ask / Deny, with the exact labels from ui.md. */
@Composable
fun ModeSelector(mode: FfiMode, onPick: (FfiMode) -> Unit) {
    val modes = listOf(FfiMode.ALLOW to R.string.allow, FfiMode.ASK to R.string.always_ask, FfiMode.DENY to R.string.deny)
    val c = LocalModes.current
    SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
        modes.forEachIndexed { i, (m, label) ->
            val (bg, fg) = when (m) {
                FfiMode.ALLOW -> c.allowBg to c.allowFg
                FfiMode.ASK -> c.askBg to c.askFg
                FfiMode.DENY -> c.denyBg to c.denyFg
            }
            SegmentedButton(
                selected = mode == m,
                onClick = { onPick(m) },
                shape = SegmentedButtonDefaults.itemShape(i, modes.size),
                icon = {},
                colors = SegmentedButtonDefaults.colors(activeContainerColor = bg, activeContentColor = fg),
            ) { Text(stringResource(label), fontSize = 13.sp, maxLines = 1) }
        }
    }
}

@Composable
fun PermissionRow(p: FfiPermission, onPick: (FfiMode) -> Unit) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                IconBox(capIcon(p.capability), 36)
                Spacer(Modifier.width(12.dp))
                Column {
                    Text(stringResource(Labels.cap(p.capability)), fontWeight = FontWeight.SemiBold)
                    Text(
                        stringResource(Labels.capDesc(p.capability)),
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            ModeSelector(p.mode, onPick)
        }
    }
}

/** This device's pairing QR code, drawn from the core's matrix. */
@Composable
fun QrCode(text: String, modifier: Modifier = Modifier) {
    val m = remember(text) { runCatching { qrMatrix(text) }.getOrNull() }
    Box(modifier.background(Color.White, RoundedCornerShape(12.dp)).padding(12.dp)) {
        if (m != null) {
            Canvas(Modifier.size(220.dp)) {
                val n = m.size.toInt()
                val cell = size.width / n
                for (y in 0 until n) for (x in 0 until n) {
                    if (m.modules[y * n + x]) {
                        drawRect(
                            Forest,
                            topLeft = androidx.compose.ui.geometry.Offset(x * cell, y * cell),
                            size = androidx.compose.ui.geometry.Size(cell + 0.5f, cell + 0.5f),
                        )
                    }
                }
            }
        }
    }
}

@Composable
fun ConfirmDialog(title: String, body: String, confirm: String, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = { Text(body) },
        confirmButton = { TextButton(onClick = onConfirm) { Text(confirm, color = LocalModes.current.denyFg) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}
