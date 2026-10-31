package com.niranjan.helm.core

import java.nio.ByteBuffer

/**
 * Relay tunnel protocol v1 (relay/README.md). The phone side carries raw tunnel bytes in binary
 * WebSocket messages of 1..[MAX_CHUNK] bytes; the 5-byte mux header ([MuxFrame]) exists only
 * between the relay and the PC agent, and is implemented here for tests and tooling.
 */
object TunnelProtocol {
    const val MAX_CHUNK = 65_536
    const val CONNECT_PATH = "/connect/"

    /** Dummy inner host name (README §4.2); never resolved. */
    fun innerHost(hostId: String): String = "${hostId.lowercase()}.relay.helm.invalid"

    const val INNER_PORT = 443

    private val UUID_RE = Regex("^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$")

    fun isHostId(s: String): Boolean = UUID_RE.matches(s)

    /**
     * `wss://<relay>/connect/<host_id>` from the agent's `relay.url`. Accepts `wss://`, `ws://`,
     * `https://`, `http://` or a bare host; trailing slashes are dropped. Null when unusable.
     */
    fun connectUrl(relayUrl: String?, hostId: String): String? {
        if (relayUrl.isNullOrBlank() || !isHostId(hostId)) return null
        var u = relayUrl.trim().trimEnd('/')
        val lower = u.lowercase()
        u = when {
            lower.startsWith("wss://") || lower.startsWith("ws://") -> u
            lower.startsWith("https://") -> "wss://" + u.substring(8)
            lower.startsWith("http://") -> "ws://" + u.substring(7)
            "://" in u -> return null
            else -> "wss://$u"
        }
        val hostPart = u.substringAfter("://").substringBefore('/')
        if (hostPart.isEmpty() || ' ' in u) return null
        return u + CONNECT_PATH + hostId.lowercase()
    }

    /** Splits [len] bytes of [b] from [off] into tunnel chunks of at most [MAX_CHUNK]. */
    fun chunks(b: ByteArray, off: Int = 0, len: Int = b.size - off): List<ByteArray> {
        val out = ArrayList<ByteArray>((len + MAX_CHUNK - 1) / MAX_CHUNK)
        var i = off
        val end = off + len
        while (i < end) {
            val n = minOf(MAX_CHUNK, end - i)
            out += b.copyOfRange(i, i + n)
            i += n
        }
        return out
    }

    /** Human text for relay close codes (README §5). */
    fun closeReason(code: Int): String = when (code) {
        1000 -> "The PC closed the relayed session."
        1009 -> "Relay frame too large."
        4400 -> "Relay protocol error."
        4401, 4403 -> "The relay rejected the PC's key."
        4408 -> "Relay connection timed out."
        4409 -> "The PC reconnected to the relay."
        4410 -> "The PC went away from the relay."
        else -> "Relay closed the connection ($code)."
    }

    fun httpReason(code: Int): String = when (code) {
        404 -> "The PC is not connected to the relay right now."
        429 -> "Too many relay connections; try again in a minute."
        503 -> "The PC already has the maximum number of relayed phones."
        400 -> "The relay did not accept this PC id."
        else -> "Relay answered HTTP $code."
    }
}

/** Relay↔host mux frame: `[type u8][channel u32 BE][payload 0..65536]`. */
data class MuxFrame(val type: Int, val channel: Long, val payload: ByteArray = ByteArray(0)) {
    init {
        require(type in 0..255) { "type must fit a byte" }
        require(channel in 0..0xFFFF_FFFFL) { "channel must fit u32" }
        require(payload.size <= TunnelProtocol.MAX_CHUNK) { "payload too large" }
    }

    fun encode(): ByteArray =
        ByteBuffer.allocate(HEADER + payload.size).put(type.toByte()).putInt(channel.toInt()).put(payload).array()

    override fun equals(other: Any?): Boolean =
        other is MuxFrame && other.type == type && other.channel == channel && other.payload.contentEquals(payload)

    override fun hashCode(): Int = (type * 31 + channel.hashCode()) * 31 + payload.contentHashCode()

    companion object {
        const val OPEN = 0x01
        const val DATA = 0x02
        const val CLOSE = 0x03
        const val HEADER = 5

        sealed interface Decoded {
            data class Ok(val frame: MuxFrame) : Decoded

            /** Shorter than 5 bytes: the relay closes with 4400. */
            data object TooShort : Decoded

            /** Payload over 65 536 bytes: the relay closes with 1009. */
            data object TooLarge : Decoded
        }

        fun decode(bytes: ByteArray): Decoded {
            if (bytes.size < HEADER) return Decoded.TooShort
            if (bytes.size - HEADER > TunnelProtocol.MAX_CHUNK) return Decoded.TooLarge
            val buf = ByteBuffer.wrap(bytes)
            val type = buf.get().toInt() and 0xff
            val ch = buf.int.toLong() and 0xFFFF_FFFFL
            return Decoded.Ok(MuxFrame(type, ch, bytes.copyOfRange(HEADER, bytes.size)))
        }
    }
}
