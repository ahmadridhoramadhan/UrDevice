package com.urdevice.app.ui.features

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.text.format.Formatter
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.urdevice.app.MainActivity
import com.urdevice.app.R
import com.urdevice.app.host.FilesHost
import com.urdevice.app.host.UriSource
import com.urdevice.app.host.describe
import com.urdevice.core.FfiMode
import com.urdevice.core.FfiPeer
import com.urdevice.core.FfiSendFile
import com.urdevice.core.FfiTransfer
import com.urdevice.core.FfiTransferState
import com.urdevice.core.FilesListener
import com.urdevice.core.UrDeviceNode
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * File transfer (#17, R8): the Send files screen with the picker and the transfer list, progress
 * notifications with Cancel, and the share-sheet target ([com.urdevice.app.ShareActivity]).
 */
object FilesFeature : Feature {
    override val id = "files"

    override fun tiles(modes: Map<String, FfiMode>) = listOf(FeatureTile("files.v1", "files.v1", R.string.tile_send_files))

    override val hasScreen get() = true

    private val _transfers = MutableStateFlow<List<FfiTransfer>>(emptyList())
    /** Every transfer, newest first. */
    val transfers: StateFlow<List<FfiTransfer>> = _transfers.asStateFlow()

    @Volatile private var node: UrDeviceNode? = null
    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private val pending = AtomicBoolean(false)

    override fun onCoreStarted(context: Context, node: UrDeviceNode) {
        app = context.applicationContext
        this.node = node
        createChannel(app)
        node.filesSetHost(FilesHost(app))
        node.filesSetListener(object : FilesListener {
            override fun onChanged() = schedule()
            override fun onIncoming(transfer: FfiTransfer) = schedule()
        })
        refresh()
    }

    private fun schedule() {
        if (pending.getAndSet(true)) return
        main.post {
            pending.set(false)
            refresh()
        }
    }

    private fun refresh() {
        val list = node?.filesTransfers() ?: return
        _transfers.value = list
        Progress.update(app, list)
    }

    /** Send content URIs to [device]. Returns false when nothing could be read. */
    fun send(context: Context, device: String, uris: List<Uri>): Boolean {
        val n = node ?: return false
        val picked = describe(context, uris)
        if (picked.isEmpty()) return false
        return runCatching {
            n.filesSend(device, picked.map { FfiSendFile(it.name, it.size.toULong()) }, UriSource(context, picked.map { it.uri }))
        }.isSuccess
    }

    fun cancel(id: ULong) = node?.filesCancel(id)
    fun retry(id: ULong) = runCatching { node?.filesRetry(id) }
    fun clearFinished() {
        node?.filesClearFinished()
        refresh()
    }

    @Composable
    override fun Screen(peer: FfiPeer, tile: String) {
        val ctx = LocalContext.current
        val list by transfers.collectAsState()
        val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
            if (uris.isNotEmpty()) send(ctx, peer.id, uris)
        }
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            Text(stringResource(R.string.files_send_to, peer.name), style = MaterialTheme.typography.titleMedium)
            Text(stringResource(R.string.files_send_hint), style = MaterialTheme.typography.bodySmall)
            Button(onClick = { pick.launch(arrayOf("*/*")) }, modifier = Modifier.fillMaxWidth()) {
                Text(stringResource(R.string.files_choose))
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(stringResource(R.string.files_transfers), style = MaterialTheme.typography.titleSmall, modifier = Modifier.weight(1f))
                if (list.any { it.state.final() }) TextButton(onClick = { clearFinished() }) { Text(stringResource(R.string.files_clear)) }
            }
            if (list.isEmpty()) Text(stringResource(R.string.files_none), style = MaterialTheme.typography.bodySmall)
            list.forEach { TransferCard(it) }
        }
    }

    @Composable
    private fun TransferCard(t: FfiTransfer) {
        val ctx = LocalContext.current
        Card(Modifier.fillMaxWidth()) {
            Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(title(ctx, t), style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text(
                            stringResource(if (t.outgoing) R.string.files_to else R.string.files_from, t.deviceName),
                            style = MaterialTheme.typography.bodySmall,
                        )
                    }
                    if (!t.state.final()) {
                        OutlinedButton(onClick = { cancel(t.id) }) { Text(stringResource(R.string.files_cancel)) }
                    }
                    if (t.outgoing && (t.state == FfiTransferState.INTERRUPTED || t.state == FfiTransferState.FAILED)) {
                        Spacer(Modifier.width(8.dp))
                        OutlinedButton(onClick = { retry(t.id) }) { Text(stringResource(R.string.files_retry)) }
                    }
                }
                if (t.state == FfiTransferState.RUNNING || t.state == FfiTransferState.INTERRUPTED) {
                    LinearProgressIndicator(progress = { fraction(t) }, modifier = Modifier.fillMaxWidth())
                }
                Text(status(ctx, t), style = MaterialTheme.typography.bodySmall)
            }
        }
    }

    fun FfiTransferState.final() = this == FfiTransferState.DONE || this == FfiTransferState.FAILED ||
        this == FfiTransferState.CANCELLED || this == FfiTransferState.DECLINED

    fun fraction(t: FfiTransfer): Float =
        if (t.total == 0UL) (if (t.state == FfiTransferState.DONE) 1f else 0f) else t.done.toFloat() / t.total.toFloat()

    fun title(ctx: Context, t: FfiTransfer): String {
        val first = t.names.firstOrNull()?.substringAfterLast('/') ?: ""
        return if (t.names.size > 1) ctx.getString(R.string.files_and_more, first, t.names.size - 1) else first
    }

    private fun size(ctx: Context, b: ULong) = Formatter.formatShortFileSize(ctx, b.toLong())

    fun status(ctx: Context, t: FfiTransfer): String = when (t.state) {
        FfiTransferState.ASKING -> ctx.getString(R.string.files_waiting, t.deviceName)
        FfiTransferState.RUNNING -> {
            val pct = (fraction(t) * 100).toInt()
            val eta = t.etaSecs
            if (eta != null && t.speed > 0UL) {
                ctx.getString(R.string.files_progress_eta, pct, size(ctx, t.done), size(ctx, t.total), size(ctx, t.speed), etaText(ctx, eta.toLong()))
            } else {
                ctx.getString(R.string.files_progress, pct, size(ctx, t.done), size(ctx, t.total))
            }
        }
        FfiTransferState.INTERRUPTED -> ctx.getString(R.string.files_interrupted, size(ctx, t.done), size(ctx, t.total), t.deviceName)
        FfiTransferState.DONE -> if (t.outgoing) ctx.getString(R.string.files_sent, size(ctx, t.total))
            else ctx.getString(R.string.files_saved, t.saved.firstOrNull() ?: "Download/UrDevice")
        FfiTransferState.FAILED -> when {
            t.failed > 0u -> ctx.getString(R.string.files_failed_check, t.failed.toInt())
            t.reason == "storage" -> ctx.getString(R.string.files_failed_storage)
            else -> ctx.getString(R.string.files_failed)
        }
        FfiTransferState.CANCELLED -> if (t.reason == "peer") ctx.getString(R.string.files_cancelled_by, t.deviceName)
            else ctx.getString(R.string.files_cancelled)
        FfiTransferState.DECLINED -> when (t.reason) {
            "denied" -> ctx.getString(R.string.files_denied, t.deviceName)
            "invalid" -> ctx.getString(R.string.files_invalid, t.deviceName)
            "busy" -> ctx.getString(R.string.files_busy, t.deviceName)
            else -> ctx.getString(R.string.files_declined, t.deviceName)
        }
    }

    private fun etaText(ctx: Context, s: Long) = when {
        s >= 3600 -> ctx.getString(R.string.files_eta_h, s / 3600)
        s >= 60 -> ctx.getString(R.string.files_eta_min, (s + 59) / 60)
        else -> ctx.getString(R.string.files_eta_s, s)
    }

    // ------------------------------------------------------------------ notifications

    private const val CHANNEL = "transfers"
    const val ACTION_CANCEL = "com.urdevice.app.files.CANCEL"
    const val EXTRA_ID = "id"

    private fun createChannel(ctx: Context) {
        ctx.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, ctx.getString(R.string.files_channel), NotificationManager.IMPORTANCE_LOW),
        )
    }

    /** One notification per transfer, with progress and Cancel (R8.3). */
    private object Progress {
        private val shown = HashMap<ULong, FfiTransferState>()
        private val last = HashMap<ULong, Long>()

        fun update(ctx: Context, list: List<FfiTransfer>) {
            if (Build.VERSION.SDK_INT >= 33 &&
                ctx.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) return
            val nm = ctx.getSystemService(NotificationManager::class.java)
            for (t in list) {
                val before = shown[t.id]
                if (before != null && before.let { it == t.state } && t.state.final()) continue
                if (before == null && t.state.final()) continue
                // Android drops updates that come faster than a few per second.
                val now = android.os.SystemClock.elapsedRealtime()
                if (before == t.state && t.state == FfiTransferState.RUNNING && now - (last[t.id] ?: 0L) < 500) continue
                last[t.id] = now
                shown[t.id] = t.state
                nm.notify(notifId(t.id), build(ctx, t))
            }
        }

        private fun notifId(id: ULong) = 50_000 + (id % 10_000UL).toInt()

        private fun build(ctx: Context, t: FfiTransfer): Notification {
            val open = PendingIntent.getActivity(
                ctx, 0, Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP), PendingIntent.FLAG_IMMUTABLE,
            )
            val b = Notification.Builder(ctx, CHANNEL)
                .setSmallIcon(R.drawable.ic_urdevice_symbol)
                .setContentTitle(
                    ctx.getString(if (t.outgoing) R.string.files_notif_sending else R.string.files_notif_receiving, title(ctx, t), t.deviceName),
                )
                .setContentText(status(ctx, t))
                .setContentIntent(open)
                .setOnlyAlertOnce(true)
            if (!t.state.final()) {
                b.setOngoing(true)
                b.setProgress(1000, (fraction(t) * 1000).toInt(), t.state == FfiTransferState.ASKING)
                val cancel = PendingIntent.getBroadcast(
                    ctx, notifId(t.id),
                    Intent(ctx, CancelReceiver::class.java).setAction(ACTION_CANCEL).putExtra(EXTRA_ID, t.id.toLong()),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                b.addAction(Notification.Action.Builder(null, ctx.getString(R.string.files_cancel), cancel).build())
            } else {
                b.setAutoCancel(true)
            }
            return b.build()
        }
    }
}

/** Cancel on a transfer notification. */
class CancelReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != FilesFeature.ACTION_CANCEL) return
        val id = intent.getLongExtra(FilesFeature.EXTRA_ID, -1)
        if (id >= 0) FilesFeature.cancel(id.toULong())
    }
}
