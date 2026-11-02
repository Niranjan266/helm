package com.niranjan.helm.core

import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
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
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeout
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
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
import okio.ByteString
import okio.ByteString.Companion.toByteString

/** One way to reach the PC. */
data class Endpoint(val host: String, val port: Int) {
    /** [path] is `/ws` for the control socket, `/stream` for the Phase 2 stream socket (§10). */
    fun url(scheme: String, path: String = CONTROL_PATH): String {
        val h = if (':' in host && !host.startsWith("[")) "[$host]" else host
        return "$scheme://$h:$port$path"
    }

    companion object {
        const val CONTROL_PATH = "/ws"
        const val STREAM_PATH = "/stream"
    }

    override fun toString(): String = "$host:$port"
}

/** First-time pairing with a QR token, or a later key-based login. */
sealed interface HandshakeMode {
    data class Pair(val token: String) : HandshakeMode {
        override fun toString(): String = "Pair(token=***)"
    }

    data object Auth : HandshakeMode

    /**
     * g4 short-code pairing (Docs/protocol/pairing-security.md): the 6-digit code shown on the
     * PC. The transport must be a [FingerprintObserver]; the observed certificate is trusted only
     * after the code exchange proves it, and is then returned in [ReadyInfo.fingerprint].
     */
    data class PairCode(val code: String) : HandshakeMode {
        override fun toString(): String = "PairCode(code=***)"
    }
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
    /** `hello.features` (e.g. `p1` for the Phase 1 methods). Empty for Phase 0 agents. */
    val features: Set<String> = emptySet(),
    /** True when this session runs inside a relay tunnel (§12). */
    val viaRelay: Boolean = false,
    /** g4 code pairing: the PC certificate fingerprint the code exchange proved (pin it). */
    val fingerprint: String? = null,
)

/** An `event` message from the agent (PROTOCOL.md §6, §9.4). */
data class HelmEvent(val name: String, val data: JsonObject)

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
    const val TRANSFER_FAILED = "transfer_failed"
    const val BAD_STREAM = "bad_stream"
    const val UNSUPPORTED = "unsupported"

    /** g4: the PC's code proof did not match (wrong code, or someone in the middle). */
    const val BAD_CODE = "bad_code"
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
    /** WebSocket path: [Endpoint.CONTROL_PATH] or [Endpoint.STREAM_PATH]. */
    private val path: String = Endpoint.CONTROL_PATH,
    /**
     * When set, every binary frame after `ready` goes here (on the reader coroutine) instead of
     * the file-transfer router. Used by the stream socket (§10).
     */
    private val binarySink: ((ByteArray) -> Unit)? = null,
    /**
     * Phase 4 fallback (§12): tried after every LAN endpoint failed to reach the agent. Only for
     * [HandshakeMode.Auth]; pairing never goes over the relay.
     */
    private val relay: HelmTransport? = null,
) {
    init {
        require(endpoints.isNotEmpty() || relay != null) { "at least one endpoint is required" }
        require(mode is HandshakeMode.Auth || path == Endpoint.CONTROL_PATH) { "pairing is only allowed on the control socket" }
        require(mode is HandshakeMode.Auth || (relay == null && !transport.relayed)) { "pairing is never allowed over the relay" }
        require(relay == null || relay.relayed) { "relay transport must be relayed" }
        require(mode !is HandshakeMode.PairCode || transport is FingerprintObserver) { "code pairing needs a FingerprintObserver transport" }
    }

    private class Route(val endpoint: Endpoint, val transport: HelmTransport)

    private val routes: List<Route> = endpoints.map { Route(it, transport) } +
        listOfNotNull(relay?.let { Route(Endpoint(TunnelProtocol.innerHost(hostId), TunnelProtocol.INNER_PORT), it) })

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val _state = MutableStateFlow<ConnectionState>(ConnectionState.Idle)
    val state: StateFlow<ConnectionState> = _state.asStateFlow()

    private val _events = MutableSharedFlow<HelmEvent>(extraBufferCapacity = 256)

    /** `event` messages from the agent (§9.4). Hot: only events after subscribing are seen. */
    val events: SharedFlow<HelmEvent> = _events.asSharedFlow()

    /** Events with one [name] only. */
    fun events(name: String): Flow<HelmEvent> = events.filter { it.name == name }

    private val _approvalPending = MutableStateFlow(false)

    /** g4: the PC asked its owner to Allow / Deny this pairing (`pair_pending`); still waiting. */
    val approvalPending: StateFlow<Boolean> = _approvalPending.asStateFlow()

    private val started = AtomicBoolean(false)
    private val closedByUser = AtomicBoolean(false)
    private val nextId = AtomicLong(0)
    private val pending = ConcurrentHashMap<Long, CompletableDeferred<JsonObject>>()

    /** Runs on the reader before a successful `res` completes (so binary frames are not missed). */
    private val resultHooks = ConcurrentHashMap<Long, (JsonObject) -> Unit>()

    /** Incoming file data per transfer id (lowercase hex); closed on `file.done`. */
    private val downloads = ConcurrentHashMap<String, Channel<BinaryFrame>>()

    @Volatile
    private var socket: WebSocket? = null

    @Volatile
    var readyInfo: ReadyInfo? = null
        private set

    private sealed interface WsEvent {
        data object Open : WsEvent
        data class Text(val text: String) : WsEvent
        class Binary(val bytes: ByteArray) : WsEvent
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
        val clients = HashMap<HelmTransport, okhttp3.OkHttpClient>()
        var lastError: HelmException? = null
        for (route in routes) {
            val endpoint = route.endpoint
            if (closedByUser.get()) throw HelmException(LocalCodes.CLOSED, "Connection was closed.")
            val client = clients[route.transport] ?: try {
                route.transport.client(fingerprint).also { clients[route.transport] = it }
            } catch (e: Exception) {
                throw fail(HelmException(LocalCodes.UNREACHABLE, "Could not set up TLS: ${e.message}", e))
            }
            _state.value = ConnectionState.Connecting(endpoint)
            val events = Channel<WsEvent>(Channel.UNLIMITED)
            val request = Request.Builder().url(endpoint.url(route.transport.scheme, path)).build()
            (route.transport as? FingerprintObserver)?.resetObserved()
            val ws = client.newWebSocket(request, ChannelListener(events))
            socket = ws
            when (val attempt = handshake(ws, events, endpoint, route.transport as? FingerprintObserver)) {
                is Attempt.Success -> {
                    readyInfo = attempt.info.copy(viaRelay = route.transport.relayed)
                    hostFeatures = attempt.info.features
                    if (closedByUser.get()) {
                        ws.close(1000, null)
                        throw HelmException(LocalCodes.CLOSED, "Connection was closed.")
                    }
                    _state.value = ConnectionState.Ready(attempt.info.tier, endpoint)
                    scope.launch { readLoop(events) }
                    return readyInfo!!
                }
                is Attempt.TryNext -> {
                    ws.cancel()
                    socket = null
                    // A pin mismatch is more informative than a later timeout; keep it.
                    if (lastError?.code != LocalCodes.PIN_MISMATCH) lastError = attempt.error                }
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

    private suspend fun handshake(
        ws: WebSocket,
        events: Channel<WsEvent>,
        endpoint: Endpoint,
        observer: FingerprintObserver? = null,
    ): Attempt {
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

        val helloFeatures = (hello["features"] as? JsonArray).orEmpty()
            .mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }
            .toSet()
        var codePhone: CodePairingPhone? = null
        var codeFingerprint: String? = null

        // 2. Answer with exactly one pair, pair_code or auth.
        val answer = when (mode) {
            is HandshakeMode.PairCode -> {
                _state.value = ConnectionState.Pairing
                if (Features.G4 !in helloFeatures) {
                    return Attempt.Stop(
                        HelmException(LocalCodes.UNSUPPORTED, "Helm on this PC is too old for code pairing. Update it, or pair with the QR code."),
                    )
                }
                val digits = CodePairingCrypto.parseCode(mode.code)
                    ?: return Attempt.Stop(HelmException(LocalCodes.BAD_CODE, "The pairing code has 6 digits."))
                val fp = observer?.observedFingerprint()
                    ?: return Attempt.Stop(HelmException(LocalCodes.UNREACHABLE, "Could not read the PC's certificate."))
                val noncePhone = HelmSigning.b64(ByteArray(32).also { java.security.SecureRandom().nextBytes(it) })
                val publicKey = Base64.getEncoder().encodeToString(identity.signer.publicKeySpki())
                val h = CodePairingCrypto.transcriptHash(helloHostId, nonce, noncePhone, fp, identity.deviceId, publicKey)
                val phone = CodePairingPhone(digits, h)
                codePhone = phone
                codeFingerprint = fp
                buildJsonObject {
                    put("type", "pair_code")
                    put("device_id", identity.deviceId)
                    put("device_name", identity.deviceName)
                    put("public_key", publicKey)
                    put("nonce", noncePhone)
                    put("commits", JsonArray(phone.commitments.map { JsonPrimitive(HelmSigning.b64(it)) }))
                    put(
                        "signature",
                        HelmSigning.signatureBase64(identity.signer, HelmSigning.PURPOSE_PAIR_CODE, helloHostId, nonce),
                    )
                }
            }
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
        codePhone?.let { phone -> codeExchange(ws, events, phone)?.let { return it } }

        // 3. Wait for ready / error (ignore anything else, e.g. future informational messages).
        //    `pair_pending` (g4): the PC owner is asked; wait as long as the PC says (+10 s).
        var deadline = System.currentTimeMillis() + readyTimeoutMs
        val obj: JsonObject = run {
            var result: Incoming?
            while (true) {
                val left = deadline - System.currentTimeMillis()
                result = if (left <= 0) null else withTimeoutOrNull(left) { nextMessage(events) }
                if (result !is Incoming.Message) break
                val t = result.obj?.str("type")
                if (t == "pair_pending") {
                    val waitS = (result.obj?.get("timeout_s") as? JsonPrimitive)?.longOrNull?.coerceIn(1, 600) ?: 60
                    deadline = System.currentTimeMillis() + (waitS + 10) * 1000
                    _approvalPending.value = true
                    continue
                }
                if (result.obj == null || t == "ready" || t == "error") break
            }
            _approvalPending.value = false
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
                features = helloFeatures,
                fingerprint = codeFingerprint,
            ),
        )
    }

    /**
     * g4: `code_commit`, then six rounds of PC opening -> check -> our opening. Returns null when
     * every PC opening matched (the observed certificate is the PC's), else the failure.
     */
    private suspend fun codeExchange(ws: WebSocket, events: Channel<WsEvent>, phone: CodePairingPhone): Attempt? {
        suspend fun next(): JsonObject? {
            val r = withTimeoutOrNull(helloTimeoutMs) { nextMessage(events) }
            return (r as? Incoming.Message)?.obj
        }
        val lost = Attempt.Stop(HelmException(LocalCodes.CONNECTION_LOST, "Connection dropped during pairing."))
        val commit = next() ?: return lost
        if (commit.str("type") == "error") return Attempt.Stop(agentError(commit))
        val commits = (commit["commits"] as? JsonArray).orEmpty().mapNotNull {
            runCatching { Base64.getDecoder().decode((it as JsonPrimitive).content) }.getOrNull()
        }
        if (commit.str("type") != "code_commit" || !phone.setAgentCommitments(commits)) {
            return Attempt.Stop(HelmException(LocalCodes.BAD_HELLO, "PC sent an invalid code commitment."))
        }
        for (round in 1..CodePairingCrypto.DIGITS) {
            val open = next() ?: return lost
            if (open.str("type") == "error") return Attempt.Stop(agentError(open))
            val r = open.str("r")?.let { runCatching { Base64.getDecoder().decode(it) }.getOrNull() }
            val i = (open["i"] as? JsonPrimitive)?.intOrNull
            if (open.str("type") != "code_open" || i != round || r == null || !phone.checkAgentOpening(round, r)) {
                ws.send(
                    buildJsonObject {
                        put("type", "error")
                        put("code", LocalCodes.BAD_CODE)
                        put("message", "code proof mismatch")
                    }.toString(),
                )
                return Attempt.Stop(
                    HelmException(
                        LocalCodes.BAD_CODE,
                        "The code did not match. Show a new code on the PC and try again. " +
                            "If it keeps failing, another device on this network may be pretending to be the PC.",
                    ),
                )
            }
            val reply = buildJsonObject {
                put("type", "code_open")
                put("i", round)
                put("r", HelmSigning.b64(phone.opening(round)))
            }
            if (!ws.send(reply.toString())) return lost
        }
        return if (phone.complete) null else lost
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
                is WsEvent.Binary -> continue
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
                is WsEvent.Binary -> handleBinary(ev.bytes)
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
                    val result = obj["result"] as? JsonObject ?: JsonObject(emptyMap())
                    resultHooks.remove(id)?.let { hook -> runCatching { hook(result) } }
                    deferred.complete(result)
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
            "event" -> {
                val name = obj.str("name") ?: return
                val data = obj["data"] as? JsonObject ?: JsonObject(emptyMap())
                if (name == "file.done") {
                    data.str("transfer_id")?.lowercase()?.let { downloads.remove(it)?.close() }
                }
                _events.tryEmit(HelmEvent(name, data))
            }
            "error" -> {
                // Agent-level error after ready; it will close the socket next.
                endSession(agentError(obj))
            }
            else -> Unit
        }
    }

    private fun handleBinary(bytes: ByteArray) {
        binarySink?.let { sink ->
            runCatching { sink(bytes) }
            return
        }
        val frame = BinaryFrame.decode(bytes) ?: return
        downloads[frame.transferIdHex]?.trySend(frame)
    }

    private fun endSession(error: HelmException?) {
        val failure = error ?: HelmException(LocalCodes.CLOSED, "Connection closed.")
        for ((_, d) in pending) d.completeExceptionally(failure)
        pending.clear()
        failDownloads(failure)
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

    private fun failDownloads(failure: HelmException) {
        for ((_, ch) in downloads) ch.close(failure)
        downloads.clear()
    }

    /** Features from `hello` once ready (empty before). */
    @Volatile
    var hostFeatures: Set<String> = emptySet()
        private set

    /** True when the agent advertises Phase 1 (`p1`). */
    val supportsPhase1: Boolean get() = "p1" in hostFeatures

    /** True when the agent advertises the Phase 2 screen/audio stream (`p2`, §10). */
    val supportsPhase2: Boolean get() = "p2" in hostFeatures

    /** Phase 3 power-user methods (`p3`, §11). */
    val supportsPhase3: Boolean get() = Features.P3 in hostFeatures

    /** Phase 4 anywhere access (`p4`, §12): relay config, `wol.send`, `gy` input. */
    val supportsPhase4: Boolean get() = Features.P4 in hostFeatures

    /** Phase 5 extras (`p5`, §13): recording, terminal, pen, notifications, plugins, triggers, HA. */
    val supportsPhase5: Boolean get() = Features.P5 in hostFeatures

    /**
     * Sends a fire-and-forget input frame (`{"type":"in","k":…}`, §9.1): no id, no reply.
     * Returns false when not connected or the socket is closing.
     */
    fun sendInput(frame: InputFrame): Boolean {
        inputDetour?.let { if (it(frame)) return true } // G3: input over the /stream socket
        val ws = socket ?: return false
        if (_state.value !is ConnectionState.Ready) return false
        return ws.send(frame.toJson().toString())
    }

    /** Sends one raw JSON text frame (e.g. `stream.open`, §10). Returns false when not connected. */
    /**
     * G3: while set, [sendInput] first offers each frame here (the running stream's `/stream`
     * socket); `false` falls back to this connection.
     */
    @Volatile
    var inputDetour: ((InputFrame) -> Boolean)? = null

    fun sendText(obj: JsonObject): Boolean {
        val ws = socket ?: return false
        if (_state.value !is ConnectionState.Ready) return false
        return ws.send(obj.toString())
    }

    /** Sends one binary frame (§9.2). Returns false when not connected. */
    fun sendBinary(frame: BinaryFrame): Boolean {
        val ws = socket ?: return false
        if (_state.value !is ConnectionState.Ready) return false
        return ws.send(frame.encode().toByteString())
    }

    /** Waits until fewer than [limit] bytes are queued (OkHttp closes the socket past 16 MiB). */
    suspend fun awaitQueueBelow(limit: Long) {
        while (true) {
            val ws = socket ?: throw HelmException(LocalCodes.CONNECTION_LOST, "Connection lost.")
            if (ws.queueSize() < limit) return
            delay(5)
        }
    }

    /**
     * Uploads one file (§9.2-9.3): `file.upload.begin`, binary frames of at most 256 KiB, then
     * `file.upload.end`. [open] must return a fresh stream of exactly [size] bytes whose
     * SHA-256 is [sha256Hex]. Returns the path on the PC.
     */
    suspend fun uploadFile(
        name: String,
        size: Long,
        sha256Hex: String,
        open: () -> InputStream,
        onProgress: (sent: Long) -> Unit = {},
    ): String {
        val begin = request(
            "file.upload.begin",
            buildJsonObject {
                put("name", name)
                put("size", size)
                put("sha256", sha256Hex.lowercase())
            },
        )
        val tid = begin.str("transfer_id")
            ?: throw HelmException(LocalCodes.TRANSFER_FAILED, "PC did not return a transfer id.")
        val idBytes = BinaryFrame.idFromHex(tid)
            ?: throw HelmException(LocalCodes.TRANSFER_FAILED, "PC returned a bad transfer id.")
        var offset = 0L
        withContext(Dispatchers.IO) {
            open().use { input ->
                val buf = ByteArray(BinaryFrame.MAX_DATA)
                while (true) {
                    val n = input.readFully(buf)
                    if (n <= 0) break
                    if (offset + n > size) {
                        throw HelmException(LocalCodes.TRANSFER_FAILED, "File is larger than expected.")
                    }
                    awaitQueueBelow(UPLOAD_QUEUE_LIMIT)
                    if (!sendBinary(BinaryFrame(idBytes, offset, buf.copyOf(n)))) {
                        throw HelmException(LocalCodes.CONNECTION_LOST, "Connection lost during upload.")
                    }
                    offset += n
                    onProgress(offset)
                    if (n < buf.size) break
                }
            }
        }
        if (offset != size) {
            throw HelmException(LocalCodes.TRANSFER_FAILED, "File changed while sending ($offset of $size bytes).")
        }
        val end = request("file.upload.end", buildJsonObject { put("transfer_id", tid) }, timeoutMs = 60_000)
        return end.str("path") ?: ""
    }

    /** Result of [downloadFile]. */
    data class Download(val name: String, val size: Long, val sha256Hex: String)

    /**
     * Downloads a file from the PC (`file.download`, admin tier) into [out]. Verifies offsets,
     * size and SHA-256. [idleTimeoutMs] bounds the wait for each frame.
     */
    suspend fun downloadFile(
        path: String,
        out: OutputStream,
        idleTimeoutMs: Long = 30_000,
        onProgress: (received: Long, total: Long) -> Unit = { _, _ -> },
    ): Download {
        var channel: Channel<BinaryFrame>? = null
        val res = request(
            "file.download",
            buildJsonObject { put("path", path) },
            beforeComplete = { r ->
                // Register the sink on the reader, before any data frame is handled.
                r.str("transfer_id")?.lowercase()?.let { id ->
                    channel = Channel<BinaryFrame>(Channel.UNLIMITED).also { downloads[id] = it }
                }
            },
        )
        val tid = res.str("transfer_id")?.lowercase()
        val ch = channel
        if (tid == null || ch == null) throw HelmException(LocalCodes.TRANSFER_FAILED, "PC did not return a transfer id.")
        val size = (res["size"] as? JsonPrimitive)?.longOrNull ?: -1
        val expected = res.str("sha256")?.lowercase()
        val name = res.str("name") ?: path.substringAfterLast('\\').substringAfterLast('/')
        val digest = MessageDigest.getInstance("SHA-256")
        var received = 0L
        try {
            while (true) {
                val next = withTimeoutOrNull(idleTimeoutMs) { ch.receiveCatching() }
                    ?: throw HelmException(LocalCodes.TRANSFER_FAILED, "PC stopped sending the file.")
                if (next.isClosed) {
                    val cause = next.exceptionOrNull() ?: break
                    if (cause is HelmException) throw cause
                    throw HelmException(LocalCodes.TRANSFER_FAILED, cause.message ?: "Download failed.")
                }
                val frame = next.getOrThrow()
                if (frame.offset != received) {
                    throw HelmException(LocalCodes.TRANSFER_FAILED, "Out-of-order data at ${frame.offset} (expected $received).")
                }
                withContext(Dispatchers.IO) { out.write(frame.data) }
                digest.update(frame.data)
                received += frame.data.size
                onProgress(received, size)
                if (size in 0 until received) throw HelmException(LocalCodes.TRANSFER_FAILED, "PC sent too much data.")
            }
        } finally {
            downloads.remove(tid)
        }
        if (size >= 0 && received != size) {
            throw HelmException(LocalCodes.TRANSFER_FAILED, "Got $received of $size bytes.")
        }
        val actual = digest.digest().joinToString("") { "%02x".format(it) }
        if (expected != null && expected != actual) {
            throw HelmException(LocalCodes.TRANSFER_FAILED, "Checksum mismatch.")
        }
        return Download(name, received, actual)
    }

    /** Sends `req` and waits for the matching `res`. Throws [HelmException] / [HelmRequestException]. */
    suspend fun request(
        method: String,
        params: JsonObject = JsonObject(emptyMap()),
        timeoutMs: Long = defaultRequestTimeoutMs,
        beforeComplete: ((JsonObject) -> Unit)? = null,
    ): JsonObject {
        val ws = socket
        if (ws == null || _state.value !is ConnectionState.Ready) {
            throw HelmException(LocalCodes.NOT_READY, "Not connected.")
        }
        val id = nextId.incrementAndGet()
        val deferred = CompletableDeferred<JsonObject>()
        pending[id] = deferred
        if (beforeComplete != null) resultHooks[id] = beforeComplete
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
            resultHooks.remove(id)
        }
    }

    /** Closes the connection (normal closure). Safe to call more than once. */
    fun close() {
        if (!closedByUser.compareAndSet(false, true)) return
        socket?.close(1000, "bye")
        val failure = HelmException(LocalCodes.CLOSED, "Connection closed.")
        for ((_, d) in pending) d.completeExceptionally(failure)
        pending.clear()
        failDownloads(failure)
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

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            events.trySend(WsEvent.Binary(bytes.toByteArray()))
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
        /** Keep at most this much file data queued in OkHttp (its hard limit is 16 MiB). */
        private const val UPLOAD_QUEUE_LIMIT = 2L * 1024 * 1024

        private fun InputStream.readFully(buf: ByteArray): Int {
            var total = 0
            while (total < buf.size) {
                val n = read(buf, total, buf.size - total)
                if (n < 0) break
                total += n
            }
            return total
        }

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
