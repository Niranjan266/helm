package com.niranjan.helm.core

/** One IPv4 network this phone is on right now (address + prefix length). */
data class PhoneSubnet(val address: String, val prefixLength: Int)

/** The phone's current IPv4 networks. Behind an interface so the ranking stays pure JVM. */
fun interface PhoneNetworks {
    fun ipv4Subnets(): List<PhoneSubnet>
}

/**
 * Orders the addresses of a PC for a connect race: addresses on the phone's own subnet(s) first
 * (the real Wi-Fi LAN), then other private ranges (and host names), then everything else.
 * Stable within each group.
 */
object EndpointRanking {
    private const val SAME_SUBNET = 0
    private const val PRIVATE = 1
    private const val OTHER = 2

    fun parseIpv4(s: String): Long? {
        val parts = s.trim().split('.')
        if (parts.size != 4) return null
        var v = 0L
        for (p in parts) {
            if (p.isEmpty() || p.length > 3 || !p.all { it in '0'..'9' }) return null
            val n = p.toInt()
            if (n > 255) return null
            v = (v shl 8) or n.toLong()
        }
        return v
    }

    private fun mask(prefix: Int): Long =
        if (prefix <= 0) 0L else if (prefix >= 32) 0xFFFFFFFFL else (0xFFFFFFFFL shl (32 - prefix)) and 0xFFFFFFFFL

    fun isPrivate(ip: Long): Boolean {
        val a = (ip shr 24).toInt() and 0xFF
        val b = (ip shr 16).toInt() and 0xFF
        return a == 10 || (a == 172 && b in 16..31) || (a == 192 && b == 168)
    }

    fun tier(host: String, nets: List<PhoneSubnet>): Int {
        val ip = parseIpv4(host) ?: return PRIVATE // a host name (mDNS): neither known-dead nor known-local
        for (n in nets) {
            val base = parseIpv4(n.address) ?: continue
            val m = mask(n.prefixLength)
            if (n.prefixLength in 1..32 && (ip and m) == (base and m)) return SAME_SUBNET
        }
        return if (isPrivate(ip)) PRIVATE else OTHER
    }

    fun rank(endpoints: List<Endpoint>, nets: List<PhoneSubnet>): List<Endpoint> =
        endpoints.withIndex()
            .sortedWith(compareBy({ tier(it.value.host, nets) }, { it.index }))
            .map { it.value }
}
