package com.niranjan.helm.core

import kotlin.math.hypot
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** G1 on-screen gamepad: mapping codecs, stick maths and the frame engine. */
class G1GamepadTest {
    private val eps = 1e-3f
    private val W = 0x57
    private val A = 0x41
    private val D = 0x44

    @Test
    fun bindingCodecsRoundTrip() {
        val buttons = listOf(ButtonBinding.None, ButtonBinding.Key(Vk.SPACE), ButtonBinding.Mouse(MouseButton.RIGHT))
        buttons.forEach { assertEquals(it, GamepadCodec.decodeButton(GamepadCodec.encodeButton(it))) }
        val sticks = listOf(StickBinding.None, StickBinding.WASD, StickBinding.Keys(0, 0, Vk.LEFT, Vk.RIGHT), StickBinding.Mouse(1400f))
        sticks.forEach { assertEquals(it, GamepadCodec.decodeStick(GamepadCodec.encodeStick(it))) }
        assertEquals("key:32", GamepadCodec.encodeButton(ButtonBinding.Key(32)))
        assertEquals("keys:87,83,65,68", GamepadCodec.encodeStick(StickBinding.WASD))
    }

    @Test
    fun malformedBindingsDecodeToNone() {
        listOf(null, "", "key", "key:abc", "key:0", "key:999", "mouse:side", "laser:1").forEach {
            assertEquals(it, ButtonBinding.None, GamepadCodec.decodeButton(it))
        }
        listOf(null, "keys:1,2,3", "keys:1,2,3,x", "keys:1,2,3,300", "mouse:-5", "mouse:NaN", "orbit").forEach {
            assertEquals(it, StickBinding.None, GamepadCodec.decodeStick(it))
        }
        assertEquals(StickBinding.Mouse(10_000f), GamepadCodec.decodeStick("mouse:99999"))
    }

    @Test
    fun presetsRoundTripThroughProfileJson() {
        for ((id, preset) in GamepadPresets.all) {
            val p = GamepadProfile(selected = GamepadPresets.CUSTOM, custom = preset)
            val back = GamepadCodec.decodeProfile(GamepadCodec.encodeProfile(p))
            assertEquals(id, preset, back.custom)
            assertEquals(preset, back.active())
        }
        val sel = GamepadCodec.decodeProfile(GamepadCodec.encodeProfile(GamepadProfile(GamepadPresets.RACING)))
        assertEquals(GamepadPresets.racing, sel.active())
    }

    @Test
    fun profileDecodeIsForgiving() {
        assertEquals(GamepadProfile(), GamepadCodec.decodeProfile(null))
        assertEquals(GamepadProfile(), GamepadCodec.decodeProfile("{not json"))
        // Unknown preset id and missing custom both fall back to the keyboard preset.
        assertEquals(GamepadPresets.keyboard, GamepadCodec.decodeProfile("""{"selected":"zzz"}""").active())
        assertEquals(GamepadPresets.keyboard, GamepadCodec.decodeProfile("""{"selected":"custom"}""").active())
        val p = GamepadCodec.decodeProfile(
            """{"selected":"custom","custom":{"buttons":{"A":"key:32","NOPE":"key:1","B":"bad"},"left":"keys:87,83,65,68","deadzone":5,"extra":1}}""",
        )
        val m = p.active()
        assertEquals(ButtonBinding.Key(32), m.button(PadButton.A))
        assertEquals(ButtonBinding.None, m.button(PadButton.B))
        assertEquals(StickBinding.WASD, m.left)
        assertEquals(StickBinding.None, m.right)
        assertEquals(0.9f, m.deadzone, eps)
    }

    @Test
    fun touchIsClampedToUnitCircleAndDeadzoneRescales() {
        val (x, y) = StickMath.fromTouch(300f, 400f, 100f)
        assertEquals(1f, hypot(x, y), eps)
        assertEquals(0.6f, x, eps)
        assertEquals(0.5f to 0f, StickMath.fromTouch(50f, 0f, 100f))
        assertEquals(0f to 0f, StickMath.fromTouch(50f, 0f, 0f))
        assertEquals(0f to 0f, StickMath.deadzone(0.1f, 0f, 0.15f))
        // Just outside the dead zone starts near zero; full tilt stays at 1.
        assertEquals(0f, StickMath.deadzone(0.15f, 0f, 0.15f).first, eps)
        assertEquals(0.5f, StickMath.deadzone(0.575f, 0f, 0.15f).first, eps)
        assertEquals(1f, StickMath.deadzone(0f, -1f, 0.15f).second.let { -it }, eps)
    }

    @Test
    fun stickDirectionsHaveHysteresisAndDiagonals() {
        assertEquals(setOf(SwipeDirection.UP), StickMath.directions(0f, -0.6f, emptySet()))
        // 0.45 is below "on" but above "off": only stays on if already held.
        assertEquals(emptySet<SwipeDirection>(), StickMath.directions(0.45f, 0f, emptySet()))
        assertEquals(setOf(SwipeDirection.RIGHT), StickMath.directions(0.45f, 0f, setOf(SwipeDirection.RIGHT)))
        assertEquals(emptySet<SwipeDirection>(), StickMath.directions(0.3f, 0f, setOf(SwipeDirection.RIGHT)))
        assertEquals(setOf(SwipeDirection.DOWN, SwipeDirection.LEFT), StickMath.directions(-0.7f, 0.7f, emptySet()))
    }

    @Test
    fun mouseDeltaUsesQuadraticCurve() {
        val (fx, fy) = StickMath.mouseDelta(1f, 0f, 1000f, 16f)
        assertEquals(16f, fx, eps)
        assertEquals(0f, fy, eps)
        assertEquals(4f, StickMath.mouseDelta(0.5f, 0f, 1000f, 16f).first, eps)
        val (dx, dy) = StickMath.mouseDelta(0f, -1f, 1000f, 500f) // dt is capped at 100 ms
        assertEquals(0f, dx, eps)
        assertEquals(-100f, dy, eps)
        assertEquals(0f to 0f, StickMath.mouseDelta(0f, 0f, 1000f, 16f))
    }

    @Test
    fun engineKeyStickPressesAndReleasesKeys() {
        val e = GamepadEngine(GamepadPresets.keyboard)
        assertEquals(listOf(InputFrame.KeyDown(W)), e.stick(PadStick.LEFT, 0f, -1f))
        assertEquals(emptyList<InputFrame>(), e.stick(PadStick.LEFT, 0f, -0.9f))
        // Roll to up-right: W stays, D goes down.
        assertEquals(listOf(InputFrame.KeyDown(D)), e.stick(PadStick.LEFT, 0.7f, -0.7f))
        val back = e.stick(PadStick.LEFT, 0f, 0f)
        assertEquals(setOf(InputFrame.KeyUp(W), InputFrame.KeyUp(D)), back.toSet())
        assertEquals(listOf(InputFrame.KeyDown(A)), e.stick(PadStick.LEFT, -1f, 0f))
        assertEquals(emptyList<InputFrame>(), e.tick(16f)) // no mouse stick in this preset
    }

    @Test
    fun engineButtonsAreRefCountedAndIdempotent() {
        val m = GamepadMapping(
            name = "t",
            buttons = mapOf(PadButton.A to ButtonBinding.Key(Vk.SPACE), PadButton.B to ButtonBinding.Key(Vk.SPACE),
                PadButton.RT to ButtonBinding.Mouse(MouseButton.LEFT)),
            left = StickBinding.None,
            right = StickBinding.None,
        )
        val e = GamepadEngine(m)
        assertEquals(listOf(InputFrame.KeyDown(Vk.SPACE)), e.press(PadButton.A))
        assertEquals(emptyList<InputFrame>(), e.press(PadButton.A))
        assertEquals(emptyList<InputFrame>(), e.press(PadButton.B))
        assertEquals(emptyList<InputFrame>(), e.release(PadButton.A))
        assertEquals(listOf(InputFrame.KeyUp(Vk.SPACE)), e.release(PadButton.B))
        assertEquals(emptyList<InputFrame>(), e.release(PadButton.B))
        assertEquals(listOf(InputFrame.Button(MouseButton.LEFT, true)), e.press(PadButton.RT))
        assertEquals(listOf(InputFrame.Button(MouseButton.LEFT, false)), e.release(PadButton.RT))
        assertEquals(emptyList<InputFrame>(), e.press(PadButton.START)) // unbound
    }

    @Test
    fun engineMouseLookMovesPointerPerTick() {
        val e = GamepadEngine(GamepadPresets.mouseLook)
        assertEquals(emptyList<InputFrame>(), e.stick(PadStick.RIGHT, 1f, 0f))
        val f = e.tick(10f).single() as InputFrame.Move
        assertEquals(14f, f.dx, eps) // 1400 px/s * 10 ms
        assertEquals(0f, f.dy, eps)
        e.stick(PadStick.RIGHT, 0.05f, 0f) // inside the dead zone
        assertEquals(emptyList<InputFrame>(), e.tick(10f))
    }

    @Test
    fun releaseAllAndMappingSwapLeaveNothingHeld() {
        val e = GamepadEngine(GamepadPresets.mouseLook)
        e.press(PadButton.RT)
        e.press(PadButton.A)
        e.stick(PadStick.LEFT, 0f, -1f)
        val released = e.setMapping(GamepadPresets.racing)
        assertEquals(
            setOf(InputFrame.Button(MouseButton.LEFT, false), InputFrame.KeyUp(Vk.SPACE), InputFrame.KeyUp(W)),
            released.toSet(),
        )
        assertEquals(GamepadPresets.racing, e.mapping)
        assertEquals(emptyList<InputFrame>(), e.releaseAll())
        // Racing: left stick steers with arrows only; vertical tilt does nothing.
        assertEquals(emptyList<InputFrame>(), e.stick(PadStick.LEFT, 0f, -1f))
        assertEquals(listOf(InputFrame.KeyDown(Vk.RIGHT)), e.stick(PadStick.LEFT, 1f, 0f))
        assertTrue(e.press(PadButton.RT).contains(InputFrame.KeyDown(Vk.UP)))
    }

    @Test
    fun editorChoicesHaveUniqueLabels() {
        val labels = GamepadKeys.buttonChoices.map { it.first }
        assertEquals(labels.size, labels.toSet().size)
        assertEquals("Space", GamepadKeys.label(ButtonBinding.Key(Vk.SPACE)))
        assertEquals("WASD", GamepadKeys.label(StickBinding.WASD))
        assertEquals("Key 0xBA", GamepadKeys.label(ButtonBinding.Key(0xBA)))
    }
}
