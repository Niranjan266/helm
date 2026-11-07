package com.niranjan.helm

import android.app.Application
import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import com.niranjan.helm.core.Cancelable
import com.niranjan.helm.core.DiscoveredHost
import com.niranjan.helm.core.ReadyInfo
import com.niranjan.helm.core.ReconnectAddress
import com.niranjan.helm.core.ReconnectClock
import com.niranjan.helm.core.ReconnectState
import com.niranjan.helm.core.ReconnectTransport
import com.niranjan.helm.core.Reconnector
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
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

    /** In-app updates (Settings + home banner). */
    val updates by lazy { com.niranjan.helm.device.AppUpdater(this) }

    override fun onCreate() {
        super.onCreate()
        controller = HelmController(this)
        com.niranjan.helm.device.HelmBackground(this, controller).start()
        com.niranjan.helm.device.EverydayBackground.start(this, controller)
        com.niranjan.helm.device.PhoneBridge.start(this, controller)
    }
}

const val QUICK_MEDIA = "media"
const val QUICK_DECK = "deck"
const val QUICK_LOCK = "lock"
const val QUICK_MUTE = "mute"
/** Power widget: `arg` is a [com.niranjan.helm.core.WidgetPower] action. */
const val QUICK_POWER = "power"

/** One live (or failed) connection the UI is showing. */
data class Session(
    val hostId: String,
    val name: String,
    val connection: HelmConnection,
    val isPairing: Boolean,
    /** Pinned certificate fingerprint (also used for the `/stream` socket). */
    val fingerprint: String,
    /**
     * Started by the app itself (auto-connect to the last PC on launch): the UI keeps showing
     * the PC list until it is ready, and a failure quietly returns there.
     */
    val quiet: Boolean = false,
)

/** App-scoped holder: identity, stores, discovery and the current connection. */
class HelmController(context: Context) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    val deviceInfo = DeviceInfoStore(context)
    val savedPcs = SavedPcStore(context)
    val discovery = HelmDiscovery(context)
    private val phoneNets: com.niranjan.helm.core.PhoneNetworks = com.niranjan.helm.device.AndroidPhoneNetworks(context)
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

    private val main = Handler(Looper.getMainLooper())
    private val connectivity = context.getSystemService(ConnectivityManager::class.java)

    private val _reconnect = MutableStateFlow<ReconnectState>(ReconnectState.Idle)

    /** Auto-reconnect state of the current PC (drives the "Reconnecting…" banner). */
    val reconnect: StateFlow<ReconnectState> = _reconnect.asStateFlow()

    private val reconnector = Reconnector(
        clock = object : ReconnectClock {
            override fun nowMs() = SystemClock.elapsedRealtime()
            override fun schedule(delayMs: Long, task: () -> Unit): Cancelable {
                val r = Runnable(task)
                main.postDelayed(r, delayMs)
                return Cancelable { main.removeCallbacks(r) }
            }
        },
        transport = object : ReconnectTransport {
            override fun connect(hostId: String, attemptId: Long, preferred: ReconnectAddress?) =
                reconnectAttempt(hostId, attemptId, preferred)

            override fun probeLan(hostId: String, probeId: Long, preferred: ReconnectAddress?) =
                lanProbe(hostId, probeId, preferred)
        },
        onState = { st ->
            _reconnect.value = st
            updateDiscovery()
        },
    )

    /** True while an activity is visible (discovery runs then, or while reconnecting). */
    private var uiVisible = false

    init {
        watchConnection()
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
            endpoints = EndpointPlanner.forPairing(uri, discovery.find(uri.hostId), phoneNets.ipv4Subnets()),
            identity = identity(),
            mode = HandshakeMode.Pair(uri.token),
            relay = EndpointPlanner.relayForPairing(uri),
        )
        _session.value = Session(uri.hostId, uri.hostName, conn, isPairing = true, fingerprint = uri.fingerprint)
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                savedPcs.upsert(EndpointPlanner.savedFromPairing(uri, ready, System.currentTimeMillis()))
                if (_session.value?.connection === conn) reconnector.online(uri.hostId, ready.viaRelay)
                updateName(conn, ready.hostName)
                rememberMacs(conn, uri.hostId)
            } catch (_: HelmException) {
                // State flow already shows Failed; nothing is saved.
            }
        }
    }

    /**
     * g4 short-code pairing with a PC found on the network: the user types the 6-digit code the
     * PC shows. Nothing is trusted until the code proves the PC's certificate.
     */
    fun startCodePairing(host: com.niranjan.helm.core.DiscoveredHost, code: String) {
        closeCurrent()
        val conn = HelmConnection(
            hostId = host.hostId,
            fingerprint = "0".repeat(64),
            endpoints = listOf(Endpoint(host.address, host.port)),
            identity = identity(),
            mode = HandshakeMode.PairCode(code),
            transport = com.niranjan.helm.core.CodePairingTransport(),
        )
        _session.value = Session(host.hostId, host.name, conn, isPairing = true, fingerprint = "")
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                val saved = EndpointPlanner.savedFromCodePairing(host, ready, System.currentTimeMillis())
                savedPcs.upsert(saved)
                // The stream socket pins the proven fingerprint.
                _session.value = _session.value?.takeIf { it.connection === conn }?.copy(fingerprint = saved.fingerprint)
                    ?: _session.value
                updateName(conn, ready.hostName)
                rememberMacs(conn, host.hostId)
            } catch (_: HelmException) {
            }
        }
    }

    fun connectSaved(pc: SavedPc, quiet: Boolean = false) {
        closeCurrent()
        val conn = authConnection(pc, discovery.find(pc.hostId))
        _session.value = Session(pc.hostId, pc.name, conn, isPairing = false, fingerprint = pc.fingerprint, quiet = quiet)
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                if (_session.value?.connection !== conn) return@launch
                rememberConnected(pc, ready)
                _session.value?.let { if (it.connection === conn && it.quiet) _session.value = it.copy(quiet = false) }
                reconnector.online(pc.hostId, ready.viaRelay)
                updateName(conn, ready.hostName)
                rememberMacs(conn, pc.hostId)
            } catch (_: HelmException) {
                // An app-started connect that fails just goes back to the PC list.
                val s = _session.value
                if (s != null && s.connection === conn && s.quiet) _session.value = null
            }
        }
    }

    /** On launch: connect to the most recently used PC, without leaving the PC list until ready. */
    fun autoConnectLast() {
        if (_session.value != null) return
        val pc = savedPcs.pcs.value.firstOrNull() ?: return
        connectSaved(pc, quiet = true)
    }

    private fun authConnection(pc: SavedPc, found: DiscoveredHost?) = HelmConnection(
        hostId = pc.hostId,
        fingerprint = pc.fingerprint,
        endpoints = EndpointPlanner.forSaved(pc, found, phoneNets.ipv4Subnets()),
        identity = identity(),
        mode = HandshakeMode.Auth,
        relay = EndpointPlanner.relayFor(pc),
    )

    /** Saves where the PC was reached (LAN address and port, unless it came via the relay). */
    private fun rememberConnected(pc: SavedPc, ready: ReadyInfo) {
        val current = savedPcs.get(pc.hostId) ?: pc
        savedPcs.upsert(
            if (ready.viaRelay) {
                current.copy(name = ready.hostName.ifBlank { current.name }, lastUsedMs = System.currentTimeMillis())
            } else {
                current.copy(
                    name = ready.hostName.ifBlank { current.name },
                    lastAddress = ready.endpoint.host,
                    lastPort = ready.endpoint.port,
                    addresses = EndpointPlanner.withAddress(current, ready.endpoint),
                    lastUsedMs = System.currentTimeMillis(),
                )
            },
        )
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
            if (relay != null) next = next.copy(relayUrl = com.niranjan.helm.core.PcReach.reportedRelay(relay.enabled, relay.url))
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
                endpoints = EndpointPlanner.forSaved(pc, discovery.find(pc.hostId), phoneNets.ipv4Subnets()),
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

    /** The per-PC "Connect from anywhere" switch. Returns an error for the user, or null. */
    fun setAnywhere(hostId: String, on: Boolean, typedUrl: String? = null): String? {
        val pc = savedPcs.get(hostId) ?: return "This PC is not saved any more."
        val next = com.niranjan.helm.core.PcReach.withAnywhere(pc, on, typedUrl)
            ?: return "That does not look like a relay address. Try wss://your-relay.example.com"
        savedPcs.upsert(next)
        return null
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
                QUICK_POWER -> {
                    val action = arg?.takeIf { com.niranjan.helm.core.WidgetPower.isSupported(it) } ?: return "Unknown action."
                    val scheduled = try {
                        api.powerAction(action, null)
                    } catch (e: HelmException) {
                        // Sleep / shut down can drop the socket before the reply arrives.
                        if (action != com.niranjan.helm.core.WidgetPower.LOCK &&
                            (e.code == LocalCodes.CONNECTION_LOST || e.code == LocalCodes.CLOSED)
                        ) null else throw e
                    }
                    com.niranjan.helm.core.WidgetPower.doneText(action, scheduled?.let { it.dueUnixMs - System.currentTimeMillis() })
                }
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
        reconnector.forget(s.hostId)
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
     * Returns false when there is nothing to reconnect (no session, still connected, a
     * one-time pairing session, or auto-reconnect is already on it).
     */
    fun reconnectIfDropped(): Boolean {
        val s = _session.value ?: return false
        if (reconnector.state.isReconnecting) return false
        val st = s.connection.state.value
        if (s.isPairing && s.connection.readyInfo == null) return false
        if (st !is ConnectionState.Failed && st !is ConnectionState.Closed) return false
        val pc = savedPcs.get(s.hostId) ?: return false
        connectSaved(pc)
        return true
    }

    /** Skip the wait: retry the reconnect now (the "Retry now" button). */
    fun reconnectNow() = reconnector.foreground()

    /** An activity became visible / hidden: runs discovery and nudges a pending reconnect. */
    fun setUiVisible(visible: Boolean) {
        uiVisible = visible
        updateDiscovery()
        if (visible) reconnector.foreground()
    }

    fun forget(hostId: String) {
        reconnector.forget(hostId)
        savedPcs.remove(hostId)
    }

    fun disconnect() = closeCurrent()

    private fun closeCurrent() {
        // Stop first: closing the socket must not look like a drop.
        reconnector.stop()
        _session.value?.connection?.close()
        _session.value = null
    }

    // ---- Auto-reconnect ---------------------------------------------------------------

    /** One reconnect attempt: a fresh `auth` connection replaces the dead one in the session. */
    private fun reconnectAttempt(hostId: String, attemptId: Long, preferred: ReconnectAddress?) {
        val s = _session.value
        val pc = savedPcs.get(hostId)
        if (s == null || pc == null || !s.hostId.equals(hostId, ignoreCase = true)) {
            reconnector.stop()
            return
        }
        val found = preferred?.let { DiscoveredHost(pc.hostId, pc.name, it.host, it.port, "") } ?: discovery.find(hostId)
        val conn = authConnection(pc, found)
        s.connection.close()
        _session.value = s.copy(connection = conn, isPairing = false, fingerprint = pc.fingerprint, quiet = false)
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                if (_session.value?.connection !== conn) {
                    conn.close()
                    return@launch
                }
                rememberConnected(pc, ready)
                reconnector.attemptSucceeded(attemptId, ready.viaRelay)
                updateName(conn, ready.hostName)
                rememberMacs(conn, pc.hostId)
            } catch (e: HelmException) {
                if (_session.value?.connection === conn) reconnector.attemptFailed(attemptId, e.code)
            }
        }
    }

    /**
     * The session runs over the relay: try the LAN on the side and swap only once it answers
     * (nothing to lose when it doesn't). A live stream follows on its own, since it restarts when
     * the control socket changes; a file transfer is left to finish first.
     */
    private fun lanProbe(hostId: String, probeId: Long, preferred: ReconnectAddress?) {
        val s = _session.value
        val pc = savedPcs.get(hostId)
        val found = preferred?.let { a -> pc?.let { DiscoveredHost(it.hostId, it.name, a.host, a.port, "") } } ?: discovery.find(hostId)
        val endpoints = pc?.let { EndpointPlanner.forSaved(it, found, phoneNets.ipv4Subnets()) }.orEmpty()
        if (s == null || pc == null || endpoints.isEmpty() || s.connection.transferActive) {
            reconnector.lanProbeFailed(probeId)
            return
        }
        val conn = HelmConnection(
            hostId = pc.hostId,
            fingerprint = pc.fingerprint,
            endpoints = endpoints,
            identity = identity(),
            mode = HandshakeMode.Auth,
            helloTimeoutMs = 4_000,
        )
        scope.launch {
            val ready = try {
                withContext(Dispatchers.IO) { conn.connect() }
            } catch (_: HelmException) {
                reconnector.lanProbeFailed(probeId)
                return@launch
            }
            val current = _session.value
            if (current == null || current.connection !== s.connection || !reconnector.lanProbeSucceeded(probeId)) {
                conn.close()
                return@launch
            }
            rememberConnected(pc, ready)
            _session.value = current.copy(connection = conn, isPairing = false, fingerprint = pc.fingerprint, quiet = false)
            current.connection.close()
            updateName(conn, ready.hostName)
            rememberMacs(conn, pc.hostId)
        }
    }

    @OptIn(ExperimentalCoroutinesApi::class)
    private fun watchConnection() {
        // A ready connection that fails or closes (not by us) is a drop.
        scope.launch {
            session
                .flatMapLatest { s -> s?.connection?.state?.map { s to it } ?: flowOf(null) }
                .collect { pair ->
                    val (s, st) = pair ?: return@collect
                    if (st is ConnectionState.Failed || st is ConnectionState.Closed) reconnector.dropped(s.hostId)
                }
        }
        // A fresh mDNS result for the PC we are trying to get back: try it at once.
        scope.launch {
            var last: DiscoveredHost? = null
            discovery.hosts.collect { hosts ->
                val target = reconnector.target
                val now = target?.let { hosts[it.lowercase()] }
                val prev = last
                last = now
                if (target == null || now == null) return@collect
                // Only a new or changed result counts (PC restarted, or got a new address).
                if (prev == null || prev.address != now.address || prev.port != now.port) {
                    reconnector.discovered(target, ReconnectAddress(now.address, now.port))
                }
            }
        }
        // Network back (Wi-Fi joined, mobile data, VPN): retry at once.
        runCatching {
            connectivity?.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
                override fun onAvailable(network: Network) {
                    main.post { reconnector.networkAvailable() }
                }
            })
        }
    }

    /** mDNS runs while the app is visible, and in the background only while reconnecting. */
    private fun updateDiscovery() {
        if (uiVisible || reconnector.state.isReconnecting) discovery.start() else discovery.stop()
    }
}
