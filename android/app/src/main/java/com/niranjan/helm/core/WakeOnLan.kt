package com.niranjan.helm.core

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.Inet4Address
import java.net.InetAddress

/** Wake-on-LAN magic packets (sent by the phone; the PC only reports its MACs). */
object WakeOnLan {
    const val PORT = 9

    /** Parses `AA:BB:CC:DD:EE:FF`, `aa-bb-…` or `aabbccddeeff` into 6 bytes, else null. */
    fun parseMac(text: String): ByteArray? {
        val hex = text.trim().replace(":", "").replace("-", "").replace(".", "")
        if (hex.length != 12) return null
        return ByteArray(6) { i ->
            val hi = Character.digit(hex[2 * i], 16)
            val lo = Character.digit(hex[2 * i + 1], 16)
            if (hi < 0 || lo < 0) return null
            ((hi shl 4) or lo).toByte()
        }
    }

    /** 6 × 0xFF followed by the MAC 16 times (102 bytes). */
    fun magicPacket(mac: ByteArray): ByteArray {
        require(mac.size == 6) { "MAC must be 6 bytes" }
        val out = ByteArray(6 + 16 * 6)
        for (i in 0 until 6) out[i] = 0xFF.toByte()
        for (r in 0 until 16) System.arraycopy(mac, 0, out, 6 + r * 6, 6)
        return out
    }

    /** True if the "locally administered" bit (0x02 of the first octet) is set. */
    fun isLocallyAdministered(mac: ByteArray): Boolean = (mac[0].toInt() and 0x02) != 0

    /**
     * MACs to wake, normalised to `AA:BB:CC:DD:EE:FF`: burned-in (not locally administered)
     * addresses first, then the rest; invalid and duplicate entries dropped.
     */
    fun chooseMacs(entries: List<MacEntry>): List<String> {
        val parsed = entries.mapNotNull { e -> parseMac(e.mac)?.let { e to it } }
        val (global, local) = parsed.partition { (e, bytes) -> !e.locallyAdministered && !isLocallyAdministered(bytes) }
        return (global + local).map { (_, b) -> format(b) }.distinct()
    }

    fun format(mac: ByteArray): String = mac.joinToString(":") { "%02X".format(it) }

    /**
     * Broadcast targets: the limited broadcast plus a /24 directed broadcast for each known
     * IPv4 address of the PC (most home LANs are /24).
     */
    fun broadcastTargets(pcAddresses: List<String>): List<InetAddress> {
        val out = LinkedHashSet<InetAddress>()
        out += InetAddress.getByName("255.255.255.255")
        for (a in pcAddresses) {
            val parts = a.split('.')
            if (parts.size != 4 || parts.any { it.toIntOrNull() !in 0..255 }) continue
            out += InetAddress.getByName("${parts[0]}.${parts[1]}.${parts[2]}.255")
        }
        return out.filterIsInstance<Inet4Address>()
    }

    /** Sends a packet for every MAC to every target. Returns the number of datagrams sent. */
    fun send(macs: List<String>, targets: List<InetAddress>, port: Int = PORT): Int {
        var sent = 0
        DatagramSocket().use { socket ->
            socket.broadcast = true
            for (m in macs) {
                val mac = parseMac(m) ?: continue
                val packet = magicPacket(mac)
                for (t in targets) {
                    try {
                        socket.send(DatagramPacket(packet, packet.size, t, port))
                        sent++
                    } catch (_: Exception) {
                        // Some targets (e.g. a directed broadcast on another subnet) may be refused.
                    }
                }
            }
        }
        return sent
    }
}
