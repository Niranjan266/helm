package com.niranjan.helm.core

import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** G3 (Docs/protocol/streaming-2.md): codecs, drag-select machine, second-screen maths, wiring. */
class G3Test {
    // ---- pure ------------------------------------------------------------------------------

    @Test
    fun gamingPresetParams() {
        val p = GamingPreset.G720_120.toParams(monitorId = JsonPrimitive("\\\\.\\DISPLAY2"), hwnd = null)
        assertEquals("gaming", p.s("preset"))
        assertEquals(1280, p.i("max_width"))
        assertEquals(720, p.i("max_height"))
        assertEquals(120, p.i("fps"))
        assertEquals(20_000, p.i("bitrate_kbps"))
        assertEquals(true, (p["audio"] as JsonPrimitive).booleanOrNull)
        assertEquals("\\\\.\\DISPLAY2", p.s("monitor_id"))
        // A window wins over a monitor; StreamRequest delegates to the gaming params.
        val r = StreamRequest(StreamPreset.P720_30, audio = false, monitorId = JsonPrimitive("m"), hwnd = JsonPrimitive(42), gaming = GamingPreset.G1080_60).toParams()
        assertEquals(42, r.i("hwnd"))
        assertNull(r["monitor_id"])
        assertEquals(1920, r.i("max_width"))
        assertEquals(true, (r["audio"] as JsonPrimitive).booleanOrNull)
        assertEquals(GamingPreset.G720_60, GamingPreset.byName("G720_60"))
        assertNull(GamingPreset.byName("nope"))
        assertTrue(GamingPreset.entries.all { it.fps in 60..120 })
    }

    @Test
    fun plainRequestUnchangedAndMaxSizeCaps() {
        val plain = StreamRequest(StreamPreset.P1080_60).toParams()
        assertNull(plain["preset"])
        assertEquals(1920, plain.i("max_width"))
        val capped = StreamRequest(StreamPreset.P1080_60, maxSize = 1080 to 2400).toParams()
        assertEquals(1080, capped.i("max_width"))
        assertEquals(1080, capped.i("max_height")) // never above the preset
        assertEquals(64, StreamRequest(StreamPreset.P720_30, maxSize = 1 to 1).toParams().i("max_width"))
    }

    @Test
    fun startInfoG3Fields() {
        val old = StreamStartInfo.from(buildJsonObject { put("token", "t") })!!
        assertEquals("default", old.preset)
        assertFalse(old.inputOnStream)
        val g = StreamStartInfo.from(buildJsonObject { put("token", "t"); put("preset", "gaming"); put("input_on_stream", true) })!!
        assertEquals("gaming", g.preset)
        assertTrue(g.inputOnStream)
    }

    @Test
    fun thumbDecodingAndPickableOrder() {
        assertArrayEquals(byteArrayOf(1, 2, 3), WindowThumb.pngOf(buildJsonObject { put("png", "AQID") }))
        assertNull(WindowThumb.pngOf(buildJsonObject { put("png", "%%") }))
        assertNull(WindowThumb.pngOf(buildJsonObject { put("png", kotlinx.serialization.json.JsonNull) }))
        fun w(h: Int, t: String, fg: Boolean = false, min: Boolean = false) = WindowEntry(JsonPrimitive(h), t, null, null, min, fg)
        val list = WindowThumb.pickable(listOf(w(1, "zeta"), w(2, ""), w(3, "Alpha", min = true), w(4, "beta"), w(5, "Game", fg = true)))
        assertEquals(listOf("Game", "beta", "zeta", "Alpha"), list.map { it.title })
    }

    private fun DragSelect.all(vararg steps: () -> List<DragAction>): List<DragAction> = steps.flatMap { it() }

    @Test
    fun dragSelectLongPressThenDragHoldsLeftButton() {
        val d = DragSelect(direct = true, slopPx = 10f, longPressMs = 450)
        assertEquals(emptyList<DragAction>(), d.down(100f, 100f, 0))
        assertTrue(d.pending)
        assertEquals(450L, d.deadline)
        assertEquals(emptyList<DragAction>(), d.tick(200)) // too early
        assertEquals(listOf(DragAction.PointAt(100f, 100f), DragAction.Press(MouseButton.LEFT), DragAction.Haptic), d.tick(460))
        assertTrue(d.holding)
        assertEquals(listOf(DragAction.PointAt(150f, 120f)), d.move(150f, 120f, 500))
        assertEquals(emptyList<DragAction>(), d.pointers(2, 5f, 5f)) // extra finger while holding: ignored
        assertEquals(listOf(DragAction.PointAt(150f, 120f), DragAction.Release(MouseButton.LEFT)), d.up(700))
        assertFalse(d.holding)
    }

    @Test
    fun dragSelectLateMoveStillPressesFirst() {
        // The timer did not fire before the finger moved: the press comes first, then the move.
        val d = DragSelect(direct = true, slopPx = 10f, longPressMs = 450)
        d.down(10f, 10f, 0)
        assertEquals(
            listOf(DragAction.PointAt(10f, 10f), DragAction.Press(MouseButton.LEFT), DragAction.Haptic, DragAction.PointAt(40f, 10f)),
            d.move(40f, 10f, 500),
        )
    }

    @Test
    fun dragSelectTapsPanAndTrackpad() {
        val d = DragSelect(direct = true, slopPx = 10f)
        d.down(5f, 6f, 0)
        assertEquals(emptyList<DragAction>(), d.move(7f, 6f, 50)) // within slop
        assertEquals(listOf(DragAction.PointAt(5f, 6f), DragAction.Click(MouseButton.LEFT)), d.up(120))
        // Quick drag before the long press = pan, never a button.
        d.down(0f, 0f, 0)
        assertEquals(listOf(DragAction.Pan(20f, 0f)), d.move(20f, 0f, 30))
        assertEquals(listOf(DragAction.Pan(5f, 5f)), d.move(25f, 5f, 40))
        assertEquals(emptyList<DragAction>(), d.tick(1000))
        assertEquals(emptyList<DragAction>(), d.up(1100))
        // Two-finger tap = right click; two-finger drag = TwoFinger.
        d.down(50f, 50f, 0)
        d.pointers(2)
        assertEquals(listOf(DragAction.PointAt(50f, 50f), DragAction.Haptic, DragAction.Click(MouseButton.RIGHT)), d.up(150))
        d.down(50f, 50f, 0)
        d.pointers(2)
        assertEquals(emptyList<DragAction>(), d.pointers(2, 4f, 0f))
        assertEquals(listOf(DragAction.TwoFinger(20f, 0f)), d.pointers(2, 20f, 0f))
        assertEquals(emptyList<DragAction>(), d.up(400))
        // Trackpad: relative moves, no PointAt anywhere.
        val t = DragSelect(direct = false, slopPx = 10f, longPressMs = 450)
        t.down(0f, 0f, 0)
        assertEquals(listOf(DragAction.Press(MouseButton.LEFT), DragAction.Haptic), t.tick(450))
        assertEquals(listOf(DragAction.MoveBy(3f, -2f)), t.move(3f, -2f, 470))
        assertEquals(listOf(DragAction.Release(MouseButton.LEFT)), t.up(600))
        t.down(0f, 0f, 0)
        assertEquals(listOf(DragAction.MoveBy(15f, 0f)), t.move(15f, 0f, 20))
        assertEquals(emptyList<DragAction>(), t.up(60))
        // Cancel while holding releases the button.
        t.down(0f, 0f, 0)
        t.tick(500)
        assertEquals(listOf(DragAction.Release(MouseButton.LEFT)), t.cancel())
        assertEquals(emptyList<DragAction>(), t.cancel())
    }

    @Test
    fun secondScreenOrientationAndSize() {
        assertEquals(Orientation.UNSPECIFIED, SecondScreen.orientation(RotationLock.AUTO, 1920, 1080))
        assertEquals(Orientation.LANDSCAPE, SecondScreen.orientation(RotationLock.MATCH_MONITOR, 1920, 1080))
        assertEquals(Orientation.PORTRAIT, SecondScreen.orientation(RotationLock.MATCH_MONITOR, 1080, 1920))
        assertEquals(Orientation.UNSPECIFIED, SecondScreen.orientation(RotationLock.MATCH_MONITOR, 0, 0))
        assertEquals(Orientation.PORTRAIT, SecondScreen.orientation(RotationLock.PORTRAIT, 1920, 1080))
        assertEquals(Orientation.LANDSCAPE, SecondScreen.orientation(RotationLock.LANDSCAPE, 1080, 1920))
        // 1440p monitor on a 1080x2400 phone (portrait): landscape 2400x1080 fits, capped at the monitor.
        assertEquals(2400 to 1080, SecondScreen.mirrorSize(2560, 1440, 1080, 2400))
        assertEquals(1080 to 1920, SecondScreen.mirrorSize(1080, 1920, 2400, 1080)) // portrait monitor
        assertEquals(1280 to 720, SecondScreen.mirrorSize(1280, 720, 1080, 2400))
        assertEquals(64 to 64, SecondScreen.mirrorSize(10, 10, 1080, 2400))
        assertEquals(1078 to 600, SecondScreen.mirrorSize(1079, 601, 1080, 2400)) // even sizes
        val m1 = MonitorInfo(JsonPrimitive("a"), "Main", 1920, 1080, true)
        val m2 = MonitorInfo(JsonPrimitive("b"), "Side", 1080, 1920, false)
        assertEquals(m2, SecondScreen.defaultMonitor(listOf(m1, m2)))
        assertEquals(m1, SecondScreen.defaultMonitor(listOf(m1)))
        assertNull(SecondScreen.defaultMonitor(emptyList()))
        assertEquals("Monitor 2 · Side 1080×1920", SecondScreen.label(1, m2))
    }

    // ---- against the FakeAgent ------------------------------------------------------------

    private lateinit var server: MockWebServer
    private lateinit var agent: FakeAgent
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())
    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder().connectTimeout(4, TimeUnit.SECONDS).build()
    }

    private fun startAgent(features: List<String>) {
        agent = FakeAgent(features = features)
        server = MockWebServer()
        server.dispatcher = agent.dispatcher()
        server.start(InetAddress.getByName("127.0.0.1"), 0)
    }

    @After
    fun tearDown() {
        if (::server.isInitialized) server.shutdown()
    }

    private suspend fun control(): HelmConnection = HelmConnection(
        hostId = agent.hostId, fingerprint = "ab".repeat(32), endpoints = listOf(Endpoint("127.0.0.1", server.port)),
        identity = identity, mode = HandshakeMode.Pair(agent.validToken), transport = plain,
        helloTimeoutMs = 5_000, readyTimeoutMs = 5_000, defaultRequestTimeoutMs = 5_000,
    ).also { it.connect() }

    private fun streamSocket(onBinary: (ByteArray) -> Unit) = HelmConnection(
        hostId = agent.hostId, fingerprint = "ab".repeat(32), endpoints = listOf(Endpoint("127.0.0.1", server.port)),
        identity = identity, mode = HandshakeMode.Auth, transport = plain,
        helloTimeoutMs = 5_000, readyTimeoutMs = 5_000, path = Endpoint.STREAM_PATH, binarySink = onBinary,
    )

    private fun waitFor(what: String, cond: () -> Boolean) {
        val until = System.currentTimeMillis() + 5_000
        while (!cond()) {
            if (System.currentTimeMillis() > until) fail("timed out waiting for $what")
            Thread.sleep(10)
        }
    }

    @Test
    fun inputGoesOverStreamSocketAndFallsBack() = runBlocking<Unit> {
        startAgent(listOf("p1", "p2", "g3"))
        val c = control()
        assertTrue(c.supportsG3)
        val s = StreamSession(c, ::streamSocket, {}, statsIntervalMs = 60_000)
        val info = s.start(StreamRequest(StreamPreset.P720_30, hwnd = JsonPrimitive(100), gaming = GamingPreset.G720_120))
        assertEquals("gaming", info.preset)
        assertTrue(info.inputOnStream)
        val start = agent.streamStarts.single()
        assertEquals("gaming", start.s("preset"))
        assertEquals(100, start.i("hwnd"))
        assertTrue(s.inputOnStream)

        assertTrue(c.sendInput(InputFrame.Button(MouseButton.LEFT, true)))
        assertTrue(c.sendInput(InputFrame.AbsMove(0.25f, 0.5f)))
        assertTrue(c.sendInput(InputFrame.Button(MouseButton.LEFT, false)))
        waitFor("3 inputs on /stream") { agent.streamInputs.size == 3 }
        assertEquals(listOf("mb", "ma", "mb"), agent.streamInputs.map { it.s("k") })
        assertTrue(agent.inputs.isEmpty())

        s.stop()
        assertFalse(s.inputOnStream)
        assertNull(c.inputDetour)
        assertTrue(c.sendInput(InputFrame.Move(1f, 2f)))
        waitFor("fallback on the control socket") { agent.inputs.size == 1 }
        c.close()
    }

    @Test
    fun olderAgentKeepsControlSocketAndNoThumbs() = runBlocking<Unit> {
        startAgent(listOf("p1", "p2"))
        val c = control()
        assertFalse(c.supportsG3)
        val s = StreamSession(c, ::streamSocket, {}, statsIntervalMs = 60_000)
        val info = s.start(StreamRequest(StreamPreset.P720_30))
        assertFalse(info.inputOnStream)
        assertFalse(s.inputOnStream)
        c.sendInput(InputFrame.Move(1f, 1f))
        waitFor("input on control") { agent.inputs.size == 1 }
        try {
            HelmApi(c).windowThumb(JsonPrimitive(1))
            fail("windows.thumb must be gated on g3")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.UNSUPPORTED, e.code)
        }
        assertTrue(agent.thumbCalls.isEmpty())
        s.stop()
        c.close()
    }

    @Test
    fun windowThumbs() = runBlocking<Unit> {
        startAgent(listOf("p1", "p2", "g3"))
        val c = control()
        val api = HelmApi(c)
        assertArrayEquals(byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47), api.windowThumb(JsonPrimitive(1)))
        assertNull(api.windowThumb(JsonPrimitive(2), size = 9999)) // minimized
        assertEquals(256, agent.thumbCalls[0].i("size"))
        assertEquals(512, agent.thumbCalls[1].i("size")) // clamped
        c.close()
    }

    private fun kotlinx.serialization.json.JsonObject.s(k: String) = (this[k] as? JsonPrimitive)?.contentOrNull
    private fun kotlinx.serialization.json.JsonObject.i(k: String) = (this[k] as? JsonPrimitive)?.intOrNull
}
