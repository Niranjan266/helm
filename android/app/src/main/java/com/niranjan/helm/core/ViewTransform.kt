package com.niranjan.helm.core

import kotlin.math.abs

/** A rectangle in view pixels. */
data class ViewRect(val left: Float, val top: Float, val width: Float, val height: Float) {
    val right: Float get() = left + width
    val bottom: Float get() = top + height
}

/**
 * Where the remote screen is drawn inside the phone's view, with pinch-zoom and pan.
 *
 * At zoom 1 the stream is fitted (letterboxed) and centred. [panX]/[panY] move the zoomed image
 * away from the centre and are clamped so the image always covers the view on the axes where
 * it is larger, and stays centred on the axes where it is smaller. Immutable: every change
 * returns a new value. Pure maths, unit tested.
 */
data class ViewTransform(
    val viewWidth: Float,
    val viewHeight: Float,
    val contentWidth: Float,
    val contentHeight: Float,
    val zoom: Float = 1f,
    val panX: Float = 0f,
    val panY: Float = 0f,
    val maxZoom: Float = 5f,
    /** Fill: scale so the image covers the whole view (edges cropped, pan to reach them). */
    val fill: Boolean = false,
) {
    val isValid: Boolean get() = viewWidth > 0 && viewHeight > 0 && contentWidth > 0 && contentHeight > 0

    /** View pixels per stream pixel at zoom 1: letterboxed (Fit) or covering (Fill). */
    private val fitScale: Float
        get() = when {
            !isValid -> 0f
            fill -> maxOf(viewWidth / contentWidth, viewHeight / contentHeight)
            else -> minOf(viewWidth / contentWidth, viewHeight / contentHeight)
        }

    /**
     * True when Fit and Fill actually differ: the view and stream aspect ratios differ by more
     * than [ASPECT_TOLERANCE]. Below that the bars are negligible and Fit is used as is.
     */
    val fillUseful: Boolean
        get() {
            if (!isValid) return false
            val a = viewWidth / viewHeight
            val b = contentWidth / contentHeight
            return abs(a / b - 1f) > ASPECT_TOLERANCE
        }

    /** The image is larger than the view on some axis (zoomed, or Fill), so panning does something. */
    val canPan: Boolean
        get() = isValid && (contentWidth * scale > viewWidth + 1f || contentHeight * scale > viewHeight + 1f)

    /** View pixels per stream pixel. */
    val scale: Float get() = fitScale * zoom

    /** Where the stream image is drawn (may extend past the view when zoomed). */
    val contentRect: ViewRect
        get() {
            val w = contentWidth * scale
            val h = contentHeight * scale
            return ViewRect((viewWidth - w) / 2f + panX, (viewHeight - h) / 2f + panY, w, h)
        }

    /**
     * A touch at view ([x], [y]) as normalised (0..1) coordinates on the streamed monitor, or
     * null when it falls outside the image (on the letterbox bars).
     */
    fun toNormalized(x: Float, y: Float): Pair<Float, Float>? {
        if (!isValid) return null
        val r = contentRect
        val nx = (x - r.left) / r.width
        val ny = (y - r.top) / r.height
        if (nx < -EDGE || nx > 1f + EDGE || ny < -EDGE || ny > 1f + EDGE) return null
        return nx.coerceIn(0f, 1f) to ny.coerceIn(0f, 1f)
    }

    /** Like [toNormalized] but clamps to the edge instead of returning null (for drags). */
    fun toNormalizedClamped(x: Float, y: Float): Pair<Float, Float> {
        if (!isValid) return 0.5f to 0.5f
        val r = contentRect
        return ((x - r.left) / r.width).coerceIn(0f, 1f) to ((y - r.top) / r.height).coerceIn(0f, 1f)
    }

    /** Stream pixel ([sx], [sy]) -> view pixel. */
    fun toView(sx: Float, sy: Float): Pair<Float, Float> {
        val r = contentRect
        return r.left + sx * scale to r.top + sy * scale
    }

    fun withView(width: Float, height: Float): ViewTransform = copy(viewWidth = width, viewHeight = height).clamped()

    /** New stream size (e.g. after a reconfigure): keeps zoom, re-clamps pan. */
    fun withContent(width: Float, height: Float): ViewTransform =
        copy(contentWidth = width, contentHeight = height).clamped()

    /** Zooms by [factor] keeping the image point under ([focusX], [focusY]) where it is. */
    fun zoomBy(factor: Float, focusX: Float, focusY: Float): ViewTransform {
        if (!isValid || !factor.isFinite() || factor <= 0f) return this
        val newZoom = (zoom * factor).coerceIn(1f, maxZoom)
        val r = contentRect
        val nx = (focusX - r.left) / r.width
        val ny = (focusY - r.top) / r.height
        val w = contentWidth * fitScale * newZoom
        val h = contentHeight * fitScale * newZoom
        val left = focusX - nx * w
        val top = focusY - ny * h
        return copy(zoom = newZoom, panX = left - (viewWidth - w) / 2f, panY = top - (viewHeight - h) / 2f).clamped()
    }

    fun panBy(dx: Float, dy: Float): ViewTransform =
        if (!dx.isFinite() || !dy.isFinite()) this else copy(panX = panX + dx, panY = panY + dy).clamped()

    /** Pans the least amount needed to bring view point ([x], [y]) at least [margin] px inside. */
    fun ensureVisible(x: Float, y: Float, margin: Float): ViewTransform {
        var dx = 0f
        var dy = 0f
        if (x < margin) dx = margin - x else if (x > viewWidth - margin) dx = viewWidth - margin - x
        if (y < margin) dy = margin - y else if (y > viewHeight - margin) dy = viewHeight - margin - y
        return if (dx == 0f && dy == 0f) this else panBy(dx, dy)
    }

    fun reset(): ViewTransform = copy(zoom = 1f, panX = 0f, panY = 0f)

    /** Switches Fit / Fill; zoom and pan start over, centred. */
    fun withFill(fill: Boolean): ViewTransform =
        if (fill == this.fill) this else copy(fill = fill, zoom = 1f, panX = 0f, panY = 0f)

    /** Stream pixel under view point ([x], [y]) (not clamped; for tests and hit maths). */
    fun toStream(x: Float, y: Float): Pair<Float, Float> {
        val r = contentRect
        return (x - r.left) / scale to (y - r.top) / scale
    }

    companion object {
        /** Float slack at the image edges (touches exactly on the border still count). */
        private const val EDGE = 1e-4f

        /** Aspect ratios closer than this (relative) are treated as equal. */
        const val ASPECT_TOLERANCE = 0.02f
    }

    private fun clamped(): ViewTransform {
        if (!isValid) return this
        val w = contentWidth * scale
        val h = contentHeight * scale
        val maxX = ((w - viewWidth) / 2f).coerceAtLeast(0f)
        val maxY = ((h - viewHeight) / 2f).coerceAtLeast(0f)
        val px = panX.coerceIn(-maxX, maxX)
        val py = panY.coerceIn(-maxY, maxY)
        return if (px == panX && py == panY) this else copy(panX = px, panY = py)
    }
}
