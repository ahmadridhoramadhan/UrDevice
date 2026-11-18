package com.urdevice.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.util.Log
import androidx.lifecycle.LifecycleService
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.launch

private data class ServiceState(
    val active: ULong?,
    val connected: Int,
    val hidden: Boolean,
    val name: String,
    val browse: Boolean,
)

/**
 * Foreground service that hosts the core (design §13), keeps discovery
 * running, reports the battery (R4.9), and keeps the notification with Stop
 * up to date (R5.4).
 */
class UrDeviceService : LifecycleService() {
    private var nsd: NsdDiscovery? = null
    private var multicast: WifiManager.MulticastLock? = null
    private var lastBattery: Pair<Int, Boolean>? = null

    private val batteryReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) = reportBattery(intent)
    }

    override fun onCreate() {
        super.onCreate()
        Notifications.createChannels(this)
        startForeground(
            Notifications.SERVICE_ID,
            Notifications.service(this, null),
            ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE,
        )
        val node = try {
            Core.start(this)
        } catch (e: Exception) {
            Log.e("UrDevice", "core failed to start", e)
            stopSelf()
            return
        }
        nsd = NsdDiscovery(this, node)
        multicast = getSystemService(WifiManager::class.java)
            .createMulticastLock("urdevice")
            .apply { setReferenceCounted(false) }

        registerReceiver(batteryReceiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED))?.let(::reportBattery)

        lifecycleScope.launch {
            combine(Core.snapshot, Core.foreground) { snap, fg ->
                ServiceState(
                    active = snap?.activities?.firstOrNull { it.incoming }?.id,
                    connected = snap?.peers?.count { it.status == "connected" } ?: 0,
                    hidden = snap?.me?.hidden ?: false,
                    name = snap?.me?.name ?: "",
                    // Battery (R15.4): look for devices only while the app is
                    // open or a paired device is still missing.
                    browse = fg || (snap?.peers?.any { it.status == "offline" } ?: true),
                )
            }.distinctUntilChanged().collect { state ->
                Notifications.updateService(this@UrDeviceService, Core.snapshot.value)
                nsd?.updateRegistration(state.hidden)
                if (state.browse) {
                    multicast?.acquire()
                    nsd?.startBrowsing()
                } else {
                    nsd?.stopBrowsing()
                    multicast?.release()
                }
            }
        }
    }

    private fun reportBattery(intent: Intent) {
        val level = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
        val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, 100)
        val status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
        if (level < 0 || scale <= 0) return
        val pct = (level * 100 / scale).coerceIn(0, 100)
        val charging = status == BatteryManager.BATTERY_STATUS_CHARGING || status == BatteryManager.BATTERY_STATUS_FULL
        val now = pct to charging
        if (now == lastBattery) return
        lastBattery = now
        Core.node()?.setBattery(pct.toUByte(), charging)
    }

    override fun onDestroy() {
        runCatching { unregisterReceiver(batteryReceiver) }
        nsd?.stop()
        multicast?.release()
        super.onDestroy()
    }

    companion object {
        fun start(context: Context) {
            context.startForegroundService(Intent(context, UrDeviceService::class.java))
        }
    }
}
