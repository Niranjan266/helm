package com.niranjan.helm.core

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class Phase3CodecTest {
    private fun obj(s: String) = Json.parseToJsonElement(s).jsonObject
    private val hostId = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f"

    @Test
    fun muxFrameLayoutAndLimits() {
        val f = MuxFrame(MuxFrame.DATA, 0xFFFF_FFFEL, byteArrayOf(9, 8))
        val b = f.encode()
        assertEquals(listOf(2, 0xff, 0xff, 0xff, 0xfe, 9, 8), b.map { it.toInt() and 0xff })
        assertEquals(MuxFrame.Companion.Decoded.Ok(f), MuxFrame.decode(b))
        assertEquals(MuxFrame.Companion.Decoded.TooShort, MuxFrame.decode(byteArrayOf(1, 0, 0, 0)))
        assertEquals(MuxFrame.Companion.Decoded.TooLarge, MuxFrame.decode(ByteArray(5 + 65_537)))
        val open = MuxFrame.decode(MuxFrame(MuxFrame.OPEN, 1).encode()) as MuxFrame.Companion.Decoded.Ok
        assertEquals(0, open.frame.payload.size)
    }

    @Test
    fun tunnelChunksAreAtMost64KiB() {
        val data = ByteArray(65_536 * 2 + 10) { it.toByte() }
        val c = TunnelProtocol.chunks(data)
        assertEquals(listOf(65_536, 65_536, 10), c.map { it.size })
        assertEquals(data.toList(), c.flatMap { it.toList() })
        assertEquals(listOf(3), TunnelProtocol.chunks(data, 5, 3).map { it.size })
        assertTrue(TunnelProtocol.chunks(ByteArray(0)).isEmpty())
    }

    @Test
    fun relayConnectUrls() {
        val u = "/connect/$hostId"
        assertEquals("wss://relay.example.com$u", TunnelProtocol.connectUrl("wss://relay.example.com/", hostId))
        assertEquals("wss://relay.example.com$u", TunnelProtocol.connectUrl("https://relay.example.com", hostId))
        assertEquals("ws://127.0.0.1:8787$u", TunnelProtocol.connectUrl("http://127.0.0.1:8787", hostId))
        assertEquals("wss://r.example$u", TunnelProtocol.connectUrl("r.example", hostId))
        assertEquals("wss://r.example/connect/$hostId", TunnelProtocol.connectUrl("wss://r.example", hostId.uppercase()))
        assertNull(TunnelProtocol.connectUrl("ftp://x", hostId))
        assertNull(TunnelProtocol.connectUrl("", hostId))
        assertNull(TunnelProtocol.connectUrl("wss://x", "not-a-uuid"))
        assertEquals("$hostId.relay.helm.invalid", TunnelProtocol.innerHost(hostId.uppercase()))
        assertTrue(TunnelProtocol.httpReason(404).contains("not connected"))
        assertTrue(TunnelProtocol.closeReason(4410).contains("went away"))
    }

    @Test
    fun savedPcRelayAndPrefsAreBackwardCompatible() {
        val old = SavedPcCodec.decode("""[{"host_id":"$hostId","name":"PC","addresses":["10.0.0.2"],"port":47820,"fingerprint":"${"ab".repeat(32)}"}]""").single()
        assertNull(old.relayUrl)
        assertEquals(PcPrefs(), old.prefs)
        assertNull(EndpointPlanner.relayFor(old))
        val withRelay = old.copy(relayUrl = "wss://relay.example.com", prefs = PcPrefs(useRelay = true, touchpadSpeed = 2f))
        val back = SavedPcCodec.decode(SavedPcCodec.encode(listOf(withRelay))).single()
        assertEquals(withRelay, back)
        assertEquals("wss://relay.example.com/connect/$hostId", EndpointPlanner.relayFor(back)!!.connectUrl)
        assertTrue(EndpointPlanner.relayFor(back)!!.relayed)
        assertNull(EndpointPlanner.relayFor(back.copy(prefs = PcPrefs(useRelay = false))))
    }

    @Test
    fun deckPhase3Actions() {
        val btn = obj(
            """{"buttons":[{"id":"a","label":"M","color":"#000000","state":{"bind":"audio_muted"},"widget":"clock",
               "action":{"macro":[{"hotkey":"ctrl+c"},{"delay_ms":50},{"repeat":{"times":2,"action":{"remote":"obs.scene:Game"}}}]}},
               {"id":"b","label":"W","color":"#000000","widget":{"widget":"ram"},"action":{"remote":"browser.back"}},
               {"id":"c","label":"X","color":"#000000","action":{"repeat":{"times":0,"action":{"text":"x"}}}}]}""",
        )
        val d = DeckCodec.decode(btn)
        assertEquals("audio_muted", d[0].stateBind)
        assertEquals("clock", d[0].widget)
        val m = d[0].action as DeckAction.Macro
        assertEquals(DeckAction.Hotkey("ctrl+c"), m.steps[0])
        assertEquals(DeckAction.Repeat(2, DeckAction.Remote("obs.scene:Game")), m.steps[2])
        assertEquals("ram", d[1].widget)
        assertTrue(d[2].action is DeckAction.Unknown) // times = 0 is invalid -> kept raw
        assertTrue(DeckCodec.needsPhase3(m))
        assertFalse(DeckCodec.needsPhase3(DeckAction.Hotkey("a")))
        assertEquals(d, DeckCodec.decode(DeckCodec.encode(d)))
        val enc = DeckCodec.encode(d)["buttons"].toString()
        assertTrue(enc.contains("\"state\":{\"bind\":\"audio_muted\"}"))
        assertTrue(enc.contains("\"widget\":\"ram\""))
    }

    @Test
    fun macroIsCappedAt50Steps() {
        val steps = List(60) { DeckAction.Delay(1) }
        val enc = DeckCodec.encodeAction(DeckAction.Macro(steps)) as JsonObject
        assertEquals(50, (DeckCodec.decodeAction(enc) as DeckAction.Macro).steps.size)
    }

    @Test
    fun alertRuleCodec() {
        val rules = listOf(AlertRule("1", AlertRule.CPU_ABOVE, 90.0, 30), AlertRule("2", AlertRule.DISK_FREE_BELOW, 5.5, mount = "D:\\"))
        val enc = AlertRule.encode(rules)
        assertEquals("""{"rules":[{"id":"1","kind":"cpu_above","threshold":90.0,"for_s":30},{"id":"2","kind":"disk_free_below","threshold":5.5,"mount":"D:\\"}]}""", enc.toString())
        assertEquals(rules, AlertRule.listFrom(enc))
        assertEquals("CPU above 90% for 30 s", rules[0].describe())
        assertEquals(AlertEvent("x", false, 3.0, null), AlertEvent.from(obj("""{"rule_id":"x","firing":false,"value":3}""")))
    }

    @Test
    fun processSortAndFilter() {
        val ps = ProcessEntry.listFrom(obj("""{"processes":[{"pid":1,"name":"b.exe","cpu_pct":1,"ram_bytes":900},{"pid":2,"name":"A.exe","cpu_pct":5,"ram_bytes":10},{"pid":3,"name":"c.exe"}]}"""))
        assertEquals(listOf(2L, 1L, 3L), ProcessSorter.sort(ps, ProcessSort.CPU).map { it.pid })
        assertEquals(listOf(1L, 2L, 3L), ProcessSorter.sort(ps, ProcessSort.RAM).map { it.pid })
        assertEquals(listOf("A.exe", "b.exe", "c.exe"), ProcessSorter.sort(ps, ProcessSort.NAME).map { it.name })
        assertEquals(listOf(3L), ProcessSorter.sort(ps, ProcessSort.NAME, "3").map { it.pid })
        assertEquals(listOf("b.exe"), ProcessSorter.sort(ps, ProcessSort.NAME, "B.").map { it.name })
    }

    @Test
    fun sessionsProfilesFsAndRelay() {
        val s = AudioSession.listFrom(obj("""{"sessions":[{"id":7,"name":"Game","level":1.4,"muted":true},{"name":"no id"}]}""")).single()
        assertEquals(JsonPrimitive(7), s.id)
        assertEquals(1f, s.level)
        val p = listOf(Profile("a", "A", "x.exe", null, 1))
        assertEquals(p, Profile.listFrom(Profile.encode(p)))
        assertEquals("""{"profiles":[{"id":"a","name":"A","match_exe":"x.exe","dock_page":1}]}""", Profile.encode(p).toString())
        val e = FsEntry.listFrom(obj("""{"entries":[{"name":"z","is_dir":false,"size":3},{"name":"B","is_dir":true}]}"""))
        assertEquals(listOf("B", "z"), FsEntry.sorted(e).map { it.name })
        assertEquals(RelayConfig(true, "wss://r"), RelayConfig.from(obj("""{"enabled":true,"url":"wss://r"}""")))
        assertEquals(RelayConfig(false, null), RelayConfig.from(obj("""{"enabled":false,"url":""}""")))
    }

    @Test
    fun pcPaths() {
        assertEquals("C:\\Users", PcPath.join("C:\\", "Users"))
        assertEquals("C:\\Users\\me", PcPath.join("C:\\Users", "me"))
        assertEquals("C:\\", PcPath.join("", "C:\\"))
        assertEquals("C:\\", PcPath.parent("C:\\Users"))
        assertEquals("C:\\Users", PcPath.parent("C:\\Users\\me\\"))
        assertEquals("", PcPath.parent("C:\\"))
        assertEquals("", PcPath.parent(""))
        assertEquals("me", PcPath.name("C:\\Users\\me"))
        assertTrue(PcPath.validName("notes.txt"))
        for (bad in listOf("", "..", "a\\b", "a/b", "c:", "x?", " ")) assertFalse(bad, PcPath.validName(bad))
    }

    @Test
    fun statsHistoryKeepsTheNewest() {
        val h = StatsHistory(3)
        var last = emptyList<Stats>()
        for (i in 1..5) last = h.add(Stats.from(FakeAgent.statsSample(i)))
        assertEquals(listOf(30f, 40f, 50f), last.map { it.cpuTotal })
        val empty = Stats.from(JsonObject(emptyMap()))
        assertEquals(0f, empty.ramPct)
    }

    @Test
    fun gyroMouse() {
        val g = GyroMouse(pxPerRad = 1000f, deadzone = 0.05f)
        g.add(rateX = 0.01f, rateZ = 0.02f, dtS = 0.01f, speed = 1f) // inside the deadzone
        assertNull(g.drain())
        g.add(rateX = -1f, rateZ = 2f, dtS = 0.01f, speed = 1.5f)
        val f = g.drain()!!
        assertEquals(-30f, f.dx, 0.001f) // turning left (positive yaw) moves the pointer left
        assertEquals(15f, f.dy, 0.001f) // tilting down (negative pitch) moves it down
        assertNull(g.drain())
        g.add(1f, 1f, dtS = 2f, speed = 1f) // stale sample ignored
        assertNull(g.drain())
        assertEquals("""{"type":"in","k":"gy","dx":1.5,"dy":-2.25}""", InputFrame.Gyro(1.5f, -2.25f).toJson().toString())
    }

    @Test
    fun usbTetherDetection() {
        val ifs = listOf(LocalIf("wlan0", "192.168.1.20", 24), LocalIf("rndis0", "192.168.42.129", 24))
        assertTrue(UsbTether.isOverUsb("192.168.42.17", ifs))
        assertFalse(UsbTether.isOverUsb("192.168.1.5", ifs))
        assertFalse(UsbTether.isOverUsb("not-an-ip", ifs))
        assertTrue(UsbTether.isOverUsb("10.1.2.3", listOf(LocalIf("ncm0", "10.1.2.1", 16))))
        assertTrue(UsbTether.isTetherInterface("usb0"))
        assertFalse(UsbTether.isTetherInterface("wlan0"))
        assertNull(UsbTether.ipv4("1.2.3.256"))
    }
}
