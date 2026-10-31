package com.niranjan.helm.core

/** A fresh address for the PC (an mDNS result); tried before the saved ones. */
data class ReconnectAddress(val host: String, val port: Int)

/** Handle for a scheduled task. */
fun interface Cancelable {
    fun cancel()
}

/** Time source and one-shot timers. Android posts to the main looper; tests use a fake. */
interface ReconnectClock {
    fun nowMs(): Long

    /** Runs [task] once after [delayMs] on the reconnector's thread. */
    fun schedule(delayMs: Long, task: () -> Unit): Cancelable
}

/**
 * Starts one connection attempt to a saved PC, always with the `auth` handshake (never a
 * re-pair). It must later report back with [Reconnector.attemptSucceeded] or
 * [Reconnector.attemptFailed], passing the same [attemptId], on the reconnector's thread.
 */
fun interface ReconnectTransport {
    fun connect(hostId: String, attemptId: Long, preferred: ReconnectAddress?)
}

sealed interface ReconnectState {
    /** Not watching any PC (never connected, or the user disconnected / forgot it). */
    data object Idle : ReconnectState

    /** The control connection to [hostId] is up; a drop will start retrying. */
    data class Online(val hostId: String) : ReconnectState

    /** Lost the PC; the next attempt ([attempt], 1-based) starts at [retryAtMs]. */
    data class Waiting(val hostId: String, val attempt: Int, val retryAtMs: Long) : ReconnectState

    /** Attempt [attempt] is running, first trying [via] when set. */
    data class Trying(val hostId: String, val attempt: Int, val via: ReconnectAddress?) : ReconnectState

    /** The PC refused this phone ([code]); retrying cannot help, the user has to act. */
    data class Stopped(val hostId: String, val code: String) : ReconnectState

    /** True while the reconnector is still trying to get the PC back. */
    val isReconnecting: Boolean get() = this is Waiting || this is Trying
}

object ReconnectPolicy {
    const val MIN_DELAY_MS = 500L
    const val MAX_DELAY_MS = 30_000L

    /** Agent codes that mean "this phone is not trusted any more": stop, never re-pair. */
    private val FATAL = setOf("unknown_device", "bad_signature", LocalCodes.PROTOCOL_VERSION)

    fun isFatal(code: String): Boolean = code in FATAL

    /** Delay before the next attempt after [failures] failed ones: 0.5 s, 1 s, 2 s … 30 s. */
    fun delayFor(failures: Int): Long {
        val n = failures.coerceIn(0, 16)
        return (MIN_DELAY_MS shl n).coerceAtMost(MAX_DELAY_MS)
    }
}

/**
 * Keeps one saved PC connected: after a drop it retries with exponential backoff (see
 * [ReconnectPolicy]), retries at once when the network comes back or mDNS reports the PC
 * (that fresh address wins), and stops for good when the user disconnects or forgets the PC
 * or the PC refuses the phone. Not thread-safe: call everything on one thread.
 */
class Reconnector(
    private val clock: ReconnectClock,
    private val transport: ReconnectTransport,
    private val onState: (ReconnectState) -> Unit = {},
) {
    var state: ReconnectState = ReconnectState.Idle
        private set

    private var hostId: String? = null
    private var failures = 0
    private var attemptSeq = 0L
    private var currentAttempt = NO_ATTEMPT
    private var timer: Cancelable? = null
    private var preferred: ReconnectAddress? = null

    /** Something new happened during an attempt; retry at once if that attempt fails. */
    private var retryAfterAttempt = false

    /** The PC currently watched (Online, Waiting, Trying or Stopped), or null. */
    val target: String? get() = hostId

    /** A control connection to [hostId] is ready (user connect or a successful attempt). */
    fun online(hostId: String) {
        cancelTimer()
        this.hostId = hostId
        failures = 0
        preferred = null
        retryAfterAttempt = false
        currentAttempt = NO_ATTEMPT
        set(ReconnectState.Online(hostId))
    }

    /** The live connection to [hostId] dropped: start retrying. */
    fun dropped(hostId: String) {
        if (state !is ReconnectState.Online || !isTarget(hostId)) return
        failures = 0
        scheduleRetry()
    }

    /** A network became available (or the app came to the foreground): retry now. */
    fun networkAvailable() = retryNow()

    /** The app came back to the foreground. */
    fun foreground() = retryNow()

    /** mDNS just reported [hostId] at [address]. */
    fun discovered(hostId: String, address: ReconnectAddress) {
        if (!isTarget(hostId)) return
        when (val s = state) {
            is ReconnectState.Waiting -> {
                preferred = address
                attemptNow()
            }
            is ReconnectState.Trying -> if (s.via != address) {
                preferred = address
                retryAfterAttempt = true
            }
            else -> Unit
        }
    }

    /** The user disconnected (or switched PC): stop watching. Late attempt results are ignored. */
    fun stop() {
        cancelTimer()
        hostId = null
        preferred = null
        retryAfterAttempt = false
        currentAttempt = NO_ATTEMPT
        set(ReconnectState.Idle)
    }

    /** [hostId] was forgotten on this phone. */
    fun forget(hostId: String) {
        if (isTarget(hostId)) stop()
    }

    fun attemptSucceeded(attemptId: Long) {
        if (attemptId != currentAttempt) return
        online(hostId ?: return)
    }

    fun attemptFailed(attemptId: Long, code: String) {
        if (attemptId != currentAttempt) return
        currentAttempt = NO_ATTEMPT
        val host = hostId ?: return
        if (ReconnectPolicy.isFatal(code)) {
            cancelTimer()
            set(ReconnectState.Stopped(host, code))
            return
        }
        failures++
        if (retryAfterAttempt) {
            retryAfterAttempt = false
            attemptNow()
        } else {
            scheduleRetry()
        }
    }

    private fun retryNow() {
        when (state) {
            is ReconnectState.Waiting -> {
                failures = 0
                attemptNow()
            }
            is ReconnectState.Trying -> {
                failures = 0
                retryAfterAttempt = true
            }
            else -> Unit
        }
    }

    private fun scheduleRetry() {
        val host = hostId ?: return
        cancelTimer()
        val delay = ReconnectPolicy.delayFor(failures)
        set(ReconnectState.Waiting(host, failures + 1, clock.nowMs() + delay))
        var handle: Cancelable? = null
        handle = clock.schedule(delay) {
            if (timer === handle && state is ReconnectState.Waiting) {
                timer = null
                attemptNow()
            }
        }
        timer = handle
    }

    /** Starts an attempt. Nothing may run after [ReconnectTransport.connect]: it can re-enter. */
    private fun attemptNow() {
        val host = hostId ?: return
        cancelTimer()
        val id = ++attemptSeq
        currentAttempt = id
        val via = preferred
        set(ReconnectState.Trying(host, failures + 1, via))
        transport.connect(host, id, via)
    }

    private fun cancelTimer() {
        timer?.cancel()
        timer = null
    }

    private fun isTarget(id: String) = hostId?.equals(id, ignoreCase = true) == true

    private fun set(s: ReconnectState) {
        state = s
        onState(s)
    }

    private companion object {
        const val NO_ATTEMPT = -1L
    }
}
