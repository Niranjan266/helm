package com.niranjan.helm.core

import java.io.ByteArrayOutputStream
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket
import java.security.KeyFactory
import java.security.spec.X509EncodedKeySpec
import java.util.UUID
import java.util.concurrent.CopyOnWriteArrayList
import java.util.concurrent.atomic.AtomicInteger
import kotlin.random.Random
import kotlinx.coroutines.runBlocking
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.mockwebserver.RecordedRequest
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import okio.ByteString
import okio.ByteString.Companion.toByteString
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

/**
 * Phase 4 relay fallback: pinned TLS + the Helm WebSocket run inside a relay tunnel. The fake
 * relay is a MockWebServer that accepts `/connect/<host_id>`, wraps every chunk in a mux frame
 * (encode/decode round trip, like the real relay↔host hop) and pipes it to a raw TCP socket on a
 * TLS MockWebServer running the FakeAgent. It records every byte it forwards.
 */
class RelayTunnelTest {
    private lateinit var agent: FakeAgent
    private lateinit var agentServer: MockWebServer
    private lateinit var relay: FakeRelay
    private lateinit var held: HeldCertificate
    private val signer = SoftwareDeviceSigner()
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Relay Phone", signer)

    @Before
    fun setUp() {
        agent = FakeAgent(features = listOf("p1", "p3", "p4"))
        val key = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(signer.publicKeySpki()))
        agent.devices[identity.deviceId] = FakeAgent.Device("Relay Phone", key)
        held = HeldCertificate.Builder().commonName("helm-agent").ecdsa256().build()
        agentServer = MockWebServer()
        agentServer.useHttps(HandshakeCertificates.Builder().heldCertificate(held).build().sslSocketFactory(), false)
        agentServer.dispatcher = agent.dispatcher()
        agentServer.start(InetAddress.getByName("127.0.0.1"), 0)
        relay = FakeRelay(agent.hostId, agentServer.port)
    }

    @After
    fun tearDown() {
        relay.server.shutdown()
        agentServer.shutdown()
    }

    private val pin get() = Fingerprints.sha256Hex(held.certificate.encoded)

    private fun deadPort(): Int = ServerSocket(0).use { it.localPort }

    private fun relayTransport() = RelayTransport(TunnelProtocol.connectUrl("ws://127.0.0.1:${relay.server.port}", agent.hostId)!!)

    private fun conn(
        endpoints: List<Endpoint> = listOf(Endpoint("127.0.0.1", deadPort())),
        fingerprint: String = pin,
        path: String = Endpoint.CONTROL_PATH,
        relayT: HelmTransport? = relayTransport(),
    ) = HelmConnection(
        agent.hostId, fingerprint, endpoints, identity, HandshakeMode.Auth, PinnedTlsTransport(),
        5_000, 5_000, 10_000, path = path, relay = relayT,
    )

    @Test
    fun lanFailsThenAuthOverRelayWithPinnedTls() = runBlocking {
        val c = conn()
        val ready = c.connect()
        assertTrue(ready.viaRelay)
        assertEquals(TunnelProtocol.innerHost(agent.hostId), ready.endpoint.host)
        assertTrue(c.request("system.ping").containsKey("pong"))
        assertEquals(3, HelmApi(c).auditLines(10).size)

        // A file larger than one tunnel chunk and one binary frame crosses intact.
        val data = Random(7).nextBytes(700_000)
        agent.downloadable["C:\\big.bin"] = data
        val out = ByteArrayOutputStream()
        c.downloadFile("C:\\big.bin", out)
        assertArrayEquals(data, out.toByteArray())
        c.close()

        assertEquals(1, relay.channels.get())
        assertTrue("phone chunks must be <= 64 KiB", relay.oversized.get() == 0)
        val seen = String(relay.seenBytes(), Charsets.ISO_8859_1)
        for (plain in listOf("hello", "helm-v1", "system.ping", "device_id", "Upgrade: websocket")) {
            assertFalse("relay saw plaintext '$plain'", plain in seen)
        }
        assertTrue(agent.paths.contains("/ws"))
    }

    @Test
    fun lanIsPreferredWhenReachable() = runBlocking {
        val c = conn(endpoints = listOf(Endpoint("127.0.0.1", agentServer.port)))
        assertFalse(c.connect().viaRelay)
        c.close()
        assertEquals(0, relay.channels.get())
    }

    @Test
    fun streamSocketOverRelayOnly() = runBlocking {
        val c = conn(endpoints = emptyList(), path = Endpoint.STREAM_PATH)
        assertTrue(c.connect().viaRelay)
        c.close()
        assertTrue(agent.paths.contains("/stream"))
    }

    @Test
    fun pinIsCheckedInsideTheTunnel() = runBlocking {
        try {
            conn(fingerprint = "00".repeat(32)).connect()
            fail("expected pin mismatch")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.PIN_MISMATCH, e.code)
        }
    }

    @Test
    fun hostNotOnRelayIsUnreachable() = runBlocking {
        relay.hostOnline = false
        try {
            conn(endpoints = emptyList()).connect()
            fail("expected unreachable")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.UNREACHABLE, e.code)
            assertTrue(e.message!!, "not connected to the relay" in e.message!!)
        }
    }

    @Test
    fun qrPairingWorksOverTheRelayWithNoLanAddress() = runBlocking {
        val phone = DeviceIdentity(UUID.randomUUID().toString(), "Away Phone", SoftwareDeviceSigner())
        val c = HelmConnection(
            agent.hostId, pin, emptyList(), phone, HandshakeMode.Pair(agent.validToken),
            relay = relayTransport(),
        )
        val ready = c.connect()
        assertTrue(ready.viaRelay)
        assertTrue(agent.devices.containsKey(phone.deviceId))
        c.close()
        // The token never crossed in the clear, and the phone can come back with auth.
        val seen = String(relay.seenBytes(), Charsets.ISO_8859_1)
        for (plain in listOf(agent.validToken, "\"pair\"", "Away Phone")) assertFalse("relay saw '$plain'", plain in seen)
        val again = HelmConnection(agent.hostId, pin, emptyList(), phone, HandshakeMode.Auth, relay = relayTransport())
        assertTrue(again.connect().viaRelay)
        again.close()
    }

    @Test
    fun qrPairingStillPrefersTheLan() = runBlocking {
        val phone = DeviceIdentity(UUID.randomUUID().toString(), "Home Phone", SoftwareDeviceSigner())
        val c = HelmConnection(
            agent.hostId, pin, listOf(Endpoint("127.0.0.1", agentServer.port)), phone, HandshakeMode.Pair(agent.validToken),
            relay = relayTransport(),
        )
        assertFalse(c.connect().viaRelay)
        c.close()
        assertEquals(0, relay.channels.get())
    }

    @Test
    fun aWrongPinOverTheRelayNeverSendsTheToken() = runBlocking {
        val phone = DeviceIdentity(UUID.randomUUID().toString(), "Phone", SoftwareDeviceSigner())
        val c = HelmConnection(
            agent.hostId, "00".repeat(32), emptyList(), phone, HandshakeMode.Pair(agent.validToken),
            relay = relayTransport(),
        )
        try {
            c.connect()
            fail("expected pin mismatch")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.PIN_MISMATCH, e.code)
        }
        assertFalse(agent.tokenUsed)
    }

    @Test
    fun codePairingIsNeverAllowedOverTheRelay() {
        val observing = CodePairingTransport()
        try {
            HelmConnection(agent.hostId, pin, listOf(Endpoint("127.0.0.1", 1)), identity, HandshakeMode.PairCode("123456"), transport = observing, relay = relayTransport())
            fail("code pairing over the relay must be refused")
        } catch (_: IllegalArgumentException) {
        }
    }

    @Test
    fun deadLanAddressesDoNotHoldUpTheRelay() = runBlocking {
        val t0 = System.currentTimeMillis()
        // 10.255.255.1 drops packets: without the LAN budget this alone would take the full limit.
        val c = HelmConnection(
            agent.hostId, pin, listOf(Endpoint("10.255.255.1", 47820), Endpoint("10.255.255.2", 47820)), identity, HandshakeMode.Auth,
            relay = relayTransport(), lanBudgetMs = 1_500,
        )
        assertTrue(c.connect().viaRelay)
        c.close()
        assertTrue("took ${System.currentTimeMillis() - t0} ms", System.currentTimeMillis() - t0 < 9_000)
    }

    @Test
    fun noRouteAtAllIsAPlainFailureNotACrash() = runBlocking {
        val c = HelmConnection(agent.hostId, pin, emptyList(), identity, HandshakeMode.Auth)
        try {
            c.connect()
            fail("expected unreachable")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.UNREACHABLE, e.code)
        }
    }

    /** A blind byte pipe like the Cloudflare relay, with mux framing on the host hop. */
    class FakeRelay(private val hostId: String, private val agentPort: Int) {
        val server = MockWebServer()
        val channels = AtomicInteger(0)
        val oversized = AtomicInteger(0)
        @Volatile var hostOnline = true
        private val seen = CopyOnWriteArrayList<ByteArray>()

        fun seenBytes(): ByteArray = ByteArrayOutputStream().also { o -> seen.forEach { o.write(it) } }.toByteArray()

        init {
            server.dispatcher = object : Dispatcher() {
                override fun dispatch(request: RecordedRequest): MockResponse = when {
                    request.path != "/connect/$hostId" -> MockResponse().setResponseCode(400)
                    !hostOnline -> MockResponse().setResponseCode(404)
                    else -> MockResponse().withWebSocketUpgrade(listener())
                }
            }
            server.start(InetAddress.getByName("127.0.0.1"), 0)
        }

        private fun hop(type: Int, ch: Long, payload: ByteArray): ByteArray {
            val d = MuxFrame.decode(MuxFrame(type, ch, payload).encode()) as MuxFrame.Companion.Decoded.Ok
            check(d.frame.type == type && d.frame.channel == ch)
            return d.frame.payload
        }

        private fun listener() = object : WebSocketListener() {
            private lateinit var tcp: Socket
            private var ch = 0L

            override fun onOpen(webSocket: WebSocket, response: Response) {
                ch = channels.incrementAndGet().toLong()
                hop(MuxFrame.OPEN, ch, ByteArray(0))
                tcp = Socket("127.0.0.1", agentPort)
                Thread {
                    val buf = ByteArray(70_000)
                    try {
                        val input = tcp.getInputStream()
                        while (true) {
                            // Odd, varying chunk sizes: tunnel boundaries carry no meaning.
                            val n = input.read(buf, 0, Random.nextInt(1, buf.size))
                            if (n < 0) break
                            var off = 0
                            while (off < n) {
                                val k = minOf(TunnelProtocol.MAX_CHUNK, n - off)
                                val chunk = hop(MuxFrame.DATA, ch, buf.copyOfRange(off, off + k))
                                seen += chunk
                                webSocket.send(chunk.toByteString())
                                off += k
                            }
                        }
                    } catch (_: Exception) {
                    }
                    hop(MuxFrame.CLOSE, ch, ByteArray(0))
                    webSocket.close(1000, null)
                }.start()
            }

            override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
                if (bytes.size > TunnelProtocol.MAX_CHUNK) oversized.incrementAndGet()
                val b = hop(MuxFrame.DATA, ch, bytes.toByteArray())
                seen += b
                try {
                    tcp.getOutputStream().write(b)
                } catch (_: Exception) {
                    webSocket.close(4410, null)
                }
            }

            override fun onMessage(webSocket: WebSocket, text: String) {
                if (text == "ping") webSocket.send("pong") else webSocket.close(4400, null)
            }

            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                webSocket.close(1000, null)
                runCatching { tcp.close() }
            }

            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                runCatching { tcp.close() }
            }
        }
    }
}
