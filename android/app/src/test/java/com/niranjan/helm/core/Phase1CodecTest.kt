package com.niranjan.helm.core

import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

private fun obj(text: String) = Json.parseToJsonElement(text) as JsonObject

class InputFrameShapeTest {
    @Test
    fun exactShapes() {
        assertEquals(obj("""{"type":"in","k":"mm","dx":1.5,"dy":-2.0}"""), InputFrame.Move(1.5f, -2f).toJson())
        assertEquals(obj("""{"type":"in","k":"mb","button":"x2","down":true}"""), InputFrame.Button(MouseButton.X2, true).toJson())
        assertEquals(obj("""{"type":"in","k":"ms","dx":0.0,"dy":-0.25}"""), InputFrame.Scroll(0f, -0.25f).toJson())
        assertEquals(obj("""{"type":"in","k":"kd","vk":17}"""), InputFrame.KeyDown(Vk.CONTROL).toJson())
        assertEquals(obj("""{"type":"in","k":"ku","vk":65}"""), InputFrame.KeyUp(0x41).toJson())
        assertEquals(obj("""{"type":"in","k":"tx","text":"hi"}"""), InputFrame.Text("hi").toJson())
        assertEquals(obj("""{"type":"in","k":"sc","keys":[91,68]}"""), InputFrame.Shortcut(listOf(Vk.LWIN, 0x44)).toJson())
    }

    @Test
    fun movesAreRoundedAndNonFiniteBecomesZero() {
        assertEquals(obj("""{"type":"in","k":"mm","dx":0.33,"dy":0.0}"""), InputFrame.Move(1f / 3f, Float.NaN).toJson())
    }

    @Test
    fun clickIsDownThenUp() {
        assertEquals(
            listOf(InputFrame.Button(MouseButton.LEFT, true), InputFrame.Button(MouseButton.LEFT, false)),
            InputFrame.click(),
        )
    }

    @Test
    fun vkHelpers() {
        assertEquals(0x70, Vk.f(1))
        assertEquals(0x7B, Vk.f(12))
        assertEquals(0x41, Vk.forChar('a'))
        assertEquals(0x5A, Vk.forChar('Z'))
        assertEquals(0x35, Vk.forChar('5'))
        assertNull(Vk.forChar('#'))
    }

    @Test
    fun keyPressUsesShortcutWhenModifiersHeld() {
        assertEquals(listOf(InputFrame.KeyDown(Vk.ESCAPE), InputFrame.KeyUp(Vk.ESCAPE)), KeyPress.frames(Vk.ESCAPE, emptyList()))
        assertEquals(listOf(InputFrame.Shortcut(listOf(Vk.CONTROL, Vk.ALT, Vk.DELETE))), KeyPress.frames(Vk.DELETE, listOf(Vk.CONTROL, Vk.ALT)))
    }
}

class BinaryFrameTest {
    private val id = ByteArray(16) { (it * 17).toByte() }

    @Test
    fun encodeLayoutIsIdThenBigEndianOffsetThenData() {
        val enc = BinaryFrame(id, 0x0102030405060708L, byteArrayOf(9, 8, 7)).encode()
        assertEquals(27, enc.size)
        assertArrayEquals(id, enc.copyOfRange(0, 16))
        assertArrayEquals(byteArrayOf(1, 2, 3, 4, 5, 6, 7, 8), enc.copyOfRange(16, 24))
        assertArrayEquals(byteArrayOf(9, 8, 7), enc.copyOfRange(24, 27))
    }

    @Test
    fun decodeRoundTrip() {
        val data = ByteArray(BinaryFrame.MAX_DATA) { it.toByte() }
        val back = BinaryFrame.decode(BinaryFrame(id, 262_144, data).encode())!!
        assertArrayEquals(id, back.transferId)
        assertEquals(262_144L, back.offset)
        assertArrayEquals(data, back.data)
        assertEquals(id.toHex(), back.transferIdHex)
    }

    @Test
    fun decodeRejectsBadFrames() {
        assertNull(BinaryFrame.decode(ByteArray(23)))
        assertNull(BinaryFrame.decode(ByteArray(24 + BinaryFrame.MAX_DATA + 1)))
        val negative = ByteArray(24).also { it[16] = 0x80.toByte() }
        assertNull(BinaryFrame.decode(negative))
        assertEquals(0, BinaryFrame.decode(ByteArray(24))!!.data.size)
    }

    @Test(expected = IllegalArgumentException::class)
    fun tooMuchDataIsRefused() {
        BinaryFrame(id, 0, ByteArray(BinaryFrame.MAX_DATA + 1))
    }

    @Test
    fun hexTransferIds() {
        val hex = "00112233445566778899aabbccddeeff"
        assertEquals(hex, BinaryFrame.idFromHex(hex)!!.toHex())
        assertEquals(hex, BinaryFrame.idFromHex(hex.uppercase())!!.toHex())
        assertNull(BinaryFrame.idFromHex("abc"))
        assertNull(BinaryFrame.idFromHex("zz112233445566778899aabbccddeeff"))
    }
}

class DockDeckCodecTest {
    @Test
    fun dockDecodeFromAgentShape() {
        val r = obj(
            """{"pages":[[{"id":"a","kind":"app","app_id":"Spotify.exe","label":"Spotify","extra":1},
               {"id":"b","kind":"open","target":"C:\\Users"}],[],[{"kind":"app"}]]}""",
        )
        val pages = DockCodec.decode(r)
        assertEquals(3, pages.size)
        assertEquals(DockTile("a", "app", appId = "Spotify.exe", label = "Spotify"), pages[0][0])
        assertEquals(DockTile("b", "open", target = "C:\\Users"), pages[0][1])
        assertTrue("tile without id is dropped", pages[2].isEmpty())
    }

    @Test
    fun dockEncodeOmitsNulls() {
        val enc = DockCodec.encode(listOf(listOf(DockTile("a", "open", target = "https://x.y"))))
        assertEquals(obj("""{"pages":[[{"id":"a","kind":"open","target":"https://x.y"}]]}"""), enc)
        assertEquals(DockCodec.decode(enc), listOf(listOf(DockTile("a", "open", target = "https://x.y"))))
    }

    @Test
    fun dockEditing() {
        fun t(id: String) = DockTile(id, "app", appId = id)
        var pages = emptyList<List<DockTile>>()
        for (i in 1..5) pages = DockCodec.append(pages, t("$i"), perPage = 4)
        assertEquals(listOf(listOf("1", "2", "3", "4"), listOf("5")), pages.map { p -> p.map { it.id } })
        pages = DockCodec.move(pages, "4", +1)
        assertEquals(listOf(listOf("1", "2", "3", "5"), listOf("4")), pages.map { p -> p.map { it.id } })
        pages = DockCodec.move(pages, "1", -1)
        assertEquals("1", pages[0][0].id)
        pages = DockCodec.moveToPage(pages, "2", 1)
        assertEquals(listOf(listOf("1", "3", "5"), listOf("4", "2")), pages.map { p -> p.map { it.id } })
        pages = DockCodec.moveToPage(pages, "3", 2)
        assertEquals(3, pages.size)
        pages = DockCodec.remove(pages, "3")
        assertEquals(listOf(listOf("1", "5"), listOf("4", "2")), pages.map { p -> p.map { it.id } })
    }

    @Test
    fun deckActionsRoundTrip() {
        val actions = listOf(
            """{"hotkey":"ctrl+shift+esc"}""" to DeckAction.Hotkey("ctrl+shift+esc"),
            """{"text":"hello"}""" to DeckAction.Text("hello"),
            """{"launch":"Spotify.exe"}""" to DeckAction.Launch("Spotify.exe"),
            """{"open":"https://example.com"}""" to DeckAction.Open("https://example.com"),
            """{"media":"next"}""" to DeckAction.Media("next"),
            """{"volume_step":-0.05}""" to DeckAction.VolumeStep(-0.05),
            """{"delay_ms":250}""" to DeckAction.Delay(250),
        )
        for ((json, action) in actions) {
            assertEquals(action, DeckCodec.decodeAction(Json.parseToJsonElement(json)))
            assertEquals(Json.parseToJsonElement(json), DeckCodec.encodeAction(action))
        }
    }

    @Test
    fun unknownDeckActionIsPreserved() {
        val raw = Json.parseToJsonElement("""{"macro":[1,2]}""")
        val a = DeckCodec.decodeAction(raw)
        assertTrue(a is DeckAction.Unknown)
        assertEquals(raw, DeckCodec.encodeAction(a))
    }

    @Test
    fun deckDecodeAndEncode() {
        val r = obj("""{"buttons":[{"id":"x","label":"Mute","color":"#112233","icon":"mic","action":{"media":"toggle_play_pause"}},{"label":"no id"}]}""")
        val buttons = DeckCodec.decode(r)
        assertEquals(listOf(DeckButton("x", "Mute", "#112233", "mic", DeckAction.Media("toggle_play_pause"))), buttons)
        assertEquals(obj("""{"buttons":[{"id":"x","label":"Mute","color":"#112233","icon":"mic","action":{"media":"toggle_play_pause"}}]}"""), DeckCodec.encode(buttons))
    }
}

class ResultParsingTest {
    @Test
    fun appsWindowsAudioDisplaysPowerMonitors() {
        val apps = AppEntry.listFrom(obj("""{"apps":[{"app_id":"a!b","name":"Calc","kind":"packaged"},{"name":"x"}]}"""))
        assertEquals(listOf(AppEntry("a!b", "Calc", "packaged")), apps)

        val wins = WindowEntry.listFrom(obj("""{"windows":[{"hwnd":1234,"title":"T","pid":9,"app_id":"a!b","is_minimized":false,"is_foreground":true}]}"""))
        assertEquals(JsonPrimitive(1234), wins[0].hwnd)
        assertTrue(wins[0].isForeground)

        assertEquals(AudioState(0.5f, true), AudioState.from(obj("""{"level":0.5,"muted":true}""")))
        assertNull(AudioState.from(obj("""{}""")))

        val d = DisplayBrightness.listFrom(obj("""{"displays":[{"id":"wmi:1","name":"Built-in","kind":"internal","level":70,"supported":true}]}"""))
        assertEquals(DisplayBrightness("wmi:1", "Built-in", "internal", 70, true), d[0])

        assertEquals(PowerSchedule("shutdown", 99L), PowerSchedule.fromHolder(obj("""{"scheduled":{"action":"shutdown","due_unix_ms":99,"remaining_ms":5}}""")))
        assertNull(PowerSchedule.fromHolder(obj("""{"scheduled":null}""")))

        val m = MonitorInfo.listFrom(obj("""{"monitors":[{"id":0,"name":"\\\\.\\DISPLAY1","x":0,"y":0,"width":1920,"height":1080,"is_primary":true}]}"""))
        assertEquals(1920, m[0].width)
        assertEquals(JsonPrimitive(0), m[0].id)

        assertNull(MediaState.fromHolder(obj("""{"state":null}""")))
        val st = MediaState.fromHolder(obj("""{"state":{"title":"A","artist":"B","status":"paused","position_ms":1000,"duration_ms":2000,"has_artwork":true,"can_seek":true}}"""))!!
        assertFalse(st.isPlaying)
        assertEquals(2000L, st.durationMs)
    }
}

class WakeOnLanTest {
    @Test
    fun parseMacFormats() {
        val expected = byteArrayOf(0xAA.toByte(), 0xBB.toByte(), 0xCC.toByte(), 0x01, 0x02, 0x03)
        assertArrayEquals(expected, WakeOnLan.parseMac("AA:BB:CC:01:02:03"))
        assertArrayEquals(expected, WakeOnLan.parseMac("aa-bb-cc-01-02-03"))
        assertArrayEquals(expected, WakeOnLan.parseMac("aabbcc010203"))
        assertNull(WakeOnLan.parseMac("aa:bb:cc"))
        assertNull(WakeOnLan.parseMac("gg:bb:cc:01:02:03"))
    }

    @Test
    fun magicPacketLayout() {
        val mac = WakeOnLan.parseMac("01:23:45:67:89:AB")!!
        val p = WakeOnLan.magicPacket(mac)
        assertEquals(102, p.size)
        for (i in 0 until 6) assertEquals(0xFF.toByte(), p[i])
        for (r in 0 until 16) assertArrayEquals(mac, p.copyOfRange(6 + r * 6, 12 + r * 6))
    }

    @Test
    fun prefersBurnedInMacs() {
        val chosen = WakeOnLan.chooseMacs(
            listOf(
                MacEntry("02:11:22:33:44:55", "Wi-Fi (random)", true),
                MacEntry("bad", "x", false),
                MacEntry("d8-bb-c1-00-11-22", "Ethernet", false),
                MacEntry("06:00:00:00:00:01", "VPN, flag missing", false),
                MacEntry("D8:BB:C1:00:11:22", "dup", false),
            ),
        )
        assertEquals(listOf("D8:BB:C1:00:11:22", "02:11:22:33:44:55", "06:00:00:00:00:01"), chosen)
    }

    @Test
    fun broadcastTargets() {
        val t = WakeOnLan.broadcastTargets(listOf("192.168.1.42", "10.0.0.7", "fe80::1", "not-an-ip"))
        assertEquals(listOf("255.255.255.255", "192.168.1.255", "10.0.0.255"), t.map { it.hostAddress })
    }

    @Test
    fun sendsMagicPacketOverUdp() {
        DatagramSocket(0, InetAddress.getByName("127.0.0.1")).use { rx ->
            rx.soTimeout = 3_000
            val sent = WakeOnLan.send(listOf("01:23:45:67:89:AB"), listOf(InetAddress.getByName("127.0.0.1")), rx.localPort)
            assertEquals(1, sent)
            val buf = ByteArray(200)
            val pkt = DatagramPacket(buf, buf.size)
            rx.receive(pkt)
            assertEquals(102, pkt.length)
            assertArrayEquals(WakeOnLan.magicPacket(WakeOnLan.parseMac("01:23:45:67:89:AB")!!), buf.copyOf(102))
        }
    }
}

class TouchpadMathTest {
    @Test
    fun slowMotionIsNearlyOneToOneAndFastIsAmplified() {
        val (sx, _) = TouchpadMath.accelerate(2f, 0f, 16f, 1f)
        assertEquals(2f, sx, 0.001f)
        val (fx, fy) = TouchpadMath.accelerate(40f, -30f, 16f, 1f)
        assertTrue(fx > 40f && fx <= 120f)
        assertTrue("direction kept", fy < 0f && kotlin.math.abs(fy / fx + 0.75f) < 0.001f)
        val (hx, _) = TouchpadMath.accelerate(2f, 0f, 16f, 2f)
        assertEquals(4f, hx, 0.001f)
        assertEquals(0f to 0f, TouchpadMath.accelerate(Float.NaN, 1f, 16f, 1f))
    }

    @Test
    fun batcherCombinesMovesPerFrame() {
        val b = MotionBatcher()
        assertNull(b.drain())
        b.add(1f, 2f)
        b.add(0.5f, -1f)
        assertEquals(InputFrame.Move(1.5f, 1f), b.drain())
        assertNull(b.drain())
        val s = ScrollBatcher()
        s.add(0f, 0.3f)
        s.add(0f, 0.3f)
        assertEquals(0.6f, s.drain()!!.dy, 0.0001f)
    }

    @Test
    fun textDiff() {
        assertEquals(listOf(InputFrame.Text("abc")), TextDiff.frames("", "abc"))
        assertEquals(listOf(InputFrame.Text("d")), TextDiff.frames("abc", "abcd"))
        assertEquals(
            listOf(InputFrame.KeyDown(Vk.BACK), InputFrame.KeyUp(Vk.BACK)),
            TextDiff.frames("abc", "ab"),
        )
        // "helo" -> "hello": autocorrect replaced the tail "o" with "lo".
        assertEquals(
            listOf(InputFrame.KeyDown(Vk.BACK), InputFrame.KeyUp(Vk.BACK), InputFrame.Text("lo")),
            TextDiff.frames("helo", "hello"),
        )
        // An emoji is one Backspace.
        assertEquals(listOf(InputFrame.KeyDown(Vk.BACK), InputFrame.KeyUp(Vk.BACK)), TextDiff.frames("a😀", "a"))
        assertTrue(TextDiff.frames("same", "same").isEmpty())
    }
}
