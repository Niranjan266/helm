package com.niranjan.helm.core

import com.niranjan.helm.core.FakeAgent.Companion.s
import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.take
import kotlinx.coroutines.flow.toList
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** Phase 3 (§11) and Phase 4 (§12) methods against the FakeAgent, plus feature gating. */
class Phase3ConnectionTest {
    private lateinit var server: MockWebServer
    private lateinit var agent: FakeAgent
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder().connectTimeout(4, TimeUnit.SECONDS).build()
    }

    @Before
    fun setUp() {
        agent = FakeAgent(features = listOf("p1", "p2", "p3", "p4"))
        server = MockWebServer()
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    private suspend fun connected(): HelmConnection {
        server.enqueue(MockResponse().withWebSocketUpgrade(agent.listener()))
        server.start(InetAddress.getByName("127.0.0.1"), 0)
        val conn = HelmConnection(
            agent.hostId, "ab".repeat(32), listOf(Endpoint("127.0.0.1", server.port)), identity,
            HandshakeMode.Pair(agent.validToken), plain, 5_000, 5_000, 5_000,
        )
        conn.connect()
        return conn
    }

    private fun last(method: String) = agent.p3Calls.last { it.first == method }.second

    @Test
    fun statsSubscribeDeliversTicks() = runBlocking {
        val conn = connected()
        assertTrue(conn.supportsPhase3 && conn.supportsPhase4)
        val ticks = async(start = CoroutineStart.UNDISPATCHED) {
            withTimeout(5_000) { conn.events("stats.tick").take(2).toList() }
        }
        HelmApi(conn).statsSubscribe(200) // clamped to the 500 ms minimum
        assertEquals(500L, (last("stats.subscribe")["interval_ms"] as JsonPrimitive).content.toLong())
        val stats = ticks.await().map { Stats.from(it.data) }
        assertEquals(20f, stats[1].cpuTotal, 0.01f)
        assertEquals(2, stats[0].cpuPerCore.size)
        assertEquals(25f, stats[0].ramPct, 0.01f)
        assertEquals("C:\\", stats[0].disks[0].mount)
        assertEquals(25f, stats[0].disks[0].freePct, 0.01f)
        assertEquals(1000L, stats[0].netRxBps)
        assertEquals(33f, stats[0].gpu[0].loadPct, 0.01f)
        HelmApi(conn).statsUnsubscribe()
        conn.close()
    }

    @Test
    fun alertsRoundTripAndEvent() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val ev = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("alert").first() } }
        val rules = listOf(
            AlertRule("cpu", AlertRule.CPU_ABOVE, 90.0, forS = 30),
            AlertRule("disk", AlertRule.DISK_FREE_BELOW, 10.0, mount = "C:\\"),
        )
        api.setAlerts(rules)
        assertEquals(rules, api.alerts())
        val a = AlertEvent.from(ev.await().data)!!
        assertEquals("cpu", a.id)
        assertTrue(a.firing)
        assertEquals(97.5, a.value, 0.001)
        conn.close()
    }

    @Test
    fun processesKillAndPriority() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val ps = api.processes()
        assertEquals("chrome.exe", ProcessSorter.sort(ps, ProcessSort.CPU)[0].name)
        api.killProcess(4321)
        assertEquals("4321", (last("process.kill")["pid"] as JsonPrimitive).content)
        api.setPriority(4321, "below_normal")
        assertEquals("below_normal", last("process.priority").s("level"))
        try {
            api.killProcess(0)
            fail("pid 0 must be refused")
        } catch (e: HelmRequestException) {
            assertEquals("bad_params", e.code)
        }
        conn.close()
    }

    @Test
    fun mixerWindowsAndDesktops() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val s = api.audioSessions().single()
        assertEquals("Spotify", s.name)
        api.setAudioSession(s.id, level = 1.5f, muted = true)
        val p = last("audio.session.set")
        assertEquals("{s-1}", p.s("id"))
        assertEquals(1.0, (p["level"] as JsonPrimitive).content.toDouble(), 0.0)
        val hwnd = JsonPrimitive(1234)
        api.moveWindow(hwnd, 1)
        assertEquals("1", (last("windows.move")["monitor_index"] as JsonPrimitive).content)
        api.snapWindow(hwnd, "left")
        assertEquals("left", last("windows.snap").s("pos"))
        api.switchDesktop("next")
        assertEquals("next", last("desktop.switch").s("dir"))
        conn.close()
    }

    @Test
    fun profilesAndActiveEvent() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val ev = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("profile.active").first() } }
        val list = listOf(Profile("games", "Games", matchExe = "steam.exe", deckId = "d1", dockPage = 2), Profile("work", "Work"))
        api.setProfiles(list)
        assertEquals(list, api.profiles())
        assertEquals("games", ev.await().data.s("id"))
        conn.close()
    }

    @Test
    fun fileBrowserAndDownload() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        agent.fs[""] = mutableListOf("C:\\")
        agent.fs["C:\\"] = mutableListOf("b.txt", "Users\\", "a.txt")
        val roots = api.fsList("")
        assertEquals("C:", roots.single().name.trimEnd('\\'))
        val c = api.fsList("C:\\")
        assertEquals(listOf("Users", "a.txt", "b.txt"), c.map { it.name })
        assertTrue(c[0].isDir)
        api.fsMkdir(PcPath.join("C:\\", "New"))
        assertTrue(api.fsList("C:\\").any { it.name == "New" && it.isDir })
        api.fsDelete("C:\\a.txt")
        assertTrue(api.fsList("C:\\").none { it.name == "a.txt" })
        api.fsOpen("C:\\b.txt")
        agent.downloadable["C:\\b.txt"] = "hello".toByteArray()
        val out = java.io.ByteArrayOutputStream()
        conn.downloadFile("C:\\b.txt", out)
        assertEquals("hello", out.toString())
        conn.close()
    }

    @Test
    fun securityRelayAndWake() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        api.setTier(identity.deviceId, "remote")
        assertEquals("remote", agent.tiers[identity.deviceId])
        assertEquals(3, api.auditLines(50).size)
        api.setRelayConfig(true, "wss://relay.example.com")
        assertEquals(RelayConfig(true, "wss://relay.example.com"), api.relayConfig())
        api.wolSend("AA:BB:CC:DD:EE:FF")
        assertEquals("AA:BB:CC:DD:EE:FF", last("wol.send").s("mac"))
        conn.close()
    }

    @Test
    fun deckMacroStateWidgetRoundTrip() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val buttons = listOf(
            DeckButton("m", "Macro", "#112233", action = DeckAction.Macro(listOf(DeckAction.Hotkey("ctrl+c"), DeckAction.Delay(100), DeckAction.Text("x")))),
            DeckButton("r", "Rep", "#112233", action = DeckAction.Repeat(3, DeckAction.Media("next"))),
            DeckButton("p", "Next slide", "#112233", action = DeckAction.Remote("powerpoint.next"), stateBind = "mic_muted"),
            DeckButton("w", "CPU", "#112233", action = DeckAction.Hotkey("ctrl+shift+esc"), widget = "cpu"),
        )
        api.setDeck(buttons)
        assertEquals(buttons, api.deck())
        assertEquals(4, agent.deckJson["buttons"]!!.jsonArray.size)
        conn.close()
    }

    @Test
    fun phase3MethodsAreGatedOnTheHelloFeature() = runBlocking {
        agent.features = listOf("p1")
        val conn = connected()
        val api = HelmApi(conn)
        for (call in listOf<suspend () -> Unit>({ api.processes() }, { api.statsSubscribe(1000) }, { api.wolSend("AA:BB:CC:DD:EE:FF") }, { api.relayConfig() })) {
            try {
                call()
                fail("expected unsupported")
            } catch (e: HelmException) {
                assertEquals(LocalCodes.UNSUPPORTED, e.code)
            }
        }
        assertTrue("nothing was sent", agent.p3Calls.isEmpty())
        assertNull(agent.received.firstOrNull { it.s("method") == "process.list" })
        conn.close()
    }

    @Test
    fun gyroFramesAreSent() = runBlocking {
        val conn = connected()
        assertTrue(conn.sendInput(InputFrame.Gyro(1.234f, -2f)))
        withTimeout(5_000) { while (agent.inputs.isEmpty()) kotlinx.coroutines.delay(10) }
        val f = agent.inputs.single()
        assertEquals("gy", f.s("k"))
        assertEquals("1.23", (f["dx"] as JsonPrimitive).content)
        conn.close()
    }
}
