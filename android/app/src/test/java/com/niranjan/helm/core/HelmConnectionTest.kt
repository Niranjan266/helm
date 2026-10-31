package com.niranjan.helm.core

import com.niranjan.helm.core.FakeAgent.Companion.s
import java.net.InetAddress
import java.net.ServerSocket
import java.util.Base64
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.async
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class HelmConnectionTest {
    private lateinit var server: MockWebServer
    private lateinit var agent: FakeAgent
    private val signer = SoftwareDeviceSigner()
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", signer)
    private val fp = "ab".repeat(32) // ignored by the plain transport

    /** Plain ws:// for MockWebServer; the TLS layer is tested separately (and below with wss). */
    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder()
            .connectTimeout(4, TimeUnit.SECONDS)
            .pingInterval(15, TimeUnit.SECONDS)
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

    private fun startServer(connections: Int = 1) {
        repeat(connections) { server.enqueue(MockResponse().withWebSocketUpgrade(agent.listener())) }
        server.start(InetAddress.getByName("127.0.0.1"), 0)
    }

    private fun endpoint() = Endpoint("127.0.0.1", server.port)

    private fun connection(
        mode: HandshakeMode,
        endpoints: List<Endpoint> = listOf(endpoint()),
        transport: HelmTransport = plain,
        fingerprint: String = fp,
        id: DeviceIdentity = identity,
    ) = HelmConnection(
        hostId = agent.hostId,
        fingerprint = fingerprint,
        endpoints = endpoints,
        identity = id,
        mode = mode,
        transport = transport,
        helloTimeoutMs = 5_000,
        readyTimeoutMs = 5_000,
        defaultRequestTimeoutMs = 5_000,
    )

    private suspend fun expectFailure(code: String, block: suspend () -> Unit): HelmException {
        try {
            block()
        } catch (e: HelmException) {
            assertEquals("${e.message}", code, e.code)
            return e
        }
        fail("expected HelmException($code)")
        throw AssertionError()
    }

    @Test
    fun pairSuccess() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Pair(agent.validToken))
        val ready = conn.connect()
        assertEquals("admin", ready.tier)
        assertEquals(agent.hostId, ready.hostId)
        assertEquals("DESKTOP-TEST", ready.hostName)
        assertEquals(endpoint(), ready.endpoint)
        assertEquals(ConnectionState.Ready("admin", endpoint()), conn.state.value)

        // What the phone sent, field by field.
        val pair = agent.received.single()
        assertEquals("pair", pair.s("type"))
        assertEquals(agent.validToken, pair.s("token"))
        assertEquals(identity.deviceId, pair.s("device_id"))
        assertEquals("Test Phone", pair.s("device_name"))
        assertArrayEquals(signer.publicKeySpki(), Base64.getDecoder().decode(pair.s("public_key")))
        assertTrue(agent.devices.containsKey(identity.deviceId)) // signature verified by the agent
        conn.close()
        assertEquals(ConnectionState.Closed, conn.state.value)
    }

    @Test
    fun pairWithBadTokenFails() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Pair("wrong-token"))
        val e = expectFailure("bad_token") { conn.connect() }
        assertEquals("wrong, expired or used token", e.message)
        val st = conn.state.value
        assertTrue(st is ConnectionState.Failed && st.code == "bad_token")
        assertTrue(agent.devices.isEmpty())
    }

    @Test
    fun requestResponseRoundTrip() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Pair(agent.validToken))
        conn.connect()

        val pong = conn.request("system.ping")
        assertEquals(true, (pong["pong"] as JsonPrimitive).booleanOrNull)

        val info = conn.request("system.info")
        assertEquals("Windows 11 Test", info.s("os"))
        assertEquals(1234L, (info["uptime_s"] as JsonPrimitive).longOrNull)

        val list = conn.request("devices.list")
        val devs = list["devices"] as JsonArray
        assertEquals(1, devs.size)
        assertEquals(true, ((devs[0] as JsonObject)["this"] as JsonPrimitive).booleanOrNull)

        // Request ids increase and params default to {}.
        val reqs = agent.received.filter { it.s("type") == "req" }
        assertEquals(listOf(1L, 2L, 3L), reqs.map { (it["id"] as JsonPrimitive).longOrNull })
        assertEquals(JsonObject(emptyMap()), reqs[0]["params"])
        assertEquals("system.ping", reqs[0].s("method"))

        // Error responses become HelmRequestException.
        val err = expectFailure("unknown_method") { conn.request("nope.nothing") }
        assertTrue(err is HelmRequestException)
        conn.close()
    }

    @Test
    fun outOfOrderResponsesReachTheRightCaller() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Pair(agent.validToken))
        conn.connect()
        val slow = async { conn.request("test.slow") }
        Thread.sleep(50)
        val fast = conn.request("system.ping")
        assertEquals(true, (fast["pong"] as JsonPrimitive).booleanOrNull)
        assertEquals(true, (slow.await()["slow"] as JsonPrimitive).booleanOrNull)
        conn.close()
    }

    @Test
    fun requestTimesOut() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Pair(agent.validToken))
        conn.connect()
        expectFailure(LocalCodes.REQUEST_TIMEOUT) { conn.request("test.never", timeoutMs = 300) }
        // Connection still usable afterwards.
        conn.request("system.ping")
        conn.close()
    }

    @Test
    fun pairThenAuthOnSecondConnection() = runBlocking<Unit> {
        startServer(connections = 2)
        val first = connection(HandshakeMode.Pair(agent.validToken))
        first.connect()
        first.close()

        val second = connection(HandshakeMode.Auth)
        val ready = second.connect()
        assertEquals("admin", ready.tier)
        val auth = agent.received.last()
        assertEquals("auth", auth.s("type"))
        assertEquals(identity.deviceId, auth.s("device_id"))
        assertEquals(setOf("type", "device_id", "signature"), auth.keys)

        // Forget this PC: revoke self.
        val r = second.request("devices.revoke", buildJsonObject { put("device_id", identity.deviceId) })
        assertEquals(true, (r["revoked"] as JsonPrimitive).booleanOrNull)
        second.close()
        assertTrue(agent.devices[identity.deviceId]!!.revoked)
    }

    @Test
    fun authWithUnknownDeviceFails() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Auth)
        expectFailure("unknown_device") { conn.connect() }
    }

    @Test
    fun authWithWrongKeyFails() = runBlocking<Unit> {
        startServer(connections = 2)
        connection(HandshakeMode.Pair(agent.validToken)).apply { connect(); close() }
        val impostor = identity.copy(signer = SoftwareDeviceSigner())
        expectFailure("bad_signature") { connection(HandshakeMode.Auth, id = impostor).connect() }
    }

    @Test
    fun helloFromDifferentHostIsRejected() = runBlocking<Unit> {
        agent.helloHostId = UUID.randomUUID().toString()
        startServer()
        val conn = connection(HandshakeMode.Pair(agent.validToken))
        expectFailure(LocalCodes.HOST_MISMATCH) { conn.connect() }
        assertTrue("must not send pair to the wrong host", agent.received.isEmpty())
    }

    @Test
    fun triesAddressesInOrder() = runBlocking<Unit> {
        startServer()
        val deadPort = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { it.localPort }
        val conn = connection(
            HandshakeMode.Pair(agent.validToken),
            endpoints = listOf(Endpoint("127.0.0.1", deadPort), endpoint()),
        )
        val ready = conn.connect()
        assertEquals(endpoint(), ready.endpoint)
        conn.close()
    }

    @Test
    fun allAddressesUnreachable() = runBlocking<Unit> {
        val deadPort = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1")).use { it.localPort }
        val conn = HelmConnection(
            agent.hostId, fp, listOf(Endpoint("127.0.0.1", deadPort)), identity,
            HandshakeMode.Auth, plain, helloTimeoutMs = 5_000,
        )
        expectFailure(LocalCodes.UNREACHABLE) { conn.connect() }
        assertTrue(conn.state.value is ConnectionState.Failed)
    }

    @Test
    fun agentClosingFailsPendingRequests() = runBlocking<Unit> {
        startServer()
        val conn = connection(HandshakeMode.Pair(agent.validToken))
        conn.connect()
        val e = try {
            conn.request("test.close")
            null
        } catch (e: HelmException) {
            e
        }
        assertTrue(e != null)
        // State leaves Ready.
        val deadline = System.currentTimeMillis() + 3_000
        while (conn.state.value is ConnectionState.Ready && System.currentTimeMillis() < deadline) Thread.sleep(20)
        assertTrue(conn.state.value !is ConnectionState.Ready)
        expectFailure(LocalCodes.NOT_READY) { conn.request("system.ping") }
    }

    @Test
    fun pinnedTlsHandshakeOverWss() = runBlocking<Unit> {
        val held = HeldCertificate.Builder().commonName("helm-agent").ecdsa256().build()
        server.useHttps(HandshakeCertificates.Builder().heldCertificate(held).build().sslSocketFactory(), false)
        startServer()
        val conn = connection(
            HandshakeMode.Pair(agent.validToken),
            transport = PinnedTlsTransport(),
            fingerprint = Fingerprints.sha256Hex(held.certificate.encoded),
        )
        assertEquals("admin", conn.connect().tier)
        assertEquals(true, (conn.request("system.ping")["pong"] as JsonPrimitive).booleanOrNull)
        conn.close()
    }

    @Test
    fun pinnedTlsWithWrongFingerprintFails() = runBlocking<Unit> {
        val held = HeldCertificate.Builder().commonName("helm-agent").ecdsa256().build()
        server.useHttps(HandshakeCertificates.Builder().heldCertificate(held).build().sslSocketFactory(), false)
        startServer()
        val conn = connection(
            HandshakeMode.Pair(agent.validToken),
            transport = PinnedTlsTransport(),
            fingerprint = "00".repeat(32),
        )
        expectFailure(LocalCodes.PIN_MISMATCH) { conn.connect() }
        assertTrue("token must never be sent to an unpinned server", agent.received.isEmpty())
    }
}
