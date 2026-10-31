package com.niranjan.helm.core

import kotlin.math.hypot
import kotlin.math.min

/** Pure touchpad maths (no Android types), so it can be unit tested. */
object TouchpadMath {
    /**
     * Pointer acceleration: slow finger motion stays close to 1:1 for precision, fast flicks
     * are amplified (up to 3×), then everything is scaled by the user's [speed].
     * [dtMs] is the time since the previous sample.
     */
    fun accelerate(dxPx: Float, dyPx: Float, dtMs: Float, speed: Float): Pair<Float, Float> {
        if (!dxPx.isFinite() || !dyPx.isFinite() || speed <= 0f) return 0f to 0f
        val dist = hypot(dxPx, dyPx)
        if (dist == 0f) return 0f to 0f
        val v = dist / dtMs.coerceAtLeast(1f) // px per ms
        val gain = min(3f, 1f + 0.8f * (v - 0.3f).coerceAtLeast(0f))
        return dxPx * gain * speed to dyPx * gain * speed
    }
}

/**
 * Collects pointer deltas between frames and hands out one combined `mm` per frame
 * (about 60 Hz), so a fast swipe does not flood the socket.
 */
class MotionBatcher {
    private var dx = 0f
    private var dy = 0f

    @Synchronized
    fun add(x: Float, y: Float) {
        if (!x.isFinite() || !y.isFinite()) return
        dx += x
        dy += y
    }

    /** The pending move, or null if nothing moved enough to matter. */
    @Synchronized
    fun drain(): InputFrame.Move? {
        if (kotlin.math.abs(dx) < 0.01f && kotlin.math.abs(dy) < 0.01f) return null
        val m = InputFrame.Move(dx, dy)
        dx = 0f
        dy = 0f
        return m
    }
}

/** Same, for scroll notches (`ms`). */
class ScrollBatcher {
    private var dx = 0f
    private var dy = 0f

    @Synchronized
    fun add(x: Float, y: Float) {
        if (!x.isFinite() || !y.isFinite()) return
        dx += x
        dy += y
    }

    @Synchronized
    fun drain(): InputFrame.Scroll? {
        if (kotlin.math.abs(dx) < 0.01f && kotlin.math.abs(dy) < 0.01f) return null
        val s = InputFrame.Scroll(dx, dy)
        dx = 0f
        dy = 0f
        return s
    }
}

/**
 * Turns edits of the phone's text box into input frames: appended text -> `tx`, deleted
 * characters -> Backspace presses. A replaced tail is deleted, then the new tail is typed.
 */
object TextDiff {
    fun frames(old: String, new: String): List<InputFrame> {
        if (old == new) return emptyList()
        var common = 0
        val max = min(old.length, new.length)
        while (common < max && old[common] == new[common]) common++
        // Don't split a surrogate pair.
        if (common > 0 && Character.isHighSurrogate(old[common - 1]) && common < old.length) common--
        val out = mutableListOf<InputFrame>()
        val deleted = old.codePointCount(common, old.length)
        repeat(deleted) {
            out += InputFrame.KeyDown(Vk.BACK)
            out += InputFrame.KeyUp(Vk.BACK)
        }
        val added = new.substring(common)
        if (added.isNotEmpty()) out += InputFrame.Text(added)
        return out
    }
}

/** Frames for one key press, wrapped with any held modifiers as a shortcut. */
object KeyPress {
    fun frames(vk: Int, modifiers: List<Int>): List<InputFrame> =
        if (modifiers.isEmpty()) listOf(InputFrame.KeyDown(vk), InputFrame.KeyUp(vk))
        else listOf(InputFrame.Shortcut(modifiers + vk))
}
