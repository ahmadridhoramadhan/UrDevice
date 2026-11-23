package com.urdevice.app.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
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
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.sp
import com.urdevice.app.R
import com.urdevice.core.FfiPermission
import com.urdevice.core.FfiSnapshot
import com.urdevice.core.capabilitiesForAndroid
import com.urdevice.core.defaultMode

/** First start: name, permissions, pair, done. Four steps, like the desktop app. */
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
    // Another device asked to pair and the user accepted: show the code.
    val hasCodes = snap.codePairings.isNotEmpty()
    LaunchedEffect(hasCodes) { if (hasCodes && step < 3) step = 3 }
    // Pairing finished while the pair step was open: jump to the last step.
    val shown = if (pairedName != null && step == 3) 4 else step

    Column(
        Modifier.fillMaxSize().background(Forest).safeDrawingPadding().imePadding().padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(16.dp),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            repeat(4) { i ->
                Box(
                    Modifier.weight(1f).height(4.dp)
                        .background(if (i < shown) Lime else Color2, RoundedCornerShape(2.dp)),
                )
            }
        }
        // Steps slide and fade: forward goes left, back goes right.
        AnimatedContent(
            targetState = shown,
            modifier = Modifier.weight(1f).fillMaxWidth(),
            transitionSpec = {
                val dir = if (targetState > initialState) 1 else -1
                (slideInHorizontally(tween(280)) { dir * it / 5 } + fadeIn(tween(280))) togetherWith
                    (slideOutHorizontally(tween(200)) { -dir * it / 5 } + fadeOut(tween(160)))
            },
            label = "onboarding-step",
        ) { page ->
            when (page) {
                1 -> Column(
                    Modifier.fillMaxSize(),
                    verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Badge(Icons.Outlined.PanTool, 88.dp)
                    Title(stringResource(R.string.ob_welcome), center = true)
                    Body(stringResource(R.string.ob_welcome_body), center = true)
                    Spacer(Modifier.height(12.dp))
                    Text(
                        stringResource(R.string.ob_name),
                        color = Mist,
                        fontWeight = FontWeight.SemiBold,
                        modifier = Modifier.align(Alignment.Start),
                    )
                    OutlinedTextField(
                        value = name,
                        onValueChange = { name = it.take(64) },
                        singleLine = true,
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
                2 -> Column(
                    Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Header(Icons.Outlined.Shield, stringResource(R.string.ob_perms), stringResource(R.string.ob_perms_body))
                    UrDeviceTheme {
                        Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                            perms.forEachIndexed { i, p -> PermissionRow(p) { m -> perms[i] = p.copy(mode = m) } }
                        }
                    }
                }
                3 -> Column(
                    Modifier.fillMaxSize().verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(12.dp),
                ) {
                    Header(Icons.Outlined.QrCode2, stringResource(R.string.ob_pair))
                    UrDeviceTheme {
                        Column(
                            Modifier.fillMaxWidth()
                                .background(MaterialTheme.colorScheme.background, RoundedCornerShape(16.dp))
                                .padding(16.dp),
                            verticalArrangement = Arrangement.spacedBy(10.dp),
                        ) {
                            if (notificationsNeeded) {
                                Explain(
                                    stringResource(R.string.why_notifications_title),
                                    stringResource(R.string.why_notifications_body),
                                    onGrant = onAskNotifications,
                                )
                            }
                            PairBody(snap, onScanned, onPairNearby, onConfirm, compact = true)
                        }
                    }
                }
                else -> Column(
                    Modifier.fillMaxSize(),
                    verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Badge(Icons.Outlined.Check, 88.dp)
                    Title(
                        if (pairedName != null) stringResource(R.string.ob_paired, pairedName) else stringResource(R.string.ob_ready),
                        center = true,
                    )
                    Body(stringResource(if (pairedName != null) R.string.ob_paired_body else R.string.ob_ready_body), center = true)
                }
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            if (shown in 2..3) {
                OutlinedButton(
                    onClick = {
                        if (shown == 3) onClosePair()
                        step -= 1
                    },
                    modifier = Modifier.heightIn(min = 48.dp),
                ) {
                    Text(stringResource(R.string.back), color = Mist)
                }
            }
            Spacer(Modifier.weight(1f))
            when (shown) {
                1 -> LimeButton(stringResource(R.string.continue_), enabled = name.isNotBlank()) { step = 2 }
                2 -> LimeButton(stringResource(R.string.continue_)) {
                    onSave(name, perms.toList())
                    onOpenPair()
                    step = 3
                }
                3 -> OutlinedButton(onClick = { onClosePair(); step = 4 }, modifier = Modifier.heightIn(min = 48.dp)) {
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
private fun Title(t: String, size: TextUnit = 24.sp, center: Boolean = false) =
    Text(t, color = Mist, fontSize = size, fontWeight = FontWeight.Bold, textAlign = if (center) TextAlign.Center else null)

@Composable
private fun Body(t: String, center: Boolean = false) =
    Text(t, color = androidx.compose.ui.graphics.Color(0xFFB9CDB4), textAlign = if (center) TextAlign.Center else null)

@Composable
private fun LimeButton(text: String, enabled: Boolean = true, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = Modifier.heightIn(min = 48.dp),
        colors = androidx.compose.material3.ButtonDefaults.buttonColors(containerColor = Lime, contentColor = Forest),
    ) { Text(text, fontWeight = FontWeight.SemiBold) }
}

@Composable
private fun Badge(icon: ImageVector, size: Dp) {
    Box(Modifier.size(size).background(Lime, RoundedCornerShape(size / 4)), contentAlignment = Alignment.Center) {
        Icon(icon, contentDescription = null, tint = Forest, modifier = Modifier.size(size / 2))
    }
}

@Composable
private fun Header(icon: ImageVector, title: String, subtitle: String? = null) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        Badge(icon, 48.dp)
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Title(title, size = 22.sp)
            if (subtitle != null) Body(subtitle)
        }
    }
}
