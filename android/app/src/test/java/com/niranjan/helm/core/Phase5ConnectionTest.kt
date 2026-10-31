package com.niranjan.helm.core

import com.niranjan.helm.core.FakeAgent.Companion.s
import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Phase 5 (§13) methods against the FakeAgent, plus feature gating. */
class Phase5ConnectionTest {
    private val server = MockWebServer()
    private var agent = FakeAgent(features = listOf("p1", "p2", "p3", "p4", "p5"))
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder().connectTimeout(4, TimeUnit.SECONDS).build()
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
    fun recordStartStop() = runBlocking {
        val conn = connected()
        assertTrue(conn.supportsPhase5)
        val api = HelmApi(conn)
        val started = api.recordStart(audio = true)
        assertEquals("true", last("record.start")["audio"]!!.jsonPrimitive.content)
        assertEquals("rec-1.mp4", started.fileName)
        val stopped = api.recordStop()
        assertEquals(4200L, stopped.durationMs)
        try {
            api.recordStop()
            fail("expected bad_params when not recording")
        } catch (e: HelmException) {
            assertEquals("bad_params", e.code)
        }
        conn.close()
    }

    @Test
    fun terminalSessionOutputWriteResizeExit() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val screen = TermScreen(40, 5)
        val prompt = async(start = CoroutineStart.UNDISPATCHED) {
            withTimeout(5_000) { conn.events("term.output").first() }
        }
        val id = api.termOpen(Shells.POWERSHELL, 40, 5)
        assertEquals("7", id)
        assertEquals("powershell", last("term.open").s("shell"))
        val out = TermEvent.from("term.output", prompt.await().data) as TermEvent.Output
        assertEquals(id, out.termId)
        screen.feed(out.data)
        assertEquals("PS C:\\>", screen.screenText()[0])

        val echo = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("term.output").first() } }
        api.termWrite(id, "dir" + TermKeys.ENTER)
        assertEquals("dir\r", agent.termInput[7L]!!.toString(Charsets.UTF_8))
        assertTrue("term_id goes back as a number", (last("term.write")["term_id"] as JsonPrimitive).isString.not())
        screen.feed((TermEvent.from("term.output", echo.await().data) as TermEvent.Output).data)
        assertTrue(screen.screenText()[0].endsWith("dir"))

        api.termResize(id, 100, 30)
        assertEquals("100", last("term.resize")["cols"]!!.jsonPrimitive.content)

        val exit = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("term.exit").first() } }
        api.termClose(id)
        assertEquals(TermEvent.Exit(id, 0), TermEvent.from("term.exit", exit.await().data))
        try {
            api.termWrite(id, "x")
            fail("closed terminal")
        } catch (e: HelmException) {
            assertEquals("bad_params", e.code)
        }
        conn.close()
    }

    @Test
    fun penFramesHaveNoId() = runBlocking {
        val conn = connected()
        assertTrue(conn.sendInput(PenMath.frame(0.5f, 0.5f, 0.3f, 0.2f, 1f, PenState.DOWN, eraser = false)))
        assertTrue(conn.sendInput(PenMath.frame(0.5f, 0.6f, 0f, 0f, 0f, PenState.UP, eraser = false)))
        withTimeout(5_000) { while (agent.inputs.size < 2) kotlinx.coroutines.delay(10) }
        assertEquals(listOf("pn", "pn"), agent.inputs.map { it.s("k") })
        assertEquals(listOf("down", "up"), agent.inputs.map { it.s("state") })
        assertNull(agent.inputs[0]["id"])
        conn.close()
    }

    @Test
    fun notifyPushAndReplyAction() = runBlocking {
        val conn = connected()
        val action = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("notify.action").first() } }
        HelmApi(conn).notifyPush(
            NotifyPush("Messages", "Sam", "Running late?", listOf(NotifyActionButton("0", "Mark read")), reply = true, key = "k-1"),
        )
        val sent = last("notify.push")
        assertEquals("k-1", sent.s("key"))
        assertEquals("Mark read", sent["actions"]!!.jsonArray[0].let { (it as kotlinx.serialization.json.JsonObject).s("label") })
        assertEquals(NotifyActionEvent("k-1", "reply", "on my way"), NotifyActionEvent.from(action.await().data))
        conn.close()
    }

    @Test
    fun pluginsListAndGrant() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val list = api.plugins()
        assertEquals(listOf("weather", "broken"), list.map { it.id })
        assertEquals("wasm trap", list[1].error)
        val w = list[0]
        api.setPlugin(w.id, enabled = true, granted = w.capabilities)
        val after = api.plugins().first { it.id == "weather" }
        assertTrue(after.enabled)
        assertTrue(after.missing.isEmpty())
        conn.close()
    }

    @Test
    fun triggersRoundTrip() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        assertTrue(api.triggers().isEmpty())
        val list = listOf(
            Trigger("morning", TriggerWhen.Time("07:30 weekdays"), DeckAction.Media("play")),
            Trigger("obs", TriggerWhen.AppStart("obs64.exe"), DeckAction.Ha("light.turn_on", "light.on_air")),
            Trigger("hello", TriggerWhen.PhoneConnected(null), DeckAction.Hotkey("Win+D")),
        )
        api.setTriggers(list)
        assertEquals(list, api.triggers())
        conn.close()
    }

    @Test
    fun homeAssistantConfigAndStates() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        try {
            api.haStates()
            fail("not configured yet")
        } catch (e: HelmException) {
            assertEquals("bad_params", e.code)
        }
        api.haConfigSet(" http://ha.local:8123/ ", " secret ")
        assertEquals("http://ha.local:8123" to "secret", agent.haConfig)
        val states = api.haStates()
        assertEquals(listOf("light.desk", "switch.fan"), states.map { it.entityId })
        assertEquals("Desk lamp", states[0].label)
        conn.close()
    }

    @Test
    fun everyCallIsGatedOnP5() = runBlocking {
        agent = FakeAgent(features = listOf("p1", "p2", "p3", "p4"))
        val conn = connected()
        assertFalse(conn.supportsPhase5)
        val api = HelmApi(conn)
        val calls: List<suspend () -> Unit> = listOf(
            { api.recordStart(false) }, { api.recordStop() }, { api.termOpen(Shells.CMD, 80, 24) },
            { api.termWrite("1", "x") }, { api.termResize("1", 80, 24) }, { api.termClose("1") },
            { api.notifyPush(NotifyPush("a", "t", "b", key = "k")) }, { api.plugins() }, { api.setPlugin("x", true, emptyList()) },
            { api.triggers() }, { api.setTriggers(emptyList()) }, { api.haConfigSet("u", "t") }, { api.haStates() },
        )
        for (c in calls) {
            try {
                c()
                fail("expected unsupported")
            } catch (e: HelmException) {
                assertEquals(LocalCodes.UNSUPPORTED, e.code)
            }
        }
        assertTrue("nothing was sent", agent.p3Calls.isEmpty())
        conn.close()
    }
}
