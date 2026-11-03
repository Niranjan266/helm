package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Full-screen Fit / Fill: direct touch and drag must still hit the exact PC pixel. */
class ViewTransformFillTest {
    private val eps = 1e-2f

    // 2400x1080 phone landscape view, 1920x1200 (16:10) PC stream.
    private val fit = ViewTransform(2400f, 1080f, 1920f, 1200f)
    private val fill = fit.withFill(true)

    /** Stream pixel -> view -> normalised -> stream pixel must round-trip. */
    private fun assertRoundTrip(t: ViewTransform, sx: Float, sy: Float) {
        val (vx, vy) = t.toView(sx, sy)
        val n = t.toNormalized(vx, vy)
        assertNotNull("($sx,$sy) visible", n)
        assertEquals(sx, n!!.first * t.contentWidth, eps)
        assertEquals(sy, n.second * t.contentHeight, eps)
        val (bx, by) = t.toStream(vx, vy)
        assertEquals(sx, bx, eps)
        assertEquals(sy, by, eps)
    }

    private fun assertPoint(ex: Float, ey: Float, p: Pair<Float, Float>?) {
        assertNotNull(p)
        assertEquals(ex, p!!.first, 1e-3f)
        assertEquals(ey, p.second, 1e-3f)
    }

    @Test
    fun fitLetterboxesSides() {
        val r = fit.contentRect
        assertEquals(1728f, r.width, eps)
        assertEquals(1080f, r.height, eps)
        assertEquals(336f, r.left, eps)
        assertNull(fit.toNormalized(100f, 500f))
        assertRoundTrip(fit, 960f, 600f)
        assertRoundTrip(fit, 0f, 0f)
        assertRoundTrip(fit, 1920f, 1200f)
    }

    @Test
    fun fillCoversViewAndCropsTopBottom() {
        val r = fill.contentRect
        assertEquals(2400f, r.width, eps)
        assertEquals(1500f, r.height, eps)
        assertEquals(-210f, r.top, eps)
        assertPoint(0.5f, 0.5f, fill.toNormalized(1200f, 540f))
        assertNotNull(fill.toNormalized(0f, 0f))
        assertNotNull(fill.toNormalized(2400f, 1080f))
        assertTrue(fill.canPan)
        assertRoundTrip(fill, 960f, 600f)
        assertRoundTrip(fill, 100f, 300f)
    }

    @Test
    fun fillPanReachesEdgesAndStaysExact() {
        val down = fill.panBy(0f, 10_000f)
        assertEquals(210f, down.panY, eps)
        assertPoint(0f, 0f, down.toNormalized(0f, 0f))
        assertEquals(0f, down.panX, eps)
        val up = fill.panBy(0f, -10_000f)
        assertPoint(1f, 1f, up.toNormalized(2400f, 1080f))
        assertRoundTrip(down, 1700f, 90f)
        assertRoundTrip(up, 30f, 1190f)
    }

    @Test
    fun zoomAndPanInFitAndFillStayExact() {
        for (base in listOf(fit, fill)) {
            val z = base.zoomBy(2.5f, 1500f, 700f).panBy(-123f, 77f)
            val vx = 1000f
            val vy = 500f
            val (sx, sy) = z.toStream(vx, vy)
            val (bx, by) = z.toView(sx, sy)
            assertEquals(vx, bx, eps)
            assertEquals(vy, by, eps)
            val n = z.toNormalized(vx, vy)!!
            assertEquals(sx / z.contentWidth, n.first, 1e-5f)
            assertEquals(sy / z.contentHeight, n.second, 1e-5f)
            // A drag past the view edge clamps to the PC edge.
            assertPoint(1f, 1f, z.toNormalizedClamped(1e6f, 1e6f))
            assertPoint(0f, 0f, z.toNormalizedClamped(-1e6f, -1e6f))
        }
    }

    @Test
    fun zoomKeepsPointUnderFingerInFill() {
        val before = fill.toStream(1800f, 300f)
        val after = fill.zoomBy(3f, 1800f, 300f).toStream(1800f, 300f)
        assertEquals(before.first, after.first, eps)
        assertEquals(before.second, after.second, eps)
    }

    @Test
    fun fillUsefulOnlyWhenAspectDiffers() {
        assertTrue(fit.fillUseful)
        assertFalse(ViewTransform(1920f, 1080f, 1920f, 1080f).fillUseful)
        assertFalse(ViewTransform(1930f, 1080f, 1920f, 1080f).fillUseful)
        assertFalse(ViewTransform(0f, 0f, 1920f, 1080f).fillUseful)
    }

    @Test
    fun withFillResetsZoomAndPan() {
        val f = fit.zoomBy(2f, 1200f, 540f).panBy(50f, 0f).withFill(true)
        assertEquals(1f, f.zoom, 0f)
        assertEquals(0f, f.panX, 0f)
        assertTrue(f.fill)
        assertTrue(f === f.withFill(true))
        assertFalse(fit.canPan)
    }

    @Test
    fun rotationChangesViewKeepsContent() {
        val portrait = fit.withView(1080f, 2400f)
        assertPoint(0.5f, 0.5f, portrait.toNormalized(540f, 1200f))
        assertNull(portrait.toNormalized(540f, 100f))
        assertRoundTrip(portrait, 1000f, 500f)
    }
}
