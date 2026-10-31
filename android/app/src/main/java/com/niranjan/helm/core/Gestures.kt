package com.niranjan.helm.core

import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.hypot

/*
 * Pure touchpad gesture maths for G1 (Docs/protocol/touchpad-gamepad.md): three-finger
 * swipes, inertial two-finger scrolling and precision pointer gain. No Android types.
 */

private const val VK_D = 0x44

enum class SwipeDirection { UP, DOWN, LEFT, RIGHT }

/** What a sideways three-finger swipe does. */
enum class SideSwipeMode { APPS, DESKTOPS }

object ThreeFingerSwipe {
    /**
     * Classifies accumulated centroid travel (screen px, y grows downwards). Returns null until
     * the travel passes [thresholdPx] and one axis clearly dominates (1.5× the other).
     */
    fun classify(dx: Float, dy: Float, thresholdPx: Float): SwipeDirection? {
        if (!dx.isFinite() || !dy.isFinite()) return null
        if (hypot(dx, dy) < thresholdPx) return null
        val ax = abs(dx)
        val ay = abs(dy)
        return when {
            ay >= ax * 1.5f -> if (dy < 0) SwipeDirection.UP else SwipeDirection.DOWN
            ax >= ay * 1.5f -> if (dx < 0) SwipeDirection.LEFT else SwipeDirection.RIGHT
            else -> null
        }
    }

    /**
     * Frames for a swipe: up = Task View (Win+Tab), down = show desktop (Win+D), left/right =
     * previous/next app (Alt+Shift+Tab / Alt+Tab) or virtual desktop (Win+Ctrl+←/→).
     */
    fun frames(dir: SwipeDirection, mode: SideSwipeMode): List<InputFrame> = listOf(
        InputFrame.Shortcut(
            when (dir) {
                SwipeDirection.UP -> listOf(Vk.LWIN, Vk.TAB)
                SwipeDirection.DOWN -> listOf(Vk.LWIN, VK_D)
                SwipeDirection.LEFT -> when (mode) {
                    SideSwipeMode.APPS -> listOf(Vk.ALT, Vk.SHIFT, Vk.TAB)
                    SideSwipeMode.DESKTOPS -> listOf(Vk.LWIN, Vk.CONTROL, Vk.LEFT)
                }
                SwipeDirection.RIGHT -> when (mode) {
                    SideSwipeMode.APPS -> listOf(Vk.ALT, Vk.TAB)
                    SideSwipeMode.DESKTOPS -> listOf(Vk.LWIN, Vk.CONTROL, Vk.RIGHT)
                }
            },
        ),
    )
}

/**
 * Tracks recent scroll samples (in notches) and reports the release velocity in notches/ms,
 * averaged over the last [windowMs] so one noisy event does not decide the fling.
 */
class ScrollVelocityTracker(private val windowMs: Long = 80) {
    private data class Sample(val t: Long, val dx: Float, val dy: Float)

    private val samples = ArrayDeque<Sample>()

    fun reset() = samples.clear()

    fun add(tMs: Long, dx: Float, dy: Float) {
        if (!dx.isFinite() || !dy.isFinite()) return
        samples.addLast(Sample(tMs, dx, dy))
        while (samples.size > 1 && tMs - samples.first().t > windowMs * 2) samples.removeFirst()
    }

    /** Velocity at [nowMs]; zero when the fingers paused for longer than the window. */
    fun velocity(nowMs: Long): Pair<Float, Float> {
        val recent = samples.filter { nowMs - it.t <= windowMs }
        if (recent.size < 2) return 0f to 0f
        // The first sample's motion happened before its own timestamp; count from it.
        val span = (recent.last().t - recent.first().t).coerceAtLeast(1).toFloat()
        val sx = recent.drop(1).sumOf { it.dx.toDouble() }.toFloat()
        val sy = recent.drop(1).sumOf { it.dy.toDouble() }.toFloat()
        return sx / span to sy / span
    }
}

/**
 * Inertial scrolling: starts at a release velocity (notches/ms) and decays exponentially with
 * time constant [tauMs]. [step] returns the notches to scroll for the elapsed time.
 */
class MomentumScroller(
    private val tauMs: Float = 325f,
    private val minVelocity: Float = 0.0015f,
    private val maxVelocity: Float = 0.12f,
) {
    private var vx = 0f
    private var vy = 0f

    val active: Boolean get() = vx != 0f || vy != 0f

    /** Starts a fling; returns false (and stays idle) if the flick was too slow. */
    fun start(velX: Float, velY: Float): Boolean {
        stop()
        if (!velX.isFinite() || !velY.isFinite()) return false
        val speed = hypot(velX, velY)
        if (speed < START_FACTOR * minVelocity) return false
        val scale = if (speed > maxVelocity) maxVelocity / speed else 1f
        vx = velX * scale
        vy = velY * scale
        return true
    }

    fun stop() {
        vx = 0f
        vy = 0f
    }

    /** Distance for [dtMs] (exact integral of the decaying velocity), or null when finished. */
    fun step(dtMs: Float): Pair<Float, Float>? {
        if (!active) return null
        val dt = dtMs.coerceIn(0f, 100f)
        val decay = exp(-dt / tauMs)
        val dist = tauMs * (1f - decay)
        val out = vx * dist to vy * dist
        vx *= decay
        vy *= decay
        if (hypot(vx, vy) < minVelocity) stop()
        return out
    }

    /** Total distance still to come if left alone (for tests and tuning). */
    fun remaining(): Pair<Float, Float> = vx * tauMs to vy * tauMs

    private companion object {
        /** A fling needs a clearly deliberate flick, not just a slow drag being let go. */
        const val START_FACTOR = 8f
    }
}

/** Pointer gain with an optional precision mode (low fixed gain, no acceleration). */
object PointerGain {
    const val PRECISION_GAIN = 0.35f

    fun apply(dxPx: Float, dyPx: Float, dtMs: Float, speed: Float, precision: Boolean): Pair<Float, Float> {
        if (!precision) return TouchpadMath.accelerate(dxPx, dyPx, dtMs, speed)
        if (!dxPx.isFinite() || !dyPx.isFinite() || speed <= 0f) return 0f to 0f
        return dxPx * PRECISION_GAIN * speed to dyPx * PRECISION_GAIN * speed
    }
}
