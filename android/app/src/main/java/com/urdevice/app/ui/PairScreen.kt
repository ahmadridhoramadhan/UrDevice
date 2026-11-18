package com.urdevice.app.ui

import android.Manifest
import android.content.pm.PackageManager
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.view.PreviewView
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.google.mlkit.vision.barcode.BarcodeScannerOptions
import com.google.mlkit.vision.barcode.BarcodeScanning
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.common.InputImage
import com.urdevice.app.R
import com.urdevice.core.FfiCodePairing
import com.urdevice.core.FfiSnapshot
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Add Device (R3): scanner with CameraX + ML Kit, this device's QR code with
 * a countdown, and the nearby list with code confirmation.
 */
@Composable
fun PairBody(
    snap: FfiSnapshot,
    onScanned: (String) -> Unit,
    onPairNearby: (String) -> Unit,
    onConfirm: (String, Boolean) -> Unit,
) {
    val ctx = LocalContext.current
    val hasCamera = remember { ctx.packageManager.hasSystemFeature(PackageManager.FEATURE_CAMERA_ANY) }
    var showQr by remember { mutableStateOf(!hasCamera) }
    var granted by remember {
        mutableStateOf(ctx.checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED)
    }
    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { granted = it }

    androidx.compose.runtime.CompositionLocalProvider(
        androidx.compose.material3.LocalContentColor provides MaterialTheme.colorScheme.onBackground,
    ) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (!showQr && hasCamera) {
            if (granted) {
                Text(stringResource(R.string.pair_scan_body), color = MaterialTheme.colorScheme.onSurfaceVariant)
                QrScanner(onScanned)
            } else {
                Explain(
                    title = stringResource(R.string.why_camera_title),
                    body = stringResource(R.string.why_camera_body),
                    onGrant = { ask.launch(Manifest.permission.CAMERA) },
                )
            }
            OutlinedButton(onClick = { showQr = true }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                Text(stringResource(R.string.pair_show_qr))
            }
        } else {
            val screen = snap.pairScreen
            Text(stringResource(R.string.pair_qr_body), color = MaterialTheme.colorScheme.onSurfaceVariant)
            Column(Modifier.fillMaxWidth(), horizontalAlignment = Alignment.CenterHorizontally) {
                when {
                    screen == null -> {}
                    screen.blockedByHidden -> Text(stringResource(R.string.pair_hidden))
                    screen.lockedSecs > 0u -> Text(stringResource(R.string.pair_locked, screen.lockedSecs.toInt()))
                    else -> {
                        QrCode(screen.qrUri)
                        Text(
                            stringResource(R.string.pair_valid_for, screen.expiresInSecs.toInt()),
                            style = MaterialTheme.typography.bodySmall,
                            modifier = Modifier.padding(top = 6.dp),
                        )
                    }
                }
            }
            if (hasCamera) {
                OutlinedButton(onClick = { showQr = false }, modifier = Modifier.fillMaxWidth().heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.pair_scan))
                }
            }
        }

        Text(stringResource(R.string.pair_nearby), fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(top = 8.dp))
        Text(stringResource(R.string.pair_nearby_body), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        snap.codePairings.forEach { CodeCard(it, onConfirm) }
        val nearby = snap.nearby.filter { n -> snap.codePairings.none { it.key.startsWith(n.key) } }
        if (nearby.isEmpty() && snap.codePairings.isEmpty()) {
            Text(stringResource(R.string.pair_none), style = MaterialTheme.typography.bodySmall)
        }
        nearby.forEach { n ->
            Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surface)) {
                Row(Modifier.fillMaxWidth().padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                    IconBox(platformIcon(n.platform), 36)
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(n.name, fontWeight = FontWeight.SemiBold)
                        Text("${platformName(n.platform)} · ${n.address}", style = MaterialTheme.typography.bodySmall)
                    }
                    Button(onClick = { onPairNearby(n.key) }, modifier = Modifier.heightIn(min = 44.dp)) {
                        Text(stringResource(R.string.pair_button))
                    }
                }
            }
        }
    }
    }
}

@Composable
private fun CodeCard(c: FfiCodePairing, onConfirm: (String, Boolean) -> Unit) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
        Column(Modifier.fillMaxWidth().padding(14.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(
                if (c.incoming) stringResource(R.string.pair_wants, c.name) else stringResource(R.string.pair_check, c.name),
                style = MaterialTheme.typography.bodySmall,
            )
            Text(
                "${c.code.take(3)} ${c.code.drop(3)}",
                fontSize = 32.sp,
                fontWeight = FontWeight.Bold,
                fontFamily = FontFamily.Monospace,
                letterSpacing = 4.sp,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                if (c.confirmedHere) {
                    Text(stringResource(R.string.pair_waiting), modifier = Modifier.weight(1f).align(Alignment.CenterVertically))
                } else {
                    Button(onClick = { onConfirm(c.key, true) }, modifier = Modifier.weight(1f).heightIn(min = 48.dp)) {
                        Text(stringResource(R.string.codes_match))
                    }
                }
                OutlinedButton(onClick = { onConfirm(c.key, false) }, modifier = Modifier.heightIn(min = 48.dp)) {
                    Text(stringResource(R.string.cancel))
                }
            }
        }
    }
}

@Composable
fun Explain(title: String, body: String, onGrant: () -> Unit, onLater: (() -> Unit)? = null) {
    Card(colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceVariant)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(title, fontWeight = FontWeight.SemiBold)
            Text(body, style = MaterialTheme.typography.bodyMedium)
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Button(onClick = onGrant, modifier = Modifier.heightIn(min = 44.dp)) { Text(stringResource(R.string.grant)) }
                if (onLater != null) {
                    OutlinedButton(onClick = onLater, modifier = Modifier.heightIn(min = 44.dp)) { Text(stringResource(R.string.not_now)) }
                }
            }
        }
    }
}

/**
 * Camera preview that reports the first UrDevice pairing code. Frames are
 * analyzed in memory and never stored. The camera unbinds when this leaves
 * the composition (R3.10).
 */
@Composable
fun QrScanner(onScanned: (String) -> Unit) {
    val ctx = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val done = remember { AtomicBoolean(false) }
    val executor = remember { Executors.newSingleThreadExecutor() }
    val scanner = remember {
        BarcodeScanning.getClient(BarcodeScannerOptions.Builder().setBarcodeFormats(Barcode.FORMAT_QR_CODE).build())
    }
    var provider by remember { mutableStateOf<ProcessCameraProvider?>(null) }
    DisposableEffect(Unit) {
        onDispose {
            provider?.unbindAll()
            scanner.close()
            executor.shutdown()
        }
    }
    AndroidView(
        modifier = Modifier.fillMaxWidth().aspectRatio(1f).clip(RoundedCornerShape(16.dp)),
        factory = { c ->
            val view = PreviewView(c)
            val future = ProcessCameraProvider.getInstance(c)
            future.addListener({
                val p = future.get()
                provider = p
                val preview = Preview.Builder().build().also { it.surfaceProvider = view.surfaceProvider }
                val analysis = ImageAnalysis.Builder()
                    .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
                    .build()
                analysis.setAnalyzer(executor) { proxy ->
                    val media = proxy.image
                    if (media == null || done.get()) {
                        proxy.close()
                        return@setAnalyzer
                    }
                    val img = InputImage.fromMediaImage(media, proxy.imageInfo.rotationDegrees)
                    scanner.process(img)
                        .addOnSuccessListener { codes ->
                            val text = codes.firstNotNullOfOrNull { it.rawValue?.takeIf { v -> v.startsWith("urdevice://pair?") } }
                            if (text != null && done.compareAndSet(false, true)) {
                                ContextCompat.getMainExecutor(ctx).execute { onScanned(text) }
                            }
                        }
                        .addOnCompleteListener { proxy.close() }
                }
                p.unbindAll()
                p.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
            }, ContextCompat.getMainExecutor(c))
            view
        },
    )
}
