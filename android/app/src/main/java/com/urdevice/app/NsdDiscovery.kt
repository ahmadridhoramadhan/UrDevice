package com.urdevice.app

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.Build
import android.util.Log
import com.urdevice.core.UrDeviceNode
import java.util.concurrent.Executors

/**
 * mDNS through NsdManager (design §5): registers `_urdevice._udp` with the
 * core's TXT records and feeds resolved devices back to the core. The core
 * does the UDP broadcast part itself.
 */
class NsdDiscovery(context: Context, private val node: UrDeviceNode) {
    private val nsd = context.getSystemService(NsdManager::class.java)
    private val executor = Executors.newSingleThreadExecutor()
    private var registration: NsdManager.RegistrationListener? = null
    private var registeredAs: String? = null
    private var discovery: NsdManager.DiscoveryListener? = null
    private val ownId = node.mdnsRecord().txt["id"]

    /** Register or unregister this device; call again after a rename or hidden-mode change. */
    fun updateRegistration(hidden: Boolean) {
        val rec = node.mdnsRecord()
        val key = if (hidden) null else "${rec.instance}:${rec.port}"
        if (key == registeredAs) return
        registration?.let { runCatching { nsd.unregisterService(it) } }
        registration = null
        registeredAs = key
        if (hidden) return
        val info = NsdServiceInfo().apply {
            serviceName = rec.instance
            serviceType = TYPE
            port = rec.port.toInt()
            rec.txt.forEach { (k, v) -> setAttribute(k, v) }
        }
        val l = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(info: NsdServiceInfo) {}
            override fun onRegistrationFailed(info: NsdServiceInfo, code: Int) {
                Log.w(TAG, "NSD register failed: $code")
            }
            override fun onServiceUnregistered(info: NsdServiceInfo) {}
            override fun onUnregistrationFailed(info: NsdServiceInfo, code: Int) {}
        }
        registration = l
        nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, l)
    }

    fun startBrowsing() {
        if (discovery != null) return
        val l = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(type: String) {}
            override fun onDiscoveryStopped(type: String) {}
            override fun onStartDiscoveryFailed(type: String, code: Int) {
                Log.w(TAG, "NSD discovery failed: $code")
                discovery = null
            }
            override fun onStopDiscoveryFailed(type: String, code: Int) {}
            override fun onServiceLost(info: NsdServiceInfo) {}
            override fun onServiceFound(info: NsdServiceInfo) = resolve(info)
        }
        discovery = l
        nsd.discoverServices(TYPE, NsdManager.PROTOCOL_DNS_SD, l)
    }

    fun stopBrowsing() {
        discovery?.let { runCatching { nsd.stopServiceDiscovery(it) } }
        discovery = null
    }

    fun stop() {
        stopBrowsing()
        registration?.let { runCatching { nsd.unregisterService(it) } }
        registration = null
        registeredAs = null
    }

    private fun resolve(found: NsdServiceInfo) {
        if (Build.VERSION.SDK_INT >= 34) {
            val cb = object : NsdManager.ServiceInfoCallback {
                override fun onServiceInfoCallbackRegistrationFailed(code: Int) {}
                override fun onServiceUpdated(info: NsdServiceInfo) {
                    feed(info, info.hostAddresses.mapNotNull { it.hostAddress })
                    runCatching { nsd.unregisterServiceInfoCallback(this) }
                }
                override fun onServiceLost() {}
                override fun onServiceInfoCallbackUnregistered() {}
            }
            runCatching { nsd.registerServiceInfoCallback(found, executor, cb) }
        } else {
            @Suppress("DEPRECATION")
            nsd.resolveService(found, object : NsdManager.ResolveListener {
                override fun onResolveFailed(info: NsdServiceInfo, code: Int) {}
                override fun onServiceResolved(info: NsdServiceInfo) {
                    @Suppress("DEPRECATION")
                    feed(info, listOfNotNull(info.host?.hostAddress))
                }
            })
        }
    }

    private fun feed(info: NsdServiceInfo, addrs: List<String>) {
        val attrs = info.attributes
        val id = attrs["id"]?.toString(Charsets.UTF_8) ?: return
        if (id == ownId) return
        val platform = attrs["p"]?.toString(Charsets.UTF_8) ?: ""
        // IPv4 only, like the rest of the core; scoped IPv6 needs interface handling.
        val v4 = addrs.filter { !it.contains(':') }
        if (v4.isEmpty()) return
        node.addDiscovered(id, info.serviceName, platform, v4, info.port.toUShort())
    }

    private companion object {
        const val TAG = "UrDevice"
        const val TYPE = "_urdevice._udp"
    }
}
