package com.niranjan.helm.core

import java.util.Base64
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.TimeoutCancellationException
import kotlinx.coroutines.cancel
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.intOrNull
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/** One way to reach the PC. */
data class Endpoint(val host: String, val port: Int) {
    fun url(scheme: String): String {
        val h = if (':' in host && !host.startsWith("[")) "[$host]" else host
        return "$scheme://$h:$port/ws"
    }

    override fun toString(): String = "$host:$port"
}

/** First-time pairing with a QR token, or a later key-based login. */
sealed interface HandshakeMode {
    data class Pair(val token: String) : HandshakeMode {
        override fun toString(): String = "Pair(token=***)"
    }

    data object Auth : HandshakeMode
}

sealed interface ConnectionState {
    data object Idle : ConnectionState
    data class Connecting(val endpoint: Endpoint) : ConnectionState
    data object Pairing : ConnectionState
    data object Authenticating : ConnectionState
    data class Ready(val tier: String, val endpoint: Endpoint) : ConnectionState
    data class Failed(val code: String, val message: String) : ConnectionState
    data object Closed : ConnectionState
}

/** Result of a successful handshake (`ready`). */
data class ReadyInfo(
    val deviceId: String,
    val tier: String,
    val hostId: String,
    val hostName: String,
    val endpoint: Endpoint,
    val agentVersion: String?,
)

/**
 * A Helm failure. [code] is either an agent code from PROTOCOL.md (`bad_token`, `unknown_device`,
 * `unknown_method`, ...) or a local one (see [LocalCodes]).
 */
open class HelmException(val code: String, message: String, cause: Throwable? = null) :
    Exception(message, cause)

/** The agent answered a request with `ok:false`. */
class HelmRequestException(code: String, message: String) : HelmException(code, message)

object LocalCodes {
    const val UNREACHABLE = "unreachable"
    const val PIN_MISMATCH = "pin_mismatch"
    const val HOST_MISMATCH = "host_mismatch"
    const val BAD_HELLO = "bad_hello"
    const val PROTOCOL_VERSION = "protocol_version"
    const val HANDSHAKE_TIMEOUT = "handshake_timeout"
    const val CONNECTION_LOST = "connection_lost"
    const val REQUEST_TIMEOUT = "request_timeout"
    const val NOT_READY = "not_ready"
    const val CLOSED = "closed"
}

/**
 * The phone side of one control connection (PROTOCOL.md §5–§6). Plain JVM code (no android.*).
 *
 * Usage: [connect] once (tries each endpoint in order), then [request] any number of times,
 * then [close]. A new connection object is needed to reconnect.
 */
class HelmConnection(
    private val hostId: String,
    private val fingerprint: String,
    private val endpoints: List<Endpoint>,
    private val identity: DeviceIdentity,
    private val mode: HandshakeMode,
    private val transport: HelmTransport = PinnedTlsTransport(),
    /** Per endpoint: TCP + TLS + WebSocket upgrade + `hello`. */
    private val helloTimeoutMs: Long = 10_000,
    /** From sending `pair`/`auth` until `ready`/`error`. */
    private val readyTimeoutMs: Long = 20_000,
    private val defaultRequestTimeoutMs: Long = 10_000,
) {
    init {
        require(endpoints.isNotEmpty()) { "at least one endpoint is required" }
    }

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _state = MutableStateFlow<ConnectionState>(ConnectionState.Idle)
    val state: StateFlow<ConnectionState> = _state.asStateFlow()

    private val _events = MutableSharedFlow<JsonObject>(extraBufferCapacity = 64)

    /** `event` messages from the agent (none in Phase 0). */
    val events: SharedFlow<JsonObject> = _events.asSharedFlow()

    private val started = AtomicBoolean(false)
    private val closedByUser = AtomicBoolean(false)
    private val nextId = AtomicLong(0)
    private val pending = ConcurrentHashMap<Long, CompletableDeferred<JsonObject>>()

    @Volatile
    private var socket: WebSocket? = null

    @Volatile
    var readyInfo: ReadyInfo? = null
        private set

    private sealed interface WsEvent {
        data object Open : WsEvent
        data class Text(val text: String) : WsEvent
        data class Failure(val error: Throwable, val httpCode: Int?) : WsEvent
        data class Closed(val code: Int, val reason: String) : WsEvent
    }

    private sealed interface Attempt {
        data class Success(val info: ReadyInfo) : Attempt

        /** Could not reach a Helm agent here; try the next endpoint. */
        data class TryNext(val error: HelmException) : Attempt

        /** Reached the agent and it refused (or broke protocol); stop. */
        data class Stop(val error: HelmException) : Attempt
    }

    /** Opens the connection and completes the handshake. Throws [HelmException] on failure. */
    suspend fun connect(): ReadyInfo {
        check(started.compareAndSet(false, true)) { "connect() may only be called once" }
        val client = try {
            transport.client(fingerprint)
        } catch (e: Exception) {
            throw fail(HelmException(LocalCodes.UNREACHABLE, "Could not set up TLS: ${e.message}", e))
        }
        var lastError: HelmException? = null
        for (endpoint in endpoints) {
            if (closedByUser.get()) throw HelmException(LocalCodes.CLOSED, "Connection was closed.")
            _state.value = ConnectionState.Connecting(endpoint)
            val events = Channel<WsEvent>(Channel.UNLIMITED)
            val request = Request.Builder().url(endpoint.url(transport.scheme)).build()
            val ws = client.newWebSocket(request, ChannelListener(events))
            socket = ws
            when (val attempt = handshake(ws, events, endpoint)) {
                is Attempt.Success -> {
                    readyInfo = attempt.info
                    if (closedByUser.get()) {
                        ws.close(1000, null)
                        throw HelmException(LocalCodes.CLOSED, "Connection was closed.")
                    }
                    _state.value = ConnectionState.Ready(attempt.info.tier, endpoint)
                    scope.launch { readLoop(events) }
                    return attempt.info
                }
                is Attempt.TryNext -> {
                    ws.cancel()
                    socket = null
                    // A pin mismatch is more informative than a later timeout; keep it.
                    if (lastError?.code != LocalCodes.PIN_MISMATCH) lastError = attempt.error
                }
                is Attempt.Stop -> {
                    ws.cancel()
                    socket = null
                    throw fail(attempt.error)
                }
            }
        }
        throw fail(lastError ?: HelmException(LocalCodes.UNREACHABLE, "Could not reach the PC."))
    }

    private fun fail(e: HelmException): HelmException {
        if (!closedByUser.get()) _state.value = ConnectionState.Failed(e.code, e.message ?: e.code)
        scope.cancel()
        return e
    }

    private suspend fun handshake(ws: WebSocket, events: Channel<WsEvent>, endpoint: Endpoint): Attempt {
        // 1. Wait for `hello` (agent speaks first). Anything that fails before a first message
        //    means "no Helm agent reachable here" -> try the next endpoint.
        val first = withTimeoutOrNull(helloTimeoutMs) { nextMessage(events) }
            ?: return Attempt.TryNext(
                HelmException(LocalCodes.UNREACHABLE, "No answer from the PC at $endpoint."),
            )
        val hello: JsonObject = when (first) {
            is Incoming.Message -> first.obj
                ?: return Attempt.Stop(HelmException(LocalCodes.BAD_HELLO, "PC sent a non-JSON message."))
            is Incoming.Failed -> return Attempt.TryNext(transportError(first.error, first.httpCode, endpoint))
            is Incoming.Closed -> return Attempt.TryNext(
                HelmException(LocalCodes.UNREACHABLE, "PC at $endpoint closed the connection."),
            )
        }

        when (hello.str("type")) {
            "hello" -> Unit
            "error" -> return Attempt.Stop(agentError(hello))
            else -> return Attempt.Stop(HelmException(LocalCodes.BAD_HELLO, "PC did not send hello."))
        }
        val protocol = (hello["protocol"] as? JsonPrimitive)?.intOrNull
        if (protocol != 1) {
            return Attempt.Stop(
                HelmException(LocalCodes.PROTOCOL_VERSION, "PC speaks protocol $protocol; this app speaks 1."),
            )
        }
        val helloHostId = hello.str("host_id")
        val nonce = hello.str("nonce")
        if (helloHostId.isNullOrEmpty() || nonce.isNullOrEmpty()) {
            return Attempt.Stop(HelmException(LocalCodes.BAD_HELLO, "PC sent an incomplete hello."))
        }
        if (!helloHostId.equals(hostId, ignoreCase = true)) {
            return Attempt.Stop(
                HelmException(LocalCodes.HOST_MISMATCH, "This address belongs to a different PC."),
            )
        }

        // 2. Answer with exactly one pair or auth.
        val answer = when (mode) {
            is HandshakeMode.Pair -> {
                _state.value = ConnectionState.Pairing
                buildJsonObject {
                    put("type", "pair")
                    put("token", mode.token)
                    put("device_id", identity.deviceId)
                    put("device_name", identity.deviceName)
                    put("public_key", Base64.getEncoder().encodeToString(identity.signer.publicKeySpki()))
                    put(
                        "signature",
                        HelmSigning.signatureBase64(identity.signer, HelmSigning.PURPOSE_PAIR, helloHostId, nonce),
                    )
                }
            }
            HandshakeMode.Auth -> {
                _state.value = ConnectionState.Authenticating
                buildJsonObject {
                    put("type", "auth")
                    put("device_id", identity.deviceId)
                    put(
                        "signature",
                        HelmSigning.signatureBase64(identity.signer, HelmSigning.PURPOSE_AUTH, helloHostId, nonce),
                    )
                }
            }
        }
        if (!ws.send(answer.toString())) {
            return Attempt.Stop(HelmException(LocalCodes.CONNECTION_LOST, "Connection dropped during handshake."))
        }

        // 3. Wait for ready / error (ignore anything else, e.g. future informational messages).
        val obj: JsonObject = withTimeoutOrNull(readyTimeoutMs) {
            var result: Incoming
            while (true) {
                result = nextMessage(events)
                if (result !is Incoming.Message) break
                val t = result.obj?.str("type")
                if (result.obj == null || t == "ready" || t == "error") break
            }
            result
        }.let { r ->
            when (r) {
                null -> return Attempt.Stop(
                    HelmException(LocalCodes.HANDSHAKE_TIMEOUT, "PC did not finish the handshake."),
                )
                is Incoming.Message -> r.obj
                    ?: return Attempt.Stop(HelmException(LocalCodes.BAD_HELLO, "PC sent a non-JSON message."))
                is Incoming.Failed -> return Attempt.Stop(
                    HelmException(
                        LocalCodes.CONNECTION_LOST,
                        "Connection dropped during handshake: ${r.error.message ?: "error"}",
                        r.error,
                    ),
                )
                else -> return Attempt.Stop(
                    HelmException(LocalCodes.CONNECTION_LOST, "PC closed the connection during handshake."),
                )
            }
        }
        if (obj.str("type") == "error") return Attempt.Stop(agentError(obj))
        val readyHost = obj.str("host_id")
        if (readyHost != null && !readyHost.equals(hostId, ignoreCase = true)) {
            return Attempt.Stop(HelmException(LocalCodes.HOST_MISMATCH, "PC identity changed during handshake."))
        }
        return Attempt.Success(
            ReadyInfo(
                deviceId = obj.str("device_id") ?: identity.deviceId,
                tier = obj.str("tier") ?: "viewer",
                hostId = readyHost ?: helloHostId,
                hostName = obj.str("host_name") ?: hello.str("host_name") ?: "PC",
                endpoint = endpoint,
                agentVersion = hello.str("agent_version"),
            ),
        )
    }

    private sealed interface Incoming {
        /** [obj] is null when the text was not a JSON object. */
        data class Message(val obj: JsonObject?) : Incoming
        data class Failed(val error: Throwable, val httpCode: Int?) : Incoming
        data class Closed(val code: Int) : Incoming
    }

    private suspend fun nextMessage(events: Channel<WsEvent>): Incoming {
        while (true) {
            when (val ev = events.receive()) {
                WsEvent.Open -> continue
                is WsEvent.Text -> return Incoming.Message(parseObject(ev.text))
                is WsEvent.Failure -> return Incoming.Failed(ev.error, ev.httpCode)
                is WsEvent.Closed -> return Incoming.Closed(ev.code)
            }
        }
    }

    private suspend fun readLoop(events: Channel<WsEvent>) {
        for (ev in events) {
            when (ev) {
                WsEvent.Open -> Unit
                is WsEvent.Text -> handleMessage(ev.text)
                is WsEvent.Failure -> {
                    endSession(
                        HelmException(LocalCodes.CONNECTION_LOST, "Connection lost: ${ev.error.message ?: "error"}"),
                    )
                    return
                }
                is WsEvent.Closed -> {
                    endSession(null)
                    return
                }
            }
        }
    }

    private fun handleMessage(text: String) {
        val obj = parseObject(text) ?: return
        when (obj.str("type")) {
            "res" -> {
                val id = (obj["id"] as? JsonPrimitive)?.longOrNull ?: return
                val deferred = pending.remove(id) ?: return
                val ok = (obj["ok"] as? JsonPrimitive)?.booleanOrNull == true
                if (ok) {
                    deferred.complete(obj["result"] as? JsonObject ?: JsonObject(emptyMap()))
                } else {
                    val err = obj["error"] as? JsonObject
                    deferred.completeExceptionally(
                        HelmRequestException(
                            err?.str("code") ?: "internal",
                            err?.str("message") ?: "Request failed.",
                        ),
                    )
                }
            }
            "event" -> _events.tryEmit(obj)
            "error" -> {
                // Agent-level error after ready; it will close the socket next.
                endSession(agentError(obj))
            }
            else -> Unit
        }
    }

    private fun endSession(error: HelmException?) {
        val failure = error ?: HelmException(LocalCodes.CLOSED, "Connection closed.")
        for ((_, d) in pending) d.completeExceptionally(failure)
        pending.clear()
        if (closedByUser.get()) {
            _state.value = ConnectionState.Closed
        } else if (error != null) {
            _state.value = ConnectionState.Failed(error.code, error.message ?: error.code)
        } else if (_state.value !is ConnectionState.Failed) {
            _state.value = ConnectionState.Closed
        }
        socket?.cancel()
        socket = null
        scope.cancel()
    }

    /** Sends `req` and waits for the matching `res`. Throws [HelmException] / [HelmRequestException]. */
    suspend fun request(
        method: String,
        params: JsonObject = JsonObject(emptyMap()),
        timeoutMs: Long = defaultRequestTimeoutMs,
    ): JsonObject {
        val ws = socket
        if (ws == null || _state.value !is ConnectionState.Ready) {
            throw HelmException(LocalCodes.NOT_READY, "Not connected.")
        }
        val id = nextId.incrementAndGet()
        val deferred = CompletableDeferred<JsonObject>()
        pending[id] = deferred
        val msg = buildJsonObject {
            put("type", "req")
            put("id", id)
            put("method", method)
            put("params", params)
        }
        try {
            if (!ws.send(msg.toString())) {
                throw HelmException(LocalCodes.CONNECTION_LOST, "Connection is closing.")
            }
            return try {
                withTimeout(timeoutMs) { deferred.await() }
            } catch (e: TimeoutCancellationException) {
                throw HelmException(LocalCodes.REQUEST_TIMEOUT, "PC did not answer $method in time.")
            }
        } finally {
            pending.remove(id)
        }
    }

    /** Closes the connection (normal closure). Safe to call more than once. */
    fun close() {
        if (!closedByUser.compareAndSet(false, true)) return
        socket?.close(1000, "bye")
        val failure = HelmException(LocalCodes.CLOSED, "Connection closed.")
        for ((_, d) in pending) d.completeExceptionally(failure)
        pending.clear()
        _state.value = ConnectionState.Closed
        // Let OkHttp finish the close handshake; drop our reader.
        scope.cancel()
    }

    private fun transportError(t: Throwable, httpCode: Int?, endpoint: Endpoint): HelmException {
        var c: Throwable? = t
        while (c != null) {
            if (c is PinMismatchException) {
                return HelmException(
                    LocalCodes.PIN_MISMATCH,
                    "The PC at $endpoint presented a different certificate than the one you paired with.",
                    t,
                )
            }
            c = c.cause
        }
        val detail = if (httpCode != null) "HTTP $httpCode" else (t.message ?: t.javaClass.simpleName)
        return HelmException(LocalCodes.UNREACHABLE, "Could not reach the PC at $endpoint ($detail).", t)
    }

    private fun agentError(obj: JsonObject): HelmException =
        HelmException(obj.str("code") ?: "error", obj.str("message") ?: "The PC refused the connection.")

    private class ChannelListener(private val events: Channel<WsEvent>) : WebSocketListener() {
        override fun onOpen(webSocket: WebSocket, response: Response) {
            events.trySend(WsEvent.Open)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            events.trySend(WsEvent.Text(text))
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
            events.trySend(WsEvent.Closed(code, reason))
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            events.trySend(WsEvent.Closed(code, reason))
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            events.trySend(WsEvent.Failure(t, response?.code))
        }
    }

    companion object {
        private val json = Json { ignoreUnknownKeys = true }

        internal fun parseObject(text: String): JsonObject? = try {
            json.parseToJsonElement(text) as? JsonObject
        } catch (_: Exception) {
            null
        }

        internal fun JsonObject.str(key: String): String? =
            (this[key] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull

        /** Helper for callers reading results. */
        fun JsonElement?.asStringOrNull(): String? = (this as? JsonPrimitive)?.contentOrNull
    }
}
