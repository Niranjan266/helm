package com.niranjan.helm.core

import java.net.URLDecoder

/**
 * A parsed `helm://pair?...` link (PROTOCOL.md §4).
 *
 * `helm://pair?v=1&id=<host_id>&n=<host_name>&a=<ip1>,<ip2>&p=<port>&fp=<64 hex>&t=<token>[&r=<relay>]`
 */
data class PairingUri(
    val hostId: String,
    val hostName: String,
    val addresses: List<String>,
    val port: Int,
    /** Lowercase hex SHA-256 of the agent's certificate DER (64 chars). */
    val fingerprint: String,
    /** One-time pairing token (base64url, no padding). */
    val token: String,
    /** Normalised relay address (`r=`, §12) when the PC has "Connect from anywhere" on. */
    val relayUrl: String? = null,
) {
    /** Never expose the token in logs. */
    override fun toString(): String =
        "PairingUri(hostId=$hostId, hostName=$hostName, addresses=$addresses, port=$port, " +
            "fingerprint=$fingerprint, relayUrl=$relayUrl, token=***)"

    companion object {
        const val SCHEME = "helm"
        const val HOST = "pair"
        const val SUPPORTED_VERSION = "1"

        private val HEX64 = Regex("^[0-9a-fA-F]{64}$")
        private val BASE64URL = Regex("^[A-Za-z0-9_-]+$")
        private val UUID_RE =
            Regex("^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")
        private val IPV4 = Regex("^(\\d{1,3})\\.(\\d{1,3})\\.(\\d{1,3})\\.(\\d{1,3})$")

        /** Parses and validates; throws [PairingUriException] with a user-readable message. */
        fun parse(raw: String): PairingUri {
            val text = raw.trim()
            val sep = text.indexOf("://")
            if (sep <= 0) fail("Not a Helm pairing link.")
            val scheme = text.substring(0, sep)
            if (!scheme.equals(SCHEME, ignoreCase = true)) {
                fail("Not a Helm pairing link (it must start with helm://).")
            }
            val rest = text.substring(sep + 3)
            val q = rest.indexOf('?')
            if (q < 0) fail("Pairing link has no parameters.")
            val host = rest.substring(0, q).trimEnd('/')
            if (!host.equals(HOST, ignoreCase = true)) fail("Not a pairing link (expected helm://pair).")
            var query = rest.substring(q + 1)
            val hash = query.indexOf('#')
            if (hash >= 0) query = query.substring(0, hash)

            val params = LinkedHashMap<String, String>()
            for (part in query.split('&')) {
                if (part.isEmpty()) continue
                val eq = part.indexOf('=')
                val key = decode(if (eq < 0) part else part.substring(0, eq))
                val value = if (eq < 0) "" else decode(part.substring(eq + 1))
                // First occurrence wins; later duplicates are ignored.
                if (key !in params) params[key] = value
            }

            val v = params["v"] ?: fail("Pairing link is missing the version (v).")
            if (v != SUPPORTED_VERSION) fail("Unsupported pairing link version \"$v\". Update Helm.")

            val id = params["id"]?.trim().orEmpty()
            if (id.isEmpty()) fail("Pairing link is missing the PC id.")
            if (!UUID_RE.matches(id)) fail("Pairing link has an invalid PC id.")

            val fp = params["fp"]?.trim().orEmpty()
            if (!HEX64.matches(fp)) fail("Pairing link has an invalid certificate fingerprint.")

            val portText = params["p"]?.trim().orEmpty()
            val port = portText.toIntOrNull() ?: fail("Pairing link has an invalid port.")
            if (port !in 1..65535) fail("Pairing link port $port is out of range.")

            // An unusable relay address only costs the away-from-home path; the LAN still works.
            val relayUrl = TunnelProtocol.normalizeRelayUrl(params["r"])

            val addresses = params["a"].orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }
            if (addresses.isEmpty() && relayUrl == null) fail("Pairing link has no PC addresses.")
            for (addr in addresses) {
                if (!isIpv4(addr)) fail("Pairing link has an invalid address: $addr")
            }

            val token = params["t"]?.trim().orEmpty()
            if (token.isEmpty()) fail("Pairing link is missing the pairing token.")
            if (!BASE64URL.matches(token)) fail("Pairing link has an invalid pairing token.")

            val name = params["n"]?.trim().orEmpty().ifEmpty { "PC ${id.take(8)}" }

            return PairingUri(
                hostId = id,
                hostName = name,
                addresses = addresses.distinct(),
                port = port,
                fingerprint = fp.lowercase(),
                token = token,
                relayUrl = relayUrl,
            )
        }

        fun parseOrNull(raw: String): PairingUri? = try {
            parse(raw)
        } catch (_: PairingUriException) {
            null
        }

        private fun isIpv4(s: String): Boolean {
            val m = IPV4.matchEntire(s) ?: return false
            return m.groupValues.drop(1).all { it.toInt() in 0..255 }
        }

        private fun decode(s: String): String = try {
            URLDecoder.decode(s, "UTF-8")
        } catch (e: IllegalArgumentException) {
            fail("Pairing link is not correctly encoded.")
        }

        private fun fail(message: String): Nothing = throw PairingUriException(message)
    }
}

class PairingUriException(message: String) : IllegalArgumentException(message)
