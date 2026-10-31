package com.niranjan.helm.core

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

sealed interface StreamState {
    data object Idle : StreamState
    data object Starting : StreamState
    data class Streaming(val info: StreamStartInfo) : StreamState
    data class Failed(val code: String, val message: String) : StreamState
    data object Stopped : StreamState
}

/** Receives parsed stream frames on the socket reader thread; must not block. */
fun interface StreamFrameSink {
    fun onFrame(frame: StreamFrame)
}

/**
 * One screen stream (PROTOCOL.md §10). Single use: [start] once, then [stop].
 *
 * 1. `stream.start` on the control socket -> token.
 * 2. A second pinned-TLS WebSocket to `/stream` with the normal `hello`/`auth` handshake.
 * 3. `{"type":"stream.open","token":…}`, then binary frames flow to [sink].
 * 4. Every [statsIntervalMs]: `system.ping` for RTT, then `stream.stats` on the control socket.
 *
 * [openStreamSocket] builds the (not yet connected) `/stream` connection, routing binary
 * frames to the given callback. Plain JVM code.
 */
class StreamSession(
    private val control: HelmConnection,
    private val openStreamSocket: (onBinary: (ByteArray) -> Unit) -> HelmConnection,
    private val sink: StreamFrameSink,
    val stats: StreamStatsCounter = StreamStatsCounter(System.currentTimeMillis()),
    private val statsIntervalMs: Long = 1_000,
    private val clock: () -> Long = System::currentTimeMillis,
) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _state = MutableStateFlow<StreamState>(StreamState.Idle)
    val state: StateFlow<StreamState> = _state.asStateFlow()

    @Volatile
    private var socket: HelmConnection? = null

    @Volatile
    private var stopping = false

    @Volatile
    var framesReceived: Long = 0
        private set

    @Volatile
    var keyframesReceived: Long = 0
        private set

    private var started = false

    /** Starts the stream. Returns the agent's answer; throws [HelmException] on failure. */
    suspend fun start(request: StreamRequest): StreamStartInfo {
        check(!started) { "start() may only be called once" }
        started = true
        _state.value = StreamState.Starting
        try {
            val res = control.request("stream.start", request.toParams(), timeoutMs = 15_000)
            val info = StreamStartInfo.from(res)
                ?: throw HelmException(LocalCodes.BAD_STREAM, "PC did not return a stream token.")
            val sc = openStreamSocket(::onBinary)
            socket = sc
            sc.connect()
            if (stopping) throw HelmException(LocalCodes.CLOSED, "Stream was stopped.")
            if (!sc.sendText(buildJsonObject {
                    put("type", "stream.open")
                    put("token", info.token)
                })
            ) {
                throw HelmException(LocalCodes.CONNECTION_LOST, "Stream socket closed before stream.open.")
            }
            _state.value = StreamState.Streaming(info)
            scope.launch { watchSocket(sc) }
            scope.launch { statsLoop() }
            return info
        } catch (e: HelmException) {
            socket?.close()
            if (!stopping) _state.value = StreamState.Failed(e.code, e.message ?: e.code)
            scope.cancel()
            throw e
        }
    }

    private fun onBinary(bytes: ByteArray) {
        stats.onBytes(bytes.size)
        val frame = StreamFrames.parse(bytes) ?: return
        if (frame is StreamFrame.Video) {
            framesReceived++
            if (frame.key) keyframesReceived++
        }
        sink.onFrame(frame)
    }

    private suspend fun watchSocket(sc: HelmConnection) {
        val end = sc.state.first { it is ConnectionState.Failed || it is ConnectionState.Closed }
        if (stopping) return
        _state.value = when (end) {
            is ConnectionState.Failed -> StreamState.Failed(end.code, end.message)
            else -> StreamState.Failed(LocalCodes.CONNECTION_LOST, "The PC closed the stream.")
        }
        scope.cancel()
    }

    private suspend fun statsLoop() {
        while (scope.isActive) {
            delay(statsIntervalMs)
            val t0 = System.nanoTime()
            try {
                control.request("system.ping", timeoutMs = 2_000)
                stats.rttMs = ((System.nanoTime() - t0) / 1_000_000).toInt()
            } catch (_: HelmException) {
            }
            val report = stats.snapshot(clock())
            try {
                control.request("stream.stats", report.toParams(), timeoutMs = 2_000)
            } catch (e: HelmException) {
                if (e.code == LocalCodes.NOT_READY || e.code == LocalCodes.CLOSED) return
            }
        }
    }

    /** `stream.idr{}`: ask for a keyframe (e.g. after the decoder was reset). */
    suspend fun requestKeyframe() {
        try {
            control.request("stream.idr")
        } catch (_: HelmException) {
        }
    }

    /** `stream.config{bitrate_kbps?, fps?}` without restarting. */
    suspend fun configure(bitrateKbps: Int? = null, fps: Int? = null): JsonObject =
        control.request("stream.config", buildJsonObject {
            bitrateKbps?.let { put("bitrate_kbps", StreamPreset.clampKbps(it)) }
            fps?.let { put("fps", it) }
        })

    /** Stops the stream (best effort `stream.stop`) and closes the socket. Safe to repeat. */
    suspend fun stop() {
        if (stopping) return
        stopping = true
        val wasStreaming = _state.value is StreamState.Streaming || _state.value is StreamState.Starting
        if (wasStreaming && control.state.value is ConnectionState.Ready) {
            try {
                control.request("stream.stop", timeoutMs = 3_000)
            } catch (_: HelmException) {
            }
        }
        socket?.close()
        _state.value = StreamState.Stopped
        scope.cancel()
    }

    /** Closes without telling the PC (used when the control socket is already gone). */
    fun abandon() {
        stopping = true
        socket?.close()
        _state.value = StreamState.Stopped
        scope.cancel()
    }
}
