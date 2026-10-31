package com.niranjan.helm.device

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build

/**
 * Keeps Wi-Fi out of power-save while held, so input packets are not delayed by the radio
 * sleeping between beacons. Low-latency mode on Android 10+, high-performance before.
 */
class WifiLatencyLock(context: Context) {
    private val lock: WifiManager.WifiLock? = runCatching {
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        @Suppress("DEPRECATION")
        val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            WifiManager.WIFI_MODE_FULL_LOW_LATENCY
        } else {
            WifiManager.WIFI_MODE_FULL_HIGH_PERF
        }
        wifi.createWifiLock(mode, "helm:input").apply { setReferenceCounted(false) }
    }.getOrNull()

    fun acquire() {
        runCatching { lock?.acquire() }
    }

    fun release() {
        runCatching { if (lock?.isHeld == true) lock.release() }
    }
}
