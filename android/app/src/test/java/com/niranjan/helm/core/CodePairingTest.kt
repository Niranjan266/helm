package com.niranjan.helm.core

import java.net.InetAddress
import java.security.SecureRandom
import java.util.Base64
import java.util.UUID
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.put
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** g4 short-code pairing (Docs/protocol/pairing-security.md) and the app lock timer. */
class CodePairingTest {
    private val hex = Fingerprints::toHex

    // ---- Shared vector with host/agent/src/g4/code.rs (`interop_vector`) -------------------

    private val vectorHash = CodePairingCrypto.transcriptHash(
        hostId = "1b4e28ba-2fa1-11d2-883f-0016d3cca427",
        nonceAgent = "bm9uY2UtYWdlbnQtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=",
        noncePhone = "bm9uY2UtcGhvbmUtMDEyMzQ1Njc4OWFiY2RlZjAxMjM=",
        fingerprint = "ab".repeat(32),
        deviceId = "9f8e7d6c-0000-4000-8000-123456789abc",
        publicKey = "MFkwEwYHKoZIzj0CAQYIKoZIzj0DAQcDQgAEexample",
    )

    @Test
    fun interopVector() {
        assertEquals(VECTOR_H, hex(vectorHash))
        assertEquals(
            VECTOR_A1,
            hex(CodePairingCrypto.commitment("A", 1, 4, vectorHash, ByteArray(32) { 0x11 })),
        )
        assertEquals(
            VECTOR_P6,
            hex(CodePairingCrypto.commitment("P", 6, 9, vectorHash, ByteArray(32) { 0x26 })),
        )
    }

    @Test
    fun hmacMatchesRfc4231() {
        val mac = CodePairingCrypto.hmacSha256("Jefe".toByteArray(), "what do ya want for nothing?".toByteArray())
        assertEquals("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843", hex(mac))
    }

    @Test
    fun parseCode() {
        assertEquals(listOf(0, 1, 2, 3, 4, 5), CodePairingCrypto.parseCode("012 345")?.toList())
        assertEquals(listOf(0, 1, 2, 3, 4, 5), CodePairingCrypto.parseCode("012-345")?.toList())
        assertNull(CodePairingCrypto.parseCode("12345"))
        assertNull(CodePairingCrypto.parseCode("1234567"))
        assertNull(CodePairingCrypto.parseCode("12a456"))
    }

    @Test
    fun phoneChecksEachRoundInOrderAndOpensOnlyAfterward() {
        val code = intArrayOf(4, 0, 1, 2, 3, 9)
        val rA = List(6) { i -> ByteArray(32) { (0x11 + i).toByte() } }
        val agentCommits = List(6) { i -> CodePairingCrypto.commitment("A", i + 1, code[i], vectorHash, rA[i]) }
        val phone = CodePairingPhone(code, vectorHash)
        assertTrue(phone.setAgentCommitments(agentCommits))
        assertFalse(phone.setAgentCommitments(agentCommits))
        try {
            phone.opening(1)
            fail("must not open before checking")
        } catch (_: IllegalStateException) {
        }
        assertFalse(phone.checkAgentOpening(2, rA[1]))
        for (i in 1..6) {
            assertTrue(phone.checkAgentOpening(i, rA[i - 1]))
            val r = phone.opening(i)
            assertTrue(CodePairingCrypto.openingMatches(phone.commitments[i - 1], "P", i, code[i - 1], vectorHash, r))
        }
        assertTrue(phone.complete)

        // Wrong digit 3 on the phone: round 3 fails.
        val wrong = CodePairingPhone(intArrayOf(4, 0, 2, 2, 3, 9), vectorHash)
        wrong.setAgentCommitments(agentCommits)
        assertTrue(wrong.checkAgentOpening(1, rA[0]))
        assertTrue(wrong.checkAgentOpening(2, rA[1]))
        assertFalse(wrong.checkAgentOpening(3, rA[2]))
        assertFalse(wrong.complete)
    }

    @Test
    fun aCodeGetsOneAttemptPerPc() {
        val used = UsedCodes()
        assertTrue(used.tryUse("PC-1", "123 456"))
        assertFalse(used.tryUse("pc-1", "123456"))
        assertTrue(used.tryUse("pc-2", "123456"))
        repeat(64) { used.tryUse("pc-3", it.toString().padStart(6, '0')) }
        assertTrue("oldest entries are dropped", used.tryUse("PC-1", "123456"))
    }

    // ---- App lock timer ----------------------------------------------------------------------

    @Test
    fun lockTimer() {
        val t = AppLockTimer(timeoutMs = 0)
        t.onBackground(0)
        assertFalse("off by default", t.onForeground(10_000_000))

        t.timeoutMs = 5 * 60_000L
        t.onBackground(1_000)
        assertFalse(t.onForeground(1_000 + 4 * 60_000L))
        t.onBackground(10_000)
        t.onBackground(20_000) // a second stop does not move the start
        assertTrue(t.onForeground(10_000 + 5 * 60_000L))
        assertTrue("stays locked until unlocked", t.locked)
        assertTrue(t.onForeground(10_000 + 6 * 60_000L))
        t.unlocked()
        assertFalse(t.locked)
        assertFalse("foreground without background never locks", t.onForeground(99_000_000))
    }

    // ---- Against a fake agent ----------------------------------------------------------------

    private val server = MockWebServer()
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    @After
    fun tearDown() {
        server.shutdown()
    }

    /** Agent side of the code exchange, written from the spec (like `g4/mod.rs`). */
    private class CodeAgent(
        val code: IntArray,
        val fingerprint: String,
        val pendingBeforeReady: Boolean = false,
        val features: List<String> = listOf("p1", "g4"),
    ) : WebSocketListener() {
        val hostId = "1b4e28ba-2fa1-11d2-883f-0016d3cca427"
        private val nonce = HelmSigning.b64(ByteArray(32).also(SecureRandom()::nextBytes))
        private var h = ByteArray(0)
        private var phoneCommits: List<ByteArray> = emptyList()
        private val rA = List(6) { ByteArray(32).also(SecureRandom()::nextBytes) }

        @Volatile
        var result: String? = null

        override fun onOpen(webSocket: WebSocket, response: Response) {
            webSocket.send(
                buildJsonObject {
                    put("type", "hello"); put("protocol", 1); put("host_id", hostId); put("host_name", "TEST-PC")
                    put("nonce", nonce); put("agent_version", "test")
                    put("features", JsonArray(features.map { JsonPrimitive(it) }))
                }.toString(),
            )
        }

        private fun fail(ws: WebSocket, code: String) {
            result = code
            ws.send(buildJsonObject { put("type", "error"); put("code", code); put("message", code) }.toString())
            ws.close(1000, null)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val o = HelmConnection.parseObject(text) ?: return
            val dec = Base64.getDecoder()
            when ((o["type"] as JsonPrimitive).content) {
                "pair_code" -> {
                    val s = { k: String -> (o[k] as JsonPrimitive).content }
                    h = CodePairingCrypto.transcriptHash(hostId, nonce, s("nonce"), fingerprint, s("device_id"), s("public_key"))
                    phoneCommits = (o["commits"] as JsonArray).map { dec.decode((it as JsonPrimitive).content) }
                    val commits = List(6) { i -> CodePairingCrypto.commitment("A", i + 1, code[i], h, rA[i]) }
                    webSocket.send(
                        buildJsonObject {
                            put("type", "code_commit")
                            put("commits", JsonArray(commits.map { JsonPrimitive(HelmSigning.b64(it)) }))
                        }.toString(),
                    )
                    sendOpen(webSocket, 1)
                }
                "code_open" -> {
                    val i = (o["i"] as JsonPrimitive).intOrNull!!
                    val r = dec.decode((o["r"] as JsonPrimitive).content)
                    if (!CodePairingCrypto.openingMatches(phoneCommits[i - 1], "P", i, code[i - 1], h, r)) return fail(webSocket, "bad_code")
                    if (i < 6) return sendOpen(webSocket, i + 1)
                    if (pendingBeforeReady) {
                        webSocket.send("""{"type":"pair_pending","timeout_s":1}""")
                        Thread.sleep(700)
                    }
                    result = "ready"
                    webSocket.send(
                        buildJsonObject {
                            put("type", "ready"); put("device_id", "d"); put("tier", "admin")
                            put("host_id", hostId); put("host_name", "TEST-PC")
                        }.toString(),
                    )
                }
                "error" -> result = "phone_rejected"
            }
        }

        private fun sendOpen(ws: WebSocket, i: Int) {
            ws.send(buildJsonObject { put("type", "code_open"); put("i", i); put("r", HelmSigning.b64(rA[i - 1])) }.toString())
        }
    }

    private fun start(agent: CodeAgent) {
        server.enqueue(MockResponse().withWebSocketUpgrade(agent))
        server.start(InetAddress.getByName("127.0.0.1"), 0)
    }

    private fun conn(agent: CodeAgent, code: String, transport: HelmTransport, readyTimeoutMs: Long = 20_000) = HelmConnection(
        hostId = agent.hostId,
        fingerprint = "0".repeat(64),
        endpoints = listOf(Endpoint("127.0.0.1", server.port)),
        identity = identity,
        mode = HandshakeMode.PairCode(code),
        transport = transport,
        readyTimeoutMs = readyTimeoutMs,
    )

    @Test
    fun codePairingOverRealTlsProvesTheCertificate() = runBlocking<Unit> {
        val held = HeldCertificate.Builder().commonName("helm-agent").ecdsa256().build()
        server.useHttps(HandshakeCertificates.Builder().heldCertificate(held).build().sslSocketFactory(), false)
        val fp = Fingerprints.sha256Hex(held.certificate.encoded)
        val agent = CodeAgent(intArrayOf(0, 7, 3, 3, 1, 8), fp)
        start(agent)
        val ready = conn(agent, "073 318", CodePairingTransport()).connect()
        assertEquals("admin", ready.tier)
        assertEquals(fp, ready.fingerprint)
        assertEquals("ready", agent.result)
    }

    @Test
    fun wrongCodeIsDetectedByThePhone() = runBlocking<Unit> {
        val agent = CodeAgent(intArrayOf(0, 7, 3, 3, 1, 8), "cd".repeat(32))
        start(agent)
        try {
            conn(agent, "073319", FixedFingerprintTransport("cd".repeat(32))).connect()
            fail("expected bad_code")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.BAD_CODE, e.code)
        }
    }

    @Test
    fun manInTheMiddleCertificateIsDetected() = runBlocking<Unit> {
        // The agent proves its real certificate; the phone saw another one.
        val agent = CodeAgent(intArrayOf(0, 7, 3, 3, 1, 8), "cd".repeat(32))
        start(agent)
        try {
            conn(agent, "073318", FixedFingerprintTransport("ee".repeat(32))).connect()
            fail("expected bad_code")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.BAD_CODE, e.code)
        }
    }

    @Test
    fun pairPendingExtendsTheWaitAndIsReported() = runBlocking<Unit> {
        val agent = CodeAgent(intArrayOf(1, 1, 1, 1, 1, 1), "cd".repeat(32), pendingBeforeReady = true)
        start(agent)
        val c = conn(agent, "111111", FixedFingerprintTransport("cd".repeat(32)), readyTimeoutMs = 300)
        val ready = c.connect()
        assertEquals("cd".repeat(32), ready.fingerprint)
        assertFalse(c.approvalPending.value)
    }

    @Test
    fun oldAgentWithoutG4IsRefusedBeforeAnythingIsSent() = runBlocking<Unit> {
        val agent = CodeAgent(intArrayOf(1, 1, 1, 1, 1, 1), "cd".repeat(32), features = listOf("p1"))
        start(agent)
        try {
            conn(agent, "111111", FixedFingerprintTransport("cd".repeat(32))).connect()
            fail("expected unsupported")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.UNSUPPORTED, e.code)
        }
        assertNull(agent.result)
    }

    private companion object {
        // Same values as host/agent/src/g4/code.rs `interop_vector` (and a Python hashlib/hmac check).
        const val VECTOR_H = "a16fbf9b52ce476d4c6c20ab349e56b7d9cd8ec271c959d63cb0a842e71aea4c"
        const val VECTOR_A1 = "e7cd6b49d48eb85513216cdb6904f2ee3279ef9dec51b8629327f57d3b2fcaf7"
        const val VECTOR_P6 = "c114c93a81becd531ae0455c5a7255f8ccb62e67e837ff4fad8875eb0faf5b80"
    }
}
