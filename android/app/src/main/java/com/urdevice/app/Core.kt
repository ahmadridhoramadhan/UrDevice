package com.urdevice.app

import android.content.Context
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import com.urdevice.app.ui.features.allFeatures
import com.urdevice.core.FfiEvent
import com.urdevice.core.FfiSnapshot
import com.urdevice.core.NodeListener
import com.urdevice.core.UrDeviceNode
import com.urdevice.core.initLogging
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update

/**
 * Holds the one Rust core of this process. The foreground service starts it
 * (design §13); the activity reads [snapshot] and [events] and calls [node].
 */
object Core {
    private const val TAG = "UrDevice"

    @Volatile private var node: UrDeviceNode? = null
    private lateinit var app: Context
    private val main = Handler(Looper.getMainLooper())
    private val pending = AtomicBoolean(false)

    private val _snapshot = MutableStateFlow<FfiSnapshot?>(null)
    val snapshot: StateFlow<FfiSnapshot?> = _snapshot.asStateFlow()

    private val _events = MutableSharedFlow<FfiEvent>(extraBufferCapacity = 64)
    val events: SharedFlow<FfiEvent> = _events.asSharedFlow()

    private val _prompts = MutableStateFlow<List<FfiEvent.Prompt>>(emptyList())
    /** Open Always Ask prompts (R5.3). */
    val prompts: StateFlow<List<FfiEvent.Prompt>> = _prompts.asStateFlow()

    /** True while the activity is visible; discovery and the multicast lock follow it (R15.4). */
    val foreground = MutableStateFlow(false)

    private val listener = object : NodeListener {
        override fun onEvent(event: FfiEvent) {
            when (event) {
                is FfiEvent.Changed -> scheduleRefresh()
                is FfiEvent.Prompt -> {
                    _prompts.update { it + event }
                    Notifications.showPrompt(app, event)
                }
                is FfiEvent.PromptClosed -> {
                    _prompts.update { list -> list.filterNot { it.id == event.id } }
                    Notifications.cancelPrompt(app, event.id)
                }
                else -> {
                    _events.tryEmit(event)
                    scheduleRefresh()
                }
            }
        }
    }

    private fun scheduleRefresh() {
        if (pending.getAndSet(true)) return
        main.post {
            pending.set(false)
            refresh()
        }
    }

    fun refresh() {
        node?.let { _snapshot.value = it.snapshot() }
    }

    @Synchronized
    fun start(context: Context): UrDeviceNode {
        node?.let { return it }
        app = context.applicationContext
        initLogging("info,quinn=warn")
        val name = Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME)
            ?: Build.MODEL ?: "Android"
        val n = UrDeviceNode.start(
            dataDir = app.filesDir.resolve("core").absolutePath,
            platform = "android",
            defaultName = name,
            secrets = KeystoreSecretStore(app),
            listener = listener,
        )
        node = n
        Log.i(TAG, "core started")
        // Each feature registers its host adapters and listeners (ui/features/).
        allFeatures.forEach { f ->
            runCatching { f.onCoreStarted(app, n) }.onFailure { Log.e(TAG, "feature ${f.id} failed to start", it) }
        }
        refresh()
        return n
    }

    /** The running core, or null before the service started it. */
    fun node(): UrDeviceNode? = node
}
