package com.niranjan.helm.core

/**
 * What the touch surface, the floating keyboard and the type box need from "the other side":
 * the Wi-Fi PC connection (`RemoteViewModel`) or the Bluetooth HID bridge ([BtInputBridge]).
 * Keeping it this small lets both modes share the same gesture and pointer code.
 */
interface TouchPadSink {
    /** Pointer motion in PC pixels, batched until [flushInput]. */
    fun touchMove(dx: Float, dy: Float)

    /** Scroll in wheel notches (`dy > 0` = up, `dx > 0` = right), batched until [flushInput]. */
    fun touchScroll(dx: Float, dy: Float)

    /** Hands out whatever motion and scroll is pending. */
    fun flushInput()

    fun sendAll(frames: List<InputFrame>)

    fun click(button: MouseButton)

    /** Pinch-zoom is Ctrl held while scrolling. */
    fun pinch(start: Boolean)
}
