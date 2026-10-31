package com.niranjan.helm.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.Json

/** A PC this phone has paired with. Saved only after `ready` from a `pair`. */
@Serializable
data class SavedPc(
    @SerialName("host_id") val hostId: String,
    val name: String,
    val addresses: List<String>,
    val port: Int,
    /** Pinned certificate fingerprint (lowercase hex SHA-256). */
    val fingerprint: String,
    @SerialName("last_address") val lastAddress: String? = null,
    @SerialName("last_port") val lastPort: Int? = null,
    @SerialName("last_used_ms") val lastUsedMs: Long = 0,
    /** Wake-on-LAN MACs from `system.info` (best first), `AA:BB:CC:DD:EE:FF`. */
    val macs: List<String> = emptyList(),
    /** Relay base URL from `system.info.relay` (§12), when the PC has the relay enabled. */
    @SerialName("relay_url") val relayUrl: String? = null,
    /** Phone-side settings for this PC (multi-PC, §12). */
    val prefs: PcPrefs = PcPrefs(),
)

/** Per-PC phone settings. */
@Serializable
data class PcPrefs(
    /** Fall back to the relay when the LAN connect fails. */
    @SerialName("use_relay") val useRelay: Boolean = true,
    /** Show Android notifications for this PC's `alert` events. */
    @SerialName("notify_alerts") val notifyAlerts: Boolean = true,
    /** Ask for fingerprint / screen lock before admin actions. */
    @SerialName("biometric_admin") val biometricAdmin: Boolean = true,
    /** Overrides the global touchpad speed for this PC. */
    @SerialName("touchpad_speed") val touchpadSpeed: Float? = null,
    /** Default stream preset for this PC (a [StreamPreset] name). */
    @SerialName("stream_preset") val streamPreset: String? = null,
)

/** An address found on the LAN for a host id (from mDNS). */
data class DiscoveredHost(
    val hostId: String,
    val name: String,
    val address: String,
    val port: Int,
    val serviceName: String,
)

object SavedPcCodec {
    private val json = Json { ignoreUnknownKeys = true; encodeDefaults = true }
    private val listSer = ListSerializer(SavedPc.serializer())

    fun encode(pcs: List<SavedPc>): String = json.encodeToString(listSer, pcs)

    fun decode(text: String?): List<SavedPc> {
        if (text.isNullOrBlank()) return emptyList()
        return try {
            json.decodeFromString(listSer, text)
        } catch (_: Exception) {
            emptyList()
        }
    }
}

object EndpointPlanner {
    /** The relay fallback for a saved PC, or null (no relay URL or turned off for this PC). */
    fun relayFor(pc: SavedPc): RelayTransport? =
        if (pc.prefs.useRelay) RelayTransport.forPc(pc.relayUrl, pc.hostId) else null

    /**
     * Order to try a saved PC: a fresh mDNS address first, then the last address that worked,
     * then the addresses from the QR code. Duplicates removed.
     */
    fun forSaved(pc: SavedPc, discovered: DiscoveredHost?): List<Endpoint> {
        val out = LinkedHashSet<Endpoint>()
        if (discovered != null && discovered.hostId.equals(pc.hostId, ignoreCase = true)) {
            out += Endpoint(discovered.address, discovered.port)
        }
        if (pc.lastAddress != null) out += Endpoint(pc.lastAddress, pc.lastPort ?: pc.port)
        for (a in pc.addresses) out += Endpoint(a, pc.port)
        return out.toList()
    }

    /** Pairing: the QR addresses in order (LAN first, as the agent lists them). */
    fun forPairing(uri: PairingUri, discovered: DiscoveredHost? = null): List<Endpoint> {
        val out = LinkedHashSet<Endpoint>()
        for (a in uri.addresses) out += Endpoint(a, uri.port)
        if (discovered != null && discovered.hostId.equals(uri.hostId, ignoreCase = true)) {
            out += Endpoint(discovered.address, discovered.port)
        }
        return out.toList()
    }

    fun savedFromPairing(uri: PairingUri, ready: ReadyInfo, nowMs: Long): SavedPc = SavedPc(
        hostId = uri.hostId,
        name = ready.hostName.ifBlank { uri.hostName },
        addresses = uri.addresses,
        port = uri.port,
        fingerprint = uri.fingerprint,
        lastAddress = ready.endpoint.host,
        lastPort = ready.endpoint.port,
        lastUsedMs = nowMs,
    )
}
