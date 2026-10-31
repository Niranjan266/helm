package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Direct-touch coordinate mapping with letterboxing, zoom and pan. */
class ViewTransformTest {
    private val eps = 1e-3f

    // Phone view 1000x1000 showing a 1920x1080 stream: fitted to 1000x562.5, bars top/bottom.
    private val t = ViewTransform(1000f, 1000f, 1920f, 1080f)

    private fun assertPoint(ex: Float, ey: Float, p: Pair<Float, Float>?) {
        assertNotNull(p)
        assertEquals(ex, p!!.first, eps)
        assertEquals(ey, p.second, eps)
    }

    @Test
    fun fitsAndCentres() {
        val r = t.contentRect
        assertEquals(0f, r.left, eps)
        assertEquals(1000f, r.width, eps)
        assertEquals(562.5f, r.height, eps)
        assertEquals(218.75f, r.top, eps)
        assertEquals(1000f / 1920f, t.scale, 1e-6f)
    }

    @Test
    fun mapsCentreCornersAndBars() {
        assertPoint(0.5f, 0.5f, t.toNormalized(500f, 500f))
        assertPoint(0f, 0f, t.toNormalized(0f, 218.75f))
        assertPoint(1f, 1f, t.toNormalized(1000f, 781.25f))
        assertNull("top letterbox bar", t.toNormalized(500f, 100f))
        assertNull("bottom letterbox bar", t.toNormalized(500f, 900f))
        assertPoint(0.5f, 0f, t.toNormalizedClamped(500f, 10f))
    }

    @Test
    fun zoomKeepsFocusPointFixed() {
        val focus = 250f to 480f // chosen so the result needs no pan clamping
        val before = t.toNormalized(focus.first, focus.second)!!
        val z = t.zoomBy(2f, focus.first, focus.second)
        assertEquals(2f, z.zoom, eps)
        assertPoint(before.first, before.second, z.toNormalized(focus.first, focus.second))
    }

    @Test
    fun zoomAtCentreThenTapMapsCorrectly() {
        val z = t.zoomBy(2f, 500f, 500f)
        // 2x: image is 2000x1125 centred; view centre is still the image centre.
        assertPoint(0.5f, 0.5f, z.toNormalized(500f, 500f))
        // The view's left edge now shows x = 0.25 of the monitor.
        assertPoint(0.25f, 0.5f, z.toNormalized(0f, 500f))
        // Height 1125 > 1000, so no bars any more: the view's top edge is inside the image.
        assertNotNull(z.toNormalized(500f, 0f))
    }

    @Test
    fun panIsClampedToTheImage() {
        val z = t.zoomBy(2f, 500f, 500f).panBy(10_000f, 10_000f)
        val r = z.contentRect
        assertEquals("left edge stops at the view edge", 0f, r.left, eps)
        assertEquals("top edge stops at the view edge", 0f, r.top, eps)
        assertPoint(0f, 0f, z.toNormalized(0f, 0f))
        val other = z.panBy(-50_000f, -50_000f).contentRect
        assertEquals(1000f, other.right, eps)
        assertEquals(1000f, other.bottom, eps)
        // At zoom 1 there is nothing to pan.
        assertEquals(t, t.panBy(100f, 100f))
    }

    @Test
    fun zoomIsClamped() {
        assertEquals(1f, t.zoomBy(0.2f, 500f, 500f).zoom, eps)
        assertEquals(5f, t.zoomBy(100f, 500f, 500f).zoom, eps)
        assertEquals(t, t.zoomBy(Float.NaN, 0f, 0f))
    }

    @Test
    fun toViewIsTheInverseOfToNormalized() {
        val z = t.zoomBy(3f, 300f, 600f).panBy(-40f, 25f)
        val n = z.toNormalized(420f, 515f)!!
        val (vx, vy) = z.toView(n.first * 1920f, n.second * 1080f)
        assertEquals(420f, vx, 0.01f)
        assertEquals(515f, vy, 0.01f)
    }

    @Test
    fun ensureVisiblePansTowardsThePoint() {
        val z = t.zoomBy(4f, 500f, 500f)
        val (x, y) = z.toView(10f, 10f) // top-left corner of the monitor, far off-screen
        assertTrue(x < 0f && y < 0f)
        val moved = z.ensureVisible(x, y, 48f)
        val (x2, y2) = moved.toView(10f, 10f)
        assertTrue(x2 >= 0f && y2 >= 0f)
        assertEquals(z, z.ensureVisible(500f, 500f, 48f))
    }

    @Test
    fun portraitViewAndResize() {
        val p = ViewTransform(400f, 800f, 1920f, 1080f)
        assertPoint(0.5f, 0.5f, p.toNormalized(200f, 400f))
        assertNull(p.toNormalized(200f, 10f))
        val rotated = p.zoomBy(2f, 200f, 400f).withView(800f, 400f)
        assertEquals(2f, rotated.zoom, eps)
        assertTrue(rotated.contentRect.left <= 0f)
        assertFalse(ViewTransform(0f, 0f, 0f, 0f).isValid)
        assertNull(ViewTransform(0f, 0f, 0f, 0f).toNormalized(1f, 1f))
    }
}
