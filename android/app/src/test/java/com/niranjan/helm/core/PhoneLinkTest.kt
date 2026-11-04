package com.niranjan.helm.core

import java.net.InetAddress
import java.util.Base64
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonObject
import okhttp3.OkHttpClient
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import okio.ByteString
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/** The g9 link against a small fake PC: phone.* calls, `/stream` + `phone.open`, frames, input. */
class PhoneLinkTest {
    private val hostId = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f"
    private lateinit var server: MockWebServer
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(4, TimeUnit.SECONDS)
            .build()
    }

    /** Every `phone.*` request, in order. */
    private val calls = CopyOnWriteArrayList<Pair<String, JsonObject>>()
    private val opened = CopyOnWriteArrayList<String>()
    private val frames = CopyOnWriteArrayList<ByteArray>()
    private val streamSockets = CopyOnWriteArrayList<WebSocket>()
    private val sockets = CopyOnWriteArrayList<WebSocket>()

    @Volatile
    private var features = listOf("p1", "g9")

    private fun listener(stream: Boolean) = object : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            sockets += webSocket
            webSocket.send(
                buildJsonObject {
                    put("type", "hello")
                    put("protocol", 1)
                    put("host_id", hostId)
                    put("host_name", "PC")
                    put("nonce", Base64.getEncoder().encodeToString(ByteArray(32)))
                    put("agent_version", "test")
                    put("features", kotlinx.serialization.json.JsonArray(features.map { kotlinx.serialization.json.JsonPrimitive(it) }))
                }.toString(),
            )
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val m = Json.parseToJsonElement(text).jsonObject
            when (m.optStr("type")) {
                "auth" -> webSocket.send(
                    buildJsonObject {
                        put("type", "ready")
                        put("device_id", m.optStr("device_id") ?: "")
                        put("tier", "admin")
                        put("host_id", hostId)
                        put("host_name", "PC")
                    }.toString(),
                )
                "phone.open" -> if (stream) {
                    opened += m.optStr("token") ?: ""
                    streamSockets += webSocket
                }
                "req" -> {
                    val method = m.optStr("method") ?: ""
                    val params = m["params"]?.jsonObject ?: JsonObject(emptyMap())
                    calls += method to params
                    webSocket.send(
                        buildJsonObject {
                            put("type", "res")
                            put("id", m.optLong("id") ?: 0)
                            put("ok", true)
                            putJsonObject("result") { if (method == "phone.accept") put("token", "tok-1") }
                        }.toString(),
                    )
                }
            }
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            if (stream) frames += bytes.toByteArray()
        }
    }

    @Before
    fun setUp() {
        server = MockWebServer()
        server.dispatcher = object : Dispatcher() {
            override fun dispatch(request: RecordedRequest): MockResponse =
                MockResponse().withWebSocketUpgrade(listener(request.path == Endpoint.STREAM_PATH))
        }
        server.start(InetAddress.getByName("127.0.0.1"), 0)
    }

    @After
    fun tearDown() {
        for (s in sockets) s.close(1001, "going away")
        // The test is over; a socket that is slow to go away must not fail it.
        runCatching { server.shutdown() }
    }

    private fun connection(path: String = Endpoint.CONTROL_PATH) = HelmConnection(
        hostId = hostId,
        fingerprint = "00".repeat(32),
        endpoints = listOf(Endpoint("127.0.0.1", server.port)),
        identity = identity,
        mode = HandshakeMode.Auth,
        transport = plain,
        path = path,
        binarySink = if (path == Endpoint.STREAM_PATH) ({ }) else null,
    )

    private fun waitUntil(cond: () -> Boolean) {
        val until = System.currentTimeMillis() + 5_000
        while (!cond()) {
            if (System.currentTimeMillis() > until) fail("timed out")
            Thread.sleep(5)
        }
    }

    @Test
    fun `calls are gated on the hello feature`() = runBlocking {
        features = listOf("p1")
        val conn = connection()
        conn.connect()
        val api = HelmApi(conn)
        assertFalse(conn.supportsPhoneControl)
        try {
            api.phoneCaps(enabled = true, accessibility = false)
            fail("should be refused")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.UNSUPPORTED, e.code)
        }
        assertTrue("nothing sent", calls.isEmpty())
        conn.close()
    }

    @Test
    fun `methods carry their params`() = runBlocking {
        val conn = connection()
        conn.connect()
        val api = HelmApi(conn)
        assertTrue(conn.supportsPhoneControl)
        api.phoneCaps(enabled = true, accessibility = false)
        api.phoneDecline(4, PhoneControl.REASON_DECLINED)
        api.phoneState(4, PhoneStateChange(rotation = 90))
        api.phoneStop(4, PhoneControl.REASON_LOCKED)
        assertEquals(
            listOf(
                """phone.caps {"enabled":true,"accessibility":false}""",
                """phone.decline {"request_id":4,"reason":"declined"}""",
                """phone.state {"request_id":4,"rotation":90}""",
                """phone.stop {"request_id":4,"reason":"screen_locked"}""",
            ),
            calls.map { "${it.first} ${it.second}" },
        )
        conn.close()
    }

    @Test
    fun `open accepts, sends the token on the stream socket, then carries frames and input`() = runBlocking {
        val conn = connection()
        conn.connect()
        val link = PhoneLink(conn) { connection(Endpoint.STREAM_PATH) }
        val sc = link.open(7, PhoneSize(720, 1600), 90, controlGranted = true)
        assertEquals(
            """{"request_id":7,"width":720,"height":1600,"rotation":90,"control":true}""",
            calls.single { it.first == "phone.accept" }.second.toString(),
        )
        waitUntil { opened.isNotEmpty() }
        assertEquals("tok-1", opened.single())

        // Frames in §10 layout, split when large.
        val small = byteArrayOf(0, 0, 1, 0x41)
        assertTrue(link.send(QueuedAu(5, small, false)))
        val big = ByteArray(PhoneFrames.MAX_PAYLOAD + 10) { (it % 251).toByte() }
        assertTrue(link.send(QueuedAu(6, big, true)))
        waitUntil { frames.size == 3 }
        assertArrayEquals(StreamFrames.video(5, small, false), frames[0])
        assertEquals(PhoneFrames.KIND_PART, frames[1][0])
        assertEquals(StreamFrames.KIND_VIDEO_KEY, frames[2][0])
        val joined = (frames[1].drop(StreamFrames.HEADER_LEN) + frames[2].drop(StreamFrames.HEADER_LEN)).toByteArray()
        assertArrayEquals(big, joined)

        // Input arrives as events on the stream socket; unknown kinds are skipped.
        val received = kotlinx.coroutines.CompletableDeferred<PhoneIn>()
        val collector = launch(kotlinx.coroutines.Dispatchers.IO) { link.input(sc).collect { received.complete(it) } }
        kotlinx.coroutines.delay(200)
        val send = { d: String ->
            streamSockets.single().send("""{"type":"event","name":"phone.in","data":$d}""")
        }
        send("""{"k":"nonsense"}""")
        send("""{"k":"tap","x":0.25,"y":0.75}""")
        assertEquals(PhoneIn.Tap(0.25, 0.75), withTimeout(5_000) { received.await() })
        collector.cancel()

        // The stream socket closing ends the link.
        streamSockets.single().close(1000, "bye")
        withTimeout(5_000) { sc.state.first { it is ConnectionState.Failed || it is ConnectionState.Closed } }
        Unit
    }
}
