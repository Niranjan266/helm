package com.niranjan.helm

import android.app.Application
import android.content.Context
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.DeviceIdentity
import com.niranjan.helm.core.DeviceSigner
import com.niranjan.helm.core.EndpointPlanner
import com.niranjan.helm.core.HandshakeMode
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.LocalCodes
import com.niranjan.helm.core.MacEntry
import com.niranjan.helm.core.WakeOnLan
import com.niranjan.helm.core.PairingUri
import com.niranjan.helm.core.SavedPc
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
    }
}

/** One live (or failed) connection the UI is showing. */
data class Session(
    val hostId: String,
    val name: String,
    val connection: HelmConnection,
    val isPairing: Boolean,
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
        _session.value = Session(uri.hostId, uri.hostName, conn, isPairing = true)
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
        )
        _session.value = Session(pc.hostId, pc.name, conn, isPairing = false)
        scope.launch {
            try {
                val ready = withContext(Dispatchers.IO) { conn.connect() }
                val current = savedPcs.get(pc.hostId) ?: pc
                savedPcs.upsert(
                    current.copy(
                        name = ready.hostName.ifBlank { current.name },
                        lastAddress = ready.endpoint.host,
                        lastPort = ready.endpoint.port,
                        lastUsedMs = System.currentTimeMillis(),
                    ),
                )
                updateName(conn, ready.hostName)
                rememberMacs(conn, pc.hostId)
            } catch (_: HelmException) {
            }
        }
    }

    /** Saves the PC's Wake-on-LAN MACs (from `system.info`) for when it is offline. */
    private suspend fun rememberMacs(conn: HelmConnection, hostId: String) {
        try {
            val macs = WakeOnLan.chooseMacs(MacEntry.listFrom(conn.request("system.info")))
            val current = savedPcs.get(hostId) ?: return
            if (macs.isNotEmpty() && macs != current.macs) savedPcs.upsert(current.copy(macs = macs))
        } catch (_: HelmException) {
        }
    }

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

    fun forget(hostId: String) = savedPcs.remove(hostId)

    fun disconnect() = closeCurrent()

    private fun closeCurrent() {
        _session.value?.connection?.close()
        _session.value = null
    }
}
