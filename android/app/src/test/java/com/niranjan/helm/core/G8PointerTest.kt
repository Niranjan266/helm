package com.niranjan.helm.core

import kotlin.math.abs
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** g8 pointer maths: dp deltas, curve, tremor filter, historical samples, sub-pixel carry. */
class G8PointerTest {
    private val ppd = PointerCurve.PX_PER_DP

    /** Drives a pipeline along a straight horizontal line, [stepPx] per [dtMs], [n] steps. */
    private fun run(
        p: PointerPipeline,
        density: Float,
        startT: Long,
        stepDp: Float,
        dtMs: Long,
        n: Int,
        x0Dp: Float = 0f,
        speed: Float = 1f,
        precision: Boolean = false,
        perEvent: Int = 1,
    ): Float {
        var sum = 0f
        var t = startT
        var xDp = x0Dp
        var i = 0
        while (i < n) {
            val batch = ArrayList<PointerSample>()
            repeat(perEvent) {
                if (i < n) {
                    xDp += stepDp
                    t += dtMs
                    batch += PointerSample(xDp * density, 0f, t)
                    i++
                }
            }
            sum += p.feed(batch, speed, precision)?.first ?: 0f
        }
        return sum
    }

    @Test
    fun theSamePhysicalSwipeMovesTheSameOnEveryDensity() {
        val outs = listOf(1.5f, 2.625f, 3.5f).map { d ->
            val p = PointerPipeline(d)
            p.begin(0f, 0f, 0)
            run(p, d, 0, stepDp = 0.5f, dtMs = 16, n = 100) // 50 dp at 31 dp/s: slow, gain 1
        }
        assertEquals(50f * ppd, outs[0], 1.0f)
        for (o in outs) assertEquals(outs[0], o, 1.0f)
    }

    @Test
    fun slowMotionIsExactlyLinear() {
        assertEquals(1f, PointerCurve.gain(0f), 0f)
        assertEquals(1f, PointerCurve.gain(PointerCurve.LINEAR_UP_TO), 0f)
        // Anything inside the linear zone: output = dp * PX_PER_DP * speed, nothing else.
        val d = 2.75f
        val p = PointerPipeline(d)
        p.begin(0f, 0f, 0)
        val out = run(p, d, 0, stepDp = 0.4f, dtMs = 16, n = 200, speed = 1.5f) // 25 dp/s
        assertEquals(80f * ppd * 1.5f, out, (0.3f * ppd + 0.2f) * 1.5f)
    }

    @Test
    fun theCurveIsGentleSmoothAndMonotonic() {
        var prev = 1f
        var v = 0f
        while (v <= 4f) {
            val g = PointerCurve.gain(v)
            assertTrue("gain($v)=$g not monotonic", g >= prev - 1e-6f)
            assertTrue(g in 1f..(PointerCurve.MAX_GAIN + 1e-5f))
            // No jumps: a tiny speed change never changes the gain by more than a hair.
            assertTrue(abs(g - prev) < 0.02f)
            prev = g
            v += 0.01f
        }
        assertEquals(PointerCurve.MAX_GAIN, PointerCurve.gain(PointerCurve.FULL_AT), 1e-5f)
        assertEquals(PointerCurve.MAX_GAIN, PointerCurve.gain(50f), 1e-5f)
        assertTrue("gentler than the old 3x cap", PointerCurve.MAX_GAIN < 3f)
        assertEquals(1f, PointerCurve.gain(Float.NaN), 0f)
    }

    @Test
    fun fastFlicksAreAmplifiedButCapped() {
        val d = 2.75f
        val slow = PointerPipeline(d).also { it.begin(0f, 0f, 0) }
        val fast = PointerPipeline(d).also { it.begin(0f, 0f, 0) }
        val s = run(slow, d, 0, stepDp = 0.5f, dtMs = 16, n = 120) // 60 dp slowly
        val f = run(fast, d, 0, stepDp = 30f, dtMs = 16, n = 2)    // 60 dp at ~1.9 dp/ms
        assertTrue("fast $f > slow $s", f > s * 1.5f)
        assertTrue(f <= 60f * ppd * PointerCurve.MAX_GAIN + 0.01f)
    }

    @Test
    fun tremorThatReversesNeverMovesThePointer() {
        val d = 2.75f
        val p = PointerPipeline(d)
        p.begin(0f, 0f, 0)
        var t = 0L
        var x = 0f
        // Leave the tap slop first.
        run(p, d, 0, stepDp = 2f, dtMs = 20, n = 5).also { x = 10f * d; t = 100 }
        var sum = 0f
        repeat(200) { i ->
            x += (if (i % 2 == 0) 0.12f else -0.12f) * d
            t += 8
            sum += p.feed(listOf(PointerSample(x, 0f, t)), 1f, false)?.first ?: 0f
        }
        assertEquals(0f, sum, 0.4f)
    }

    @Test
    fun slowDeliberateMotionIsDelayedByLessThanAPixelButNeverLost() {
        val d = 2.75f
        val p = PointerPipeline(d)
        p.begin(0f, 0f, 0)
        val lead = run(p, d, 0, stepDp = 2f, dtMs = 20, n = 5) // 10 dp: out of the slop
        // 0.04 dp per sample is below the tremor radius on its own; 150 samples = 6 dp.
        val slow = run(p, d, 100, stepDp = 0.04f, dtMs = 16, n = 150, x0Dp = 10f)
        val expected = 6f * ppd
        assertTrue("slow=$slow expected=$expected", slow <= expected + 0.1f)
        assertTrue("lost more than the tremor radius: $slow vs $expected", slow >= expected - 0.3f * ppd - 0.1f)
        assertEquals(10f * ppd, lead, 0.2f) // the slop hold released all of it
    }

    @Test
    fun movementInsideTheTapSlopIsHeldAndReleasedInOnePiece() {
        val d = 2f
        val p = PointerPipeline(d, slopDp = 8f)
        p.begin(0f, 0f, 0)
        assertNull(p.feed(listOf(PointerSample(6f, 0f, 100)), 1f, false)) // 3 dp
        assertNull(p.feed(listOf(PointerSample(12f, 0f, 200)), 1f, false)) // 6 dp
        val out = p.feed(listOf(PointerSample(20f, 0f, 300)), 1f, false) // 10 dp: crosses
        assertNotNull(out)
        assertEquals(10f * ppd, out!!.first, 0.3f)
    }

    @Test
    fun everyHistoricalSampleIsConsumed() {
        val d = 2.75f
        // The same path delivered one sample per event or four per event ends at the same place.
        fun total(perEvent: Int): Float {
            val p = PointerPipeline(d)
            p.begin(0f, 0f, 0)
            return run(p, d, 0, stepDp = 0.5f, dtMs = 4, n = 240, perEvent = perEvent)
        }
        val one = total(1)
        val four = total(4)
        assertEquals(one, four, 0.3f)
        assertEquals(120f * ppd, four, 0.8f)
        // And a vertical/diagonal path keeps both axes.
        val p = PointerPipeline(d)
        p.begin(0f, 0f, 0)
        val out = p.feed(
            listOf(
                PointerSample(10f, 30f, 4),
                PointerSample(20f, 60f, 8),
                PointerSample(30f, 90f, 12),
            ),
            1f,
            false,
        )!!
        assertEquals(3f, out.second / out.first, 0.05f)
    }

    @Test
    fun precisionModeIsLinearAtTheFixedGain() {
        val d = 2.75f
        val p = PointerPipeline(d)
        p.begin(0f, 0f, 0)
        val out = run(p, d, 0, stepDp = 30f, dtMs = 16, n = 2, precision = true) // fast, but no boost
        assertEquals(60f * ppd * PointerGain.PRECISION_GAIN, out, 0.5f)
    }

    @Test
    fun garbageIsIgnored() {
        val p = PointerPipeline(2f)
        // Not started yet.
        assertNull(p.feed(listOf(PointerSample(5f, 5f, 1)), 1f, false))
        p.begin(0f, 0f, 0)
        assertNull(p.feed(listOf(PointerSample(Float.NaN, 5f, 1)), 1f, false))
        assertNull(p.feed(listOf(PointerSample(50f, 5f, 2)), 0f, false))
        assertNull(PointerPipeline(0f).also { it.begin(0f, 0f, 0) }.feed(emptyList(), 1f, false))
        assertNull(TremorFilter().filter(Float.NaN, 0f))
    }

    @Test
    fun theBatcherCarriesTheSubPixelRemainder() {
        val b = MotionBatcher()
        var sent = 0f
        var added = 0f
        repeat(1000) {
            b.add(0.0037f, -0.0013f)
            added += 0.0037f
            b.drain()?.let { sent += it.dx }
        }
        b.drain()?.let { sent += it.dx }
        assertEquals(added, sent, 0.011f)
        // Rounded to the wire's two decimals, never drifting.
        val m = MotionBatcher().also { it.add(1.2349f, 0f) }.drain()!!
        assertEquals(1.23f, m.dx, 1e-6f)
    }
}
