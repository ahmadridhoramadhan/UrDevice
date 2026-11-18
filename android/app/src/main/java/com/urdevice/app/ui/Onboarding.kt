package com.urdevice.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.Check
import androidx.compose.material.icons.outlined.PanTool
import androidx.compose.material.icons.outlined.QrCode2
import androidx.compose.material.icons.outlined.Shield
import androidx.compose.material.icons.outlined.Tune
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.urdevice.app.R
import com.urdevice.core.FfiPermission
import com.urdevice.core.FfiSnapshot
import com.urdevice.core.capabilitiesForAndroid
import com.urdevice.core.defaultMode

/** First start (docs/mockup/onboarding.html): name, safe and local, permissions, pair, done. */
@Composable
fun Onboarding(
    snap: FfiSnapshot,
    pairedName: String?,
    notificationsNeeded: Boolean,
    onAskNotifications: () -> Unit,
    onSave: (String, List<FfiPermission>) -> Unit,
    onOpenPair: () -> Unit,
    onClosePair: () -> Unit,
    onScanned: (String) -> Unit,
    onPairNearby: (String) -> Unit,
    onConfirm: (String, Boolean) -> Unit,
    onFinish: () -> Unit,
) {
    var step by rememberSaveable { mutableStateOf(1) }
    var name by rememberSaveable { mutableStateOf(snap.me.name) }
    val perms = remember {
        mutableStateListOf<FfiPermission>().apply {
            capabilitiesForAndroid()
                .filter { it != "term.v1" && it != "fs.write.v1" }
                .forEach { add(FfiPermission(it, defaultMode(it), false)) }
        }
    }
    val shown = if (pairedName != null && step in 4..5) 6 else step

    Column(
        Modifier.fillMaxSize().background(Forest).safeDrawingPadding().imePadding().padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            repeat(6) { i ->
                Box(
                    Modifier.weight(1f).height(4.dp)
                        .background(if (i < shown) Lime else Color2, RoundedCornerShape(2.dp)),
                )
            }
        }
        Box(Modifier.size(56.dp).background(Lime, RoundedCornerShape(14.dp)), contentAlignment = androidx.compose.ui.Alignment.Center) {
            Icon(
                when (shown) {
                    1 -> Icons.Outlined.PanTool
                    2 -> Icons.Outlined.Shield
                    3 -> Icons.Outlined.Tune
                    6 -> Icons.Outlined.Check
                    else -> Icons.Outlined.QrCode2
                },
                contentDescription = null, tint = Forest,
            )
        }
        Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            when (shown) {
                1 -> {
                    Title(stringResource(R.string.ob_welcome))
                    Body(stringResource(R.string.ob_welcome_body))
                    Text(stringResource(R.string.ob_name), color = Mist, fontWeight = FontWeight.SemiBold)
                    OutlinedTextField(value = name, onValueChange = { name = it.take(64) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Body(stringResource(R.string.ob_name_hint))
                }
                2 -> {
                    Title(stringResource(R.string.ob_safe))
                    Body(stringResource(R.string.ob_safe_body))
                    Body(stringResource(R.string.ob_safe_key))
                    Box(Modifier.fillMaxWidth().background(Color(0xFF0C180D), RoundedCornerShape(8.dp)).padding(12.dp)) {
                        Text(snap.me.fingerprint, color = Lime, fontFamily = FontFamily.Monospace, fontSize = 15.sp)
                    }
                    Body(stringResource(R.string.ob_safe_check))
                }
                3 -> {
                    Title(stringResource(R.string.ob_perms))
                    Body(stringResource(R.string.ob_perms_body))
                    UrDeviceTheme {
                        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            perms.forEachIndexed { i, p -> PermissionRow(p) { m -> perms[i] = p.copy(mode = m) } }
                        }
                    }
                }
                4, 5 -> {
                    Title(stringResource(R.string.ob_pair))
                    UrDeviceTheme {
                        Column(
                            Modifier.background(MaterialTheme.colorScheme.background, RoundedCornerShape(12.dp)).padding(12.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            if (notificationsNeeded) {
                                Explain(
                                    stringResource(R.string.why_notifications_title),
                                    stringResource(R.string.why_notifications_body),
                                    onGrant = onAskNotifications,
                                )
                            }
                            PairBody(snap, onScanned, onPairNearby, onConfirm)
                        }
                    }
                }
                else -> {
                    Title(if (pairedName != null) stringResource(R.string.ob_paired, pairedName) else stringResource(R.string.ob_ready))
                    Body(stringResource(if (pairedName != null) R.string.ob_paired_body else R.string.ob_ready_body))
                }
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (shown in 2..3) {
                OutlinedButton(onClick = { step -= 1 }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.back), color = Mist)
                }
            }
            Spacer(Modifier.weight(1f))
            when (shown) {
                1, 2 -> LimeButton(stringResource(R.string.continue_), enabled = name.isNotBlank()) { step += 1 }
                3 -> LimeButton(stringResource(R.string.ob_save)) {
                    onSave(name, perms.toList())
                    onOpenPair()
                    step = 4
                }
                4, 5 -> OutlinedButton(onClick = { onClosePair(); step = 6 }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.ob_skip), color = Mist)
                }
                else -> LimeButton(stringResource(R.string.ob_open)) {
                    onClosePair()
                    onFinish()
                }
            }
        }
    }
}

private val Mist = androidx.compose.ui.graphics.Color(0xFFE8F5DC)
private val Color2 = androidx.compose.ui.graphics.Color(0xFF2A3D2C)

@Suppress("FunctionName")
private fun Color(v: Long) = androidx.compose.ui.graphics.Color(v)

@Composable
private fun Title(t: String) = Text(t, color = Mist, fontSize = 24.sp, fontWeight = FontWeight.Bold)

@Composable
private fun Body(t: String) = Text(t, color = androidx.compose.ui.graphics.Color(0xFFB9CDB4))

@Composable
private fun LimeButton(text: String, enabled: Boolean = true, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.heightIn(min = 48.dp),
        colors = androidx.compose.material3.ButtonDefaults.buttonColors(containerColor = Lime, contentColor = Forest),
    ) { Text(text, fontWeight = FontWeight.SemiBold) }
}
