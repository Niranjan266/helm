package com.niranjan.helm.core

import kotlin.math.abs

/**
 * Gyro air mouse (§12 `gy`): turns gyroscope angular rates into pointer deltas. Phone held like
 * a remote, top edge pointing at the screen: turning left/right (yaw, device z) moves X,
 * tilting up/down (pitch, device x) moves Y. Pure maths, unit-tested.
 */
class GyroMouse(
    /** PC pixels per radian at speed 1. */
    private val pxPerRad: Float = 1400f,
    /** Rates below this (rad/s) are hand tremor / drift and ignored. */
    private val deadzone: Float = 0.03f,
) {
    private var dx = 0f
    private var dy = 0f

    /** [rateX], [rateZ] in rad/s from the sensor, [dtS] since the previous sample, [speed] 0.4..3. */
    @Synchronized
    fun add(rateX: Float, rateZ: Float, dtS: Float, speed: Float) {
        if (!rateX.isFinite() || !rateZ.isFinite() || dtS <= 0f || dtS > 0.5f) return
        val yaw = if (abs(rateZ) < deadzone) 0f else rateZ
        val pitch = if (abs(rateX) < deadzone) 0f else rateX
        dx += -yaw * dtS * pxPerRad * speed
        dy += -pitch * dtS * pxPerRad * speed
    }

    /** The pending delta as a `gy` frame, or null when below a hundredth of a pixel. */
    @Synchronized
    fun drain(): InputFrame.Gyro? {
        if (abs(dx) < 0.01f && abs(dy) < 0.01f) return null
        return InputFrame.Gyro(dx, dy).also { dx = 0f; dy = 0f }
    }

    @Synchronized
    fun reset() {
        dx = 0f
        dy = 0f
    }
}

/** One local IPv4 address of a phone network interface. */
data class LocalIf(val name: String, val address: String, val prefixLength: Int)

/** "USB" badge (§12): is the PC reached over the phone's USB tethering interface? */
object UsbTether {
    private val TETHER_NAME = Regex("^(rndis|usb|ncm|eth_usb)\\d*", RegexOption.IGNORE_CASE)

    fun isTetherInterface(name: String): Boolean = TETHER_NAME.containsMatchIn(name)

    /** True when [peer] (IPv4 literal) is in the subnet of a tethering interface. */
    fun isOverUsb(peer: String, interfaces: List<LocalIf>): Boolean {
        val p = ipv4(peer) ?: return false
        return interfaces.any { i ->
            if (!isTetherInterface(i.name)) return@any false
            val a = ipv4(i.address) ?: return@any false
            val bits = i.prefixLength.coerceIn(0, 32)
            val mask = if (bits == 0) 0 else (-1 shl (32 - bits))
            (a and mask) == (p and mask)
        }
    }

    internal fun ipv4(s: String): Int? {
        val parts = s.trim().split('.')
        if (parts.size != 4) return null
        var v = 0
        for (x in parts) {
            val n = x.toIntOrNull() ?: return null
            if (n !in 0..255) return null
            v = (v shl 8) or n
        }
        return v
    }
}
