package com.niranjan.helm.ui

import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.runtime.getValue
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerEvent
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.AbsMoveBatcher
import com.niranjan.helm.core.InputFrame
import com.niranjan.helm.core.MouseButton
import com.niranjan.helm.core.TouchpadMath
import com.niranjan.helm.core.ViewTransform
import kotlin.math.abs
import kotlin.math.max

private enum class Multi { NONE, SCROLL, PINCH }

/**
 * Touch handling for the live screen.
 *
 * Trackpad mode: 1 finger moves the PC cursor (relative `mm`), tap = left click, 2-finger tap =
 * right click, 2-finger drag = scroll.
 * Direct mode: tap = move there (`ma`) + left click, drag = left-button drag, long press = right
 * click; 2-finger drag pans the view when zoomed, otherwise scrolls.
 * Swipe mode (direct + [swipe]): like a phone. Tap = click there, one-finger swipe scrolls (pans
 * the view when zoomed), long press = right click.
 * Both: pinch zooms the view (not the PC), 3-finger tap toggles the controls.
 */
fun Modifier.streamGestures(
    direct: Boolean,
    swipe: Boolean = false,
    speed: Float,
    haptics: Boolean,
    remote: RemoteViewModel,
    abs: AbsMoveBatcher,
    transform: () -> ViewTransform,
    setTransform: (ViewTransform) -> Unit,
    onThreeFingerTap: () -> Unit,
): Modifier = composed {
    val density = LocalDensity.current
    val haptic = LocalHapticFeedback.current
    val currentSpeed by rememberUpdatedState(speed)
    val currentHaptics by rememberUpdatedState(haptics)
    val slop = with(density) { 8.dp.toPx() }
    val notchPx = with(density) { 36.dp.toPx() }

    fun buzz() {
        if (currentHaptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
    }

    fun absAt(p: Offset): Boolean {
        val n = transform().toNormalized(p.x, p.y) ?: return false
        abs.set(n.first, n.second)
        abs.take()?.let { remote.send(it) }
        return true
    }

    pointerInput(direct, swipe) {
        awaitEachGesture {
            val first = awaitFirstDown(requireUnconsumed = false)
            val start = first.uptimeMillis
            val downAt = first.position
            val startWall = System.currentTimeMillis()
            var last = start
            var end = start
            var maxPointers = 1
            var moved = 0f
            var mode = Multi.NONE
            var centroidTravel = 0f
            var spanTravel = 0f
            var dragging = false
            var longPressed = false
            var swiping = false
            while (true) {
                val waitLong = direct && maxPointers == 1 && !dragging && !swiping && !longPressed && moved < slop
                val event: PointerEvent? = if (waitLong) {
                    val left = LONG_PRESS_MS - (System.currentTimeMillis() - startWall)
                    if (left <= 0) null else withTimeoutOrNull(left) { awaitPointerEvent() }
                } else {
                    awaitPointerEvent()
                }
                if (event == null) {
                    // Direct mode long press: right click where the finger is.
                    longPressed = true
                    if (absAt(downAt)) {
                        buzz()
                        remote.sendAll(InputFrame.click(MouseButton.RIGHT))
                    }
                    continue
                }
                end = event.changes.maxOf { it.uptimeMillis }
                val pressed = event.changes.filter { it.pressed }
                if (pressed.isEmpty()) break
                maxPointers = max(maxPointers, pressed.size)

                if (pressed.size == 1 && (maxPointers == 1 || dragging || swiping)) {
                    val c = pressed[0]
                    val d = c.position - c.previousPosition
                    moved += d.getDistance()
                    if (direct && swipe) {
                        if (!longPressed && !swiping && moved > slop) {
                            // Put the cursor under the finger so Windows scrolls that window.
                            swiping = true
                            if (!transform().canPan) absAt(downAt)
                        }
                        if (swiping) {
                            if (transform().canPan) {
                                setTransform(transform().panBy(d.x, d.y))
                            } else {
                                // Natural scrolling: finger down = content down = wheel up.
                                remote.touchScroll(-d.x / notchPx, d.y / notchPx)
                            }
                        }
                    } else if (direct) {
                        if (!longPressed && !dragging && moved > slop) {
                            if (absAt(downAt)) {
                                remote.send(InputFrame.Button(MouseButton.LEFT, true))
                                dragging = true
                            }
                        }
                        if (dragging) {
                            val n = transform().toNormalizedClamped(c.position.x, c.position.y)
                            abs.set(n.first, n.second)
                        }
                    } else if (moved > slop) {
                        val (dx, dy) = TouchpadMath.accelerate(d.x, d.y, (c.uptimeMillis - last).toFloat(), currentSpeed)
                        remote.touchMove(dx, dy)
                    }
                    last = c.uptimeMillis
                } else if (pressed.size >= 2 && !dragging && !swiping) {
                    val a = pressed[0]
                    val b = pressed[1]
                    val centroid = (a.position + b.position) / 2f
                    val dc = centroid - (a.previousPosition + b.previousPosition) / 2f
                    val span = (a.position - b.position).getDistance()
                    val prevSpan = (a.previousPosition - b.previousPosition).getDistance()
                    centroidTravel += dc.getDistance()
                    spanTravel += span - prevSpan
                    moved += dc.getDistance() + abs(span - prevSpan)
                    if (mode == Multi.NONE && moved > slop * 1.5f && pressed.size == 2) {
                        mode = if (abs(spanTravel) > centroidTravel) Multi.PINCH else Multi.SCROLL
                    }
                    when (mode) {
                        Multi.PINCH -> if (prevSpan > 1f) {
                            setTransform(transform().zoomBy(span / prevSpan, centroid.x, centroid.y).panBy(dc.x, dc.y))
                        }
                        Multi.SCROLL -> if (direct && transform().canPan) {
                            setTransform(transform().panBy(dc.x, dc.y))
                        } else {
                            // Natural scrolling: fingers down = content down = wheel up (dy > 0).
                            remote.touchScroll(-dc.x / notchPx, dc.y / notchPx)
                        }
                        Multi.NONE -> Unit
                    }
                }
                event.changes.forEach { it.consume() }
            }

            if (dragging) {
                abs.take()?.let { remote.send(it) }
                remote.send(InputFrame.Button(MouseButton.LEFT, false))
                return@awaitEachGesture
            }
            remote.flushInput()
            val tap = moved < slop && end - start < TAP_MS && !longPressed
            if (!tap) return@awaitEachGesture
            when {
                maxPointers >= 3 -> onThreeFingerTap()
                maxPointers == 2 -> {
                    buzz()
                    if (direct) absAt(downAt)
                    remote.click(MouseButton.RIGHT)
                }
                direct -> if (absAt(downAt)) {
                    buzz()
                    remote.sendAll(InputFrame.click(MouseButton.LEFT))
                }
                else -> {
                    buzz()
                    remote.click(MouseButton.LEFT)
                }
            }
        }
    }
}

private const val LONG_PRESS_MS = 550L
private const val TAP_MS = 300L
