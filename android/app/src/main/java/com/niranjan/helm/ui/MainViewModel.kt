package com.niranjan.helm.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.Session
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.DiscoveredHost
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.PairingUri
import com.niranjan.helm.core.PairingUriException
import com.niranjan.helm.core.SavedPc
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull

data class DeviceRow(
    val deviceId: String,
    val name: String,
    val tier: String,
    val pairedAtMs: Long?,
    val lastSeenMs: Long?,
    val isThis: Boolean,
)

data class SessionUi(
    val pingMs: Long? = null,
    val pingBusy: Boolean = false,
    val info: List<Pair<String, String>>? = null,
    val devices: List<DeviceRow>? = null,
    val forgetting: Boolean = false,
)

@OptIn(ExperimentalCoroutinesApi::class)
class MainViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    private val usedCodes = com.niranjan.helm.core.UsedCodes()

    val savedPcs: StateFlow<List<SavedPc>> = controller.savedPcs.pcs
    val discovered: StateFlow<Map<String, DiscoveredHost>> = controller.discovery.hosts
    val session: StateFlow<Session?> = controller.session

    val connectionState: StateFlow<ConnectionState> = controller.session
        .flatMapLatest { it?.connection?.state ?: flowOf(ConnectionState.Idle) }
        .stateIn(viewModelScope, SharingStarted.Eagerly, ConnectionState.Idle)

    private val _ui = MutableStateFlow(SessionUi())
    val ui: StateFlow<SessionUi> = _ui.asStateFlow()

    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages = _messages.asSharedFlow()

    init {
        // Reset per-session data, and load info + devices once each session becomes ready.
        viewModelScope.launch {
            controller.session.map { it?.connection }.distinctUntilChanged().collect { _ui.value = SessionUi() }
        }
        viewModelScope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.filterIsInstance<ConnectionState.Ready>() ?: flowOf() }
                .collect {
                    refreshInfo()
                    refreshDevices()
                }
        }
    }

    fun message(text: String) {
        _messages.tryEmit(text)
    }

    /** Returns an error message, or null if pairing started. */
    fun pairFromText(text: String?): String? {
        if (text.isNullOrBlank()) return "That QR code is empty."
        return try {
            controller.startPairing(PairingUri.parse(text))
            null
        } catch (e: PairingUriException) {
            e.message
        }
    }

    /** g4 short-code pairing. Returns an error message, or null if pairing started. */
    fun pairWithCode(host: DiscoveredHost, code: String): String? {
        if (com.niranjan.helm.core.CodePairingCrypto.parseCode(code) == null) return "Enter the 6-digit code shown on the PC."
        if (!usedCodes.tryUse(host.hostId, code)) return "That code was already tried. Show a new code on the PC."
        controller.startCodePairing(host, code)
        return null
    }

    fun connect(pc: SavedPc) = controller.connectSaved(pc)

    fun retry() {
        val s = controller.session.value ?: return
        if (s.isPairing) {
            message("Pairing links and codes work once. Show a fresh QR code or code on the PC.")
            controller.disconnect()
        } else {
            controller.savedPcs.get(s.hostId)?.let { controller.connectSaved(it) }
        }
    }

    fun disconnect() = controller.disconnect()

    /** Auto-reconnect state for the current PC. */
    val reconnect = controller.reconnect

    fun reconnectNow() = controller.reconnectNow()

    fun forgetLocally(hostId: String) = controller.forget(hostId)

    val settings = controller.settings.settings

    fun updateSettings(change: (com.niranjan.helm.device.Settings) -> com.niranjan.helm.device.Settings) =
        controller.settings.update(change)

    /** Wake-on-LAN for an offline saved PC. */
    fun wake(pc: SavedPc) {
        if (pc.macs.isEmpty()) {
            message("No network address known for ${pc.name} yet. Connect once while it is on.")
            return
        }
        viewModelScope.launch {
            val n = try {
                controller.wake(pc)
            } catch (e: Exception) {
                message("Wake failed: ${e.message}")
                return@launch
            }
            message(if (n > 0) "Wake signal sent to ${pc.name}. It may take a minute to start." else "Could not send the wake signal.")
        }
    }

    /** Remote wake (§12) through another awake Helm PC. */
    fun wakeVia(pc: SavedPc) {
        if (pc.macs.isEmpty()) return message("No network address known for ${pc.name} yet. Connect once while it is on.")
        message("Asking another PC to wake ${pc.name}…")
        viewModelScope.launch {
            val by = controller.wakeVia(pc)
            message(if (by != null) "$by sent the wake signal to ${pc.name}." else "No other awake Helm PC could wake ${pc.name}.")
        }
    }

    /** "Connect from anywhere" for one PC. Returns an error message, or null. */
    fun setAnywhere(hostId: String, on: Boolean, typedUrl: String? = null): String? = controller.setAnywhere(hostId, on, typedUrl)

    fun prefsFor(hostId: String): com.niranjan.helm.core.PcPrefs = controller.prefsFor(hostId)

    fun updatePcPrefs(hostId: String, change: (com.niranjan.helm.core.PcPrefs) -> com.niranjan.helm.core.PcPrefs) =
        controller.updatePcPrefs(hostId, change)

    fun ping() {
        val conn = controller.session.value?.connection ?: return
        _ui.value = _ui.value.copy(pingBusy = true)
        viewModelScope.launch {
            val start = System.nanoTime()
            try {
                conn.request("system.ping")
                val ms = (System.nanoTime() - start) / 1_000_000
                _ui.value = _ui.value.copy(pingMs = ms, pingBusy = false)
            } catch (e: HelmException) {
                _ui.value = _ui.value.copy(pingBusy = false)
                message("Ping failed: ${e.message}")
            }
        }
    }

    /** Background latency sample for the status line (no busy state, no error message). */
    fun pingQuiet() {
        val conn = controller.readyConnection() ?: return
        viewModelScope.launch {
            val start = System.nanoTime()
            try {
                conn.request("system.ping")
                _ui.value = _ui.value.copy(pingMs = (System.nanoTime() - start) / 1_000_000)
            } catch (_: HelmException) {
            }
        }
    }

    fun refreshInfo() {
        val conn = controller.session.value?.connection ?: return
        viewModelScope.launch {
            try {
                val r = conn.request("system.info")
                val rows = listOfNotNull(
                    r.text("host_name")?.let { "Name" to it },
                    r.text("os")?.let { "OS" to it },
                    r.text("agent_version")?.let { "Agent" to it },
                    r.long("uptime_s")?.let { "Uptime" to formatUptime(it) },
                    r.text("host_id")?.let { "Host id" to it },
                )
                _ui.value = _ui.value.copy(info = rows)
            } catch (e: HelmException) {
                message("System info failed: ${e.message}")
            }
        }
    }

    fun refreshDevices() {
        val conn = controller.session.value?.connection ?: return
        viewModelScope.launch {
            try {
                val r = conn.request("devices.list")
                val rows = (r["devices"] as? JsonArray).orEmpty().mapNotNull { el ->
                    val o = el as? JsonObject ?: return@mapNotNull null
                    DeviceRow(
                        deviceId = o.text("device_id") ?: return@mapNotNull null,
                        name = o.text("device_name") ?: "Unknown device",
                        tier = o.text("tier") ?: "?",
                        pairedAtMs = o.long("paired_at_ms"),
                        lastSeenMs = o.long("last_seen_ms"),
                        isThis = (o["this"] as? JsonPrimitive)?.booleanOrNull == true,
                    )
                }
                _ui.value = _ui.value.copy(devices = rows)
            } catch (e: HelmException) {
                message("Devices failed: ${e.message}")
            }
        }
    }

    fun forgetCurrent() {
        _ui.value = _ui.value.copy(forgetting = true)
        viewModelScope.launch {
            val revoked = controller.forgetCurrent()
            message(
                if (revoked) "PC forgotten." else "Removed from this phone. The PC may still list it until you remove it there.",
            )
        }
    }

    private fun JsonObject.text(k: String): String? = (this[k] as? JsonPrimitive)?.contentOrNull

    private fun JsonObject.long(k: String): Long? = (this[k] as? JsonPrimitive)?.longOrNull

    private fun formatUptime(s: Long): String {
        val d = s / 86_400
        val h = (s % 86_400) / 3_600
        val m = (s % 3_600) / 60
        return when {
            d > 0 -> "${d}d ${h}h ${m}m"
            h > 0 -> "${h}h ${m}m"
            else -> "${m}m"
        }
    }
}
