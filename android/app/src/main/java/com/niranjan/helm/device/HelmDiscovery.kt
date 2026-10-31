package com.niranjan.helm.device

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.util.Log
import com.niranjan.helm.core.DiscoveredHost
import java.net.Inet4Address
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * Finds Helm agents on the LAN with DNS-SD (`_helm._tcp`, PROTOCOL.md §3). Resolves one service
 * at a time (NsdManager rejects parallel resolves on older Android versions). Discovery only finds
 * addresses; trust always comes from the pinned fingerprint.
 */
class HelmDiscovery(context: Context) {
    private val nsd = context.applicationContext.getSystemService(NsdManager::class.java)
    private val wifi = context.applicationContext.getSystemService(WifiManager::class.java)

    private val _hosts = MutableStateFlow<Map<String, DiscoveredHost>>(emptyMap())

    /** host_id (lowercase) -> latest resolved address. */
    val hosts: StateFlow<Map<String, DiscoveredHost>> = _hosts.asStateFlow()

    private var discoveryListener: NsdManager.DiscoveryListener? = null
    private var multicastLock: WifiManager.MulticastLock? = null
    private val queue = ArrayDeque<NsdServiceInfo>()
    private var resolving = false

    fun find(hostId: String): DiscoveredHost? = _hosts.value[hostId.lowercase()]

    @Synchronized
    fun start() {
        if (discoveryListener != null || nsd == null) return
        multicastLock = wifi?.createMulticastLock("helm-discovery")?.apply {
            setReferenceCounted(false)
            try {
                acquire()
            } catch (e: Exception) {
                Log.w(TAG, "multicast lock failed", e)
            }
        }
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) = Unit
            override fun onDiscoveryStopped(serviceType: String) = Unit

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "discovery start failed: $errorCode")
                synchronized(this@HelmDiscovery) {
                    if (discoveryListener === this) discoveryListener = null
                }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) = Unit

            override fun onServiceFound(info: NsdServiceInfo) = enqueue(info)

            override fun onServiceLost(info: NsdServiceInfo) {
                _hosts.value = _hosts.value.filterValues { it.serviceName != info.serviceName }
            }
        }
        discoveryListener = listener
        try {
            nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
        } catch (e: Exception) {
            Log.w(TAG, "discoverServices failed", e)
            discoveryListener = null
        }
    }

    @Synchronized
    fun stop() {
        discoveryListener?.let {
            try {
                nsd?.stopServiceDiscovery(it)
            } catch (_: Exception) {
            }
        }
        discoveryListener = null
        queue.clear()
        multicastLock?.let { if (it.isHeld) it.release() }
        multicastLock = null
        // Keep the last known hosts; they may still be valid and are only hints anyway.
    }

    @Synchronized
    private fun enqueue(info: NsdServiceInfo) {
        if (queue.none { it.serviceName == info.serviceName }) queue.addLast(info)
        resolveNext()
    }

    @Synchronized
    private fun resolveNext() {
        if (resolving || discoveryListener == null) return
        val next = queue.removeFirstOrNull() ?: return
        resolving = true
        try {
            @Suppress("DEPRECATION")
            nsd?.resolveService(next, object : NsdManager.ResolveListener {
                override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                    Log.d(TAG, "resolve failed for ${serviceInfo.serviceName}: $errorCode")
                    done()
                }

                override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                    record(serviceInfo)
                    done()
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "resolveService failed", e)
            resolving = false
        }
    }

    @Synchronized
    private fun done() {
        resolving = false
        resolveNext()
    }

    private fun record(info: NsdServiceInfo) {
        val attrs = info.attributes ?: emptyMap()
        fun attr(key: String) = attrs[key]?.let { String(it, Charsets.UTF_8) }
        val version = attr("v")
        if (version != null && version != "1") return
        val id = attr("id")?.trim()?.lowercase() ?: return
        @Suppress("DEPRECATION")
        val host = info.host
        // Phase 0 works with IPv4 (as in the QR link); skip IPv6 link-local results.
        if (host !is Inet4Address) return
        val address = host.hostAddress ?: return
        val name = attr("name")?.takeIf { it.isNotBlank() } ?: info.serviceName
        val found = DiscoveredHost(id, name, address, info.port, info.serviceName)
        _hosts.value = _hosts.value + (id to found)
    }

    private companion object {
        const val TAG = "HelmDiscovery"
        // NsdManager wants the type without the trailing dot or ".local".
        const val SERVICE_TYPE = "_helm._tcp"
    }
}
