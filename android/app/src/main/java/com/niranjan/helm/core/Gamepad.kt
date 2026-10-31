package com.niranjan.helm.core

import kotlin.math.hypot
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.floatOrNull
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.put

/*
 * On-screen gamepad (G1, Docs/protocol/touchpad-gamepad.md). Every control is mapped to plain
 * `in` key / mouse frames (§9.1), so the PC needs no virtual-controller driver. Pure Kotlin.
 */

enum class PadButton(val label: String) {
    A("A"), B("B"), X("X"), Y("Y"),
    LB("LB"), RB("RB"), LT("LT"), RT("RT"),
    START("Start"), SELECT("Select"),
    DPAD_UP("D-pad up"), DPAD_DOWN("D-pad down"), DPAD_LEFT("D-pad left"), DPAD_RIGHT("D-pad right"),
}

enum class PadStick(val label: String) { LEFT("Left stick"), RIGHT("Right stick") }

/** What a button sends while held. */
sealed interface ButtonBinding {
    data object None : ButtonBinding
    data class Key(val vk: Int) : ButtonBinding
    data class Mouse(val button: MouseButton) : ButtonBinding
}

/** What a thumbstick sends. */
sealed interface StickBinding {
    data object None : StickBinding

    /** Directions held as keys; 0 = no key for that direction. */
    data class Keys(val up: Int, val down: Int, val left: Int, val right: Int) : StickBinding

    /** Relative mouse moves; [speed] = PC px per second at full deflection. */
    data class Mouse(val speed: Float) : StickBinding

    companion object {
        val WASD = Keys(0x57, 0x53, 0x41, 0x44)
        val ARROWS = Keys(Vk.UP, Vk.DOWN, Vk.LEFT, Vk.RIGHT)
    }
}

data class GamepadMapping(
    val name: String,
    val buttons: Map<PadButton, ButtonBinding>,
    val left: StickBinding,
    val right: StickBinding,
    /** Radial dead zone, 0..0.9 of the stick radius. */
    val deadzone: Float = 0.15f,
) {
    fun button(b: PadButton): ButtonBinding = buttons[b] ?: ButtonBinding.None
    fun stick(s: PadStick): StickBinding = if (s == PadStick.LEFT) left else right
}

object GamepadPresets {
    const val KEYBOARD = "keyboard"
    const val RACING = "racing"
    const val MOUSE_LOOK = "mouse_look"
    const val CUSTOM = "custom"

    private fun key(c: Char) = ButtonBinding.Key(Vk.forChar(c)!!)
    private fun vk(v: Int) = ButtonBinding.Key(v)

    val keyboard = GamepadMapping(
        name = "Keyboard (WASD + arrows)",
        buttons = mapOf(
            PadButton.A to vk(Vk.SPACE), PadButton.B to vk(Vk.ESCAPE),
            PadButton.X to key('e'), PadButton.Y to key('q'),
            PadButton.LB to vk(Vk.SHIFT), PadButton.RB to vk(Vk.CONTROL),
            PadButton.LT to ButtonBinding.Mouse(MouseButton.RIGHT), PadButton.RT to ButtonBinding.Mouse(MouseButton.LEFT),
            PadButton.START to vk(Vk.RETURN), PadButton.SELECT to vk(Vk.TAB),
            PadButton.DPAD_UP to vk(Vk.UP), PadButton.DPAD_DOWN to vk(Vk.DOWN),
            PadButton.DPAD_LEFT to vk(Vk.LEFT), PadButton.DPAD_RIGHT to vk(Vk.RIGHT),
        ),
        left = StickBinding.WASD,
        right = StickBinding.ARROWS,
    )

    val racing = GamepadMapping(
        name = "Racing",
        buttons = mapOf(
            PadButton.RT to vk(Vk.UP), PadButton.LT to vk(Vk.DOWN),
            PadButton.A to vk(Vk.SPACE), PadButton.B to key('c'),
            PadButton.X to key('r'), PadButton.Y to key('n'),
            PadButton.LB to key('z'), PadButton.RB to key('x'),
            PadButton.START to vk(Vk.ESCAPE), PadButton.SELECT to vk(Vk.TAB),
            PadButton.DPAD_UP to vk(Vk.UP), PadButton.DPAD_DOWN to vk(Vk.DOWN),
            PadButton.DPAD_LEFT to vk(Vk.LEFT), PadButton.DPAD_RIGHT to vk(Vk.RIGHT),
        ),
        // Steering only: the triggers are throttle and brake.
        left = StickBinding.Keys(0, 0, Vk.LEFT, Vk.RIGHT),
        right = StickBinding.None,
        deadzone = 0.2f,
    )

    val mouseLook = GamepadMapping(
        name = "Mouse look (right stick = mouse)",
        buttons = mapOf(
            PadButton.RT to ButtonBinding.Mouse(MouseButton.LEFT), PadButton.LT to ButtonBinding.Mouse(MouseButton.RIGHT),
            PadButton.A to vk(Vk.SPACE), PadButton.B to vk(Vk.CONTROL),
            PadButton.X to key('r'), PadButton.Y to key('e'),
            PadButton.LB to key('q'), PadButton.RB to key('g'),
            PadButton.START to vk(Vk.ESCAPE), PadButton.SELECT to vk(Vk.TAB),
            PadButton.DPAD_UP to key('1'), PadButton.DPAD_RIGHT to key('2'),
            PadButton.DPAD_DOWN to key('3'), PadButton.DPAD_LEFT to key('4'),
        ),
        left = StickBinding.WASD,
        right = StickBinding.Mouse(1400f),
    )

    val all: List<Pair<String, GamepadMapping>> = listOf(KEYBOARD to keyboard, RACING to racing, MOUSE_LOOK to mouseLook)

    fun byId(id: String): GamepadMapping? = all.firstOrNull { it.first == id }?.second
}

/** Thumbstick maths. Coordinates use screen axes: x right, y down, unit circle = full tilt. */
object StickMath {
    /** Finger offset from the stick centre -> a vector clamped to the unit circle. */
    fun fromTouch(dxPx: Float, dyPx: Float, radiusPx: Float): Pair<Float, Float> {
        if (!dxPx.isFinite() || !dyPx.isFinite() || radiusPx <= 0f) return 0f to 0f
        val x = dxPx / radiusPx
        val y = dyPx / radiusPx
        val m = hypot(x, y)
        return if (m > 1f) x / m to y / m else x to y
    }

    /** Radial dead zone, rescaled so motion starts smoothly from 0 just outside it. */
    fun deadzone(x: Float, y: Float, dz: Float): Pair<Float, Float> {
        val m = hypot(x, y)
        val d = dz.coerceIn(0f, 0.9f)
        if (m <= d || m == 0f) return 0f to 0f
        val scaled = ((m - d) / (1f - d)).coerceAtMost(1f)
        return x / m * scaled to y / m * scaled
    }

    /**
     * Directions held for a key-mapped stick, with hysteresis: a direction turns on past
     * [on] and stays on until it falls below [off]. Diagonals hold two keys.
     */
    fun directions(
        x: Float,
        y: Float,
        held: Set<SwipeDirection>,
        on: Float = 0.5f,
        off: Float = 0.35f,
    ): Set<SwipeDirection> {
        fun want(d: SwipeDirection, v: Float) = if (d in held) v > off else v > on
        val out = mutableSetOf<SwipeDirection>()
        if (want(SwipeDirection.UP, -y)) out += SwipeDirection.UP
        if (want(SwipeDirection.DOWN, y)) out += SwipeDirection.DOWN
        if (want(SwipeDirection.LEFT, -x)) out += SwipeDirection.LEFT
        if (want(SwipeDirection.RIGHT, x)) out += SwipeDirection.RIGHT
        return out
    }

    /** Mouse delta for one tick: quadratic response curve for fine aim near the centre. */
    fun mouseDelta(x: Float, y: Float, speedPxPerS: Float, dtMs: Float): Pair<Float, Float> {
        val m = hypot(x, y)
        if (m == 0f || !m.isFinite() || speedPxPerS <= 0f) return 0f to 0f
        val mag = m.coerceAtMost(1f)
        val px = speedPxPerS * mag * mag * dtMs.coerceIn(0f, 100f) / 1000f
        return x / m * px to y / m * px
    }
}

/**
 * Turns pad presses and stick positions into `in` frames. Keys and mouse buttons are
 * reference counted, so two controls bound to the same key never release it early.
 */
class GamepadEngine(mapping: GamepadMapping) {
    var mapping: GamepadMapping = mapping
        private set

    private val keyRefs = mutableMapOf<Int, Int>()
    private val mouseRefs = mutableMapOf<MouseButton, Int>()
    private val held = mutableSetOf<PadButton>()
    private val stickDirs = mutableMapOf<PadStick, Set<SwipeDirection>>()
    private val stickPos = mutableMapOf<PadStick, Pair<Float, Float>>()

    /** Swaps the mapping, releasing everything held under the old one first. */
    fun setMapping(m: GamepadMapping): List<InputFrame> {
        val out = releaseAll()
        mapping = m
        return out
    }

    fun press(b: PadButton): List<InputFrame> {
        if (!held.add(b)) return emptyList()
        return down(mapping.button(b))
    }

    fun release(b: PadButton): List<InputFrame> {
        if (!held.remove(b)) return emptyList()
        return up(mapping.button(b))
    }

    /** Raw stick position (unit circle); returns key frames for key-mapped sticks. */
    fun stick(s: PadStick, x: Float, y: Float): List<InputFrame> {
        val v = StickMath.deadzone(x, y, mapping.deadzone)
        stickPos[s] = v
        val binding = mapping.stick(s) as? StickBinding.Keys ?: return emptyList()
        val before = stickDirs[s].orEmpty()
        val after = StickMath.directions(v.first, v.second, before)
        stickDirs[s] = after
        val out = mutableListOf<InputFrame>()
        for (d in before - after) out += up(keyFor(binding, d))
        for (d in after - before) out += down(keyFor(binding, d))
        return out
    }

    /** Called every frame: relative moves for mouse-mapped sticks. */
    fun tick(dtMs: Float): List<InputFrame> {
        var dx = 0f
        var dy = 0f
        for (s in PadStick.entries) {
            val b = mapping.stick(s) as? StickBinding.Mouse ?: continue
            val (x, y) = stickPos[s] ?: continue
            val (mx, my) = StickMath.mouseDelta(x, y, b.speed, dtMs)
            dx += mx
            dy += my
        }
        return if (dx == 0f && dy == 0f) emptyList() else listOf(InputFrame.Move(dx, dy))
    }

    /** Releases every key and button still held (screen closed, mapping changed). */
    fun releaseAll(): List<InputFrame> {
        val out = mutableListOf<InputFrame>()
        for (vk in keyRefs.keys) out += InputFrame.KeyUp(vk)
        for (b in mouseRefs.keys) out += InputFrame.Button(b, false)
        keyRefs.clear()
        mouseRefs.clear()
        held.clear()
        stickDirs.clear()
        stickPos.clear()
        return out
    }

    private fun keyFor(b: StickBinding.Keys, d: SwipeDirection): ButtonBinding {
        val vk = when (d) {
            SwipeDirection.UP -> b.up
            SwipeDirection.DOWN -> b.down
            SwipeDirection.LEFT -> b.left
            SwipeDirection.RIGHT -> b.right
        }
        return if (vk > 0) ButtonBinding.Key(vk) else ButtonBinding.None
    }

    private fun down(b: ButtonBinding): List<InputFrame> = when (b) {
        ButtonBinding.None -> emptyList()
        is ButtonBinding.Key -> {
            val n = keyRefs[b.vk] ?: 0
            keyRefs[b.vk] = n + 1
            if (n == 0) listOf(InputFrame.KeyDown(b.vk)) else emptyList()
        }
        is ButtonBinding.Mouse -> {
            val n = mouseRefs[b.button] ?: 0
            mouseRefs[b.button] = n + 1
            if (n == 0) listOf(InputFrame.Button(b.button, true)) else emptyList()
        }
    }

    private fun up(b: ButtonBinding): List<InputFrame> = when (b) {
        ButtonBinding.None -> emptyList()
        is ButtonBinding.Key -> {
            val n = keyRefs[b.vk] ?: 0
            when {
                n <= 0 -> emptyList()
                n == 1 -> {
                    keyRefs.remove(b.vk)
                    listOf(InputFrame.KeyUp(b.vk))
                }
                else -> {
                    keyRefs[b.vk] = n - 1
                    emptyList()
                }
            }
        }
        is ButtonBinding.Mouse -> {
            val n = mouseRefs[b.button] ?: 0
            when {
                n <= 0 -> emptyList()
                n == 1 -> {
                    mouseRefs.remove(b.button)
                    listOf(InputFrame.Button(b.button, false))
                }
                else -> {
                    mouseRefs[b.button] = n - 1
                    emptyList()
                }
            }
        }
    }
}

/** Labels and choices for the mapping editor. */
object GamepadKeys {
    val buttonChoices: List<Pair<String, ButtonBinding>> = buildList {
        add("None" to ButtonBinding.None)
        add("Mouse left" to ButtonBinding.Mouse(MouseButton.LEFT))
        add("Mouse right" to ButtonBinding.Mouse(MouseButton.RIGHT))
        add("Mouse middle" to ButtonBinding.Mouse(MouseButton.MIDDLE))
        listOf(
            "Space" to Vk.SPACE, "Enter" to Vk.RETURN, "Esc" to Vk.ESCAPE, "Tab" to Vk.TAB,
            "Shift" to Vk.SHIFT, "Ctrl" to Vk.CONTROL, "Alt" to Vk.ALT, "Backspace" to Vk.BACK,
            "↑" to Vk.UP, "↓" to Vk.DOWN, "←" to Vk.LEFT, "→" to Vk.RIGHT,
        ).forEach { (l, v) -> add(l to ButtonBinding.Key(v)) }
        ('A'..'Z').forEach { add(it.toString() to ButtonBinding.Key(Vk.forChar(it)!!)) }
        ('0'..'9').forEach { add(it.toString() to ButtonBinding.Key(Vk.forChar(it)!!)) }
        (1..12).forEach { add("F$it" to ButtonBinding.Key(Vk.f(it))) }
    }

    val stickChoices: List<Pair<String, StickBinding>> = listOf(
        "None" to StickBinding.None,
        "WASD" to StickBinding.WASD,
        "Arrow keys" to StickBinding.ARROWS,
        "Steer ← →" to StickBinding.Keys(0, 0, Vk.LEFT, Vk.RIGHT),
        "Mouse (slow)" to StickBinding.Mouse(800f),
        "Mouse" to StickBinding.Mouse(1400f),
        "Mouse (fast)" to StickBinding.Mouse(2400f),
    )

    fun label(b: ButtonBinding): String =
        buttonChoices.firstOrNull { it.second == b }?.first ?: when (b) {
            is ButtonBinding.Key -> "Key 0x%02X".format(b.vk)
            is ButtonBinding.Mouse -> "Mouse ${b.button.wire}"
            ButtonBinding.None -> "None"
        }

    fun label(b: StickBinding): String =
        stickChoices.firstOrNull { it.second == b }?.first ?: when (b) {
            is StickBinding.Keys -> "Keys"
            is StickBinding.Mouse -> "Mouse ${b.speed.toInt()} px/s"
            StickBinding.None -> "None"
        }
}

/** What the phone remembers per PC: the selected preset, plus the custom mapping if any. */
data class GamepadProfile(val selected: String = GamepadPresets.KEYBOARD, val custom: GamepadMapping? = null) {
    /** The mapping to use; falls back to the keyboard preset when the selection is unknown. */
    fun active(): GamepadMapping =
        if (selected == GamepadPresets.CUSTOM) custom ?: GamepadPresets.keyboard
        else GamepadPresets.byId(selected) ?: GamepadPresets.keyboard
}

/**
 * JSON codec for [GamepadMapping] / [GamepadProfile]. Bindings are compact strings:
 * `none`, `key:<vk>`, `mouse:<button>`; sticks `none`, `keys:<up>,<down>,<left>,<right>`,
 * `mouse:<px per s>`. Unknown or malformed entries decode to `none`.
 */
object GamepadCodec {
    private val json = Json { ignoreUnknownKeys = true }

    fun encodeButton(b: ButtonBinding): String = when (b) {
        ButtonBinding.None -> "none"
        is ButtonBinding.Key -> "key:${b.vk}"
        is ButtonBinding.Mouse -> "mouse:${b.button.wire}"
    }

    fun decodeButton(s: String?): ButtonBinding {
        val (kind, arg) = split(s) ?: return ButtonBinding.None
        return when (kind) {
            "key" -> arg.toIntOrNull()?.takeIf { it in 1..0xFE }?.let { ButtonBinding.Key(it) } ?: ButtonBinding.None
            "mouse" -> MouseButton.entries.firstOrNull { it.wire == arg }?.let { ButtonBinding.Mouse(it) } ?: ButtonBinding.None
            else -> ButtonBinding.None
        }
    }

    fun encodeStick(b: StickBinding): String = when (b) {
        StickBinding.None -> "none"
        is StickBinding.Keys -> "keys:${b.up},${b.down},${b.left},${b.right}"
        is StickBinding.Mouse -> "mouse:${b.speed}"
    }

    fun decodeStick(s: String?): StickBinding {
        val (kind, arg) = split(s) ?: return StickBinding.None
        return when (kind) {
            "keys" -> {
                val v = arg.split(',').map { it.trim().toIntOrNull() }
                if (v.size != 4 || v.any { it == null || it !in 0..0xFE }) StickBinding.None
                else StickBinding.Keys(v[0]!!, v[1]!!, v[2]!!, v[3]!!)
            }
            "mouse" -> arg.toFloatOrNull()?.takeIf { it.isFinite() && it > 0f }
                ?.let { StickBinding.Mouse(it.coerceAtMost(10_000f)) } ?: StickBinding.None
            else -> StickBinding.None
        }
    }

    fun mappingToJson(m: GamepadMapping): JsonObject = buildJsonObject {
        put("name", m.name)
        put("buttons", buildJsonObject { m.buttons.forEach { (b, v) -> put(b.name, encodeButton(v)) } })
        put("left", encodeStick(m.left))
        put("right", encodeStick(m.right))
        put("deadzone", m.deadzone)
    }

    fun mappingFromJson(o: JsonObject): GamepadMapping {
        val buttons = (o["buttons"] as? JsonObject).orEmpty().mapNotNull { (k, v) ->
            val b = PadButton.entries.firstOrNull { it.name == k } ?: return@mapNotNull null
            b to decodeButton((v as? JsonPrimitive)?.contentOrNull)
        }.toMap()
        val dz = (o["deadzone"] as? JsonPrimitive)?.floatOrNull?.takeIf { it.isFinite() } ?: 0.15f
        return GamepadMapping(
            name = (o["name"] as? JsonPrimitive)?.contentOrNull ?: "Custom",
            buttons = buttons,
            left = decodeStick((o["left"] as? JsonPrimitive)?.contentOrNull),
            right = decodeStick((o["right"] as? JsonPrimitive)?.contentOrNull),
            deadzone = dz.coerceIn(0f, 0.9f),
        )
    }

    fun encodeProfile(p: GamepadProfile): String = buildJsonObject {
        put("selected", p.selected)
        p.custom?.let { put("custom", mappingToJson(it)) }
    }.toString()

    /** Never throws: anything unreadable gives the default profile. */
    fun decodeProfile(text: String?): GamepadProfile {
        if (text.isNullOrBlank()) return GamepadProfile()
        return try {
            val o = json.parseToJsonElement(text).jsonObject
            val selected = o["selected"]?.jsonPrimitive?.contentOrNull ?: GamepadPresets.KEYBOARD
            val custom = (o["custom"] as? JsonObject)?.let(::mappingFromJson)
            GamepadProfile(selected, custom)
        } catch (_: Exception) {
            GamepadProfile()
        }
    }

    private fun split(s: String?): Pair<String, String>? {
        if (s == null) return null
        val i = s.indexOf(':')
        if (i <= 0) return null
        return s.substring(0, i) to s.substring(i + 1)
    }
}
