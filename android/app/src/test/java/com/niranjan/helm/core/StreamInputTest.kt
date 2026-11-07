package com.niranjan.helm.core

import kotlin.math.abs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Screen-tab scroll tuning, fling and quick keys. */
class StreamInputTest {
    private val eps = 1e-4f

    @Test
    fun naturalScrollByDefault() {
        // Finger down 36 px with 36 px per notch = one notch, wheel up (positive dy).
        val (dx, dy) = ScrollTuning.notches(18f, 36f, 36f, 1f, reverse = false)
        assertEquals(-0.5f, dx, eps)
        assertEquals(1f, dy, eps)
    }

    @Test
    fun speedScalesAndReverseFlips() {
        val (dx, dy) = ScrollTuning.notches(18f, 36f, 36f, 2f, reverse = true)
        assertEquals(1f, dx, eps)
        assertEquals(-2f, dy, eps)
    }

    @Test
    fun speedIsClampedAndBadInputIgnored() {
        assertEquals(ScrollTuning.MAX_SPEED, ScrollTuning.clampSpeed(10f), eps)
        assertEquals(ScrollTuning.MIN_SPEED, ScrollTuning.clampSpeed(0f), eps)
        assertEquals(ScrollTuning.DEFAULT_SPEED, ScrollTuning.clampSpeed(Float.NaN), eps)
        assertEquals(0f to 0f, ScrollTuning.notches(Float.NaN, 1f, 36f, 1f, false))
        assertEquals(0f to 0f, ScrollTuning.notches(1f, 1f, 0f, 1f, false))
    }

    /** A quick flick: 1 notch every 8 ms (0.125 notches/ms), released straight away. */
    private fun flicked(): ScrollFling {
        val f = ScrollFling()
        var t = 1000L
        repeat(8) {
            f.sample(t, 0f, 1f)
            t += 8
        }
        assertTrue(f.release(t - 8))
        return f
    }

    @Test
    fun quickFlickKeepsScrollingAndDecays() {
        val f = flicked()
        val steps = generateSequence { f.step(16f)?.second }.take(1000).toList()
        assertTrue(steps.size > 10)
        // Same direction as the finger, each frame smaller than the last, then it stops.
        assertTrue(steps.all { it > 0f })
        assertTrue(steps.zipWithNext().all { (a, b) -> b < a })
        assertFalse(f.active)
        assertNull(f.step(16f))
    }

    @Test
    fun newTouchStopsTheFling() {
        val f = flicked()
        assertTrue(f.step(16f) != null)
        f.touch()
        assertFalse(f.active)
        assertNull(f.step(16f))
    }

    @Test
    fun slowDragOrPauseDoesNotFling() {
        val slow = ScrollFling()
        repeat(8) { slow.sample(1000L + it * 16, 0f, 0.05f) }
        assertFalse(slow.release(1000L + 7 * 16))

        // Fast drag, then the finger rests for 300 ms before lifting.
        val paused = ScrollFling()
        repeat(8) { paused.sample(1000L + it * 8, 0f, 1f) }
        assertFalse(paused.release(1000L + 7 * 8 + 300))
    }

    @Test
    fun flingIsFrameRateIndependent() {
        val a = flicked()
        val b = flicked()
        var sa = 0f
        var sb = 0f
        repeat(30) { sa += a.step(16f)?.second ?: 0f }
        repeat(60) { sb += b.step(8f)?.second ?: 0f }
        assertTrue(abs(sa - sb) < 0.01f * sa)
    }

    @Test
    fun quickKeysUseShortcutFrames() {
        fun keys(k: QuickKey) = (k.frames().single() as InputFrame.Shortcut).keys
        assertEquals(listOf(Vk.CONTROL, 0x43), keys(QuickKey.COPY))
        assertEquals(listOf(Vk.CONTROL, 0x56), keys(QuickKey.PASTE))
        assertEquals(listOf(Vk.CONTROL, 0x5A), keys(QuickKey.UNDO))
        assertEquals(listOf(Vk.ALT, Vk.TAB), keys(QuickKey.ALT_TAB))
        assertEquals(listOf(Vk.ALT, Vk.f(4)), keys(QuickKey.CLOSE))
        assertTrue(QuickKey.CLOSE.confirm)
        assertEquals(listOf(QuickKey.CLOSE), QuickKey.entries.filter { it.confirm })
        // Single keys are a plain press and release.
        assertEquals(listOf(InputFrame.KeyDown(Vk.LWIN), InputFrame.KeyUp(Vk.LWIN)), QuickKey.WIN.frames())
        assertEquals(listOf(InputFrame.KeyDown(Vk.ESCAPE), InputFrame.KeyUp(Vk.ESCAPE)), QuickKey.ESC.frames())
    }
}
