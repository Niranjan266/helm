package com.niranjan.helm.device

import android.annotation.SuppressLint
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothHidDevice
import android.bluetooth.BluetoothHidDeviceAppSdpSettings
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import com.niranjan.helm.core.BtHidState
import com.niranjan.helm.core.BtHidTransport
import com.niranjan.helm.core.BtHost
import com.niranjan.helm.core.BtHosts
import com.niranjan.helm.core.BtLink
import com.niranjan.helm.core.BtSupport
import com.niranjan.helm.core.HidDescriptor
import java.util.concurrent.Executor
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

/** The last PC the phone connected to as a Bluetooth keyboard/mouse. */
class BtHostStore(context: Context) {
    private val prefs = context.getSharedPreferences("helm_bt", Context.MODE_PRIVATE)

    fun last(): BtHost? {
        val a = prefs.getString("last_addr", null) ?: return null
        return BtHost(a, prefs.getString("last_name", null) ?: a)
    }

    fun speed(default: Float): Float = prefs.getFloat("speed", default).coerceIn(0.4f, 3f)

    fun setSpeed(v: Float) {
        prefs.edit().putFloat("speed", v).apply()
    }

    fun setLast(h: BtHost) {
        prefs.edit().putString("last_addr", h.address).putString("last_name", h.name).apply()
    }
}

/**
 * [BtHidTransport] on `BluetoothHidDevice` (API 28+). Thin on purpose: it registers the HID
 * app with the descriptor from core, tracks the connection and queues reports onto its own
 * thread. Callers must hold BLUETOOTH_CONNECT (Android 12+) before [start] or [connect];
 * a missing permission is caught and shown as "not connected" rather than crashing.
 */
@SuppressLint("MissingPermission")
class AndroidBtHid(context: Context) : BtHidTransport {
    private val app = context.applicationContext
    private val store = BtHostStore(app)
    private val adapter: BluetoothAdapter? = (app.getSystemService(Context.BLUETOOTH_SERVICE) as? BluetoothManager)?.adapter
    private val io: ExecutorService = Executors.newSingleThreadExecutor { r -> Thread(r, "helm-bt-hid").apply { isDaemon = true } }
    private val callbackExecutor = Executor { r -> runCatching { io.execute(r) } }

    private val _state = MutableStateFlow(BtHidState(lastHost = store.last()))
    override val state: StateFlow<BtHidState> = _state

    @Volatile private var hid: BluetoothHidDevice? = null
    @Volatile private var device: BluetoothDevice? = null
    @Volatile private var stopped = false
    private var profileAvailable: Boolean? = null
    private var receiverRegistered = false

    private val adapterReceiver = object : BroadcastReceiver() {
        override fun onReceive(c: Context?, intent: Intent?) {
            if (intent?.action == BluetoothAdapter.ACTION_STATE_CHANGED) refresh()
        }
    }

    private fun supportNow(): BtSupport =
        BtSupport.evaluate(Build.VERSION.SDK_INT, adapter != null, profileAvailable)

    override fun refresh() {
        if (stopped) return
        val support = supportNow()
        val on = runCatching { adapter?.isEnabled == true }.getOrDefault(false)
        val bonded = if (support == BtSupport.OK && on) {
            runCatching { adapter?.bondedDevices.orEmpty().map { BtHost(it.address, it.name ?: it.address) } }.getOrDefault(emptyList())
        } else {
            emptyList()
        }
        _state.update { s ->
            s.copy(
                support = support,
                adapterOn = on,
                bonded = BtHosts.order(bonded, store.last()?.address),
                lastHost = store.last(),
                // Bluetooth switched off: nothing is registered or connected any more.
                registered = if (on) s.registered else false,
                link = if (on) s.link else BtLink.NOT_CONNECTED,
                host = if (on) s.host else null,
            )
        }
    }

    override fun start() {
        if (stopped || Build.VERSION.SDK_INT < BtSupport.MIN_SDK) {
            refresh()
            return
        }
        if (!receiverRegistered) {
            receiverRegistered = true
            app.registerReceiver(adapterReceiver, IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED))
        }
        refresh()
        val a = adapter ?: return
        if (!_state.value.adapterOn || hid != null) return
        val asked = runCatching { a.getProfileProxy(app, profileListener, BluetoothProfile.HID_DEVICE) }.getOrDefault(false)
        if (!asked) {
            profileAvailable = false
            refresh()
        }
    }

    private val profileListener = object : BluetoothProfile.ServiceListener {
        override fun onServiceConnected(profile: Int, proxy: BluetoothProfile?) {
            if (profile != BluetoothProfile.HID_DEVICE) return
            val h = proxy as? BluetoothHidDevice
            if (h == null || stopped) {
                if (proxy != null) runCatching { adapter?.closeProfileProxy(BluetoothProfile.HID_DEVICE, proxy) }
                return
            }
            hid = h
            profileAvailable = true
            val sdp = BluetoothHidDeviceAppSdpSettings(
                "Helm keyboard and mouse",
                "Phone as a Bluetooth keyboard and mouse",
                "Helm",
                BluetoothHidDevice.SUBCLASS1_COMBO,
                HidDescriptor.bytes,
            )
            val ok = runCatching { h.registerApp(sdp, null, null, callbackExecutor, callback) }.getOrDefault(false)
            if (!ok) {
                profileAvailable = false
                refresh()
            }
        }

        override fun onServiceDisconnected(profile: Int) {
            if (profile != BluetoothProfile.HID_DEVICE) return
            hid = null
            device = null
            _state.update { it.copy(registered = false, link = BtLink.NOT_CONNECTED, host = null) }
        }
    }

    private val callback = object : BluetoothHidDevice.Callback() {
        override fun onAppStatusChanged(pluggedDevice: BluetoothDevice?, registered: Boolean) {
            _state.update { it.copy(registered = registered) }
            if (registered && !stopped) {
                // Reconnect to the last host (or the one the system says is plugged in).
                val last = store.last()
                if (_state.value.link == BtLink.NOT_CONNECTED) {
                    val target = pluggedDevice?.let { BtHost(it.address, it.name ?: it.address) } ?: last
                    if (target != null) connect(target)
                }
            }
        }

        override fun onConnectionStateChanged(d: BluetoothDevice?, state: Int) {
            d ?: return
            val host = BtHost(d.address, runCatching { d.name }.getOrNull() ?: d.address)
            when (state) {
                BluetoothProfile.STATE_CONNECTED -> {
                    device = d
                    store.setLast(host)
                    _state.update { it.copy(link = BtLink.CONNECTED, host = host, lastHost = host) }
                }
                BluetoothProfile.STATE_CONNECTING ->
                    _state.update { it.copy(link = BtLink.CONNECTING, host = host) }
                else -> {
                    if (device?.address == d.address) device = null
                    _state.update { s ->
                        if (s.host == null || s.host.address.equals(d.address, true)) s.copy(link = BtLink.NOT_CONNECTED, host = null) else s
                    }
                }
            }
        }

        override fun onGetReport(d: BluetoothDevice?, type: Byte, id: Byte, bufferSize: Int) {
            if (d != null) runCatching { hid?.reportError(d, BluetoothHidDevice.ERROR_RSP_UNSUPPORTED_REQ) }
        }

        override fun onSetReport(d: BluetoothDevice?, type: Byte, id: Byte, data: ByteArray?) {
            // Keyboard LEDs from the PC: accepted and ignored.
            if (d != null) runCatching { hid?.reportError(d, BluetoothHidDevice.ERROR_RSP_SUCCESS) }
        }
    }

    override fun connect(host: BtHost) {
        val h = hid ?: return
        val a = adapter ?: return
        if (stopped) return
        runCatching {
            val d = a.getRemoteDevice(host.address)
            _state.update { it.copy(link = BtLink.CONNECTING, host = host) }
            if (!h.connect(d)) _state.update { it.copy(link = BtLink.NOT_CONNECTED, host = null) }
        }.onFailure {
            _state.update { s -> s.copy(link = BtLink.NOT_CONNECTED, host = null) }
        }
    }

    override fun disconnect() {
        val h = hid ?: return
        val d = device ?: _state.value.host?.let { runCatching { adapter?.getRemoteDevice(it.address) }.getOrNull() } ?: return
        runCatching { h.disconnect(d) }
        _state.update { it.copy(link = BtLink.NOT_CONNECTED, host = null) }
    }

    override fun send(reportId: Int, data: ByteArray) {
        if (stopped) return
        io.execute {
            val h = hid
            val d = device
            if (h != null && d != null && _state.value.link == BtLink.CONNECTED) {
                runCatching { h.sendReport(d, reportId, data) }
            }
        }
    }

    override fun stop() {
        if (stopped) return
        // Queued reports (the all-up ones) go out first, then the app is unregistered.
        io.execute {
            val h = hid
            val d = device
            runCatching { if (h != null && d != null) h.disconnect(d) }
            runCatching { h?.unregisterApp() }
            runCatching { h?.let { adapter?.closeProfileProxy(BluetoothProfile.HID_DEVICE, it) } }
            hid = null
            device = null
            _state.update { it.copy(registered = false, link = BtLink.NOT_CONNECTED, host = null) }
        }
        stopped = true
        io.shutdown()
        if (receiverRegistered) {
            receiverRegistered = false
            runCatching { app.unregisterReceiver(adapterReceiver) }
        }
    }
}
