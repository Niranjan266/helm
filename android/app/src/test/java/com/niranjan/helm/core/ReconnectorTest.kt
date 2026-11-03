package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ReconnectorTest {
    private class FakeClock : ReconnectClock {
        var now = 0L
        private class Task(val at: Long, val run: () -> Unit) {
            var cancelled = false
        }
        private val tasks = mutableListOf<Task>()

        override fun nowMs() = now

        override fun schedule(delayMs: Long, task: () -> Unit): Cancelable {
            val t = Task(now + delayMs, task)
            tasks += t
            return Cancelable { t.cancelled = true }
        }

        fun advance(ms: Long) {
            val until = now + ms
            while (true) {
                val next = tasks.filter { !it.cancelled && it.at <= until }.minByOrNull { it.at } ?: break
                tasks.remove(next)
                now = next.at
                next.run()
            }
            now = until
        }

        val pending: Int get() = tasks.count { !it.cancelled }
    }

    private data class Call(val hostId: String, val id: Long, val via: ReconnectAddress?)

    private class FakeTransport : ReconnectTransport {
        val calls = mutableListOf<Call>()
        val probes = mutableListOf<Call>()
        override fun connect(hostId: String, attemptId: Long, preferred: ReconnectAddress?) {
            calls += Call(hostId, attemptId, preferred)
        }
        override fun probeLan(hostId: String, probeId: Long, preferred: ReconnectAddress?) {
            probes += Call(hostId, probeId, preferred)
        }
        val last: Call get() = calls.last()
    }

    private val host = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f"
    private val clock = FakeClock()
    private val transport = FakeTransport()
    private val states = mutableListOf<ReconnectState>()
    private val r = Reconnector(clock, transport) { states += it }

    @Test
    fun backoffDoublesFromHalfASecondToThirtySeconds() {
        assertEquals(
            listOf(500L, 1_000L, 2_000L, 4_000L, 8_000L, 16_000L, 30_000L, 30_000L),
            (0..7).map { ReconnectPolicy.delayFor(it) },
        )
        assertEquals(30_000L, ReconnectPolicy.delayFor(1_000))
    }

    @Test
    fun dropThenBackoffThenSuccess() {
        r.online(host)
        r.dropped(host.uppercase())
        assertEquals(ReconnectState.Waiting(host, 1, 500), r.state)
        clock.advance(499)
        assertTrue(transport.calls.isEmpty())
        clock.advance(1)
        assertEquals(1, transport.calls.size)
        assertTrue(r.state is ReconnectState.Trying)

        r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        assertEquals(ReconnectState.Waiting(host, 2, 1_500), r.state)
        clock.advance(1_000)
        assertEquals(2, transport.calls.size)
        r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        assertEquals(ReconnectState.Waiting(host, 3, clock.now + 2_000), r.state)

        clock.advance(2_000)
        r.attemptSucceeded(transport.last.id)
        assertEquals(ReconnectState.Online(host), r.state)
        assertEquals(0, clock.pending)

        // Backoff starts over after a success.
        r.dropped(host)
        assertEquals(ReconnectState.Waiting(host, 1, clock.now + 500), r.state)
    }

    @Test
    fun networkAvailableRetriesAtOnceAndResetsBackoff() {
        r.online(host)
        r.dropped(host)
        repeat(5) {
            clock.advance(ReconnectPolicy.MAX_DELAY_MS)
            r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        }
        val waiting = r.state as ReconnectState.Waiting
        assertEquals(16_000L, waiting.retryAtMs - clock.now)
        val before = transport.calls.size

        r.networkAvailable()
        assertEquals(before + 1, transport.calls.size)
        r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        assertEquals(clock.now + 1_000, (r.state as ReconnectState.Waiting).retryAtMs)

        // The old timer was cancelled: nothing extra fires.
        clock.advance(999)
        assertEquals(before + 1, transport.calls.size)
        clock.advance(1)
        assertEquals(before + 2, transport.calls.size)
        r.attemptSucceeded(transport.last.id)
        assertEquals(ReconnectState.Online(host), r.state)
    }

    @Test
    fun networkAvailableDuringAnAttemptRetriesRightAfterItFails() {
        r.online(host)
        r.dropped(host)
        clock.advance(500)
        r.networkAvailable()
        assertEquals(1, transport.calls.size)
        r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        assertEquals(2, transport.calls.size)
        assertTrue(r.state is ReconnectState.Trying)
    }

    @Test
    fun nsdResultIsPreferredAndTriesAtOnce() {
        r.online(host)
        r.dropped(host)
        val fresh = ReconnectAddress("192.168.1.77", 47821)
        r.discovered("someone-else", fresh)
        assertTrue(transport.calls.isEmpty())

        r.discovered(host, fresh)
        assertEquals(Call(host, transport.last.id, fresh), transport.last)
        assertEquals(ReconnectState.Trying(host, 1, fresh), r.state)

        // The same address again while trying it changes nothing.
        r.discovered(host, fresh)
        r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        assertTrue(r.state is ReconnectState.Waiting)
        assertEquals(1, transport.calls.size)

        // Later attempts keep the fresh address first.
        clock.advance(1_000)
        assertEquals(fresh, transport.last.via)

        // A newer address while an attempt runs: retry with it as soon as that one fails.
        val newer = ReconnectAddress("10.0.0.9", 47820)
        r.discovered(host, newer)
        r.attemptFailed(transport.last.id, LocalCodes.UNREACHABLE)
        assertEquals(newer, transport.last.via)
        r.attemptSucceeded(transport.last.id)
        assertEquals(ReconnectState.Online(host), r.state)
    }

    @Test
    fun userDisconnectStopsRetriesAndIgnoresLateResults() {
        r.online(host)
        r.dropped(host)
        clock.advance(500)
        val running = transport.last.id
        r.stop()
        assertEquals(ReconnectState.Idle, r.state)
        assertNull(r.target)

        r.attemptSucceeded(running)
        r.attemptFailed(running, LocalCodes.UNREACHABLE)
        r.networkAvailable()
        r.discovered(host, ReconnectAddress("192.168.1.2", 47820))
        clock.advance(10 * ReconnectPolicy.MAX_DELAY_MS)
        assertEquals(ReconnectState.Idle, r.state)
        assertEquals(1, transport.calls.size)
        // A drop report after stop does not restart anything either.
        r.dropped(host)
        assertEquals(ReconnectState.Idle, r.state)
    }

    @Test
    fun stopWhileWaitingCancelsTheTimer() {
        r.online(host)
        r.dropped(host)
        r.forget("other-pc")
        assertTrue(r.state is ReconnectState.Waiting)
        r.forget(host)
        clock.advance(ReconnectPolicy.MAX_DELAY_MS)
        assertTrue(transport.calls.isEmpty())
        assertEquals(ReconnectState.Idle, r.state)
    }

    @Test
    fun refusedByThePcStopsWithoutRepairing() {
        r.online(host)
        r.dropped(host)
        clock.advance(500)
        r.attemptFailed(transport.last.id, "unknown_device")
        assertEquals(ReconnectState.Stopped(host, "unknown_device"), r.state)
        r.networkAvailable()
        clock.advance(10 * ReconnectPolicy.MAX_DELAY_MS)
        assertEquals(1, transport.calls.size)
        assertTrue(ReconnectPolicy.isFatal("bad_signature"))
        assertTrue(!ReconnectPolicy.isFatal(LocalCodes.PIN_MISMATCH))
    }

    @Test
    fun noRetriesBeforeTheFirstSuccessfulConnect() {
        r.dropped(host)
        r.networkAvailable()
        clock.advance(ReconnectPolicy.MAX_DELAY_MS)
        assertTrue(transport.calls.isEmpty())
        assertEquals(ReconnectState.Idle, r.state)
        assertTrue(states.isEmpty())
    }

    @Test
    fun attemptThatFailsSynchronouslyStillBacksOff() {
        val sync = object : ReconnectTransport {
            var n = 0
            lateinit var target: Reconnector
            override fun connect(hostId: String, attemptId: Long, preferred: ReconnectAddress?) {
                n++
                target.attemptFailed(attemptId, LocalCodes.UNREACHABLE)
            }
        }
        val rr = Reconnector(clock, sync)
        sync.target = rr
        rr.online(host)
        rr.dropped(host)
        clock.advance(500)
        assertEquals(1, sync.n)
        assertEquals(ReconnectState.Waiting(host, 2, clock.now + 1_000), rr.state)
    }

    @Test
    fun onTheLanThereIsNothingToProbe() {
        val r = Reconnector(clock, transport)
        r.online(host)
        assertEquals(ReconnectState.Online(host, viaRelay = false), r.state)
        assertEquals(0, clock.pending)
        r.discovered(host, ReconnectAddress("10.0.0.2", 47820))
        r.networkAvailable()
        assertTrue(transport.probes.isEmpty())
    }

    @Test
    fun attemptThatEndedOnTheRelayStartsLookingForTheLan() {
        val r = Reconnector(clock, transport)
        r.online(host)
        r.dropped(host)
        clock.advance(500)
        r.attemptSucceeded(transport.last.id, viaRelay = true)
        assertEquals(ReconnectState.Online(host, viaRelay = true), r.state)
        assertEquals(1, clock.pending)
        clock.advance(ReconnectPolicy.LAN_PROBE_MS)
        assertEquals(1, transport.probes.size)
        assertNull(transport.probes.last().via)
    }

    @Test
    fun movesBackToLanWhenMdnsFindsThePc() {
        val r = Reconnector(clock, transport)
        r.online(host, viaRelay = true)
        val a = ReconnectAddress("192.168.1.5", 47820)
        r.discovered(host, a)
        assertEquals(a, transport.probes.single().via)
        assertTrue("still on the relay while probing", (r.state as ReconnectState.Online).viaRelay)
        assertTrue(r.lanProbeSucceeded(transport.probes.single().id))
        assertEquals(ReconnectState.Online(host, viaRelay = false), r.state)
        assertEquals("no more probes on the LAN", 0, clock.pending)
    }

    @Test
    fun failedProbeStaysOnTheRelayAndTriesAgainLater() {
        val r = Reconnector(clock, transport)
        r.online(host, viaRelay = true)
        r.networkAvailable()
        val id = transport.probes.single().id
        r.lanProbeFailed(id)
        assertEquals(ReconnectState.Online(host, viaRelay = true), r.state)
        clock.advance(ReconnectPolicy.LAN_PROBE_MS - 1)
        assertEquals(1, transport.probes.size)
        clock.advance(1)
        assertEquals(2, transport.probes.size)
        r.lanProbeFailed(id) // stale: ignored
        assertEquals(2, transport.probes.size)
    }

    @Test
    fun aDiscoveryDuringAProbeIsNotLost() {
        val r = Reconnector(clock, transport)
        r.online(host, viaRelay = true)
        r.foreground()
        val a = ReconnectAddress("192.168.1.5", 47820)
        r.discovered(host, a)
        assertEquals("one probe at a time", 1, transport.probes.size)
        r.lanProbeFailed(transport.probes[0].id)
        assertEquals(a, transport.probes[1].via)
    }

    @Test
    fun aLateProbeResultCannotTouchADroppedSession() {
        val r = Reconnector(clock, transport)
        r.online(host, viaRelay = true)
        r.networkAvailable()
        val id = transport.probes.single().id
        r.dropped(host)
        assertFalse("swap refused", r.lanProbeSucceeded(id))
        assertTrue(r.state is ReconnectState.Waiting)
        clock.advance(500)
        r.attemptSucceeded(transport.last.id, viaRelay = true)
        r.stop()
        assertEquals(0, clock.pending)
        assertEquals(ReconnectState.Idle, r.state)
    }
}
