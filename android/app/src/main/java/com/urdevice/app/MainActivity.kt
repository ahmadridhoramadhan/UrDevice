package com.urdevice.app

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
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
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.urdevice.app.ui.ActiveBanner
import com.urdevice.app.ui.DeviceScreen
import com.urdevice.app.ui.Explain
import com.urdevice.app.ui.HomeScreen
import com.urdevice.app.ui.LogScreen
import com.urdevice.app.ui.Onboarding
import com.urdevice.app.ui.PairBody
import com.urdevice.app.ui.SettingsScreen
import com.urdevice.app.ui.ThisDeviceScreen
import com.urdevice.app.ui.UrDeviceTheme
import com.urdevice.app.ui.features.feature
import com.urdevice.core.FfiEvent
import com.urdevice.core.FfiException
import com.urdevice.core.FfiLogEntry
import com.urdevice.core.UrDeviceNode
import com.urdevice.core.coreVersion
import kotlinx.coroutines.delay

/** Pages with a back button (Android v1 in docs/mockup/urdevice-layout.html). */
sealed interface Page {
    data object Home : Page
    data object ThisDevice : Page
    data class Device(val id: String) : Page
    data object Pair : Page
    data object Settings : Page
    data object Log : Page

    /** A feature screen (ui/features/) for a paired device, opened from the tile with key [tile]. */
    data class Feature(val device: String, val feature: String, val tile: String) : Page
}

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        UrDeviceService.start(this)
        setContent { UrDeviceTheme { App() } }
    }

    override fun onStart() {
        super.onStart()
        Core.foreground.value = true
        Core.refresh()
    }

    override fun onStop() {
        Core.foreground.value = false
        super.onStop()
    }
}

private fun needsNotificationPermission(ctx: android.content.Context) = Build.VERSION.SDK_INT >= 33 &&
    ctx.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED

private fun needsNetworkPermission(ctx: android.content.Context) = Build.VERSION.SDK_INT >= 37 &&
    ctx.checkSelfPermission("android.permission.ACCESS_LOCAL_NETWORK") != PackageManager.PERMISSION_GRANTED

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun App() {
    val ctx = LocalContext.current
    val snap by Core.snapshot.collectAsStateWithLifecycle()
    val prompts by Core.prompts.collectAsStateWithLifecycle()
    val snackbar = remember { SnackbarHostState() }
    val stack = remember { mutableStateListOf<Page>(Page.Home) }
    var pairedName by rememberSaveable { mutableStateOf<String?>(null) }
    var inOnboarding by rememberSaveable { mutableStateOf<Boolean?>(null) }
    var notifNeeded by remember { mutableStateOf(needsNotificationPermission(ctx)) }
    var netNeeded by remember { mutableStateOf(needsNetworkPermission(ctx)) }
    var log by remember { mutableStateOf<List<FfiLogEntry>>(emptyList()) }
    val askNotif = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        notifNeeded = needsNotificationPermission(ctx)
    }
    val askNet = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) {
        netNeeded = needsNetworkPermission(ctx)
    }
    val page = stack.last()

    fun node(): UrDeviceNode? = Core.node()
    fun go(p: Page) {
        if (p == Page.Pair) node()?.openPairScreen()
        stack.add(p)
    }
    fun back() {
        if (stack.size > 1) {
            if (stack.removeAt(stack.lastIndex) == Page.Pair) node()?.closePairScreen()
        }
    }
    fun pairNearby(key: String) {
        val name = snap?.nearby?.firstOrNull { it.key == key }?.name ?: ""
        if (runCatching { node()?.pairNearby(key) }.isSuccess) {
            android.widget.Toast.makeText(ctx, ctx.getString(R.string.pair_waiting_for, name), android.widget.Toast.LENGTH_SHORT).show()
        }
    }
    fun scanned(text: String) {
        try {
            node()?.scanQr(text)
        } catch (e: FfiException) {
            val reason = (e as? FfiException.Pairing)?.reason ?: "invalid"
            pairedName = null
            android.widget.Toast.makeText(ctx, ctx.getString(Labels.pairingFailed(reason)), android.widget.Toast.LENGTH_LONG).show()
        }
    }

    LaunchedEffect(Unit) {
        Core.events.collect { e ->
            if (e is FfiEvent.PairingFinished && e.ok) {
                pairedName = e.name
                if (stack.last() == Page.Pair) back()
            }
            Labels.event(ctx, e)?.let { snackbar.showSnackbar(it) }
        }
    }

    val s = snap
    if (s == null) {
        Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { CircularProgressIndicator() }
        LaunchedEffect(Unit) {
            while (Core.node() == null) delay(100)
            Core.refresh()
        }
        return
    }
    // Another device asked to pair and the user accepted: show the code.
    val hasCodes = s.codePairings.isNotEmpty()
    LaunchedEffect(hasCodes) {
        if (hasCodes && inOnboarding != true && stack.last() != Page.Pair) go(Page.Pair)
    }
    if (inOnboarding == null) inOnboarding = !s.me.onboarded
    if (!s.me.onboarded && inOnboarding == false) inOnboarding = true

    if (inOnboarding == true) {
        Onboarding(
            snap = s,
            pairedName = pairedName,
            notificationsNeeded = notifNeeded,
            onAskNotifications = { askNotif.launch(Manifest.permission.POST_NOTIFICATIONS) },
            onSave = { name, perms -> runCatching { node()?.completeOnboarding(name, perms) } },
            onOpenPair = { node()?.openPairScreen() },
            onClosePair = { node()?.closePairScreen() },
            onScanned = ::scanned,
            onPairNearby = ::pairNearby,
            onConfirm = { k, ok -> runCatching { node()?.confirmCode(k, ok) } },
            onFinish = { inOnboarding = false; stack.clear(); stack.add(Page.Home) },
        )
    } else {
        BackHandler(enabled = stack.size > 1) { back() }
        // The app fades and rises in when it opens, and after onboarding.
        var appShown by remember { mutableStateOf(false) }
        LaunchedEffect(Unit) { appShown = true }
        val appIn by animateFloatAsState(if (appShown) 1f else 0f, tween(320), label = "app-in")
        Scaffold(
            modifier = Modifier.graphicsLayer {
                alpha = appIn
                translationY = (1f - appIn) * 24.dp.toPx()
            },
            snackbarHost = { SnackbarHost(snackbar) },
            topBar = {
                TopAppBar(
                    title = {
                        Text(
                            when (page) {
                                Page.Home -> stringResource(R.string.app_name)
                                Page.ThisDevice -> stringResource(R.string.this_device)
                                is Page.Device -> s.peers.firstOrNull { it.id == page.id }?.name ?: ""
                                Page.Pair -> stringResource(R.string.add_device)
                                Page.Settings -> stringResource(R.string.settings)
                                Page.Log -> stringResource(R.string.set_log)
                                is Page.Feature -> s.peers.firstOrNull { it.id == page.device }?.name ?: ""
                            },
                        )
                    },
                    navigationIcon = {
                        if (stack.size > 1) {
                            IconButton(onClick = ::back) { Icon(Icons.AutoMirrored.Outlined.ArrowBack, stringResource(R.string.back)) }
                        } else {
                            Image(painterResource(R.drawable.ic_urdevice_symbol), null, modifier = Modifier.padding(start = 12.dp).size(28.dp))
                        }
                    },
                    actions = {
                        if (page == Page.Home) {
                            IconButton(onClick = { go(Page.Settings) }) { Icon(Icons.Outlined.Settings, stringResource(R.string.settings)) }
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
                )
            },
            containerColor = MaterialTheme.colorScheme.background,
        ) { inner ->
            Column(Modifier.padding(inner).fillMaxSize()) {
                s.activities.filter { it.incoming }.forEach { a -> ActiveBanner(a) { node()?.stopActivity(a.id) } }
                Column(Modifier.verticalScroll(rememberScrollState()).padding(16.dp)) {
                    if (netNeeded) {
                        Explain(
                            stringResource(R.string.why_network_title), stringResource(R.string.why_network_body),
                            onGrant = { askNet.launch("android.permission.ACCESS_LOCAL_NETWORK") },
                        )
                    }
                    if (notifNeeded && page == Page.Home) {
                        Explain(
                            stringResource(R.string.why_notifications_title), stringResource(R.string.why_notifications_body),
                            onGrant = { askNotif.launch(Manifest.permission.POST_NOTIFICATIONS) },
                            onLater = { notifNeeded = false },
                        )
                    }
                    when (page) {
                        Page.Home -> HomeScreen(s, onThisDevice = { go(Page.ThisDevice) }, onPeer = { go(Page.Device(it)) }, onAdd = { go(Page.Pair) })
                        Page.ThisDevice -> ThisDeviceScreen()
                        is Page.Device -> {
                            val peer = s.peers.firstOrNull { it.id == page.id }
                            if (peer == null) {
                                LaunchedEffect(page) { back() }
                            } else {
                                DeviceScreen(
                                    s, peer,
                                    onDisconnect = { runCatching { node()?.disconnect(peer.id) } },
                                    onReconnect = { runCatching { node()?.reconnect(peer.id) } },
                                    onForget = { runCatching { node()?.forget(peer.id) } },
                                    onRequest = { cap -> runCatching { node()?.requestActivity(peer.id, cap) } },
                                    onOpenFeature = { f, tile -> go(Page.Feature(peer.id, f, tile)) },
                                    onSettings = { go(Page.Settings) },
                                    onStop = { node()?.stopActivity(it) },
                                )
                            }
                        }
                        Page.Pair -> PairBody(
                            s, ::scanned,
                            onPairNearby = ::pairNearby,
                            onConfirm = { k, ok -> runCatching { node()?.confirmCode(k, ok) } },
                            compact = true,
                        )
                        Page.Settings -> SettingsScreen(
                            s,
                            onRename = { runCatching { node()?.setName(it) } },
                            onHidden = { runCatching { node()?.setHidden(it) } },
                            onLog = {
                                log = runCatching { node()?.activityLog(500u) }.getOrNull() ?: emptyList()
                                go(Page.Log)
                            },
                            onExport = {
                                val text = runCatching { node()?.exportData() }.getOrNull() ?: return@SettingsScreen
                                ctx.startActivity(
                                    Intent.createChooser(
                                        Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text),
                                        null,
                                    ),
                                )
                            },
                            onReset = { runCatching { node()?.resetIdentity() } },
                            onSetPermission = { c, m, confirmed -> runCatching { node()?.setPermission(c, m, confirmed) } },
                            onDeleteAll = {
                                runCatching { node()?.deleteAllData() }
                                pairedName = null
                                inOnboarding = true
                            },
                            version = coreVersion(),
                        )
                        Page.Log -> LogScreen(log)
                        is Page.Feature -> {
                            val peer = s.peers.firstOrNull { it.id == page.device }
                            val f = feature(page.feature)
                            if (peer == null || f == null) {
                                LaunchedEffect(page) { back() }
                            } else {
                                f.Screen(peer, page.tile)
                            }
                        }
                    }
                }
            }
        }
    }

    // The Always Ask prompt in the app, next to the notification (R5.3).
    prompts.firstOrNull()?.let { p -> PromptDialog(p) }
}

@Composable
private fun PromptDialog(p: FfiEvent.Prompt) {
    var left by remember(p.id) { mutableStateOf(p.timeoutSecs.toInt()) }
    LaunchedEffect(p.id) {
        while (left > 0) {
            delay(1000)
            left -= 1
        }
    }
    val pairing = p.capability == "pair"
    AlertDialog(
        onDismissRequest = {},
        title = {
            Text(
                if (pairing) stringResource(R.string.prompt_pair_title, p.deviceName)
                else stringResource(R.string.prompt_title, p.deviceName, stringResource(Labels.cap(p.capability))),
            )
        },
        text = { Text(stringResource(if (pairing) R.string.prompt_pair_body else R.string.prompt_body, left)) },
        confirmButton = {
            TextButton(onClick = { Core.node()?.answerPrompt(p.id, true) }) {
                Text(stringResource(if (pairing) R.string.pair_accept else R.string.allow))
            }
        },
        dismissButton = {
            TextButton(onClick = { Core.node()?.answerPrompt(p.id, false) }) {
                Text(stringResource(if (pairing) R.string.pair_decline else R.string.deny))
            }
        },
    )
}
