package com.niranjan.helm.core

/** Pure helpers for the landscape full-screen Screen tab. */
object StreamFullscreen {
    /** The Screen tab goes full screen in landscape unless the user left it for this stint. */
    fun active(landscape: Boolean, exited: Boolean): Boolean = landscape && !exited

    /**
     * Bounding box to request in `stream.start`: the phone's physical landscape resolution
     * ([phoneW] x [phoneH], either order) capped by the preset ([presetW] x [presetH]). When the
     * PC picture size ([pcW] x [pcH]) is known the box shrinks to its aspect ratio (never
     * upscaled past the PC picture). Sizes are even and at least 64.
     */
    fun streamBox(phoneW: Int, phoneH: Int, presetW: Int, presetH: Int, pcW: Int? = null, pcH: Int? = null): Pair<Int, Int> {
        val long = maxOf(phoneW, phoneH)
        val short = minOf(phoneW, phoneH)
        val boxW = minOf(long, presetW).toFloat()
        val boxH = minOf(short, presetH).toFloat()
        var w = boxW
        var h = boxH
        if (pcW != null && pcH != null && pcW > 0 && pcH > 0) {
            val k = minOf(boxW / pcW, boxH / pcH, 1f)
            w = pcW * k
            h = pcH * k
        }
        return (w.toInt().coerceAtLeast(64) and 1.inv()) to (h.toInt().coerceAtLeast(64) and 1.inv())
    }
}
