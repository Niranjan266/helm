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

    private fun log(s: String) = println("[helm-e2e] $s")
}
