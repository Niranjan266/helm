package com.niranjan.helm.core

import org.junit.Assert.assertArrayEquals
import org.junit.Test

private fun <T> eq(expected: T, actual: T, msg: String = "") = org.junit.Assert.assertEquals(msg, expected, actual)

private fun ok(cond: Boolean, msg: String = "") = org.junit.Assert.assertTrue(msg, cond)

class BtHidTest {
    // ---- Descriptor -------------------------------------------------------------------

    private data class Item(val tag: Int, val type: Int, val value: Int, val size: Int)

    private fun parse(d: ByteArray): List<Item> {
        val out = ArrayList<Item>()
        var i = 0
        while (i < d.size) {
            val prefix = d[i].toInt() and 0xFF
            val size = if ((prefix and 3) == 3) 4 else prefix and 3
            val tag = prefix shr 4
            val type = (prefix shr 2) and 3
            var v = 0
            for (k in 0 until size) v = v or ((d[i + 1 + k].toInt() and 0xFF) shl (8 * k))
            out += Item(tag, type, v, size)
            i += 1 + size
        }
        eq(d.size, i, "no truncated item at the end")
        return out
    }

    /** Input / output report sizes in bits per report id (Report Size x Report Count at each main item). */
    private fun reportBits(d: ByteArray): Map<Int, Pair<Int, Int>> {
        var id = 0
        var size = 0
        var count = 0
        val inBits = HashMap<Int, Int>()
        val outBits = HashMap<Int, Int>()
        for (item in parse(d)) {
            when {
                item.type == 1 && item.tag == 0x8 -> id = item.value
                item.type == 1 && item.tag == 0x7 -> size = item.value
                item.type == 1 && item.tag == 0x9 -> count = item.value
                item.type == 0 && item.tag == 0x8 -> inBits[id] = (inBits[id] ?: 0) + size * count
                item.type == 0 && item.tag == 0x9 -> outBits[id] = (outBits[id] ?: 0) + size * count
            }
        }
        return (inBits.keys + outBits.keys).associateWith { (inBits[it] ?: 0) to (outBits[it] ?: 0) }
    }

    @Test
    fun descriptorIsWellFormedAndBalanced() {
        val items = parse(HidDescriptor.bytes)
        var depth = 0
        for (item in items) {
            if (item.type == 0 && item.tag == 0xA) depth++
            if (item.type == 0 && item.tag == 0xC) depth--
            ok(depth >= 0)
        }
        eq(0, depth, "collections balanced")
        eq(listOf(1, 2, 3), items.filter { it.type == 1 && it.tag == 0x8 }.map { it.value })
    }

    @Test
    fun reportSizesMatchTheBuilders() {
        val bits = reportBits(HidDescriptor.bytes)
        eq(MouseReport.SIZE * 8, bits[HidIds.MOUSE]!!.first)
        eq(KeyboardReport.SIZE * 8, bits[HidIds.KEYBOARD]!!.first)
        eq(8, bits[HidIds.KEYBOARD]!!.second, "5 LED bits + 3 padding")
        eq(ConsumerReport.SIZE * 8, bits[HidIds.CONSUMER]!!.first)
    }

    @Test
    fun descriptorDeclaresFiveButtonsWideAxesAndConsumerUsages() {
        val items = parse(HidDescriptor.bytes)
        ok(items.any { it.type == 1 && it.tag == 0x1 && it.size == 2 && (it.value and 0xFFFF) == 0x8001 }, "logical min -32767")
        ok(items.any { it.type == 1 && it.tag == 0x2 && it.size == 2 && it.value == 0x7FFF }, "logical max 32767")
        ok(items.any { it.type == 2 && it.tag == 0x2 && it.value == 5 }, "usage max 5 = five buttons")
        val usages = items.filter { it.type == 2 && it.tag == 0x0 }.map { it.value }
        for (u in listOf(0xCD, 0xB5, 0xB6, 0xE9, 0xEA, 0xE2)) ok(u in usages, "usage 0x${u.toString(16)}")
        ok(items.any { it.type == 2 && it.tag == 0x0 && it.value == 0x0238 }, "AC Pan")
    }

    // ---- Report builders --------------------------------------------------------------

    @Test
    fun mouseReportLayoutAndClamping() {
        assertArrayEquals(
            byteArrayOf(0x05, 0x0A, 0x00, 0xF6.toByte(), 0xFF.toByte(), 0x03, 0xFD.toByte()),
            MouseReport.build(0x05, 10, -10, 3, -3),
        )
        val big = MouseReport.build(0xFF, 99999, -99999, 500, -500)
        assertArrayEquals(byteArrayOf(0x1F, 0xFF.toByte(), 0x7F, 0x01, 0x80.toByte(), 127, -127), big)
        eq(MouseReport.SIZE, big.size)
    }

    @Test
    fun keyboardReportHasModifiersReservedAndSixKeys() {
        val r = KeyboardReport.build(HidKeys.MOD_LCTRL or HidKeys.MOD_LSHIFT, listOf(4, 5, 6, 7, 8, 9, 10, 11))
        assertArrayEquals(byteArrayOf(0x03, 0, 4, 5, 6, 7, 8, 9), r)
        assertArrayEquals(ByteArray(8), KeyboardReport.build(0, emptyList()))
    }

    @Test
    fun consumerReportBits() {
        assertArrayEquals(byteArrayOf(0x01), ConsumerReport.build(ConsumerKey.PLAY_PAUSE.bit))
        assertArrayEquals(byteArrayOf(0x18), ConsumerReport.build(ConsumerKey.VOLUME_UP.bit or ConsumerKey.VOLUME_DOWN.bit))
        eq(6, ConsumerKey.entries.map { it.bit }.toSet().size)
    }

    @Test
    fun deltaAccumulatorClampsAndCarries() {
        val a = DeltaAccumulator(127)
        a.add(0.4f, 0f)
        eq(null, a.drain(), "under half a count is held")
        a.add(0.4f, 0f)
        eq(1 to 0, a.drain(), "held fraction is not lost")
        a.add(100f, -100f)
        eq(100 to -100, a.drain())
        a.add(300f, -300f)
        eq(127 to -127, a.drain(), "one report never exceeds the range")
        eq(127 to -127, a.drain(), "the rest comes in the next report")
        a.add(Float.NaN, 1f)
        eq(46 to -46, a.drain(), "non-finite input is ignored; the leftover 46 comes out")
        a.clear()
        a.add(1e9f, 0f)
        var total = 0
        while (true) total += (a.drain() ?: break).first
        ok(total <= 127 * 4, "stored remainder is capped, got $total")
    }

    // ---- Keys -------------------------------------------------------------------------

    @Test
    fun vkToUsage() {
        eq(0x04, HidKeys.usageForVk('A'.code))
        eq(0x1D, HidKeys.usageForVk('Z'.code))
        eq(0x1E, HidKeys.usageForVk('1'.code))
        eq(0x27, HidKeys.usageForVk('0'.code))
        eq(0x28, HidKeys.usageForVk(Vk.RETURN))
        eq(0x3A, HidKeys.usageForVk(Vk.f(1)))
        eq(0x45, HidKeys.usageForVk(Vk.f(12)))
        eq(0x52, HidKeys.usageForVk(Vk.UP))
        eq(0x4F, HidKeys.usageForVk(Vk.RIGHT))
        eq(0x35, HidKeys.usageForVk(KbdVk.OEM_3))
        eq(null, HidKeys.usageForVk(0xFF))
        eq(HidKeys.MOD_LGUI, HidKeys.modifierBit(Vk.LWIN))
        eq(HidKeys.MOD_LCTRL, HidKeys.modifierBit(Vk.CONTROL))
        eq(0, HidKeys.modifierBit('A'.code))
    }

    @Test
    fun everyKeyOfTheFloatingKeyboardMaps() {
        for (key in PcKeyboard.rows.flatten()) {
            ok(HidKeys.modifierBit(key.vk) != 0 || HidKeys.usageForVk(key.vk) != null, "no mapping for ${key.label}")
        }
    }

    @Test
    fun printableAsciiMapsAndAgreesWithTheVkTable() {
        for (cp in 0x20..0x7E) ok(HidKeys.strokeForChar(cp) != null, "char ${cp.toChar()}")
        eq(KeyStroke(0, 0x04), HidKeys.strokeForChar('a'.code))
        eq(KeyStroke(HidKeys.MOD_LSHIFT, 0x04), HidKeys.strokeForChar('A'.code))
        eq(KeyStroke(HidKeys.MOD_LSHIFT, 0x1E), HidKeys.strokeForChar('!'.code))
        eq(KeyStroke(HidKeys.MOD_LSHIFT, 0x34), HidKeys.strokeForChar('"'.code))
        eq(KeyStroke(0, 0x2C), HidKeys.strokeForChar(' '.code))
        eq(KeyStroke(0, 0x28), HidKeys.strokeForChar('\n'.code))
        eq(null, HidKeys.strokeForChar('é'.code))
        eq(null, HidKeys.strokeForChar(0x1F600))
        for (c in ('a'..'z') + ('0'..'9')) {
            eq(HidKeys.usageForVk(Vk.forChar(c)!!), HidKeys.strokeForChar(c.code)!!.usage, "char $c")
        }
    }

    // ---- Bridge -----------------------------------------------------------------------

    private class Recorder : HidReportSink {
        val sent = ArrayList<Pair<Int, List<Int>>>()
        override fun send(reportId: Int, data: ByteArray) {
            sent += reportId to data.map { it.toInt() and 0xFF }
        }

        fun ofId(id: Int) = sent.filter { it.first == id }.map { it.second }
    }

    private fun bridge(rec: Recorder, clock: () -> Long = { 0L }) = BtInputBridge(rec, clock, minIntervalMs = 8)

    @Test
    fun moveIsRateLimitedAndAccumulated() {
        val rec = Recorder()
        var now = 0L
        val b = bridge(rec) { now }
        b.touchMove(3f, 1f)
        b.flushInput()
        eq(1, rec.ofId(HidIds.MOUSE).size)
        now = 3
        b.touchMove(2f, 2f)
        b.flushInput() // too soon: held
        eq(1, rec.ofId(HidIds.MOUSE).size)
        b.touchMove(1f, 0f)
        now = 20
        b.flushInput()
        val r = rec.ofId(HidIds.MOUSE).last()
        eq(3, r[1], "2 + 1 accumulated")
        eq(2, r[3])
    }

    @Test
    fun hugeMoveIsClampedPerReportAndKeepsPressedButtons() {
        val rec = Recorder()
        var now = 0L
        val b = bridge(rec) { now }
        b.sendAll(listOf(InputFrame.Button(MouseButton.LEFT, true)))
        b.touchMove(50000f, 0f)
        b.flushInput()
        val r = rec.ofId(HidIds.MOUSE).last()
        eq(1, r[0])
        eq(0xFF, r[1])
        eq(0x7F, r[2])
        now = 10
        b.flushInput()
        val second = rec.ofId(HidIds.MOUSE).last()
        eq(1, second[0])
        ok(second[1] != 0 || second[2] != 0, "leftover goes out in a second report")
    }

    @Test
    fun scrollMapsToWheelAndPan() {
        val rec = Recorder()
        val b = bridge(rec)
        b.touchScroll(2f, -3f)
        b.flushInput()
        val r = rec.ofId(HidIds.MOUSE).single()
        eq(0xFD, r[5], "wheel -3")
        eq(2, r[6], "pan +2")
    }

    @Test
    fun clickDownThenUpAndDragLock() {
        val rec = Recorder()
        val b = bridge(rec)
        b.click(MouseButton.RIGHT)
        eq(listOf(2, 0), rec.ofId(HidIds.MOUSE).map { it[0] })
        rec.sent.clear()
        ok(b.setDragLock(true))
        eq(1, rec.ofId(HidIds.MOUSE).last()[0])
        b.click(MouseButton.LEFT)
        ok(!b.isDragLocked, "a left click while locked releases the lock")
        eq(0, rec.ofId(HidIds.MOUSE).last()[0])
    }

    @Test
    fun shortcutPressesInOrderAndReleasesInReverse() {
        val rec = Recorder()
        val b = bridge(rec)
        b.sendAll(listOf(InputFrame.Shortcut(listOf(Vk.LWIN, Vk.TAB))))
        val k = rec.ofId(HidIds.KEYBOARD)
        eq(listOf(8, 0, 0, 0, 0, 0, 0, 0), k[0])
        eq(listOf(8, 0, 0x2B, 0, 0, 0, 0, 0), k[1])
        eq(listOf(8, 0, 0, 0, 0, 0, 0, 0), k[2])
        eq(List(8) { 0 }, k[3])
    }

    @Test
    fun textTypesStrokesAndSkipsUnmapped() {
        val rec = Recorder()
        val b = bridge(rec)
        b.sendAll(listOf(InputFrame.Text("Hi é😀!")))
        val k = rec.ofId(HidIds.KEYBOARD)
        eq(8, k.size, "H, i, space and ! as press+release; the accent and emoji are skipped")
    }

    @Test
    fun textKeepsSpaceAndShift() {
        val rec = Recorder()
        val b = bridge(rec)
        b.typeText("H!")
        val k = rec.ofId(HidIds.KEYBOARD)
        eq(listOf(2, 0, 0x0B, 0, 0, 0, 0, 0), k[0])
        eq(List(8) { 0 }, k[1])
        eq(listOf(2, 0, 0x1E, 0, 0, 0, 0, 0), k[2])
        eq(List(8) { 0 }, k[3])
    }

    @Test
    fun skippedCharactersAreCounted() {
        val rec = Recorder()
        val b = bridge(rec)
        b.typeText("aé😀b")
        eq(2, b.skippedChars, "the accent and the emoji (one code point)")
        eq(4, rec.ofId(HidIds.KEYBOARD).size, "only a and b were typed")
        b.clearSkipped()
        eq(0, b.skippedChars)
    }

    @Test
    fun repeatedCharactersAreReleasedBetween() {
        val rec = Recorder()
        val b = bridge(rec)
        b.typeText("ll")
        eq(4, rec.ofId(HidIds.KEYBOARD).size)
    }

    @Test
    fun mediaKeyIsPressedAndReleased() {
        val rec = Recorder()
        val b = bridge(rec)
        b.pressConsumer(ConsumerKey.MUTE)
        eq(listOf(listOf(0x20), listOf(0)), rec.ofId(HidIds.CONSUMER))
    }

    @Test
    fun releaseAllLetsGoOfEverything() {
        val rec = Recorder()
        val b = bridge(rec)
        b.sendAll(listOf(InputFrame.KeyDown(Vk.CONTROL), InputFrame.KeyDown('C'.code), InputFrame.Button(MouseButton.LEFT, true)))
        b.pinch(true)
        b.touchMove(40f, 40f)
        rec.sent.clear()
        b.releaseAll()
        eq(listOf(List(7) { 0 }), rec.ofId(HidIds.MOUSE))
        eq(listOf(List(8) { 0 }), rec.ofId(HidIds.KEYBOARD))
        eq(listOf(listOf(0)), rec.ofId(HidIds.CONSUMER))
        rec.sent.clear()
        b.flushInput()
        ok(rec.sent.isEmpty(), "pending motion was dropped")
        b.sendAll(listOf(InputFrame.KeyDown('X'.code)))
        eq(listOf(0, 0, 0x1B, 0, 0, 0, 0, 0), rec.ofId(HidIds.KEYBOARD).last(), "nothing stays held")
    }

    @Test
    fun pinchHoldsControl() {
        val rec = Recorder()
        val b = bridge(rec)
        b.pinch(true)
        eq(1, rec.ofId(HidIds.KEYBOARD).last()[0])
        b.pinch(false)
        eq(0, rec.ofId(HidIds.KEYBOARD).last()[0])
    }

    @Test
    fun sevenKeysAtOnceStopAtRollover() {
        val rec = Recorder()
        val b = bridge(rec)
        for (c in "ABCDEFG") b.sendAll(listOf(InputFrame.KeyDown(c.code)))
        eq(listOf(0, 0, 4, 5, 6, 7, 8, 9), rec.ofId(HidIds.KEYBOARD).last())
    }

    // ---- Support, permissions, hosts --------------------------------------------------

    @Test
    fun supportChecks() {
        eq(BtSupport.API_TOO_OLD, BtSupport.evaluate(27, true, true))
        eq(BtSupport.NO_ADAPTER, BtSupport.evaluate(34, false, true))
        eq(BtSupport.NO_HID_PROFILE, BtSupport.evaluate(34, true, false))
        eq(BtSupport.OK, BtSupport.evaluate(28, true, null))
        eq(BtSupport.OK, BtSupport.evaluate(34, true, true))
        eq(null, BtSupport.OK.message)
        ok(BtSupport.NO_HID_PROFILE.message != null)
    }

    @Test
    fun permissionsDependOnAndroidVersion() {
        eq(emptyList<String>(), BtPermissions.required(30))
        eq(listOf(BtPermissions.CONNECT, BtPermissions.ADVERTISE), BtPermissions.required(31))
        eq(listOf(BtPermissions.ADVERTISE), BtPermissions.missing(33) { it == BtPermissions.CONNECT })
        ok(BtPermissions.missing(29) { false }.isEmpty())
    }

    @Test
    fun lastHostComesFirst() {
        val a = BtHost("AA:00", "Zed PC")
        val b = BtHost("BB:00", "Alpha PC")
        val c = BtHost("CC:00", "Mid PC")
        eq(listOf(b, c, a), BtHosts.order(listOf(a, b, c), null))
        eq(listOf(c, b, a), BtHosts.order(listOf(a, b, c), "cc:00"))
        eq(listOf(a), BtHosts.order(listOf(a, a.copy(name = "dup")), null))
    }

    @Test
    fun statusText() {
        eq("Not connected", BtHidState().statusText())
        eq("Connected to Desk", BtHidState(link = BtLink.CONNECTED, host = BtHost("x", "Desk")).statusText())
        ok(BtHidState(link = BtLink.CONNECTING, host = BtHost("x", "Desk")).statusText().startsWith("Connecting"))
    }
}
