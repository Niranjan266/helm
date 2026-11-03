package com.niranjan.helm.core

/*
 * Pure state behind the landscape touchpad (no Android types): which panel sits on the
 * right, the floating PC keyboard's layout, sticky modifiers and where the keyboard may be.
 */

/** What the landscape touchpad's right panel shows (chosen per PC). */
enum class PadPanel(val label: String) {
    CLICKS("Click buttons"),
    KEYS("Modifier & special keys"),
    MEDIA("Media"),
    VOLUME("Volume"),
    DECK("Deck buttons"),
    NONE("Nothing");

    companion object {
        val DEFAULT = CLICKS

        /** Unknown or missing names fall back to [DEFAULT]. */
        fun parse(name: String?): PadPanel = entries.firstOrNull { it.name == name } ?: DEFAULT
    }
}

/** Windows virtual-key codes the keyboard needs beyond [Vk]. */
object KbdVk {
    const val CAPITAL = 0x14
    const val INSERT = 0x2D
    const val RWIN = 0x5C
    const val OEM_1 = 0xBA // ;
    const val OEM_PLUS = 0xBB
    const val OEM_COMMA = 0xBC
    const val OEM_MINUS = 0xBD
    const val OEM_PERIOD = 0xBE
    const val OEM_2 = 0xBF // /
    const val OEM_3 = 0xC0 // `
    const val OEM_4 = 0xDB // [
    const val OEM_5 = 0xDC // \
    const val OEM_6 = 0xDD // ]
    const val OEM_7 = 0xDE // '
}

/** One key of the floating keyboard. [weight] is its width relative to a letter key. */
data class KbdKey(val label: String, val vk: Int, val weight: Float = 1f, val shifted: String? = null) {
    val isModifier: Boolean get() = vk in StickyMods.ORDER
}

object PcKeyboard {
    private fun letters(s: String) = s.map { KbdKey(it.toString(), it.code) } // 'A'..'Z' = VK A..Z

    private fun digit(d: Char, shifted: String) = KbdKey(d.toString(), d.code, shifted = shifted)

    val rows: List<List<KbdKey>> = listOf(
        listOf(KbdKey("Esc", Vk.ESCAPE, 1.4f)) + (1..12).map { KbdKey("F$it", Vk.f(it)) } + KbdKey("Del", Vk.DELETE, 1.4f),
        listOf(KbdKey("`", KbdVk.OEM_3, shifted = "~")) +
            listOf('1' to "!", '2' to "@", '3' to "#", '4' to "$", '5' to "%", '6' to "^", '7' to "&", '8' to "*", '9' to "(", '0' to ")")
                .map { (d, s) -> digit(d, s) } +
            listOf(KbdKey("-", KbdVk.OEM_MINUS, shifted = "_"), KbdKey("=", KbdVk.OEM_PLUS, shifted = "+"), KbdKey("Bksp", Vk.BACK, 1.8f)),
        listOf(KbdKey("Tab", Vk.TAB, 1.5f)) + letters("QWERTYUIOP") +
            listOf(KbdKey("[", KbdVk.OEM_4, shifted = "{"), KbdKey("]", KbdVk.OEM_6, shifted = "}"), KbdKey("\\", KbdVk.OEM_5, 1.3f, shifted = "|")),
        listOf(KbdKey("Caps", KbdVk.CAPITAL, 1.8f)) + letters("ASDFGHJKL") +
            listOf(KbdKey(";", KbdVk.OEM_1, shifted = ":"), KbdKey("'", KbdVk.OEM_7, shifted = "\""), KbdKey("Enter", Vk.RETURN, 2.2f)),
        listOf(KbdKey("Shift", Vk.SHIFT, 2.3f)) + letters("ZXCVBNM") +
            listOf(KbdKey(",", KbdVk.OEM_COMMA, shifted = "<"), KbdKey(".", KbdVk.OEM_PERIOD, shifted = ">"), KbdKey("/", KbdVk.OEM_2, shifted = "?"), KbdKey("Shift", Vk.SHIFT, 2.3f)),
        listOf(
            KbdKey("Ctrl", Vk.CONTROL, 1.4f), KbdKey("Win", Vk.LWIN, 1.2f), KbdKey("Alt", Vk.ALT, 1.4f),
            KbdKey("Space", Vk.SPACE, 4.2f),
            KbdKey("Alt", Vk.ALT, 1.4f), KbdKey("Ctrl", Vk.CONTROL, 1.4f),
            KbdKey("←", Vk.LEFT), KbdKey("↑", Vk.UP), KbdKey("↓", Vk.DOWN), KbdKey("→", Vk.RIGHT),
        ),
        listOf(
            KbdKey("Ins", KbdVk.INSERT), KbdKey("Home", Vk.HOME), KbdKey("End", Vk.END),
            KbdKey("PgUp", Vk.PAGE_UP), KbdKey("PgDn", Vk.PAGE_DOWN),
        ),
    )

    /** Frames for pressing [key] with the sticky modifiers [mods] held (a shortcut when any). */
    fun frames(key: KbdKey, mods: List<Int>): List<InputFrame> =
        KeyPress.frames(key.vk, mods.filter { it != key.vk })
}

/**
 * Sticky modifiers for an on-screen keyboard. Tap a modifier to hold it for the next key;
 * tap it again to release it; double-tap to lock it until tapped once more. [consume] hands
 * out what is held for a key press and clears everything that is not locked.
 */
class StickyMods(private val doubleTapMs: Long = 400) {
    private val latched = LinkedHashSet<Int>()
    private val locked = LinkedHashSet<Int>()
    private var lastVk = -1
    private var lastAt = 0L

    /** Held modifiers in the order Ctrl, Alt, Shift, Win. */
    fun active(): List<Int> = ORDER.filter { it in latched || it in locked }

    fun isLocked(vk: Int) = vk in locked

    fun isActive(vk: Int) = vk in latched || vk in locked

    fun tap(vk: Int, nowMs: Long) {
        if (vk !in ORDER) return
        val double = vk == lastVk && nowMs - lastAt <= doubleTapMs
        lastVk = vk
        lastAt = nowMs
        when {
            vk in locked -> { locked.remove(vk); latched.remove(vk) }
            double && vk in latched -> locked.add(vk)
            vk in latched -> latched.remove(vk)
            else -> latched.add(vk)
        }
    }

    /** The modifiers for this key press; one-shot ones are cleared, locked ones stay. */
    fun consume(): List<Int> {
        val out = active()
        latched.clear()
        return out
    }

    fun clear() {
        latched.clear()
        locked.clear()
        lastVk = -1
    }

    companion object {
        val ORDER = listOf(Vk.CONTROL, Vk.ALT, Vk.SHIFT, Vk.LWIN)
    }
}

/**
 * Where the floating keyboard sits, independent of the screen size: position as a fraction of
 * the free space (so it survives rotation), width as a fraction of the container width.
 */
data class KbdGeometry(
    val xFrac: Float = 0.5f,
    val yFrac: Float = 1f,
    val widthFrac: Float = 0.62f,
    val opacity: Float = 0.92f,
) {
    fun sane() = KbdGeometry(
        xFrac.finiteOr(0.5f).coerceIn(0f, 1f),
        yFrac.finiteOr(1f).coerceIn(0f, 1f),
        widthFrac.finiteOr(0.62f).coerceIn(MIN_WIDTH_FRAC, MAX_WIDTH_FRAC),
        opacity.finiteOr(0.92f).coerceIn(MIN_OPACITY, 1f),
    )

    companion object {
        const val MIN_WIDTH_FRAC = 0.4f
        const val MAX_WIDTH_FRAC = 1f
        const val MIN_OPACITY = 0.25f

        /** Keyboard height for a width: the rows are about this much shorter than they are wide. */
        const val ASPECT = 0.46f

        fun heightFor(widthPx: Float) = widthPx * ASPECT

        private fun Float.finiteOr(d: Float) = if (isFinite()) this else d
    }
}

/** Pure placement maths for [KbdGeometry]. */
object KbdPlacement {
    /**
     * Keyboard size in px inside a container: width from the geometry (at least [minWidthPx],
     * at most the container), height from the aspect, shrunk to fit a short container.
     */
    fun size(g: KbdGeometry, containerW: Float, containerH: Float, minWidthPx: Float = 0f): Pair<Float, Float> {
        val w = maxOf(containerW * g.sane().widthFrac, minOf(minWidthPx, containerW)).coerceAtMost(containerW).coerceAtLeast(1f)
        val h = KbdGeometry.heightFor(w)
        // Too tall for a short landscape screen: shrink both, keeping the aspect.
        if (h > containerH) {
            val s = containerH / h
            return (w * s).coerceAtLeast(1f) to containerH.coerceAtLeast(1f)
        }
        return w to h
    }

    /** Top-left corner in px. */
    fun origin(g: KbdGeometry, containerW: Float, containerH: Float, minWidthPx: Float = 0f): Pair<Float, Float> {
        val (w, h) = size(g, containerW, containerH, minWidthPx)
        val s = g.sane()
        return (containerW - w).coerceAtLeast(0f) * s.xFrac to (containerH - h).coerceAtLeast(0f) * s.yFrac
    }

    /** The geometry after dragging the keyboard to top-left [x], [y] (px). */
    fun moved(g: KbdGeometry, x: Float, y: Float, containerW: Float, containerH: Float, minWidthPx: Float = 0f): KbdGeometry {
        val (w, h) = size(g, containerW, containerH, minWidthPx)
        val fx = if (containerW > w) (x / (containerW - w)).coerceIn(0f, 1f) else 0f
        val fy = if (containerH > h) (y / (containerH - h)).coerceIn(0f, 1f) else 0f
        return g.copy(xFrac = fx, yFrac = fy).sane()
    }

    /**
     * The geometry after dragging the resize handle to a new width [widthPx], keeping the
     * top-left corner where it is.
     */
    fun resized(g: KbdGeometry, widthPx: Float, containerW: Float, containerH: Float, minWidthPx: Float = 0f): KbdGeometry {
        if (containerW <= 0f) return g
        val (ox, oy) = origin(g, containerW, containerH, minWidthPx)
        val next = g.copy(widthFrac = (widthPx / containerW)).sane()
        return moved(next, ox, oy, containerW, containerH, minWidthPx)
    }
}
