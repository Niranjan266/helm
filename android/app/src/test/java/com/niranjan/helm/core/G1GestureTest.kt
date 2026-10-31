package com.niranjan.helm.core

import kotlin.math.abs
import kotlin.math.hypot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** G1 touchpad maths: three-finger swipes, momentum scrolling, precision gain. */
class G1GestureTest {
    private val eps = 1e-3f

    @Test
    fun swipeNeedsThresholdAndDominantAxis() {
        assertNull(ThreeFingerSwipe.classify(20f, 0f, 48f))
        assertEquals(SwipeDirection.UP, ThreeFingerSwipe.classify(5f, -60f, 48f))
        assertEquals(SwipeDirection.DOWN, ThreeFingerSwipe.classify(-5f, 60f, 48f))
        assertEquals(SwipeDirection.LEFT, ThreeFingerSwipe.classify(-60f, 10f, 48f))
        assertEquals(SwipeDirection.RIGHT, ThreeFingerSwipe.classify(60f, -10f, 48f))
        // Diagonal: ambiguous, wait for more travel.
        assertNull(ThreeFingerSwipe.classify(50f, 50f, 48f))
        assertNull(ThreeFingerSwipe.classify(Float.NaN, 100f, 48f))
    }

    @Test
    fun swipeFramesMapToWindowsShortcuts() {
        fun keys(d: SwipeDirection, m: SideSwipeMode) = (ThreeFingerSwipe.frames(d, m).single() as InputFrame.Shortcut).keys
        assertEquals(listOf(Vk.LWIN, Vk.TAB), keys(SwipeDirection.UP, SideSwipeMode.APPS))
        assertEquals(listOf(Vk.LWIN, 0x44), keys(SwipeDirection.DOWN, SideSwipeMode.DESKTOPS))
        assertEquals(listOf(Vk.ALT, Vk.TAB), keys(SwipeDirection.RIGHT, SideSwipeMode.APPS))
        assertEquals(listOf(Vk.ALT, Vk.SHIFT, Vk.TAB), keys(SwipeDirection.LEFT, SideSwipeMode.APPS))
        assertEquals(listOf(Vk.LWIN, Vk.CONTROL, Vk.LEFT), keys(SwipeDirection.LEFT, SideSwipeMode.DESKTOPS))
        assertEquals(listOf(Vk.LWIN, Vk.CONTROL, Vk.RIGHT), keys(SwipeDirection.RIGHT, SideSwipeMode.DESKTOPS))
        // Wire shape is the existing §9.1 `sc` frame.
        val json = ThreeFingerSwipe.frames(SwipeDirection.UP, SideSwipeMode.APPS).single().toJson().toString()
        assertEquals("""{"type":"in","k":"sc","keys":[91,9]}""", json)
    }

    @Test
    fun velocityTrackerAveragesRecentWindow() {
        val t = ScrollVelocityTracker(windowMs = 80)
        // 0.1 notches every 10 ms = 0.01 notches/ms downwards (dy > 0).
        for (i in 0..8) t.add(i * 10L, 0f, 0.1f)
        val (vx, vy) = t.velocity(80)
        assertEquals(0f, vx, eps)
        assertEquals(0.01f, vy, 1e-4f)
        // Fingers paused before lifting: no fling.
        assertEquals(0f, t.velocity(300).second, eps)
        t.reset()
        assertEquals(0f, t.velocity(80).second, eps)
    }

    @Test
    fun momentumDecaysAndTravelsTauTimesVelocity() {
        val m = MomentumScroller(tauMs = 300f, minVelocity = 0.001f, maxVelocity = 1f)
        assertTrue(m.start(0f, 0.05f))
        var total = 0f
        var last = Float.MAX_VALUE
        var steps = 0
        while (true) {
            val d = m.step(16f) ?: break
            assertTrue("each step smaller than the last", d.second <= last + 1e-6f)
            last = d.second
            total += d.second
            steps++
            assertTrue(steps < 10_000)
        }
        assertFalse(m.active)
        // Integral of v0·e^(-t/tau) is v0·tau = 15 notches; it stops once below minVelocity.
        assertTrue("total=$total", total in 14f..15.01f)
    }

    @Test
    fun momentumStepIsFrameRateIndependent() {
        val a = MomentumScroller(tauMs = 300f, minVelocity = 1e-6f)
        val b = MomentumScroller(tauMs = 300f, minVelocity = 1e-6f)
        a.start(0.03f, 0f)
        b.start(0.03f, 0f)
        var sa = 0f
        var sb = 0f
        repeat(30) { sa += a.step(8f)!!.first }
        repeat(10) { sb += b.step(24f)!!.first }
        assertEquals(sa, sb, 1e-3f)
    }

    @Test
    fun momentumIgnoresSlowFlicksCapsFastOnesAndStops() {
        val m = MomentumScroller(minVelocity = 0.0015f, maxVelocity = 0.1f)
        assertFalse(m.start(0.001f, 0f))
        assertFalse(m.active)
        assertFalse(m.start(Float.NaN, 0f))
        assertTrue(m.start(3f, 4f))
        val (rx, ry) = m.remaining()
        assertEquals(0.1f * 325f, hypot(rx, ry), 0.01f)
        assertEquals(0.75f, abs(rx / ry), eps)
        m.stop()
        assertNull(m.step(16f))
    }

    @Test
    fun precisionIsLinearAndUnaccelerated() {
        val (sx, _) = PointerGain.apply(10f, 0f, 1f, 1f, precision = true)
        val (fx, _) = PointerGain.apply(10f, 0f, 100f, 1f, precision = true)
        assertEquals(10f * PointerGain.PRECISION_GAIN, sx, eps)
        assertEquals(sx, fx, eps)
        // Normal mode still accelerates fast motion and matches TouchpadMath exactly.
        val normal = PointerGain.apply(10f, 0f, 1f, 1f, precision = false)
        assertEquals(TouchpadMath.accelerate(10f, 0f, 1f, 1f), normal)
        assertTrue(normal.first > sx)
        assertEquals(0f to 0f, PointerGain.apply(Float.NaN, 1f, 1f, 1f, precision = true))
    }
}
