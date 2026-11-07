package com.niranjan.helm.core

/*
 * Pure helpers for Screen-tab input: scroll tuning (speed + direction) shared by swipe,
 * two-finger and fling scrolling, and the one-tap quick keys. No Android types.
 */

/** Scroll speed and direction from Settings, applied to every Screen-tab scroll. */
object ScrollTuning {
    const val MIN_SPEED = 0.25f
    const val MAX_SPEED = 3f
    const val DEFAULT_SPEED = 1f

    fun clampSpeed(speed: Float): Float =
        if (!speed.isFinite()) DEFAULT_SPEED else speed.coerceIn(MIN_SPEED, MAX_SPEED)

    /**
     * Wheel notches for a finger movement of ([dxPx], [dyPx]) with [notchPx] per notch.
     * Natural scrolling by default (finger down = content down = wheel up, which the wire
     * expresses as positive dy); [reverse] flips both axes.
     */
    fun notches(dxPx: Float, dyPx: Float, notchPx: Float, speed: Float, reverse: Boolean): Pair<Float, Float> {
        if (!dxPx.isFinite() || !dyPx.isFinite() || !(notchPx > 0f)) return 0f to 0f
        val k = clampSpeed(speed) / notchPx * (if (reverse) -1f else 1f)
        return -dxPx * k to dyPx * k
    }
}

/** A one-tap key combo on the Screen tab. [confirm] = ask before sending (destructive). */
enum class QuickKey(val label: String, val keys: List<Int>, val confirm: Boolean = false) {
    COPY("Ctrl+C", listOf(Vk.CONTROL, 0x43)),
    PASTE("Ctrl+V", listOf(Vk.CONTROL, 0x56)),
    UNDO("Ctrl+Z", listOf(Vk.CONTROL, 0x5A)),
    ALT_TAB("Alt+Tab", listOf(Vk.ALT, Vk.TAB)),
    WIN("Win", listOf(Vk.LWIN)),
    ESC("Esc", listOf(Vk.ESCAPE)),
    CLOSE("Close window", listOf(Vk.ALT, 0x73), confirm = true),
    ;

    /** Frames to send, via the same path as the on-screen keyboard ([KeyPress]). */
    fun frames(): List<InputFrame> = KeyPress.frames(keys.last(), keys.dropLast(1))
}

/**
 * Fling for Screen-tab scrolling: feed it the (already tuned) notches sent while the finger
 * scrolls, call [release] when it lifts, then [step] once per frame until it returns null.
 * [touch] stops it (any new touch on the screen). Speed and direction carry over because the
 * samples are tuned notches.
 */
class ScrollFling(
    private val tracker: ScrollVelocityTracker = ScrollVelocityTracker(),
    private val scroller: MomentumScroller = MomentumScroller(),
) {
    val active: Boolean get() = scroller.active

    fun touch() {
        scroller.stop()
        tracker.reset()
    }

    fun sample(tMs: Long, dx: Float, dy: Float) = tracker.add(tMs, dx, dy)

    /** Starts the fling from the velocity at [tMs]; false if the flick was too slow. */
    fun release(tMs: Long): Boolean {
        val (vx, vy) = tracker.velocity(tMs)
        tracker.reset()
        return scroller.start(vx, vy)
    }

    /** Notches to scroll for [dtMs], or null once the fling has died away. */
    fun step(dtMs: Float): Pair<Float, Float>? = scroller.step(dtMs)
}
