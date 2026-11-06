package com.niranjan.helm.core

import java.net.InetAddress
import java.net.ServerSocket
import java.util.UUID
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean
import kotlinx.coroutines.runBlocking
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test

class LanRaceTest {
    private lateinit var good: MockWebServer
    private lateinit var agent: FakeAgent
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())
    private val sockets = mutableListOf<ServerSocket>()

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient =
            OkHttpClient.Builder().connectTimeout(4, TimeUnit.SECONDS).build()
    }

    @Before
    fun setUp() {
        agent = FakeAgent()
        good = MockWebServer()
        good.enqueue(MockResponse().withWebSocketUpgrade(agent.listener()))
        good.start(InetAddress.getByName("127.0.0.1"), 0)
    }

    @After
    fun tearDown() {
        good.shutdown()
        sockets.forEach { runCatching { it.close() } }
    }

    /** A port that refuses connections. */
    private fun deadEndpoint(): Endpoint {
        val s = ServerSocket(0, 1, InetAddress.getByName("127.0.0.1"))
        val port = s.localPort
        s.close()
        return Endpoint("127.0.0.1", port)
    }

    /** Accepts TCP (backlog) but never answers: a half-dead address. */
    private fun slowEndpoint(): Endpoint {
        val s = ServerSocket(0, 50, InetAddress.getByName("127.0.0.1"))
        sockets += s
        return Endpoint("127.0.0.1", s.localPort)
    }

    private class FakeRelay : HelmTransport {
        val used = AtomicBoolean(false)
        override val scheme = "ws"
        override val relayed = true
        override fun client(fingerprintHex: String): OkHttpClient {
            used.set(true)
            throw IllegalStateException("relay must not be used")
        }
    }

    private fun conn(endpoints: List<Endpoint>, relay: HelmTransport? = null, helloMs: Long = 6_000, budget: Long = 8_000) =
        HelmConnection(
            hostId = agent.hostId,
            fingerprint = "ab".repeat(32),
            endpoints = endpoints,
            identity = identity,
            mode = HandshakeMode.Pair(agent.validToken),
            transport = plain,
            helloTimeoutMs = helloMs,
            readyTimeoutMs = 5_000,
            relay = relay,
            lanBudgetMs = budget,
        )

    @Test
    fun goodEndpointWinsQuicklyBehindDeadAndSlowOnes() = runBlocking<Unit> {
        val goodEp = Endpoint("127.0.0.1", good.port)
        val relay = FakeRelay()
        val c = conn(listOf(deadEndpoint(), slowEndpoint(), slowEndpoint(), goodEp), relay)
        val t0 = System.nanoTime()
        val ready = c.connect()
        val ms = (System.nanoTime() - t0) / 1_000_000
        assertEquals(goodEp, ready.endpoint)
        assertFalse(ready.viaRelay)
        assertFalse("relay must not be touched", relay.used.get())
        assertTrue("took $ms ms; serial tries would have waited for the slow addresses", ms < 3_000)
        c.close()
    }

    @Test
    fun relayOnlyAfterEveryLanAddressFails() = runBlocking<Unit> {
        val relay = FakeRelay()
        val c = conn(listOf(deadEndpoint(), deadEndpoint()), relay)
        try {
            c.connect()
            fail("expected failure")
        } catch (e: HelmException) {
            // The relay was tried (its fake client refused) after the LAN attempts.
            assertTrue(relay.used.get())
        }
    }

    @Test
    fun relayBudgetStartsWithTheRace() = runBlocking<Unit> {
        val relay = FakeRelay()
        val c = conn(listOf(slowEndpoint(), slowEndpoint()), relay, helloMs = 30_000, budget = 800)
        val t0 = System.nanoTime()
        try {
            c.connect()
            fail("expected failure")
        } catch (e: HelmException) {
            val ms = (System.nanoTime() - t0) / 1_000_000
            assertTrue(relay.used.get())
            assertTrue("budget not honoured: $ms ms", ms < 5_000)
        }
    }

    @Test
    fun agentRefusalIsReportedOverTimeouts() = runBlocking<Unit> {
        val c = HelmConnection(
            hostId = agent.hostId,
            fingerprint = "ab".repeat(32),
            endpoints = listOf(slowEndpoint(), Endpoint("127.0.0.1", good.port)),
            identity = identity,
            mode = HandshakeMode.Pair("wrong-token"),
            transport = plain,
            helloTimeoutMs = 5_000,
            readyTimeoutMs = 5_000,
        )
        try {
            c.connect()
            fail("expected failure")
        } catch (e: HelmException) {
            assertEquals("bad_token", e.code)
        }
    }

    @Test
    fun allDeadReportsUnreachable() = runBlocking<Unit> {
        val c = conn(listOf(deadEndpoint(), deadEndpoint()))
        try {
            c.connect()
            fail("expected failure")
        } catch (e: HelmException) {
            assertEquals(LocalCodes.UNREACHABLE, e.code)
        }
    }
}

class EndpointRankingTest {
    private val wifi = PhoneSubnet("192.168.31.7", 24)

    private fun hosts(l: List<Endpoint>) = l.map { it.host }

    @Test
    fun ownSubnetFirstThenPrivateThenRest() {
        val eps = listOf("26.121.94.40", "192.168.56.1", "10.0.0.5", "192.168.31.56", "8.8.8.8").map { Endpoint(it, 47820) }
        assertEquals(
            listOf("192.168.31.56", "192.168.56.1", "10.0.0.5", "26.121.94.40", "8.8.8.8"),
            hosts(EndpointRanking.rank(eps, listOf(wifi))),
        )
    }

    @Test
    fun noNetworksKeepsPrivateBeforePublic() {
        val eps = listOf("26.1.2.3", "192.168.56.1", "192.168.31.56").map { Endpoint(it, 1) }
        assertEquals(listOf("192.168.56.1", "192.168.31.56", "26.1.2.3"), hosts(EndpointRanking.rank(eps, emptyList())))
    }

    @Test
    fun prefixLengthMatters() {
        val eps = listOf("192.168.32.9", "192.168.31.9").map { Endpoint(it, 1) }
        assertEquals(listOf("192.168.31.9", "192.168.32.9"), hosts(EndpointRanking.rank(eps, listOf(wifi))))
        // A /16 makes both "own subnet": original order stays.
        assertEquals(
            listOf("192.168.32.9", "192.168.31.9"),
            hosts(EndpointRanking.rank(eps, listOf(PhoneSubnet("192.168.31.7", 16)))),
        )
    }

    @Test
    fun hostNamesSitWithPrivateAndMultipleNetsWork() {
        val eps = listOf("8.8.4.4", "pc.local", "172.20.1.9", "10.1.1.1").map { Endpoint(it, 1) }
        val nets = listOf(wifi, PhoneSubnet("10.1.1.200", 24))
        assertEquals(listOf("10.1.1.1", "pc.local", "172.20.1.9", "8.8.4.4"), hosts(EndpointRanking.rank(eps, nets)))
    }

    @Test
    fun plannerRanksSavedAddresses() {
        val pc = SavedPc("h", "PC", listOf("26.121.94.40", "192.168.56.1", "192.168.31.56"), 47820, "ab".repeat(32))
        val found = DiscoveredHost("h", "PC", "192.168.56.1", 47820, "svc")
        assertEquals(
            listOf("192.168.31.56", "192.168.56.1", "26.121.94.40"),
            hosts(EndpointPlanner.forSaved(pc, found, listOf(wifi))),
        )
    }
}

class RelayUrlMigrationTest {
    @Test
    fun normalisesTheCases() {
        for ((typed, want) in listOf(
            "wss://https://helm-relay.example.workers.dev" to "wss://helm-relay.example.workers.dev",
            "https://relay.example.com" to "wss://relay.example.com",
            "wss://relay.example.com/" to "wss://relay.example.com",
            "relay.example.com" to "wss://relay.example.com",
            "wss://wss://https://relay.example.com" to "wss://relay.example.com",
        )) {
            assertEquals(typed, want, TunnelProtocol.normalizeRelayUrl(typed))
        }
        assertNull(TunnelProtocol.normalizeRelayUrl("https"))
        assertNull(TunnelProtocol.normalizeRelayUrl("wss://https"))
        assertNull(TunnelProtocol.normalizeRelayUrl("wss://wss/"))
    }

    @Test
    fun storedBadValueIsMigratedOnLoad() {
        val pc = SavedPc("h", "PC", listOf("1.2.3.4"), 47820, "ab".repeat(32), relayUrl = "wss://https://helm-relay.x.workers.dev")
        val text = SavedPcCodec.encode(listOf(pc))
        assertEquals("wss://helm-relay.x.workers.dev", SavedPcCodec.decode(text).single().relayUrl)
        val junk = SavedPcCodec.encode(listOf(pc.copy(relayUrl = "wss://https")))
        assertNull(SavedPcCodec.decode(junk).single().relayUrl)
        assertEquals("wss://helm-relay.x.workers.dev", PcReach.reportedRelay(true, "wss://https://helm-relay.x.workers.dev"))
    }
}
