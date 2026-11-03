package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Landscape pad: panel choice, floating keyboard layout, sticky modifiers, placement. */
class G8PadTest {
    @Test
    fun panelChoiceFallsBackToClicks() {
        assertEquals(PadPanel.CLICKS, PadPanel.parse(null))
        assertEquals(PadPanel.CLICKS, PadPanel.parse("nope"))
        for (p in PadPanel.entries) assertEquals(p, PadPanel.parse(p.name))
        assertEquals(6, PadPanel.entries.size)
    }

    @Test
    fun keyboardHasAFullQwertyWithTheRightKeyCodes() {
        val keys = PcKeyboard.rows.flatten()
        for (c in 'A'..'Z') assertTrue("$c", keys.any { it.label == c.toString() && it.vk == c.code })
        for (c in '0'..'9') assertTrue("$c", keys.any { it.label == c.toString() && it.vk == c.code })
        assertEquals(12, keys.count { it.label.matches(Regex("F\\d+")) })
        listOf(Vk.ESCAPE, Vk.TAB, Vk.BACK, Vk.RETURN, Vk.SPACE, Vk.DELETE, Vk.LEFT, Vk.UP, Vk.DOWN, Vk.RIGHT, Vk.HOME, Vk.END)
            .forEach { vk -> assertTrue("vk $vk", keys.any { it.vk == vk }) }
        assertEquals(2, keys.count { it.vk == Vk.SHIFT })
        assertTrue(keys.any { it.vk == Vk.LWIN })
        // QWERTY order on the letter rows.
        assertEquals("QWERTYUIOP", PcKeyboard.rows[2].filter { it.label.length == 1 && it.label[0].isLetter() }.joinToString("") { it.label })
        assertEquals("ASDFGHJKL", PcKeyboard.rows[3].filter { it.label.length == 1 && it.label[0].isLetter() }.joinToString("") { it.label })
        assertEquals("ZXCVBNM", PcKeyboard.rows[4].filter { it.label.length == 1 && it.label[0].isLetter() }.joinToString("") { it.label })
        // The main rows are about equally wide so the keyboard is a clean rectangle.
        val widths = PcKeyboard.rows.take(6).map { r -> r.sumOf { it.weight.toDouble() } }
        assertTrue("$widths", widths.max() - widths.min() < 1.5)
    }

    @Test
    fun keyPressBecomesAShortcutWhenModifiersAreHeld() {
        val c = PcKeyboard.rows[4].first { it.label == "C" }
        assertEquals(listOf(InputFrame.KeyDown(c.vk), InputFrame.KeyUp(c.vk)), PcKeyboard.frames(c, emptyList()))
        assertEquals(listOf<InputFrame>(InputFrame.Shortcut(listOf(Vk.CONTROL, c.vk))), PcKeyboard.frames(c, listOf(Vk.CONTROL)))
        assertEquals(
            listOf<InputFrame>(InputFrame.Shortcut(listOf(Vk.CONTROL, Vk.SHIFT, c.vk))),
            PcKeyboard.frames(c, listOf(Vk.CONTROL, Vk.SHIFT)),
        )
    }

    @Test
    fun stickyModifiersLatchOnceUnlessLocked() {
        val s = StickyMods()
        s.tap(Vk.CONTROL, 0)
        s.tap(Vk.SHIFT, 1000)
        assertEquals(listOf(Vk.CONTROL, Vk.SHIFT), s.active())
        assertEquals(listOf(Vk.CONTROL, Vk.SHIFT), s.consume())
        assertTrue(s.active().isEmpty())

        // Tap again releases before use.
        s.tap(Vk.ALT, 5000)
        s.tap(Vk.ALT, 9000)
        assertFalse(s.isActive(Vk.ALT))

        // Double-tap locks; locked survives key presses; a tap unlocks.
        s.tap(Vk.CONTROL, 20000)
        s.tap(Vk.CONTROL, 20200)
        assertTrue(s.isLocked(Vk.CONTROL))
        assertEquals(listOf(Vk.CONTROL), s.consume())
        assertEquals(listOf(Vk.CONTROL), s.consume())
        s.tap(Vk.CONTROL, 30000)
        assertFalse(s.isActive(Vk.CONTROL))

        // Not a modifier: ignored. Order is always Ctrl, Alt, Shift, Win.
        s.tap(Vk.SPACE, 40000)
        s.tap(Vk.LWIN, 41000)
        s.tap(Vk.ALT, 42000)
        s.tap(Vk.CONTROL, 43000)
        assertEquals(listOf(Vk.CONTROL, Vk.ALT, Vk.LWIN), s.active())
        s.clear()
        assertTrue(s.active().isEmpty())
    }

    @Test
    fun modifierKeysAreRecognised() {
        val keys = PcKeyboard.rows.flatten()
        assertTrue(keys.filter { it.label in listOf("Ctrl", "Alt", "Shift", "Win") }.all { it.isModifier })
        assertFalse(keys.first { it.label == "Space" }.isModifier)
    }

    @Test
    fun placementStaysInsideTheContainer() {
        val g = KbdGeometry(xFrac = 1f, yFrac = 1f, widthFrac = 0.6f)
        val (w, h) = KbdPlacement.size(g, 800f, 400f)
        val (x, y) = KbdPlacement.origin(g, 800f, 400f)
        assertEquals(480f, w, 0.01f)
        assertEquals(KbdGeometry.heightFor(480f), h, 0.01f)
        assertEquals(800f, x + w, 0.01f)
        assertEquals(400f, y + h, 0.01f)
        // Dragging far outside clamps to the edges; the position is remembered as fractions.
        val moved = KbdPlacement.moved(g, -500f, -500f, 800f, 400f)
        assertEquals(0f, moved.xFrac, 0f)
        assertEquals(0f, moved.yFrac, 0f)
        val mid = KbdPlacement.moved(g, 160f, 50f, 800f, 400f)
        assertEquals(0.5f, mid.xFrac, 0.001f)
        val o = KbdPlacement.origin(mid, 800f, 400f)
        assertEquals(160f, o.first, 0.5f)
    }

    @Test
    fun theSamePositionSurvivesRotation() {
        val g = KbdGeometry(xFrac = 0.25f, yFrac = 0.75f, widthFrac = 0.5f)
        val (x1, _) = KbdPlacement.origin(g, 900f, 400f)
        val (x2, _) = KbdPlacement.origin(g, 400f, 900f)
        assertEquals(0.25f, x1 / (900f - 450f), 0.001f)
        assertEquals(0.25f, x2 / (400f - 200f), 0.001f)
    }

    @Test
    fun aShortScreenShrinksTheKeyboardKeepingItsShape() {
        val g = KbdGeometry(widthFrac = 1f)
        val (w, h) = KbdPlacement.size(g, 1000f, 200f)
        assertEquals(200f, h, 0.01f)
        assertEquals(KbdGeometry.heightFor(w), h, 0.01f)
    }

    @Test
    fun resizingKeepsTheTopLeftCornerAndRespectsTheLimits() {
        val g = KbdGeometry(xFrac = 0.2f, yFrac = 0.1f, widthFrac = 0.5f)
        val (ox, oy) = KbdPlacement.origin(g, 1000f, 600f)
        val bigger = KbdPlacement.resized(g, 600f, 1000f, 600f)
        val (nx, ny) = KbdPlacement.origin(bigger, 1000f, 600f)
        assertEquals(600f / 1000f, bigger.widthFrac, 0.001f)
        assertEquals(ox, nx, 1f)
        assertEquals(oy, ny, 1f)
        assertEquals(KbdGeometry.MIN_WIDTH_FRAC, KbdPlacement.resized(g, 10f, 1000f, 600f).widthFrac, 0f)
        assertEquals(KbdGeometry.MAX_WIDTH_FRAC, KbdPlacement.resized(g, 9999f, 1000f, 600f).widthFrac, 0f)
        // A minimum pixel width keeps keys tappable on a narrow screen.
        val (w, _) = KbdPlacement.size(KbdGeometry(widthFrac = 0.4f), 500f, 900f, minWidthPx = 300f)
        assertEquals(300f, w, 0.01f)
    }

    @Test
    fun badStoredGeometryIsRepaired() {
        val s = KbdGeometry(Float.NaN, 5f, 0f, -3f).sane()
        assertEquals(0.5f, s.xFrac, 0f)
        assertEquals(1f, s.yFrac, 0f)
        assertEquals(KbdGeometry.MIN_WIDTH_FRAC, s.widthFrac, 0f)
        assertEquals(KbdGeometry.MIN_OPACITY, s.opacity, 0f)
    }
}
