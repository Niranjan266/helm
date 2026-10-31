package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class SavedPcTest {
    private val pc = SavedPc(
        hostId = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f",
        name = "DESKTOP-1",
        addresses = listOf("192.168.1.20", "10.0.0.5"),
        port = 47820,
        fingerprint = "ab".repeat(32),
        lastAddress = "10.0.0.5",
        lastPort = 47820,
        lastUsedMs = 42,
    )

    @Test
    fun codecRoundTripAndTolerance() {
        val text = SavedPcCodec.encode(listOf(pc))
        assertTrue(text.contains("\"host_id\""))
        assertEquals(listOf(pc), SavedPcCodec.decode(text))
        assertEquals(emptyList<SavedPc>(), SavedPcCodec.decode(null))
        assertEquals(emptyList<SavedPc>(), SavedPcCodec.decode("{not json"))
    }

    @Test
    fun savedPcPrefersDiscoveredThenLastThenQrAddresses() {
        val found = DiscoveredHost(pc.hostId.uppercase(), "DESKTOP-1", "192.168.1.77", 47821, "DESKTOP-1")
        assertEquals(
            listOf(
                Endpoint("192.168.1.77", 47821),
                Endpoint("10.0.0.5", 47820),
                Endpoint("192.168.1.20", 47820),
            ),
            EndpointPlanner.forSaved(pc, found),
        )
        // Discovered host for another id is ignored.
        val other = found.copy(hostId = "00000000-0000-4000-8000-000000000000")
        assertEquals(
            listOf(Endpoint("10.0.0.5", 47820), Endpoint("192.168.1.20", 47820)),
            EndpointPlanner.forSaved(pc, other),
        )
    }

    @Test
    fun reachedAddressIsRememberedFirstOnce() {
        assertEquals(
            listOf("192.168.1.99", "192.168.1.20", "10.0.0.5"),
            EndpointPlanner.withAddress(pc, Endpoint("192.168.1.99", 47820)),
        )
        // Known address, or a different port (kept as last_port only): unchanged.
        assertEquals(pc.addresses, EndpointPlanner.withAddress(pc, Endpoint("10.0.0.5", 47820)))
        assertEquals(pc.addresses, EndpointPlanner.withAddress(pc, Endpoint("192.168.1.99", 9)))
        val many = pc.copy(addresses = (1..8).map { "10.0.0.$it" })
        assertEquals(EndpointPlanner.MAX_ADDRESSES, EndpointPlanner.withAddress(many, Endpoint("10.0.1.1", 47820)).size)
    }
}
