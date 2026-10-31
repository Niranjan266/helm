package com.niranjan.helm

import android.app.Application
import android.content.Context
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.DeviceIdentity
import com.niranjan.helm.core.DeviceSigner
import com.niranjan.helm.core.Endpoint
import com.niranjan.helm.core.EndpointPlanner
import com.niranjan.helm.core.HandshakeMode
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.LocalCodes
import com.niranjan.helm.core.MacEntry
import com.niranjan.helm.core.WakeOnLan
import com.niranjan.helm.core.PairingUri
import com.niranjan.helm.core.SavedPc
import com.niranjan.helm.core.PcPrefs
import com.niranjan.helm.core.RelayConfig
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.wolSend
import com.niranjan.helm.device.AppSettings
import com.niranjan.helm.device.DeviceInfoStore
import com.niranjan.helm.device.IconCache
import com.niranjan.helm.device.PhoneFiles
import com.niranjan.helm.device.SharedItem
import com.niranjan.helm.device.HelmDiscovery
import com.niranjan.helm.device.KeystoreDeviceSigner
import com.niranjan.helm.device.SavedPcStore
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

class HelmApp : Application() {
    lateinit var controller: HelmController
        private set

    override fun onCreate() {
        super.onCreate()
        controller = HelmController(this)
        com.niranjan.helm.device.HelmBackground(this, controller).start()
    }
}

const val QUICK_MEDIA = "media"
const val QUICK_DECK = "deck"
const val QUICK_LOCK = "lock"
const val QUICK_MUTE = "mute"

/** One live (or failed) connection the UI is showing. */
data class Session(
    val hostId: String,
    val name: String,
    val connection: HelmConnection,
    val isPairing: Boolean,
    /** Pinned certificate fingerprint (also used for the `/stream` socket). */
    val fingerprint: String,
)

/** App-scoped holder: identity, stores, discovery and the current connection. */
class HelmController(context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    val deviceInfo = DeviceInfoStore(context)
    val savedPcs = SavedPcStore(context)
    val discovery = HelmDiscovery(context)
    val settings = AppSettings(context)
    val icons = IconCache(context)
    val files = PhoneFiles(context)

    /** Items shared into Helm that have not been sent yet. */
    val pendingShare = MutableStateFlow<List<SharedItem>>(emptyList())

    /** Keystore access is slow-ish; create the key lazily on a background thread. */
    private val signer: DeviceSigner = object : DeviceSigner {
        private val real by lazy { KeystoreDeviceSigner() }
        override fun publicKeySpki(): ByteArray = real.publicKeySpki()
        override fun sign(data: ByteArray): ByteArray = real.sign(data)
    }

    private val _session = MutableStateFlow<Session?>(null)
    val session: StateFlow<Session?> = _session.asStateFlow()

    init {
        // Warm up the key off the main thread so the first handshake is quick.
        scope.launch(Dispatchers.IO) {
            try {
                signer.publicKeySpki()
            } catch (_: Exception) {
            }
        }
    }

    private fun identity() = DeviceIdentity(deviceInfo.deviceId, deviceInfo.deviceName, signer)

    fun startPairing(uri: PairingUri) {
        closeCurrent()
        val conn = HelmConnection(
            hostId = uri.hostId,
            fingerprint = uri.fingerprint,
            endpoints = EndpointPlanner.forPairing(uri, discovery.find(uri.hostId)),
            identity = identity(),
            mode = HandshakeMode.Pair(uri.token),
        )
        _session.value = Session(uri.hostId, uri.hostName, conn, isPairing = true, fingerprint = uri.fingerprint)
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                savedPcs.upsert(EndpointPlanner.savedFromPairing(uri, ready, System.currentTimeMillis()))
                updateName(conn, ready.hostName)
                rememberMacs(conn, uri.hostId)
            } catch (_: HelmException) {
                // State flow already shows Failed; nothing is saved.
            }
        }
    }

    fun connectSaved(pc: SavedPc) {
        closeCurrent()
        val conn = HelmConnection(
            hostId = pc.hostId,
            fingerprint = pc.fingerprint,
            endpoints = EndpointPlanner.forSaved(pc, discovery.find(pc.hostId)),
            identity = identity(),
            mode = HandshakeMode.Auth,
            relay = EndpointPlanner.relayFor(pc),
        )
        _session.value = Session(pc.hostId, pc.name, conn, isPairing = false, fingerprint = pc.fingerprint)
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                val current = savedPcs.get(pc.hostId) ?: pc
                savedPcs.upsert(
                    if (ready.viaRelay) {
                        current.copy(name = ready.hostName.ifBlank { current.name }, lastUsedMs = System.currentTimeMillis())
                    } else {
                        current.copy(
                            name = ready.hostName.ifBlank { current.name },
                            lastAddress = ready.endpoint.host,
                            lastPort = ready.endpoint.port,
                            lastUsedMs = System.currentTimeMillis(),
                        )
                    },
                )
                updateName(conn, ready.hostName)
                rememberMacs(conn, pc.hostId)
            } catch (_: HelmException) {
            }
        }
    }

    /**
     * Saves the PC's Wake-on-LAN MACs and relay URL (from `system.info`, §12) for when it is
     * offline or away from the LAN.
     */
    private suspend fun rememberMacs(conn: HelmConnection, hostId: String) {
        try {
            val info = conn.request("system.info")
            val macs = WakeOnLan.chooseMacs(MacEntry.listFrom(info))
            val relay = RelayConfig.from(info["relay"] as? kotlinx.serialization.json.JsonObject)
            val current = savedPcs.get(hostId) ?: return
            var next = current
            if (macs.isNotEmpty() && macs != current.macs) next = next.copy(macs = macs)
            if (relay != null) next = next.copy(relayUrl = relay.url?.takeIf { relay.enabled })
            if (next != current) savedPcs.upsert(next)
        } catch (_: HelmException) {
        }
    }

    /**
     * Remote wake (§12): asks another awake Helm PC (`p4`) to send the magic packet on its LAN.
     * Uses the current session when it is a different, ready PC; otherwise connects briefly to
     * each other saved PC (LAN, then relay). Returns the name of the PC that sent it, or null.
     */
    suspend fun wakeVia(target: SavedPc): String? {
        if (target.macs.isEmpty()) return null
        suspend fun via(conn: HelmConnection): Boolean {
            if (!conn.supportsPhase4) return false
            val api = HelmApi(conn)
            var sent = false
            for (mac in target.macs) {
                sent = try { api.wolSend(mac); true } catch (_: HelmException) { sent }
            }
            return sent
        }
        val s = _session.value
        if (s != null && !s.hostId.equals(target.hostId, true) && s.connection.state.value is ConnectionState.Ready) {
            if (via(s.connection)) return s.name
        }
        for (pc in savedPcs.pcs.value) {
            if (pc.hostId.equals(target.hostId, true) || pc.hostId.equals(s?.hostId, true)) continue
            val conn = HelmConnection(
                hostId = pc.hostId, fingerprint = pc.fingerprint,
                endpoints = EndpointPlanner.forSaved(pc, discovery.find(pc.hostId)),
                identity = identity(), mode = HandshakeMode.Auth, relay = EndpointPlanner.relayFor(pc),
            )
            try {
                withContext(Dispatchers.IO) { conn.connect() }
                if (via(conn)) return pc.name
            } catch (_: HelmException) {
            } finally {
                conn.close()
            }
        }
        return null
    }

    /** Updates the phone-side settings of one saved PC. */
    fun updatePcPrefs(hostId: String, change: (PcPrefs) -> PcPrefs) {
        val pc = savedPcs.get(hostId) ?: return
        savedPcs.upsert(pc.copy(prefs = change(pc.prefs)))
    }

    fun prefsFor(hostId: String?): PcPrefs = hostId?.let { savedPcs.get(it)?.prefs } ?: PcPrefs()

    /**
     * For widgets, tiles and the notification: the ready connection, connecting to the most
     * recently used PC first when there is no session. Null if that fails within [timeoutMs].
     */
    suspend fun ensureConnected(timeoutMs: Long = 12_000): HelmConnection? {
        readyConnection()?.let { return it }
        if (_session.value?.connection?.state?.value !is ConnectionState.Connecting) {
            val pc = savedPcs.pcs.value.firstOrNull() ?: return null
            withContext(Dispatchers.Main) { connectSaved(pc) }
        }
        val conn = _session.value?.connection ?: return null
        return kotlinx.coroutines.withTimeoutOrNull(timeoutMs) {
            conn.state.first { it is ConnectionState.Ready || it is ConnectionState.Failed || it is ConnectionState.Closed }
        }?.let { if (it is ConnectionState.Ready) conn else null }
    }

    /** One quick action from outside the app UI; returns a short result text. */
    suspend fun quickAction(kind: String, arg: String? = null): String {
        val conn = ensureConnected() ?: return "Could not reach the PC."
        val api = HelmApi(conn)
        return try {
            when (kind) {
                QUICK_MEDIA -> { api.mediaControl(arg ?: "toggle_play_pause"); "OK" }
                QUICK_DECK -> { api.runDeck(arg ?: return "No button."); "OK" }
                QUICK_LOCK -> { api.powerAction("lock", null); "Locked the PC." }
                QUICK_MUTE -> {
                    val muted = api.audio()?.muted ?: false
                    api.setAudio(muted = !muted)
                    if (muted) "Sound on." else "Muted."
                }
                else -> "Unknown action."
            }
        } catch (e: HelmException) {
            if (e.code == "forbidden") "This phone's access level does not allow it." else (e.message ?: e.code)
        }
    }

    /** The ready connection of the current session, or null. */
    fun readyConnection(): HelmConnection? =
        _session.value?.connection?.takeIf { it.state.value is ConnectionState.Ready }

    /** Sends Wake-on-LAN magic packets for a saved PC. Returns the number of datagrams sent. */
    suspend fun wake(pc: SavedPc): Int = withContext(Dispatchers.IO) {
        val addrs = listOfNotNull(pc.lastAddress) + pc.addresses
        WakeOnLan.send(pc.macs, WakeOnLan.broadcastTargets(addrs))
    }

    private fun updateName(conn: HelmConnection, name: String) {
        val s = _session.value
        if (s != null && s.connection === conn && name.isNotBlank()) _session.value = s.copy(name = name)
    }

    /**
     * "Forget this PC": revoke our own device on the PC, then delete it locally. If the PC
     * can't be reached the local copy is removed anyway; returns false in that case.
     */
    suspend fun forgetCurrent(): Boolean {
        val s = _session.value ?: return false
        var revoked = false
        if (s.connection.state.value is ConnectionState.Ready) {
            revoked = try {
                s.connection.request(
                    "devices.revoke",
                    buildJsonObject { put("device_id", deviceInfo.deviceId) },
                )
                true
            } catch (e: HelmException) {
                // The agent may close the socket right after revoking us.
                e.code == LocalCodes.CONNECTION_LOST || e.code == LocalCodes.CLOSED
            }
        }
        savedPcs.remove(s.hostId)
        disconnect()
        return revoked
    }

    /**
     * Builds (does not connect) the Phase 2 `/stream` socket for the current session: same
     * pinned certificate, same address the control socket reached, `auth` handshake (§10).
     */
    fun streamSocket(onBinary: (ByteArray) -> Unit): HelmConnection {
        val s = _session.value ?: throw HelmException(LocalCodes.NOT_READY, "Not connected.")
        val ready = s.connection.readyInfo ?: throw HelmException(LocalCodes.NOT_READY, "Not connected.")
        val relay = if (ready.viaRelay) savedPcs.get(s.hostId)?.let { EndpointPlanner.relayFor(it) } else null
        return HelmConnection(
            hostId = s.hostId,
            fingerprint = s.fingerprint,
            endpoints = if (relay != null) emptyList() else listOf(ready.endpoint),
            relay = relay,
            identity = identity(),
            mode = HandshakeMode.Auth,
            path = Endpoint.STREAM_PATH,
            binarySink = onBinary,
        )
    }

    /**
     * Reconnects the control socket of a saved PC after it dropped (e.g. a Wi-Fi change).
     * Returns false when there is nothing to reconnect (no session, still connected, or a
     * one-time pairing session).
     */
    fun reconnectIfDropped(): Boolean {
        val s = _session.value ?: return false
        val st = s.connection.state.value
        if (s.isPairing && s.connection.readyInfo == null) return false
        if (st !is ConnectionState.Failed && st !is ConnectionState.Closed) return false
        val pc = savedPcs.get(s.hostId) ?: return false
        connectSaved(pc)
        return true
    }

    fun forget(hostId: String) = savedPcs.remove(hostId)

    fun disconnect() = closeCurrent()

    private fun closeCurrent() {
        _session.value?.connection?.close()
        _session.value = null
    }
}
