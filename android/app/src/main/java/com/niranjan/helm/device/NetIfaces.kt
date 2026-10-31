package com.niranjan.helm.device

import com.niranjan.helm.core.LocalIf
import java.net.Inet4Address
import java.net.NetworkInterface

/** This phone's up IPv4 interfaces (for the USB tethering badge, §12). */
object NetIfaces {
    fun local(): List<LocalIf> = try {
        NetworkInterface.getNetworkInterfaces()?.toList().orEmpty()
            .filter { runCatching { it.isUp && !it.isLoopback }.getOrDefault(false) }
            .flatMap { ni ->
                ni.interfaceAddresses.mapNotNull { ia ->
                    val a = ia.address as? Inet4Address ?: return@mapNotNull null
                    LocalIf(ni.name, a.hostAddress ?: return@mapNotNull null, ia.networkPrefixLength.toInt())
                }
            }
    } catch (_: Exception) {
        emptyList()
    }
}
