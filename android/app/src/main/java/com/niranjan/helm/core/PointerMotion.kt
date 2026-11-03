package com.niranjan.helm.core

import kotlin.math.exp
import kotlin.math.hypot

/*
 * Touchpad pointer maths for g8 (Docs/protocol/pointer.md), no Android types:
 *  - every touch delta is converted to dp first, so a swipe of the same physical length moves
 *    the PC cursor the same distance on every phone;
 *  - every historical sample of a touch event is consumed, not just the newest one;
 *  - a tremor filter holds back tiny back-and-forth jitter but releases anything that adds up,
 *    so slow deliberate motion is delayed by well under a pixel, never lost;
 *  - a gentle gain curve: 1:1 up to a slow speed, then a smooth rise to a modest maximum.
 */

/** One touch position at [tMs] (event uptime in ms), screen pixels. */
data class PointerSample(val x: Float, val y: Float, val tMs: Long)

object PointerCurve {
    /** PC pixels per dp at gain 1 and speed 1 (the feel of a typical 420 dpi phone before g8). */
    const val PX_PER_DP = 2.6f

    /** Finger speed (dp/ms) up to which the gain stays exactly 1. */
    const val LINEAR_UP_TO = 0.12f

    /** Finger speed (dp/ms) from which the gain is [MAX_GAIN]. */
    const val FULL_AT = 1.6f
    const val MAX_GAIN = 2.2f

    /** Gain for a finger speed in dp/ms: flat, then a smoothstep rise, then flat again. */
    fun gain(speedDpPerMs: Float): Float {
        if (!speedDpPerMs.isFinite() || speedDpPerMs <= LINEAR_UP_TO) return 1f
        val t = ((speedDpPerMs - LINEAR_UP_TO) / (FULL_AT - LINEAR_UP_TO)).coerceIn(0f, 1f)
        return 1f + (MAX_GAIN - 1f) * t * t * (3f - 2f * t)
    }
}

/**
 * Holds back motion until it adds up to [thresholdDp]. Jitter that reverses direction nets to
 * zero and never leaves; a steady slow drag is released in chunks of at least the threshold,
 * so at most `thresholdDp` is ever held (and it is not lost while the finger keeps moving).
 */
class TremorFilter(private val thresholdDp: Float = 0.3f) {
    private var px = 0f
    private var py = 0f

    fun reset() {
        px = 0f
        py = 0f
    }

    /** The motion to apply now, or null while it is still inside the tremor radius. */
    fun filter(dx: Float, dy: Float): Pair<Float, Float>? {
        if (!dx.isFinite() || !dy.isFinite()) return null
        px += dx
        py += dy
        if (hypot(px, py) < thresholdDp) return null
        val out = px to py
        px = 0f
        py = 0f
        return out
    }
}

/**
 * One finger's path from touch-down to a PC-pixel delta. Feed it the samples of each event in
 * order (historical ones first); sum what it returns and pass it to `MotionBatcher`.
 */
class PointerPipeline(
    density: Float,
    private val slopDp: Float = 8f,
    private val tremor: TremorFilter = TremorFilter(),
) {
    private val dpPerPx = if (density.isFinite() && density > 0f) 1f / density else 1f
    private var lastX = 0f
    private var lastY = 0f
    private var lastT = 0L
    private var started = false
    private var path = 0f
    private var heldX = 0f
    private var heldY = 0f
    private var velocity = 0f // dp/ms, smoothed
    private var haveVelocity = false

    /** A new touch went down at [x], [y]. */
    fun begin(x: Float, y: Float, tMs: Long) {
        lastX = x
        lastY = y
        lastT = tMs
        started = true
        path = 0f
        heldX = 0f
        heldY = 0f
        velocity = 0f
        haveVelocity = false
        tremor.reset()
    }

    /** The smoothed finger speed in dp/ms (for tests and tuning). */
    val speedDpPerMs: Float get() = velocity

    /**
     * Consumes [samples] (oldest first). Returns the PC-pixel delta to send, or null when
     * everything was held back (inside the tap slop or the tremor radius).
     */
    fun feed(samples: List<PointerSample>, speed: Float, precision: Boolean): Pair<Float, Float>? {
        if (!started || speed <= 0f || !speed.isFinite()) return null
        var outX = 0f
        var outY = 0f
        var any = false
        for (s in samples) {
            if (!s.x.isFinite() || !s.y.isFinite()) continue
            val dx = (s.x - lastX) * dpPerPx
            val dy = (s.y - lastY) * dpPerPx
            val dt = (s.tMs - lastT).coerceAtLeast(MIN_DT_MS).toFloat()
            lastX = s.x
            lastY = s.y
            if (s.tMs > lastT) lastT = s.tMs
            val dist = hypot(dx, dy)
            if (dist > 0f) updateVelocity(dist / dt, dt)
            path += dist
            var mx = dx
            var my = dy
            if (path <= slopDp) {
                heldX += dx
                heldY += dy
                continue
            }
            if (heldX != 0f || heldY != 0f) {
                // Leaving the tap slop: release what was held back in one piece.
                mx += heldX
                my += heldY
                heldX = 0f
                heldY = 0f
            }
            val (fx, fy) = tremor.filter(mx, my) ?: continue
            val gain = if (precision) PointerGain.PRECISION_GAIN else PointerCurve.gain(velocity)
            outX += fx * gain * speed * PointerCurve.PX_PER_DP
            outY += fy * gain * speed * PointerCurve.PX_PER_DP
            any = true
        }
        return if (any) outX to outY else null
    }

    private fun updateVelocity(instant: Float, dtMs: Float) {
        if (!haveVelocity) {
            velocity = instant
            haveVelocity = true
            return
        }
        // Time-based smoothing so the result does not depend on how events are batched.
        val alpha = 1f - exp(-dtMs / VELOCITY_TAU_MS)
        velocity += (instant - velocity) * alpha
    }

    private companion object {
        const val MIN_DT_MS = 2L
        const val VELOCITY_TAU_MS = 24f
    }
}
