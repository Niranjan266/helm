package com.niranjan.helm.core

import java.util.UUID
import kotlinx.coroutines.runBlocking
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Test

/**
 * Opt-in end-to-end test against a real running Helm PC agent. Skipped unless the environment
 * variable HELM_E2E_URI holds a fresh `helm://pair?...` link printed by the agent.
 *
 * Uses the real pinned-TLS transport and a software P-256 key in place of the Android Keystore.
 */
class E2eRealAgentTest {
    @Test
    fun pairAuthListAndRevokeAgainstRealAgent() = runBlocking<Unit> {
        val raw = System.getenv("HELM_E2E_URI")
        assumeTrue("HELM_E2E_URI not set; skipping real-agent E2E test", !raw.isNullOrBlank())

        val uri = PairingUri.parse(raw!!)
        log("pairing link: $uri")
        val identity = DeviceIdentity(UUID.randomUUID().toString(), "Helm E2E test", SoftwareDeviceSigner())
        log("device_id: ${identity.deviceId}")
        val endpoints = EndpointPlanner.forPairing(uri)

        // 1. Pair.
        val pairConn = HelmConnection(uri.hostId, uri.fingerprint, endpoints, identity, HandshakeMode.Pair(uri.token))
        val ready = try {
            pairConn.connect()
        } catch (e: HelmException) {
            throw AssertionError("pair failed: ${e.code}: ${e.message}", e)
        }
        log("paired: tier=${ready.tier} host=${ready.hostName} via ${ready.endpoint} agent=${ready.agentVersion}")
        assertEquals(uri.hostId.lowercase(), ready.hostId.lowercase())

        // 2. Ping + info.
        val t0 = System.nanoTime()
        val pong = pairConn.request("system.ping")
        log("system.ping: $pong (${(System.nanoTime() - t0) / 1_000_000} ms round trip)")
        assertEquals(true, (pong["pong"] as? JsonPrimitive)?.booleanOrNull)
        val info = pairConn.request("system.info")
        log("system.info: $info")
        assertEquals(uri.hostId.lowercase(), (info["host_id"] as? JsonPrimitive)?.contentOrNull?.lowercase())
        pairConn.close()
        log("disconnected")

        // 3. Reconnect with auth (same key + device id).
        val authEndpoints = (listOf(ready.endpoint) + endpoints).distinct()
        val authConn = HelmConnection(uri.hostId, uri.fingerprint, authEndpoints, identity, HandshakeMode.Auth)
        val ready2 = try {
            authConn.connect()
        } catch (e: HelmException) {
            throw AssertionError("auth failed: ${e.code}: ${e.message}", e)
        }
        log("authenticated: tier=${ready2.tier} via ${ready2.endpoint}")

        // 4. devices.list must include us, flagged as this device.
        val list = authConn.request("devices.list")
        log("devices.list: $list")
        val me = (list["devices"] as JsonArray).map { it as JsonObject }
            .firstOrNull { (it["device_id"] as? JsonPrimitive)?.contentOrNull == identity.deviceId }
        assertTrue("our device must be listed", me != null)
        assertEquals(true, (me!!["this"] as? JsonPrimitive)?.booleanOrNull)

        // 4b. Phase 1 read-only methods (nothing with side effects).
        log("features: ${ready2.features}")
        if (authConn.supportsPhase1) {
            val api = HelmApi(authConn)
            suspend fun <T> timed(name: String, block: suspend () -> T): T {
                val t = System.nanoTime()
                val r = try {
                    block()
                } catch (e: HelmException) {
                    throw AssertionError("$name failed: ${e.code}: ${e.message}", e)
                }
                log("$name: ${(System.nanoTime() - t) / 1_000_000} ms")
                return r
            }
            val apps = timed("apps.list") { api.apps() }
            log("  ${apps.size} apps, e.g. ${apps.take(3).map { it.name }}")
            assertTrue("apps.list should not be empty on a real PC", apps.isNotEmpty())
            val windows = timed("windows.list") { api.windows() }
            log("  ${windows.size} windows, foreground: ${windows.firstOrNull { it.isForeground }?.title}")
            val media = timed("media.state") { api.mediaState() }
            log("  media: ${media?.let { "${it.status} '${it.title}' by '${it.artist}'" } ?: "nothing playing"}")
            val audio = timed("audio.get") { api.audio() }
            log("  audio: $audio")
            assertTrue("audio.get must return level", audio != null)
            val dock = timed("dock.get") { api.dock() }
            log("  dock: ${dock.size} pages, ${dock.sumOf { it.size }} tiles")
            val deck = timed("deck.get") { api.deck() }
            log("  deck: ${deck.size} buttons")
            val monitors = timed("screen.monitors") { api.monitors() }
            log("  monitors: ${monitors.map { "${it.name} ${it.width}x${it.height}${if (it.isPrimary) " (primary)" else ""}" }}")
            assertTrue("screen.monitors should list a monitor", monitors.isNotEmpty())
            val macs = MacEntry.listFrom(timed("system.info") { api.systemInfo() })
            log("  macs: $macs -> wake order ${WakeOnLan.chooseMacs(macs)}")
        } else {
            log("agent does not advertise p1; skipping Phase 1 read-only checks")
        }

        // 4c. Phase 2: a 720p30 stream must deliver a keyframe within 3 s.
        if (authConn.supportsPhase2) {
            val session = StreamSession(
                control = authConn,
                openStreamSocket = { sink ->
                    HelmConnection(
                        uri.hostId, uri.fingerprint, listOf(ready2.endpoint), identity, HandshakeMode.Auth,
                        path = Endpoint.STREAM_PATH, binarySink = sink,
                    )
                },
                sink = { },
            )
            val t = System.nanoTime()
            val info = try {
                session.start(StreamRequest(StreamPreset.P720_30, audio = false))
            } catch (e: HelmException) {
                throw AssertionError("stream.start failed: ${e.code}: ${e.message}", e)
            }
            log("stream.start: ${info.width}x${info.height}@${info.fps} ${info.codec} (${info.encoder})")
            val deadline = System.currentTimeMillis() + 3_000
            while (session.keyframesReceived == 0L && System.currentTimeMillis() < deadline) Thread.sleep(10)
            log("first keyframe after ${(System.nanoTime() - t) / 1_000_000} ms; ${session.framesReceived} video frames")
            session.stop()
            assertTrue("no keyframe within 3 s", session.keyframesReceived > 0)
        } else {
            log("agent does not advertise p2; skipping stream check")
        }

        // 5. Revoke ourselves ("forget this PC").
        val revoked = authConn.request("devices.revoke", buildJsonObject { put("device_id", identity.deviceId) })
        log("devices.revoke: $revoked")
        assertEquals(true, (revoked["revoked"] as? JsonPrimitive)?.booleanOrNull)
        authConn.close()

        // 6. Auth must now be refused.
        val after = HelmConnection(uri.hostId, uri.fingerprint, authEndpoints, identity, HandshakeMode.Auth)
        try {
            after.connect()
            after.close()
            throw AssertionError("auth after revoke unexpectedly succeeded")
        } catch (e: HelmException) {
            log("auth after revoke: ${e.code} (${e.message})")
            assertEquals("unknown_device", e.code)
        }
        log("E2E OK")
    }

    /**
     * The real thing: phone code -> live `wss://` relay -> real agent, using only the QR link and
     * no LAN address. Run with HELM_E2E_RELAY=1 and HELM_E2E_URI = a link carrying `r=`
     * (scripts/e2e-relay.ps1 starts a throwaway agent and does this).
     */
    @Test
    fun pairPingStreamAndRevokeThroughTheRealRelay() = runBlocking<Unit> {
        assumeTrue("HELM_E2E_RELAY not set; skipping real-relay E2E test", System.getenv("HELM_E2E_RELAY") == "1")
        val raw = System.getenv("HELM_E2E_URI")
        assumeTrue("HELM_E2E_URI not set", !raw.isNullOrBlank())
        val uri = PairingUri.parse(raw!!)
        log("relay: ${uri.relayUrl}; LAN addresses in the link are ignored")
        val relayUrl = requireNotNull(uri.relayUrl) { "the QR link must carry r=" }
        assertTrue("must be the real TLS path", relayUrl.startsWith("wss://"))
        val identity = DeviceIdentity(UUID.randomUUID().toString(), "Helm relay E2E", SoftwareDeviceSigner())
        fun relay() = RelayTransport.forPc(relayUrl, uri.hostId)!!
        fun connection(mode: HandshakeMode, path: String = Endpoint.CONTROL_PATH, sink: ((ByteArray) -> Unit)? = null) =
            HelmConnection(uri.hostId, uri.fingerprint, emptyList(), identity, mode, path = path, binarySink = sink, relay = relay())

        // 1. Pair over the relay. The agent may still be signing in to the relay: wait for it.
        val t0 = System.nanoTime()
        var pairConn: HelmConnection
        var ready: ReadyInfo
        val waitUntil = System.currentTimeMillis() + 45_000
        while (true) {
            pairConn = connection(HandshakeMode.Pair(uri.token))
            try {
                ready = pairConn.connect()
                break
            } catch (e: HelmException) {
                log("pair attempt: ${e.code}: ${e.message}")
                if (e.code != LocalCodes.UNREACHABLE || System.currentTimeMillis() > waitUntil) {
                    throw AssertionError("pair over the relay failed: ${e.code}: ${e.message}", e)
                }
                Thread.sleep(2_000)
            }
        }
        log("paired over the relay in ${ms(t0)} ms: tier=${ready.tier} host=${ready.hostName} agent=${ready.agentVersion}")
        assertTrue(ready.viaRelay)
        assertEquals(uri.hostId.lowercase(), ready.hostId.lowercase())
        assertEquals("admin", ready.tier)

        // 2. system.ping (measured) and system.info.
        val pings = pingStats(pairConn, 20)
        log("system.ping x20: min ${pings.first} / median ${pings.second} / max ${pings.third} ms")
        val info = pairConn.request("system.info")
        assertEquals(uri.hostId.lowercase(), (info["host_id"] as? JsonPrimitive)?.contentOrNull?.lowercase())
        val relayInfo = RelayConfig.from(info["relay"] as? JsonObject)
        log("system.info: relay=$relayInfo")
        assertEquals(true, relayInfo?.enabled)
        assertEquals(relayUrl, TunnelProtocol.normalizeRelayUrl(relayInfo?.url))
        pairConn.close()

        // 3. Reconnect with auth, still only over the relay.
        val t1 = System.nanoTime()
        val authConn = connection(HandshakeMode.Auth)
        val ready2 = try {
            authConn.connect()
        } catch (e: HelmException) {
            throw AssertionError("auth over the relay failed: ${e.code}: ${e.message}", e)
        }
        log("auth over the relay in ${ms(t1)} ms: tier=${ready2.tier}")
        assertTrue(ready2.viaRelay)
        val pings2 = pingStats(authConn, 20)
        log("system.ping x20 after auth: min ${pings2.first} / median ${pings2.second} / max ${pings2.third} ms")

        // 3b. Optional soak (HELM_E2E_IDLE_S): both sockets idle past the relay's idle limits.
        System.getenv("HELM_E2E_IDLE_S")?.toLongOrNull()?.let { idle ->
            log("idling $idle s")
            Thread.sleep(idle * 1000)
            val soak = pingStats(authConn, 5)
            log("system.ping x5 after idling $idle s: min ${soak.first} / median ${soak.second} / max ${soak.third} ms")
        }

        // 4. A short 720p30 stream: first keyframe, then what actually gets through.
        assertTrue("the agent must advertise p2", authConn.supportsPhase2)
        val session = StreamSession(
            control = authConn,
            openStreamSocket = { sink -> connection(HandshakeMode.Auth, Endpoint.STREAM_PATH, sink) },
            sink = { },
        )
        val choice = RelayStreamPolicy.choose(viaRelay = true, perPc = null, global = "P1080_30", globalKbps = 0)
        assertEquals(StreamPreset.P720_30, choice.preset)
        val t2 = System.nanoTime()
        val started = try {
            session.start(StreamRequest(choice.preset, bitrateKbps = choice.kbps, audio = false))
        } catch (e: HelmException) {
            throw AssertionError("stream.start over the relay failed: ${e.code}: ${e.message}", e)
        }
        log("stream.start in ${ms(t2)} ms: ${started.width}x${started.height}@${started.fps} ${started.codec} (${started.encoder}) ${choice.kbps} kbps")
        val deadline = System.currentTimeMillis() + 15_000
        while (session.keyframesReceived == 0L && System.currentTimeMillis() < deadline) Thread.sleep(10)
        log("first keyframe after ${ms(t2)} ms")
        assertTrue("no keyframe within 15 s over the relay", session.keyframesReceived > 0)
        val framesBefore = session.framesReceived
        val tRun = System.nanoTime()
        Thread.sleep(8_000)
        val frames = session.framesReceived - framesBefore
        val secs = (System.nanoTime() - tRun) / 1e9
        log("stream over the relay: $frames frames in ${"%.1f".format(secs)} s = ${"%.1f".format(frames / secs)} fps, ${session.keyframesReceived} keyframes")
        val during = pingStats(authConn, 5)
        log("system.ping x5 while streaming: min ${during.first} / median ${during.second} / max ${during.third} ms")
        session.stop()

        // 5. Revoke ourselves, disconnect, and the relay path must refuse us from now on.
        val revoked = authConn.request("devices.revoke", buildJsonObject { put("device_id", identity.deviceId) })
        assertEquals(true, (revoked["revoked"] as? JsonPrimitive)?.booleanOrNull)
        authConn.close()
        val after = connection(HandshakeMode.Auth)
        try {
            after.connect()
            after.close()
            throw AssertionError("auth after revoke unexpectedly succeeded")
        } catch (e: HelmException) {
            log("auth after revoke: ${e.code}")
            assertEquals("unknown_device", e.code)
        }
        log("REAL RELAY E2E OK")
    }

    private fun ms(since: Long) = (System.nanoTime() - since) / 1_000_000

    private suspend fun pingStats(c: HelmConnection, n: Int): Triple<Long, Long, Long> {
        val all = List(n) {
            val t = System.nanoTime()
            c.request("system.ping")
            ms(t)
        }.sorted()
        return Triple(all.first(), all[n / 2], all.last())
    }

    private fun log(s: String) = println("[helm-e2e] $s")
}
