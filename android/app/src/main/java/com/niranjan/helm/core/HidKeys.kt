package com.niranjan.helm.core

/** A key usage plus the modifier byte needed to type one character. */
data class KeyStroke(val modifiers: Int, val usage: Int)

/**
 * Windows virtual keys (what the Wi-Fi keyboard and gestures speak) and characters mapped to
 * USB HID keyboard usages (page 0x07), for a US layout. The PC turns usages into characters
 * with its own layout, so a PC set to another language types that language's characters.
 */
object HidKeys {
    const val MOD_LCTRL = 0x01
    const val MOD_LSHIFT = 0x02
    const val MOD_LALT = 0x04
    const val MOD_LGUI = 0x08
    const val MOD_RCTRL = 0x10
    const val MOD_RSHIFT = 0x20
    const val MOD_RALT = 0x40
    const val MOD_RGUI = 0x80

    /** The modifier bit for a virtual key, or 0 when it is not a modifier. */
    fun modifierBit(vk: Int): Int = when (vk) {
        Vk.CONTROL, 0xA2 -> MOD_LCTRL
        Vk.SHIFT, 0xA0 -> MOD_LSHIFT
        Vk.ALT, 0xA4 -> MOD_LALT
        Vk.LWIN -> MOD_LGUI
        0xA3 -> MOD_RCTRL
        0xA1 -> MOD_RSHIFT
        0xA5 -> MOD_RALT
        0x5C -> MOD_RGUI
        else -> 0
    }

    /** The HID usage for a non-modifier virtual key, or null when there is no mapping. */
    fun usageForVk(vk: Int): Int? = when (vk) {
        in 0x41..0x5A -> 0x04 + (vk - 0x41) // A..Z
        in 0x31..0x39 -> 0x1E + (vk - 0x31) // 1..9
        0x30 -> 0x27
        Vk.RETURN -> 0x28
        Vk.ESCAPE -> 0x29
        Vk.BACK -> 0x2A
        Vk.TAB -> 0x2B
        Vk.SPACE -> 0x2C
        KbdVk.OEM_MINUS -> 0x2D
        KbdVk.OEM_PLUS -> 0x2E
        KbdVk.OEM_4 -> 0x2F
        KbdVk.OEM_6 -> 0x30
        KbdVk.OEM_5 -> 0x31
        KbdVk.OEM_1 -> 0x33
        KbdVk.OEM_7 -> 0x34
        KbdVk.OEM_3 -> 0x35
        KbdVk.OEM_COMMA -> 0x36
        KbdVk.OEM_PERIOD -> 0x37
        KbdVk.OEM_2 -> 0x38
        KbdVk.CAPITAL -> 0x39
        in Vk.F1..(Vk.F1 + 11) -> 0x3A + (vk - Vk.F1) // F1..F12
        0x2C -> 0x46 // Print Screen
        0x91 -> 0x47 // Scroll Lock
        0x13 -> 0x48 // Pause
        KbdVk.INSERT -> 0x49
        Vk.HOME -> 0x4A
        Vk.PAGE_UP -> 0x4B
        Vk.DELETE -> 0x4C
        Vk.END -> 0x4D
        Vk.PAGE_DOWN -> 0x4E
        Vk.RIGHT -> 0x4F
        Vk.LEFT -> 0x50
        Vk.DOWN -> 0x51
        Vk.UP -> 0x52
        0x5D -> 0x65 // Menu / Application
        in (Vk.F1 + 12)..(Vk.F1 + 23) -> 0x68 + (vk - Vk.F1 - 12) // F13..F24
        else -> null
    }

    private val chars: Map<Int, KeyStroke> by lazy {
        val m = HashMap<Int, KeyStroke>()
        for (c in 'a'..'z') {
            m[c.code] = KeyStroke(0, 0x04 + (c - 'a'))
            m[c.uppercaseChar().code] = KeyStroke(MOD_LSHIFT, 0x04 + (c - 'a'))
        }
        "1234567890".forEachIndexed { i, c -> m[c.code] = KeyStroke(0, 0x1E + i) }
        "!@#$%^&*()".forEachIndexed { i, c -> m[c.code] = KeyStroke(MOD_LSHIFT, 0x1E + i) }
        fun pair(plain: Char, shifted: Char, usage: Int) {
            m[plain.code] = KeyStroke(0, usage)
            m[shifted.code] = KeyStroke(MOD_LSHIFT, usage)
        }
        pair('-', '_', 0x2D)
        pair('=', '+', 0x2E)
        pair('[', '{', 0x2F)
        pair(']', '}', 0x30)
        pair('\\', '|', 0x31)
        pair(';', ':', 0x33)
        pair('\'', '"', 0x34)
        pair('`', '~', 0x35)
        pair(',', '<', 0x36)
        pair('.', '>', 0x37)
        pair('/', '?', 0x38)
        m[' '.code] = KeyStroke(0, 0x2C)
        m['\n'.code] = KeyStroke(0, 0x28)
        m['\r'.code] = KeyStroke(0, 0x28)
        m['\t'.code] = KeyStroke(0, 0x2B)
        m
    }

    /** The key stroke that types [codePoint] on a US layout, or null when there is none. */
    fun strokeForChar(codePoint: Int): KeyStroke? = chars[codePoint]
}
