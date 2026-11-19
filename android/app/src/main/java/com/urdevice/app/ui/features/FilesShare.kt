package com.urdevice.app.ui.features

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import com.urdevice.app.Core
import com.urdevice.app.R
import com.urdevice.app.UrDeviceService
import com.urdevice.app.ui.UrDeviceTheme
import com.urdevice.core.FfiMode

/**
 * The share-sheet target for files (#17, R8.1, design §13): `ACTION_SEND` / `ACTION_SEND_MULTIPLE`.
 * Shows the connected devices that accept files and sends to the one the user taps.
 */
class ShareActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        UrDeviceService.start(this)
        val uris = sharedUris(intent)
        if (uris.isEmpty()) {
            finish()
            return
        }
        setContent {
            UrDeviceTheme {
                Surface {
                    val snap by Core.snapshot.collectAsState()
                    val peers = snap?.peers.orEmpty().filter { p ->
                        p.status == "connected" &&
                            p.remotePermissions.firstOrNull { it.capability == "files.v1" }?.mode != FfiMode.DENY
                    }
                    Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        Text(
                            stringResource(R.string.files_share_title, uris.size),
                            style = MaterialTheme.typography.titleMedium,
                        )
                        if (peers.isEmpty()) Text(stringResource(R.string.files_share_none))
                        peers.forEach { p ->
                            Card(
                                Modifier.fillMaxWidth().heightIn(min = 56.dp).clickable {
                                    val ok = FilesFeature.send(this@ShareActivity, p.id, uris)
                                    Toast.makeText(
                                        this@ShareActivity,
                                        getString(if (ok) R.string.files_share_started else R.string.files_share_failed, p.name),
                                        Toast.LENGTH_SHORT,
                                    ).show()
                                    finish()
                                },
                            ) { Text(p.name, Modifier.padding(16.dp), style = MaterialTheme.typography.bodyLarge) }
                        }
                        TextButton(onClick = { finish() }) { Text(stringResource(android.R.string.cancel)) }
                    }
                }
            }
        }
    }

    @Suppress("DEPRECATION")
    private fun sharedUris(i: Intent): List<Uri> = when (i.action) {
        Intent.ACTION_SEND -> listOfNotNull(
            if (Build.VERSION.SDK_INT >= 33) i.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
            else i.getParcelableExtra(Intent.EXTRA_STREAM),
        )
        Intent.ACTION_SEND_MULTIPLE ->
            (if (Build.VERSION.SDK_INT >= 33) i.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
            else i.getParcelableArrayListExtra(Intent.EXTRA_STREAM)).orEmpty()
        else -> emptyList()
    }.take(1000)
}
