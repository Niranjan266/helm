package com.niranjan.helm.device

import android.content.Context
import android.net.ConnectivityManager
import com.niranjan.helm.core.PhoneNetworks
import com.niranjan.helm.core.PhoneSubnet
import java.net.Inet4Address

/** The phone's current IPv4 addresses and prefixes, from ConnectivityManager's LinkProperties. */
class AndroidPhoneNetworks(context: Context) : PhoneNetworks {
    private val cm = context.applicationContext.getSystemService(ConnectivityManager::class.java)

    @Suppress("DEPRECATION")
    override fun ipv4Subnets(): List<PhoneSubnet> = try {
        val nets = cm?.allNetworks.orEmpty()
        nets.flatMap { n ->
            cm?.getLinkProperties(n)?.linkAddresses.orEmpty().mapNotNull { la ->
                val a = la.address as? Inet4Address ?: return@mapNotNull null
                if (a.isLoopbackAddress) return@mapNotNull null
                PhoneSubnet(a.hostAddress ?: return@mapNotNull null, la.prefixLength)
            }
        }.distinct().ifEmpty { NetIfaces.local().map { PhoneSubnet(it.address, it.prefixLength) } }
    } catch (_: Exception) {
        emptyList()
    }
}
