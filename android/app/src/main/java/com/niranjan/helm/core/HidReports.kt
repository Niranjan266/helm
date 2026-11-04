package com.niranjan.helm.core

import kotlin.math.roundToInt

/*
 * Report builders for [HidDescriptor]. Each returns the report *data* (without the report id
 * byte; Android's `sendReport` takes the id separately).
 */

object MouseReport {
    const val SIZE = 7
    const val MAX_XY = 32767
    const val MAX_WHEEL = 127

    /** [buttons] bit 0 = left, 1 = right, 2 = middle, 3 = back, 4 = forward. Values are clamped to the report range. */
    fun build(buttons: Int, dx: Int, dy: Int, wheel: Int = 0, pan: Int = 0): ByteArray {
        val x = dx.coerceIn(-MAX_XY, MAX_XY)
        val y = dy.coerceIn(-MAX_XY, MAX_XY)
        return byteArrayOf(
            (buttons and 0x1F).toByte(),
            (x and 0xFF).toByte(), ((x shr 8) and 0xFF).toByte(),
            (y and 0xFF).toByte(), ((y shr 8) and 0xFF).toByte(),
            wheel.coerceIn(-MAX_WHEEL, MAX_WHEEL).toByte(),
            pan.coerceIn(-MAX_WHEEL, MAX_WHEEL).toByte(),
        )
    }

    fun buttonBit(b: MouseButton): Int = when (b) {
        MouseButton.LEFT -> 0x01
        MouseButton.RIGHT -> 0x02
        MouseButton.MIDDLE -> 0x04
        MouseButton.X1 -> 0x08
        MouseButton.X2 -> 0x10
    }
}

object KeyboardReport {
    const val SIZE = 8
    const val ROLLOVER = 6

    /** [modifiers] is the HID modifier byte; at most [ROLLOVER] key [usages] are used. */
    fun build(modifiers: Int, usages: List<Int>): ByteArray {
        val out = ByteArray(SIZE)
        out[0] = modifiers.toByte()
        usages.take(ROLLOVER).forEachIndexed { i, u -> out[2 + i] = u.toByte() }
        return out
    }
}

/** Consumer-control keys, one bit each in the report. */
enum class ConsumerKey(val bit: Int) {
    PLAY_PAUSE(0x01),
    NEXT(0x02),
    PREVIOUS(0x04),
    VOLUME_UP(0x08),
    VOLUME_DOWN(0x10),
    MUTE(0x20),
}

object ConsumerReport {
    const val SIZE = 1

    fun build(keys: Int): ByteArray = byteArrayOf((keys and 0x3F).toByte())
}

/**
 * Sums fractional deltas and hands out whole counts, at most [limit] in either direction per
 * report. What does not fit (or is under half a count) stays for the next report, so slow
 * motion never drifts short; the stored remainder is capped so a stalled link cannot build up
 * an enormous jump.
 */
class DeltaAccumulator(private val limit: Int) {
    private var x = 0f
    private var y = 0f
    private val cap = limit * 4f

    @Synchronized
    fun add(dx: Float, dy: Float) {
        if (!dx.isFinite() || !dy.isFinite()) return
        x = (x + dx).coerceIn(-cap, cap)
        y = (y + dy).coerceIn(-cap, cap)
    }

    /** The whole counts to send now, or null when both round to zero. */
    @Synchronized
    fun drain(): Pair<Int, Int>? {
        val ix = x.roundToInt().coerceIn(-limit, limit)
        val iy = y.roundToInt().coerceIn(-limit, limit)
        if (ix == 0 && iy == 0) return null
        x -= ix
        y -= iy
        return ix to iy
    }

    @Synchronized
    fun clear() {
        x = 0f
        y = 0f
    }
}
