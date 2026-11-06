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
    /** Relay address from the QR (`r=`), `system.info.relay` or the user (§12). */
    @SerialName("relay_url") val relayUrl: String? = null,
    /** Phone-side settings for this PC (multi-PC, §12). */
    val prefs: PcPrefs = PcPrefs(),
)

/** Per-PC phone settings. */
@Serializable
data class PcPrefs(
    /** "Connect from anywhere": fall back to the relay when the LAN connect fails. */
    @SerialName("use_relay") val useRelay: Boolean = true,
    /** Show Android notifications for this PC's `alert` events. */
    @SerialName("notify_alerts") val notifyAlerts: Boolean = true,
    /** Ask for fingerprint / screen lock before admin actions. */
    @SerialName("biometric_admin") val biometricAdmin: Boolean = true,
    /** Overrides the global touchpad speed for this PC. */
    @SerialName("touchpad_speed") val touchpadSpeed: Float? = null,
    /** Default stream preset for this PC (a [StreamPreset] name). */
    @SerialName("stream_preset") val streamPreset: String? = null,
    /** Full-screen picture: true = Fill (crop edges), false/null = Fit (letterboxed). */
    @SerialName("stream_fill") val streamFill: Boolean? = null,
)

/**
 * [SavedPc] with its relay URL normalised like the agent does (`wss://https://host` from an old
 * agent becomes `wss://host`). An unusable value is dropped. Migrates stored values on load.
 */
fun SavedPc.withNormalizedRelay(): SavedPc {
    val raw = relayUrl ?: return this
    val fixed = TunnelProtocol.normalizeRelayUrl(raw)
    return if (fixed == raw) this else copy(relayUrl = fixed)
}

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
            json.decodeFromString(listSer, text).map { it.withNormalizedRelay() }
        } catch (_: Exception) {
            emptyList()
        }
    }
}

/** How a saved PC can be reached right now, for the Home screen. */
enum class PcLink { CONNECTING, WIFI, RELAY, SEARCHING }

data class PcReach(val link: PcLink, val text: String) {
    companion object {
        fun of(pc: SavedPc, found: DiscoveredHost?, connecting: Boolean): PcReach = when {
            connecting -> PcReach(PcLink.CONNECTING, "Connecting…")
            found != null -> PcReach(PcLink.WIFI, "Wi-Fi · ${found.address}")
            anywhereOn(pc) -> PcReach(PcLink.RELAY, "Relay · from anywhere")
            else -> PcReach(PcLink.SEARCHING, "Searching the network…")
        }

        /** The per-PC "Connect from anywhere" switch. */
        fun anywhereOn(pc: SavedPc): Boolean = pc.prefs.useRelay && !pc.relayUrl.isNullOrBlank()

        /**
         * [pc] with the switch set. Turning on uses [typedUrl] when given (normalised like the
         * PC does), else the address already saved. Null = no usable relay address: ask for one.
         */
        fun withAnywhere(pc: SavedPc, on: Boolean, typedUrl: String? = null): SavedPc? {
            if (!on) return pc.copy(prefs = pc.prefs.copy(useRelay = false))
            val url = TunnelProtocol.normalizeRelayUrl(if (typedUrl.isNullOrBlank()) pc.relayUrl else typedUrl) ?: return null
            return pc.copy(relayUrl = url, prefs = pc.prefs.copy(useRelay = true))
        }

        /** The relay address the PC reports (`system.info.relay`), normalised; null when off. */
        fun reportedRelay(enabled: Boolean, url: String?): String? =
            if (enabled) TunnelProtocol.normalizeRelayUrl(url) else null
    }
}

object EndpointPlanner {
    /** The relay fallback for a saved PC, or null (no relay URL or turned off for this PC). */
    fun relayFor(pc: SavedPc): RelayTransport? =
        if (pc.prefs.useRelay) RelayTransport.forPc(pc.relayUrl, pc.hostId) else null

    /**
     * Order to try a saved PC: a fresh mDNS address first, then the last address that worked,
     * then the addresses from the QR code. Duplicates removed; with the phone's [nets], addresses
     * on its own subnet(s) go first, then other private ranges, then the rest.
     */
    fun forSaved(pc: SavedPc, discovered: DiscoveredHost?, nets: List<PhoneSubnet> = emptyList()): List<Endpoint> {
        val out = LinkedHashSet<Endpoint>()
        if (discovered != null && discovered.hostId.equals(pc.hostId, ignoreCase = true)) {
            out += Endpoint(discovered.address, discovered.port)
        }
        if (pc.lastAddress != null) out += Endpoint(pc.lastAddress, pc.lastPort ?: pc.port)
        for (a in pc.addresses) out += Endpoint(a, pc.port)
        return EndpointRanking.rank(out.toList(), nets)
    }

    /**
     * The saved address list after reaching the PC at [reached]: a new LAN address on the
     * saved port goes first (so a later reconnect still knows it), capped at [MAX_ADDRESSES].
     */
    fun withAddress(pc: SavedPc, reached: Endpoint): List<String> {
        if (reached.port != pc.port || reached.host in pc.addresses) return pc.addresses
        return (listOf(reached.host) + pc.addresses).take(MAX_ADDRESSES)
    }

    const val MAX_ADDRESSES = 8

    /** The relay route for a pairing link (`r=`), tried after the LAN addresses. */
    fun relayForPairing(uri: PairingUri): RelayTransport? = RelayTransport.forPc(uri.relayUrl, uri.hostId)

    /** Pairing: the QR addresses, ranked by [EndpointRanking] when the phone's [nets] are known. */
    fun forPairing(uri: PairingUri, discovered: DiscoveredHost? = null, nets: List<PhoneSubnet> = emptyList()): List<Endpoint> {
        val out = LinkedHashSet<Endpoint>()
        for (a in uri.addresses) out += Endpoint(a, uri.port)
        if (discovered != null && discovered.hostId.equals(uri.hostId, ignoreCase = true)) {
            out += Endpoint(discovered.address, discovered.port)
        }
        return EndpointRanking.rank(out.toList(), nets)
    }

    /** g4 code pairing: the PC found by mDNS, pinned to the fingerprint the code proved. */
    fun savedFromCodePairing(host: DiscoveredHost, ready: ReadyInfo, nowMs: Long): SavedPc = SavedPc(
        hostId = host.hostId,
        name = ready.hostName.ifBlank { host.name },
        addresses = listOf(host.address),
        port = host.port,
        fingerprint = requireNotNull(ready.fingerprint) { "code pairing must prove a fingerprint" },
        lastAddress = ready.endpoint.host,
        lastPort = ready.endpoint.port,
        lastUsedMs = nowMs,
    )

    fun savedFromPairing(uri: PairingUri, ready: ReadyInfo, nowMs: Long): SavedPc = SavedPc(
        hostId = uri.hostId,
        name = ready.hostName.ifBlank { uri.hostName },
        addresses = uri.addresses,
        port = uri.port,
        fingerprint = uri.fingerprint,
        // The dummy inner host of a relay session is not an address to come back to.
        lastAddress = ready.endpoint.host.takeUnless { ready.viaRelay },
        lastPort = ready.endpoint.port.takeUnless { ready.viaRelay },
        lastUsedMs = nowMs,
        relayUrl = TunnelProtocol.normalizeRelayUrl(uri.relayUrl),
    )
}
