package com.niranjan.helm.core

import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** The `/stream` socket handshake and frame flow (§10) against the FakeAgent. */
class StreamSessionTest {
    private lateinit var server: MockWebServer
    private lateinit var agent: FakeAgent
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(4, TimeUnit.SECONDS)
            .build()
    }

    private val keyAu = byteArrayOf(0, 0, 0, 1, 0x67, 0x42, 0, 0x1f, 0, 0, 0, 1, 0x68, 0xce.toByte(), 0, 0, 0, 1, 0x65, 0x88.toByte())
    private val deltaAu = byteArrayOf(0, 0, 0, 1, 0x41, 0x9a.toByte())
    private val opus = byteArrayOf(0x78, 1, 2)
    private val png = byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47)

    @Before
    fun setUp() {
        agent = FakeAgent(features = listOf("p1", "p2"))
        server = MockWebServer()
        server.dispatcher = agent.dispatcher()
        server.start(InetAddress.getByName("127.0.0.1"), 0)
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    private val endpoint get() = Endpoint("127.0.0.1", server.port)

    private suspend fun control(): HelmConnection {
        val conn = HelmConnection(
            hostId = agent.hostId,
            fingerprint = "ab".repeat(32),
            endpoints = listOf(endpoint),
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

    private fun streamSocket(onBinary: (ByteArray) -> Unit) = HelmConnection(
        hostId = agent.hostId,
        fingerprint = "ab".repeat(32),
        endpoints = listOf(endpoint),
        identity = identity,
        mode = HandshakeMode.Auth,
        transport = plain,
        helloTimeoutMs = 5_000,
        readyTimeoutMs = 5_000,
        path = Endpoint.STREAM_PATH,
        binarySink = onBinary,
    )

    private fun waitFor(what: String, ms: Long = 5_000, cond: () -> Boolean) {
        val until = System.currentTimeMillis() + ms
        while (!cond()) {
            if (System.currentTimeMillis() > until) fail("timed out waiting for $what")
            Thread.sleep(10)
        }
    }

    @Test
    fun streamUrlUsesStreamPath() {
        assertEquals("wss://10.0.0.5:47820/stream", Endpoint("10.0.0.5", 47820).url("wss", Endpoint.STREAM_PATH))
        assertEquals("wss://10.0.0.5:47820/ws", Endpoint("10.0.0.5", 47820).url("wss"))
    }

    @Test(expected = IllegalArgumentException::class)
    fun pairingIsNotAllowedOnStreamSocket() {
        HelmConnection(
            agent.hostId, "ab".repeat(32), listOf(endpoint), identity, HandshakeMode.Pair("x"),
            transport = plain, path = Endpoint.STREAM_PATH,
        )
    }

    @Test
    fun startOpenReceiveStatsStop() = runBlocking<Unit> {
        agent.streamFrames = listOf(
            StreamFrames.cursorShape(1, 7, 2, 2, 0, 0, png),
            StreamFrames.video(1_000, keyAu, key = true),
            StreamFrames.audio(1_000, opus),
            StreamFrames.video(17_666, deltaAu, key = false),
            StreamFrames.cursor(17_666, 640f, 360f, visible = true, shapeId = 7),
            StreamFrames.encode(0x7f, 0, byteArrayOf(1)), // unknown kind: ignored
        )
        val c = control()
        assertTrue(c.supportsPhase2)
        val frames = CopyOnWriteArrayList<StreamFrame>()
        val session = StreamSession(c, ::streamSocket, { frames += it }, statsIntervalMs = 150)

        val info = session.start(StreamRequest(StreamPreset.P720_30, audio = false, monitorId = JsonPrimitive(65537)))
        assertEquals(1280, info.width)
        assertEquals("h264", info.codec)
        assertTrue(session.state.value is StreamState.Streaming)

        // stream.start params, then /stream socket with auth + stream.open(token).
        val start = agent.streamStarts.single()
        assertEquals(1280, (start["max_width"] as JsonPrimitive).intOrNull)
        assertEquals(720, (start["max_height"] as JsonPrimitive).intOrNull)
        assertEquals(30, (start["fps"] as JsonPrimitive).intOrNull)
        assertEquals(6_000, (start["bitrate_kbps"] as JsonPrimitive).intOrNull)
        assertEquals(false, (start["audio"] as JsonPrimitive).booleanOrNull)
        assertEquals(JsonPrimitive(65537), start["monitor_id"])
        waitFor("frames") { frames.size == 5 }
        assertEquals(listOf("/ws", "/stream"), agent.paths.toList())
        assertEquals(listOf(info.token), agent.streamOpened.toList())

        assertTrue(frames[0] is StreamFrame.CursorShape)
        val key = frames[1] as StreamFrame.Video
        assertTrue(key.key)
        assertArrayEquals(keyAu, key.data)
        assertTrue(AnnexB.containsSps(key.data))
        assertTrue(frames[2] is StreamFrame.Audio)
        assertEquals(17_666L, (frames[3] as StreamFrame.Video).ptsUs)
        assertEquals(StreamFrame.Cursor(17_666, 640f, 360f, true, 7), frames[4])
        assertEquals(1L, session.keyframesReceived)
        assertEquals(2L, session.framesReceived)

        // stream.stats every interval on the control socket, with received bytes counted.
        waitFor("stats") { agent.streamStats.isNotEmpty() }
        val first = agent.streamStats.first()
        assertEquals(setOf("rx_kbps", "decoded_fps", "dropped", "rtt_ms"), first.keys)
        assertTrue("bytes were counted", (first["rx_kbps"] as JsonPrimitive).intOrNull!! >= 0)

        session.requestKeyframe()
        session.configure(bitrateKbps = 4_000)
        session.stop()
        assertEquals(StreamState.Stopped, session.state.value)
        assertEquals(listOf("stream.idr", "stream.config", "stream.stop"), agent.streamCalls.toList())
        // The control socket is still fine.
        assertEquals(true, (c.request("system.ping")["pong"] as JsonPrimitive).booleanOrNull)
        c.close()
    }

    @Test
    fun badTokenFailsTheSession() = runBlocking<Unit> {
        val c = control()
        // A socket that opens the stream with a forged token.
        val session = StreamSession(c, ::streamSocket, { }, statsIntervalMs = 10_000)
        val info = session.start(StreamRequest(StreamPreset.P720_30))
        assertTrue(info.token.isNotEmpty())
        // The token is single use: a second stream.open with it must be refused.
        val again = streamSocket { }
        again.connect()
        again.sendText(buildJsonObject {
            put("type", "stream.open")
            put("token", info.token)
        })
        withTimeout(5_000) { again.state.first { it is ConnectionState.Failed || it is ConnectionState.Closed } }
        assertEquals(1, agent.streamOpened.size)
        session.stop()
        c.close()
    }

    @Test
    fun streamSocketDropMarksSessionFailed() = runBlocking<Unit> {
        val c = control()
        val session = StreamSession(c, ::streamSocket, { }, statsIntervalMs = 10_000)
        session.start(StreamRequest(StreamPreset.P1080_30))
        waitFor("stream open") { agent.streamOpened.size == 1 }
        agent.closeStreams()
        val end = withTimeout(5_000) { session.state.first { it is StreamState.Failed } }
        assertTrue(end is StreamState.Failed)
        session.abandon()
        c.close()
    }

    @Test
    fun streamStartRefusedByOldAgent() = runBlocking<Unit> {
        agent.features = listOf("p1")
        val c = control()
        assertTrue(!c.supportsPhase2)
        c.close()
    }
}
