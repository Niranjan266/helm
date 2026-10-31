package com.niranjan.helm.core

import com.niranjan.helm.core.FakeAgent.Companion.s
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlin.random.Random
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** Phase 1 transport features against the FakeAgent: features, events, input, files, dock/deck. */
class Phase1ConnectionTest {
    private lateinit var server: MockWebServer
    private lateinit var agent: FakeAgent
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(4, TimeUnit.SECONDS)
            .build()
    }

    @Before
    fun setUp() {
        agent = FakeAgent()
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
            hostId = agent.hostId,
            fingerprint = "ab".repeat(32),
            endpoints = listOf(Endpoint("127.0.0.1", server.port)),
            identity = identity,
            mode = HandshakeMode.Pair(agent.validToken),
            transport = plain,
            helloTimeoutMs = 5_000,
            readyTimeoutMs = 5_000,
            defaultRequestTimeoutMs = 5_000,
        )
        conn.connect()
        return conn
    }

    private fun waitFor(what: String, cond: () -> Boolean) {
        val until = System.currentTimeMillis() + 5_000
        while (!cond()) {
            if (System.currentTimeMillis() > until) fail("timed out waiting for $what")
            Thread.sleep(10)
        }
    }

    @Test
    fun featuresFromHello() = runBlocking<Unit> {
        val conn = connected()
        assertEquals(setOf("p1"), conn.readyInfo!!.features)
        assertTrue(conn.supportsPhase1)
        conn.close()
    }

    @Test
    fun phase0AgentHasNoFeatures() = runBlocking<Unit> {
        agent.features = null
        val conn = connected()
        assertEquals(emptySet<String>(), conn.readyInfo!!.features)
        assertFalse(conn.supportsPhase1)
        conn.close()
    }

    @Test
    fun eventsAreDeliveredWithNameAndData() = runBlocking<Unit> {
        val conn = connected()
        val waiting = async(start = CoroutineStart.UNDISPATCHED) {
            withTimeout(5_000) { conn.events("media.changed").first() }
        }
        conn.request("test.emit", buildJsonObject {
            put("name", "media.changed")
            putJsonObject("data") { putJsonObject("state") { put("title", "Song"); put("status", "playing") } }
        })
        val ev = waiting.await()
        assertEquals("media.changed", ev.name)
        val st = MediaState.fromHolder(ev.data)!!
        assertEquals("Song", st.title)
        assertTrue(st.isPlaying)
        conn.close()
    }

    @Test
    fun eventWithoutDataGetsEmptyObject() = runBlocking<Unit> {
        val conn = connected()
        val waiting = async(start = CoroutineStart.UNDISPATCHED) {
            withTimeout(5_000) { conn.events.first { it.name == "windows.changed" } }
        }
        conn.request("test.emit", buildJsonObject { put("name", "windows.changed") })
        assertEquals(JsonObject(emptyMap()), waiting.await().data)
        conn.close()
    }

    @Test
    fun inputFramesHaveNoIdAndNoReply() = runBlocking<Unit> {
        val conn = connected()
        assertTrue(conn.sendInput(InputFrame.Move(3.5f, -2f)))
        for (f in InputFrame.click(MouseButton.RIGHT)) assertTrue(conn.sendInput(f))
        assertTrue(conn.sendInput(InputFrame.Scroll(0f, 1.25f)))
        assertTrue(conn.sendInput(InputFrame.Text("hé 😀")))
        assertTrue(conn.sendInput(InputFrame.Shortcut(listOf(Vk.CONTROL, 0x43))))
        // A request after the frames proves the connection still works and nothing replied.
        conn.request("system.ping")
        waitFor("6 input frames") { agent.inputs.size == 6 }
        val kinds = agent.inputs.map { it.s("k") }
        assertEquals(listOf("mm", "mb", "mb", "ms", "tx", "sc"), kinds)
        for (f in agent.inputs) {
            assertEquals("in", f.s("type"))
            assertNull("input frames carry no id", f["id"])
        }
        assertEquals(3.5, (agent.inputs[0]["dx"] as JsonPrimitive).content.toDouble(), 1e-6)
        assertEquals("right", agent.inputs[1].s("button"))
        assertEquals("true", (agent.inputs[1]["down"] as JsonPrimitive).content)
        assertEquals("false", (agent.inputs[2]["down"] as JsonPrimitive).content)
        assertEquals("hé 😀", agent.inputs[4].s("text"))
        assertEquals(listOf(0x11, 0x43), (agent.inputs[5]["keys"] as JsonArray).map { (it as JsonPrimitive).content.toInt() })
        conn.close()
    }

    @Test
    fun inputIsNotSentAfterClose() = runBlocking<Unit> {
        val conn = connected()
        conn.close()
        assertFalse(conn.sendInput(InputFrame.Move(1f, 1f)))
    }

    @Test
    fun uploadInManyChunksIsReassembledAndVerified() = runBlocking<Unit> {
        val conn = connected()
        val data = Random(7).nextBytes(BinaryFrame.MAX_DATA * 2 + 12_345)
        val progress = mutableListOf<Long>()
        val path = conn.uploadFile(
            name = "photo.jpg",
            size = data.size.toLong(),
            sha256Hex = FakeAgent.sha256Hex(data),
            open = { ByteArrayInputStream(data) },
            onProgress = { progress += it },
        )
        assertTrue(path.endsWith("photo.jpg"))
        assertArrayEquals(data, agent.uploaded["photo.jpg"])
        assertEquals(listOf(262_144L, 524_288L, data.size.toLong()), progress)
        conn.close()
    }

    @Test
    fun emptyFileUpload() = runBlocking<Unit> {
        val conn = connected()
        conn.uploadFile("empty.txt", 0, FakeAgent.sha256Hex(ByteArray(0)), { ByteArrayInputStream(ByteArray(0)) })
        assertEquals(0, agent.uploaded["empty.txt"]!!.size)
        conn.close()
    }

    @Test
    fun uploadWithWrongHashFailsWithBadParams() = runBlocking<Unit> {
        val conn = connected()
        val data = "hello".toByteArray()
        try {
            conn.uploadFile("a.txt", data.size.toLong(), "00".repeat(32), { ByteArrayInputStream(data) })
            fail("expected bad_params")
        } catch (e: HelmRequestException) {
            assertEquals("bad_params", e.code)
        }
        conn.close()
    }

    @Test
    fun uploadOfFileThatGrewFails() = runBlocking<Unit> {
        val conn = connected()
        val data = ByteArray(100)
        try {
            conn.uploadFile("b.bin", 10, FakeAgent.sha256Hex(data), { ByteArrayInputStream(data) })
            fail("expected transfer_failed")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.TRANSFER_FAILED, e.code)
        }
        conn.close()
    }

    @Test
    fun downloadReceivesBinaryFramesUntilFileDone() = runBlocking<Unit> {
        val conn = connected()
        val data = Random(3).nextBytes(BinaryFrame.MAX_DATA + 999)
        agent.downloadable["C:\\Users\\test\\report.pdf"] = data
        val out = ByteArrayOutputStream()
        val d = conn.downloadFile("C:\\Users\\test\\report.pdf", out)
        assertEquals("report.pdf", d.name)
        assertEquals(data.size.toLong(), d.size)
        assertArrayEquals(data, out.toByteArray())
        conn.close()
    }

    @Test
    fun downloadWithBadHashFails() = runBlocking<Unit> {
        val conn = connected()
        agent.badDownloadHash = true
        agent.downloadable["C:\\x.bin"] = ByteArray(10) { it.toByte() }
        try {
            conn.downloadFile("C:\\x.bin", ByteArrayOutputStream())
            fail("expected checksum failure")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.TRANSFER_FAILED, e.code)
        }
        conn.close()
    }

    @Test
    fun dockSetBroadcastsDockChangedAndRoundTrips() = runBlocking<Unit> {
        val conn = connected()
        val api = HelmApi(conn)
        val pages = listOf(
            listOf(DockTile("t1", DockTile.KIND_APP, appId = "Microsoft.WindowsCalculator_8wekyb3d8bbwe!App", label = "Calc")),
            listOf(DockTile("t2", DockTile.KIND_OPEN, target = "https://example.com")),
        )
        val changed = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("dock.changed").first() } }
        api.setDock(pages)
        changed.await()
        assertEquals(pages, api.dock())
        conn.close()
    }

    @Test
    fun deckSetBroadcastsDeckChangedAndRoundTrips() = runBlocking<Unit> {
        val conn = connected()
        val api = HelmApi(conn)
        val buttons = listOf(
            DeckButton("b1", "Task Mgr", "#EF4444", action = DeckAction.Hotkey("ctrl+shift+esc")),
            DeckButton("b2", "Vol+", "#22C55E", icon = "volume", action = DeckAction.VolumeStep(0.05)),
        )
        val changed = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("deck.changed").first() } }
        api.setDeck(buttons)
        changed.await()
        assertEquals(buttons, api.deck())
        conn.close()
    }
}
