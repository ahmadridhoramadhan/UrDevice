package com.urdevice.app.host

import android.Manifest
import android.annotation.SuppressLint
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothGatt
import android.bluetooth.BluetoothGattCharacteristic
import android.bluetooth.BluetoothGattDescriptor
import android.bluetooth.BluetoothGattServer
import android.bluetooth.BluetoothGattServerCallback
import android.bluetooth.BluetoothGattService
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.bluetooth.le.AdvertiseCallback
import android.bluetooth.le.AdvertiseData
import android.bluetooth.le.AdvertiseSettings
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.os.ParcelUuid
import android.util.Log
import com.urdevice.core.BluetoothRadioFfi
import com.urdevice.core.UrDeviceNode
import java.util.UUID
import java.util.concurrent.ConcurrentHashMap

/**
 * The phone's side of the Bluetooth LE fallback (#23, design §8.2): a GATT server with the UrDevice
 * service, one write-without-response characteristic from the desktop and one notify characteristic
 * to it. It only moves packets; the core frames them, runs Noise IK with the pinned keys, and
 * decides when to advertise (only while a paired desktop has no session). No Bluetooth pairing is
 * needed and nothing here is plaintext.
 */
@SuppressLint("MissingPermission") // Every radio call checks hasPermission() first.
class BluetoothHost(private val context: Context, private val node: UrDeviceNode) : BluetoothRadioFfi {
    companion object {
        private const val TAG = "UrDeviceBle"
        val SERVICE: UUID = UUID.fromString("6b1f0001-5d7a-4c1e-9b8a-2f6d1c3e8a10")
        val RX: UUID = UUID.fromString("6b1f0002-5d7a-4c1e-9b8a-2f6d1c3e8a10")
        val TX: UUID = UUID.fromString("6b1f0003-5d7a-4c1e-9b8a-2f6d1c3e8a10")
        private val CCCD: UUID = UUID.fromString("00002902-0000-1000-8000-00805f9b34fb")
        private const val MAX_QUEUE = 256

        /** The runtime permissions the fallback needs ("Nearby devices" on Android 12+). */
        fun permissions(): Array<String> =
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
                arrayOf(Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.BLUETOOTH_ADVERTISE)
            } else {
                emptyArray()
            }

        fun hasPermission(ctx: Context): Boolean =
            permissions().all { ctx.checkSelfPermission(it) == PackageManager.PERMISSION_GRANTED }

        @Volatile var instance: BluetoothHost? = null
            private set

        fun start(ctx: Context, node: UrDeviceNode) {
            if (instance != null) return
            val host = BluetoothHost(ctx.applicationContext, node)
            instance = host
            node.bluetoothSetRadio(host)
        }
    }

    private class Link(val id: ULong, val device: BluetoothDevice) {
        var mtu = 23
        var opened = false
        var sending = false
        val queue = ArrayDeque<ByteArray>()
    }

    private val thread = HandlerThread("urdevice-ble").also { it.start() }
    private val handler = Handler(thread.looper)
    private val manager = context.getSystemService(BluetoothManager::class.java)
    private var server: BluetoothGattServer? = null
    private var tx: BluetoothGattCharacteristic? = null
    private var wantAdvertising = false
    private var advertising = false
    private var nextId = 1UL
    private val byAddress = HashMap<String, Link>()
    private val byId = ConcurrentHashMap<ULong, Link>()

    private val advertiseCallback = object : AdvertiseCallback() {
        override fun onStartFailure(errorCode: Int) {
            Log.w(TAG, "advertising failed: $errorCode")
            handler.post { advertising = false }
        }
    }

    override fun setAdvertising(on: Boolean) {
        handler.post {
            wantAdvertising = on
            apply()
        }
    }

    /** Call after the user granted the Bluetooth permissions. */
    fun permissionChanged() {
        handler.post { apply() }
    }

    override fun send(link: ULong, packet: ByteArray): Boolean {
        if (!byId.containsKey(link)) return false
        handler.post {
            val l = byId[link] ?: return@post
            if (l.queue.size >= MAX_QUEUE) {
                close(l)
                return@post
            }
            l.queue.addLast(packet)
            pump(l)
        }
        return true
    }

    override fun disconnect(link: ULong) {
        handler.post { byId[link]?.let { close(it) } }
    }

    private fun close(l: Link) {
        if (hasPermission(context)) server?.cancelConnection(l.device)
        drop(l)
    }

    private fun drop(l: Link) {
        byAddress.remove(l.device.address)
        if (byId.remove(l.id) != null && l.opened) node.bluetoothLinkClosed(l.id)
    }

    private fun apply() {
        if (!hasPermission(context)) return
        val adapter = manager?.adapter ?: return
        if (!adapter.isEnabled) {
            advertising = false
            return
        }
        if (wantAdvertising && !advertising) {
            if (!openServer()) return
            val advertiser = adapter.bluetoothLeAdvertiser ?: return
            val settings = AdvertiseSettings.Builder()
                .setAdvertiseMode(AdvertiseSettings.ADVERTISE_MODE_BALANCED)
                .setTxPowerLevel(AdvertiseSettings.ADVERTISE_TX_POWER_MEDIUM)
                .setConnectable(true)
                .setTimeout(0)
                .build()
            val data = AdvertiseData.Builder()
                .setIncludeDeviceName(false)
                .addServiceUuid(ParcelUuid(SERVICE))
                .build()
            advertiser.startAdvertising(settings, data, advertiseCallback)
            advertising = true
        } else if (!wantAdvertising && advertising) {
            adapter.bluetoothLeAdvertiser?.stopAdvertising(advertiseCallback)
            advertising = false
        }
    }

    private fun openServer(): Boolean {
        if (server != null) return true
        val m = manager ?: return false
        val s = m.openGattServer(context, callback) ?: return false
        val service = BluetoothGattService(SERVICE, BluetoothGattService.SERVICE_TYPE_PRIMARY)
        val rx = BluetoothGattCharacteristic(
            RX,
            BluetoothGattCharacteristic.PROPERTY_WRITE_NO_RESPONSE or BluetoothGattCharacteristic.PROPERTY_WRITE,
            BluetoothGattCharacteristic.PERMISSION_WRITE,
        )
        val notify = BluetoothGattCharacteristic(
            TX,
            BluetoothGattCharacteristic.PROPERTY_NOTIFY,
            BluetoothGattCharacteristic.PERMISSION_READ,
        )
        notify.addDescriptor(
            BluetoothGattDescriptor(
                CCCD,
                BluetoothGattDescriptor.PERMISSION_READ or BluetoothGattDescriptor.PERMISSION_WRITE,
            ),
        )
        service.addCharacteristic(rx)
        service.addCharacteristic(notify)
        s.addService(service)
        server = s
        tx = notify
        return true
    }

    private fun link(device: BluetoothDevice): Link =
        byAddress.getOrPut(device.address) {
            Link(nextId++, device).also { byId[it.id] = it }
        }

    private fun pump(l: Link) {
        if (l.sending) return
        val s = server ?: return
        val c = tx ?: return
        val p = l.queue.firstOrNull() ?: return
        if (!hasPermission(context)) return
        val ok = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            s.notifyCharacteristicChanged(l.device, c, false, p) == BluetoothGatt.GATT_SUCCESS
        } else {
            notifyLegacy(s, l.device, c, p)
        }
        if (ok) {
            l.queue.removeFirst()
            l.sending = true
        } else {
            // The stack is busy; try again shortly.
            handler.postDelayed({ pump(l) }, 10)
        }
    }

    @Suppress("DEPRECATION")
    private fun notifyLegacy(s: BluetoothGattServer, d: BluetoothDevice, c: BluetoothGattCharacteristic, p: ByteArray): Boolean {
        c.value = p
        return s.notifyCharacteristicChanged(d, c, false)
    }

    private val callback = object : BluetoothGattServerCallback() {
        override fun onConnectionStateChange(device: BluetoothDevice, status: Int, newState: Int) {
            handler.post {
                if (newState == BluetoothProfile.STATE_DISCONNECTED) byAddress[device.address]?.let { drop(it) }
            }
        }

        override fun onMtuChanged(device: BluetoothDevice, mtu: Int) {
            handler.post { link(device).mtu = mtu }
        }

        override fun onCharacteristicWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            characteristic: BluetoothGattCharacteristic,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?,
        ) {
            val ok = characteristic.uuid == RX && !preparedWrite && offset == 0 && value != null
            if (responseNeeded && hasPermission(context)) {
                server?.sendResponse(device, requestId, if (ok) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE, 0, null)
            }
            if (!ok) return
            val data = value!!
            handler.post {
                val l = link(device)
                if (!l.opened) {
                    // The central settles the MTU before its first write.
                    l.opened = true
                    node.bluetoothLinkOpened(l.id, l.mtu.toUInt())
                }
                node.bluetoothLinkData(l.id, data)
            }
        }

        override fun onDescriptorWriteRequest(
            device: BluetoothDevice,
            requestId: Int,
            descriptor: BluetoothGattDescriptor,
            preparedWrite: Boolean,
            responseNeeded: Boolean,
            offset: Int,
            value: ByteArray?,
        ) {
            if (responseNeeded && hasPermission(context)) {
                val ok = descriptor.uuid == CCCD
                server?.sendResponse(device, requestId, if (ok) BluetoothGatt.GATT_SUCCESS else BluetoothGatt.GATT_FAILURE, 0, null)
            }
        }

        override fun onNotificationSent(device: BluetoothDevice, status: Int) {
            handler.post {
                val l = byAddress[device.address] ?: return@post
                l.sending = false
                pump(l)
            }
        }
    }
}
