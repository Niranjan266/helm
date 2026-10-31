package com.niranjan.helm.core

import java.nio.ByteBuffer
import kotlin.math.roundToInt
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray

/** Mouse buttons for `mb` frames (§9.1). */
enum class MouseButton(val wire: String) { LEFT("left"), RIGHT("right"), MIDDLE("middle"), X1("x1"), X2("x2") }

/**
 * A fire-and-forget input frame (PROTOCOL.md §9.1): `{"type":"in","k":<kind>,…}`, never an `id`.
 */
sealed interface InputFrame {
    val kind: String
    fun toJson(): JsonObject

    /** Relative pointer move in PC pixels (fractional allowed). */
    data class Move(val dx: Float, val dy: Float) : InputFrame {
        override val kind get() = "mm"
        override fun toJson() = frame(kind) {
            put("dx", round2(dx))
            put("dy", round2(dy))
        }
    }

    /** Absolute move (§10 direct touch): [x], [y] in 0..1, normalised to the streamed monitor. */
    data class AbsMove(val x: Float, val y: Float) : InputFrame {
        override val kind get() = "ma"
        override fun toJson() = frame(kind) {
            put("x", round4(x.coerceIn(0f, 1f)))
            put("y", round4(y.coerceIn(0f, 1f)))
        }
    }

    /** Phase 4 gyro air-mouse delta in PC pixels (handled like `mm`, §12). */
    data class Gyro(val dx: Float, val dy: Float) : InputFrame {
        override val kind get() = "gy"
        override fun toJson() = frame(kind) {
            put("dx", round2(dx))
            put("dy", round2(dy))
        }
    }

    data class Button(val button: MouseButton, val down: Boolean) : InputFrame {
        override val kind get() = "mb"
        override fun toJson() = frame(kind) {
            put("button", button.wire)
            put("down", down)
        }
    }

    /** Scroll in wheel notches; `dy > 0` = up, `dx > 0` = right. */
    data class Scroll(val dx: Float, val dy: Float) : InputFrame {
        override val kind get() = "ms"
        override fun toJson() = frame(kind) {
            put("dx", round2(dx))
            put("dy", round2(dy))
        }
    }

    data class KeyDown(val vk: Int) : InputFrame {
        override val kind get() = "kd"
        override fun toJson() = frame(kind) { put("vk", vk) }
    }

    data class KeyUp(val vk: Int) : InputFrame {
        override val kind get() = "ku"
        override fun toJson() = frame(kind) { put("vk", vk) }
    }

    data class Text(val text: String) : InputFrame {
        override val kind get() = "tx"
        override fun toJson() = frame(kind) { put("text", text) }
    }

    /** Keys pressed in order, released in reverse by the agent. */
    data class Shortcut(val keys: List<Int>) : InputFrame {
        override val kind get() = "sc"
        override fun toJson() = frame(kind) { putJsonArray("keys") { keys.forEach { add(kotlinx.serialization.json.JsonPrimitive(it)) } } }
    }

    companion object {
        private inline fun frame(kind: String, crossinline body: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit) =
            buildJsonObject {
                put("type", "in")
                put("k", kind)
                body()
            }

        /** Two decimals keep frames small; the agent accumulates fractions. */
        internal fun round2(v: Float): Float = if (v.isFinite()) (v * 100f).roundToInt() / 100f else 0f

        /** Four decimals = 0.2 px on a 4K monitor; enough for absolute positions. */
        internal fun round4(v: Float): Float = if (v.isFinite()) (v * 10_000f).roundToInt() / 10_000f else 0f

        /** Left click = down then up. */
        fun click(button: MouseButton = MouseButton.LEFT) = listOf(Button(button, true), Button(button, false))
    }
}

/** Windows virtual-key codes used by the phone UI. */
object Vk {
    const val BACK = 0x08
    const val TAB = 0x09
    const val RETURN = 0x0D
    const val SHIFT = 0x10
    const val CONTROL = 0x11
    const val ALT = 0x12
    const val ESCAPE = 0x1B
    const val SPACE = 0x20
    const val PAGE_UP = 0x21
    const val PAGE_DOWN = 0x22
    const val END = 0x23
    const val HOME = 0x24
    const val LEFT = 0x25
    const val UP = 0x26
    const val RIGHT = 0x27
    const val DOWN = 0x28
    const val DELETE = 0x2E
    const val LWIN = 0x5B
    const val F1 = 0x70

    fun f(n: Int): Int {
        require(n in 1..24) { "F$n" }
        return F1 + n - 1
    }

    /** VK for a letter or digit typed while a modifier is held, or null. */
    fun forChar(c: Char): Int? = when (c) {
        in 'a'..'z' -> 0x41 + (c - 'a')
        in 'A'..'Z' -> 0x41 + (c - 'A')
        in '0'..'9' -> 0x30 + (c - '0')
        ' ' -> SPACE
        else -> null
    }
}

/**
 * Binary file frame (§9.2): `[16 bytes transfer id][8 bytes offset, big-endian][data]`, data
 * at most 256 KiB.
 */
class BinaryFrame(val transferId: ByteArray, val offset: Long, val data: ByteArray) {
    init {
        require(transferId.size == ID_LEN) { "transfer id must be $ID_LEN bytes" }
        require(offset >= 0) { "offset must be >= 0" }
        require(data.size <= MAX_DATA) { "data must be <= $MAX_DATA bytes" }
    }

    val transferIdHex: String get() = transferId.toHex()

    fun encode(): ByteArray =
        ByteBuffer.allocate(HEADER_LEN + data.size).put(transferId).putLong(offset).put(data).array()

    companion object {
        const val ID_LEN = 16
        const val HEADER_LEN = 24
        const val MAX_DATA = 256 * 1024

        /** Parses a frame; null if it is too short, too long or has a negative offset. */
        fun decode(bytes: ByteArray): BinaryFrame? {
            if (bytes.size < HEADER_LEN || bytes.size > HEADER_LEN + MAX_DATA) return null
            val buf = ByteBuffer.wrap(bytes)
            val id = ByteArray(ID_LEN).also { buf.get(it) }
            val offset = buf.long
            if (offset < 0) return null
            return BinaryFrame(id, offset, bytes.copyOfRange(HEADER_LEN, bytes.size))
        }

        /** 32 hex chars -> 16 bytes, else null. */
        fun idFromHex(hex: String): ByteArray? {
            if (hex.length != ID_LEN * 2) return null
            val out = ByteArray(ID_LEN)
            for (i in 0 until ID_LEN) {
                val hi = Character.digit(hex[2 * i], 16)
                val lo = Character.digit(hex[2 * i + 1], 16)
                if (hi < 0 || lo < 0) return null
                out[i] = ((hi shl 4) or lo).toByte()
            }
            return out
        }
    }
}

fun ByteArray.toHex(): String = joinToString("") { "%02x".format(it) }
