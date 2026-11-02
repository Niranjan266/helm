package com.niranjan.helm.core

import java.security.KeyFactory
import java.security.PublicKey
import java.security.SecureRandom
import java.security.Signature
import java.security.spec.X509EncodedKeySpec
import java.util.Base64
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.mockwebserver.Dispatcher
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.RecordedRequest
import okhttp3.WebSocketListener
import okio.ByteString
import okio.ByteString.Companion.toByteString

/**
 * Test double that plays the PC agent side of PROTOCOL.md over a MockWebServer WebSocket.
 * Keeps paired devices across connections so pair-then-auth can be tested.
 */
class FakeAgent(
    val hostId: String = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f",
    val hostName: String = "DESKTOP-TEST",
    var validToken: String = "tok_" + "A".repeat(39),
    /** Overrides the host_id put in hello (to test mismatch). */
    var helloHostId: String? = null,
    /** `hello.features`; null leaves the field out (a Phase 0 agent). */
    var features: List<String>? = listOf("p1"),
) {
    /** Input frames (`type:"in"`) received, in order. */
    val inputs = CopyOnWriteArrayList<JsonObject>()

    /** Files completed by `file.upload.end`, by name. */
    val uploaded = ConcurrentHashMap<String, ByteArray>()

    /** Files `file.download` can serve, by path. */
    val downloadable = ConcurrentHashMap<String, ByteArray>()

    /** Corrupts the sha256 announced for downloads. */
    @Volatile
    var badDownloadHash = false

    @Volatile
    var dockJson: JsonObject = buildJsonObject { putJsonArray("pages") {} }

    @Volatile
    var deckJson: JsonObject = buildJsonObject { putJsonArray("buttons") {} }

    private val sockets = CopyOnWriteArrayList<WebSocket>()

    private class Upload(val name: String, val size: Long, val sha: String) {
        val data = java.io.ByteArrayOutputStream()
        var nextOffset = 0L
        var badOffset = false
    }

    private val uploads = ConcurrentHashMap<String, Upload>()

    /** Sends an event to every authenticated connection. */
    fun broadcast(name: String, data: JsonObject = JsonObject(emptyMap())) {
        val msg = buildJsonObject {
            put("type", "event")
            put("name", name)
            put("data", data)
        }.toString()
        for (ws in sockets) ws.send(msg)
    }

    data class Device(val name: String, val key: PublicKey, var revoked: Boolean = false)

    val devices = ConcurrentHashMap<String, Device>()
    val received = CopyOnWriteArrayList<JsonObject>()
    var tokenUsed = false

    // ---- Phase 2 stream (§10) -----------------------------------------------------------

    /** `stream.start` params received, in order. */
    val streamStarts = CopyOnWriteArrayList<JsonObject>()

    /** `stream.stats` params received, in order. */
    val streamStats = CopyOnWriteArrayList<JsonObject>()

    /** Other stream control methods received (`stream.stop`, `stream.idr`, `stream.config`). */
    val streamCalls = CopyOnWriteArrayList<String>()

    /** Tokens accepted by `stream.open` on the `/stream` socket. */
    val streamOpened = CopyOnWriteArrayList<String>()

    /** G3: `in` frames received on the `/stream` socket, in order. */
    val streamInputs = CopyOnWriteArrayList<JsonObject>()

    /** G3: `windows.thumb` params received, in order. */
    val thumbCalls = CopyOnWriteArrayList<JsonObject>()

    /** Paths of every WebSocket the agent accepted (`/ws`, `/stream`). */
    val paths = CopyOnWriteArrayList<String>()

    // ---- Phase 3 / 4 (§11-§12) ---------------------------------------------------------

    /** Every p3/p4 call received: method + params. */
    val p3Calls = CopyOnWriteArrayList<Pair<String, JsonObject>>()

    @Volatile var alertsJson: JsonObject = buildJsonObject { putJsonArray("rules") {} }
    @Volatile var profilesJson: JsonObject = buildJsonObject { putJsonArray("profiles") {} }
    @Volatile var relayJson: JsonObject = buildJsonObject { put("enabled", false); put("url", "") }

    /** Fake file system for `fs.*`: path -> children (names ending in `\` are folders). */
    val fs = ConcurrentHashMap<String, MutableList<String>>()

    /** Tiers set via `devices.set_tier`. */
    val tiers = ConcurrentHashMap<String, String>()

    /** `system.info.relay`, when set. */
    @Volatile var infoRelay: JsonObject? = null

    // ---- Phase 5 (§13) -------------------------------------------------------------------

    @Volatile var pluginsJson: JsonObject = buildJsonObject {
        putJsonArray("plugins") {
            addJsonObject {
                put("id", "weather"); put("name", "Weather"); put("version", "1.2.0")
                putJsonArray("capabilities") { add(JsonPrimitive("deck.action")); add(JsonPrimitive("http:api.example.com")) }
                putJsonArray("granted") { add(JsonPrimitive("deck.action")) }
                put("enabled", false)
            }
            addJsonObject {
                put("id", "broken"); put("name", "Broken"); put("version", "0.1.0")
                putJsonArray("capabilities") {}
                putJsonArray("granted") {}
                put("enabled", true)
                put("error", "wasm trap")
            }
        }
    }
    @Volatile var triggersJson: JsonObject = buildJsonObject { putJsonArray("triggers") {} }
    @Volatile var haConfig: Pair<String, String>? = null
    @Volatile var recording = false

    /** Bytes written to each open terminal (`term.write`), by term id. */
    val termInput = ConcurrentHashMap<Long, java.io.ByteArrayOutputStream>()

    private val streamTokens = ConcurrentHashMap<String, Boolean>()
    private val streamSockets = CopyOnWriteArrayList<WebSocket>()

    /** Closes every open `/stream` socket (the agent going away). */
    fun closeStreams() {
        for (ws in streamSockets) ws.close(1001, "going away")
        streamSockets.clear()
    }

    /** Binary frames sent right after a valid `stream.open`. */
    @Volatile
    var streamFrames: List<ByteArray> = emptyList()

    /** Routes `/stream` to the stream listener and everything else to the control listener. */
    fun dispatcher(): Dispatcher = object : Dispatcher() {
        override fun dispatch(request: RecordedRequest): MockResponse {
            val path = request.path ?: "/ws"
            paths += path
            return MockResponse().withWebSocketUpgrade(listener(stream = path == "/stream"))
        }
    }

    fun listener(stream: Boolean = false): WebSocketListener = object : WebSocketListener() {
        private lateinit var nonce: String
        private var deviceId: String? = null
        private var authed = false

        override fun onOpen(webSocket: WebSocket, response: Response) {
            nonce = Base64.getEncoder().encodeToString(ByteArray(32).also { SecureRandom().nextBytes(it) })
            webSocket.send(
                buildJsonObject {
                    put("type", "hello")
                    put("protocol", 1)
                    put("host_id", helloHostId ?: hostId)
                    put("host_name", hostName)
                    put("nonce", nonce)
                    put("agent_version", "0.1.0-test")
                    features?.let { f -> putJsonArray("features") { f.forEach { add(JsonPrimitive(it)) } } }
                }.toString(),
            )
        }

        override fun onMessage(webSocket: WebSocket, bytes: ByteString) {
            if (!authed) return error(webSocket, "bad_message", "not authenticated")
            val b = bytes.toByteArray()
            if (b.size < 24) return
            val tid = b.copyOfRange(0, 16).joinToString("") { "%02x".format(it) }
            val offset = java.nio.ByteBuffer.wrap(b, 16, 8).long
            val up = uploads[tid] ?: return
            if (offset != up.nextOffset) up.badOffset = true
            up.data.write(b, 24, b.size - 24)
            up.nextOffset += b.size - 24
        }

        override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
            sockets.remove(webSocket)
        }

        override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
            sockets.remove(webSocket)
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val msg = Json.parseToJsonElement(text) as JsonObject
            received += msg
            if (stream) {
                when (msg.s("type")) {
                    "pair" -> error(webSocket, "bad_message", "pairing is not allowed on /stream")
                    "auth" -> onAuth(webSocket, msg)
                    "stream.open" -> {
                        if (!authed) return error(webSocket, "bad_message", "not authenticated")
                        val token = msg.s("token")
                        if (token == null || streamTokens.remove(token) == null) return error(webSocket, "bad_token", "bad stream token")
                        streamOpened += token
                        streamSockets += webSocket
                        for (f in streamFrames) webSocket.send(f.toByteString())
                    }
                    "in" -> if (authed && "g3" in features.orEmpty()) streamInputs += msg else error(webSocket, "bad_message", "unexpected on /stream")
                    else -> error(webSocket, "bad_message", "unexpected on /stream")
                }
                return
            }
            when (msg.s("type")) {
                "pair" -> onPair(webSocket, msg)
                "auth" -> onAuth(webSocket, msg)
                "req" -> if (authed) onReq(webSocket, msg) else error(webSocket, "bad_message", "not authenticated")
                "in" -> if (authed) inputs += msg else error(webSocket, "bad_message", "not authenticated")
                else -> error(webSocket, "bad_message", "unknown type")
            }
        }

        override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
            webSocket.close(1000, null)
        }

        private fun onPair(ws: WebSocket, msg: JsonObject) {
            val token = msg.s("token")
            if (token != validToken || tokenUsed) return error(ws, "bad_token", "wrong, expired or used token")
            val key = try {
                KeyFactory.getInstance("EC").generatePublic(
                    X509EncodedKeySpec(Base64.getDecoder().decode(msg.s("public_key")!!)),
                )
            } catch (e: Exception) {
                return error(ws, "bad_signature", "bad key")
            }
            if (!verify(key, "pair", msg.s("signature"))) return error(ws, "bad_signature", "bad signature")
            val id = msg.s("device_id")!!
            devices[id] = Device(msg.s("device_name") ?: "?", key)
            tokenUsed = true
            ready(ws, id)
        }

        private fun onAuth(ws: WebSocket, msg: JsonObject) {
            val id = msg.s("device_id")
            val dev = id?.let { devices[it] }
            if (dev == null || dev.revoked) return error(ws, "unknown_device", "not paired")
            if (!verify(dev.key, "auth", msg.s("signature"))) return error(ws, "bad_signature", "bad signature")
            ready(ws, id)
        }

        private fun ready(ws: WebSocket, id: String) {
            deviceId = id
            authed = true
            if (!stream) sockets += ws
            ws.send(
                buildJsonObject {
                    put("type", "ready")
                    put("device_id", id)
                    put("tier", "admin")
                    put("host_id", hostId)
                    put("host_name", hostName)
                }.toString(),
            )
        }

        private fun verify(key: PublicKey, purpose: String, sigB64: String?): Boolean = try {
            val s = Signature.getInstance("SHA256withECDSA")
            s.initVerify(key)
            s.update("helm-v1|$purpose|$hostId|$nonce".toByteArray(Charsets.UTF_8))
            s.verify(Base64.getDecoder().decode(sigB64!!))
        } catch (_: Exception) {
            false
        }

        private fun onReq(ws: WebSocket, msg: JsonObject) {
            val id = (msg["id"] as JsonPrimitive).longOrNull!!
            val params = msg["params"] as? JsonObject ?: JsonObject(emptyMap())
            when (msg.s("method")) {
                "system.ping" -> ok(ws, id, buildJsonObject {
                    put("pong", true)
                    put("time_ms", System.currentTimeMillis())
                })
                "system.info" -> ok(ws, id, buildJsonObject {
                    put("host_id", hostId)
                    put("host_name", hostName)
                    put("os", "Windows 11 Test")
                    put("agent_version", "0.1.0-test")
                    put("uptime_s", 1234)
                    infoRelay?.let { put("relay", it) }
                })
                "devices.list" -> ok(ws, id, buildJsonObject {
                    putJsonArray("devices") {
                        for ((did, d) in devices) {
                            if (d.revoked) continue
                            addJsonObject {
                                put("device_id", did)
                                put("device_name", d.name)
                                put("tier", "admin")
                                put("paired_at_ms", 1L)
                                put("last_seen_ms", 2L)
                                put("this", did == deviceId)
                            }
                        }
                    }
                })
                "devices.revoke" -> {
                    devices[params.s("device_id")]?.revoked = true
                    ok(ws, id, buildJsonObject { put("revoked", true) })
                }
                // Answers after a delay so a later request is answered first.
                "test.slow" -> Thread {
                    Thread.sleep(300)
                    ok(ws, id, buildJsonObject { put("slow", true) })
                }.start()
                "test.never" -> Unit
                "test.emit" -> {
                    ok(ws, id, JsonObject(emptyMap()))
                    broadcast(params.s("name")!!, params["data"] as? JsonObject ?: JsonObject(emptyMap()))
                }
                "dock.get" -> ok(ws, id, dockJson)
                "dock.set" -> {
                    dockJson = buildJsonObject { put("pages", params["pages"]!!) }
                    ok(ws, id, JsonObject(emptyMap()))
                    broadcast("dock.changed")
                }
                "deck.get" -> ok(ws, id, deckJson)
                "deck.set" -> {
                    deckJson = buildJsonObject { put("buttons", params["buttons"]!!) }
                    ok(ws, id, JsonObject(emptyMap()))
                    broadcast("deck.changed")
                }
                "file.upload.begin" -> {
                    val tid = ByteArray(16).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
                    uploads[tid] = Upload(params.s("name")!!, (params["size"] as JsonPrimitive).longOrNull!!, params.s("sha256")!!)
                    ok(ws, id, buildJsonObject { put("transfer_id", tid) })
                }
                "file.upload.end" -> {
                    val up = uploads.remove(params.s("transfer_id"))
                    val bytes = up?.data?.toByteArray()
                    val sha = bytes?.let { sha256Hex(it) }
                    if (up == null || up.badOffset || bytes!!.size.toLong() != up.size || sha != up.sha) {
                        reqError(ws, id, "bad_params", "hash mismatch")
                    } else {
                        uploaded[up.name] = bytes
                        ok(ws, id, buildJsonObject { put("path", "C:\\Users\\test\\Downloads\\Helm\\" + up.name) })
                    }
                }
                "file.download" -> {
                    val path = params.s("path")!!
                    val data = downloadable[path] ?: return reqError(ws, id, "bad_params", "no such file")
                    val tidBytes = ByteArray(16).also { SecureRandom().nextBytes(it) }
                    val tid = tidBytes.joinToString("") { "%02x".format(it) }
                    ok(ws, id, buildJsonObject {
                        put("transfer_id", tid)
                        put("size", data.size)
                        put("sha256", if (badDownloadHash) "00".repeat(32) else sha256Hex(data))
                        put("name", path.substringAfterLast('\\'))
                    })
                    var off = 0
                    while (off < data.size) {
                        val n = minOf(256 * 1024, data.size - off)
                        val frame = java.nio.ByteBuffer.allocate(24 + n).put(tidBytes).putLong(off.toLong()).put(data, off, n).array()
                        ws.send(frame.toByteString())
                        off += n
                    }
                    ws.send(
                        buildJsonObject {
                            put("type", "event")
                            put("name", "file.done")
                            putJsonObject("data") { put("transfer_id", tid) }
                        }.toString(),
                    )
                }
                "test.close" -> ws.close(1000, "bye")
                "stream.start" -> {
                    streamStarts += params
                    val token = "st_" + ByteArray(12).also { SecureRandom().nextBytes(it) }.joinToString("") { "%02x".format(it) }
                    streamTokens[token] = true
                    ok(ws, id, buildJsonObject {
                        put("token", token)
                        put("width", (params["max_width"] as? JsonPrimitive)?.longOrNull ?: 1280)
                        put("height", (params["max_height"] as? JsonPrimitive)?.longOrNull ?: 720)
                        put("fps", (params["fps"] as? JsonPrimitive)?.longOrNull ?: 30)
                        put("codec", "h264")
                        put("encoder", "software")
                        if ("g3" in features.orEmpty()) {
                            put("preset", (params["preset"] as? JsonPrimitive)?.contentOrNull ?: "default")
                            put("input_on_stream", true)
                        }
                    })
                }
                "windows.thumb" -> {
                    thumbCalls += params
                    val hwnd = (params["hwnd"] as? JsonPrimitive)?.longOrNull
                    when (hwnd) {
                        null -> reqError(ws, id, "bad_params", "hwnd required")
                        2L -> ok(ws, id, buildJsonObject { put("png", JsonNull) }) // minimized
                        else -> ok(ws, id, buildJsonObject { put("png", Base64.getEncoder().encodeToString(byteArrayOf(0x89.toByte(), 0x50, 0x4e, 0x47))) })
                    }
                }
                "stream.stats" -> {
                    streamStats += params
                    ok(ws, id, JsonObject(emptyMap()))
                }
                "stream.stop", "stream.idr", "stream.config" -> {
                    streamCalls += msg.s("method")!!
                    ok(ws, id, JsonObject(emptyMap()))
                }
                in P3_METHODS, in P5_METHODS -> {
                    p3Calls += msg.s("method")!! to params
                    val r = phase3(ws, msg.s("method")!!, params)
                    if (r == null) reqError(ws, id, "bad_params", "bad params") else ok(ws, id, r)
                }
                else -> ws.send(
                    buildJsonObject {
                        put("type", "res")
                        put("id", id)
                        put("ok", false)
                        putJsonObject("error") {
                            put("code", "unknown_method")
                            put("message", "no such method")
                        }
                    }.toString(),
                )
            }
        }

        private fun event(ws: WebSocket, name: String, data: JsonObject) {
            ws.send(buildJsonObject { put("type", "event"); put("name", name); put("data", data) }.toString())
        }

        /** Phase 3/4 methods; null = `bad_params`. Events are sent after the reply. */
        private fun phase3(ws: WebSocket, method: String, p: JsonObject): JsonObject? {
            val empty = JsonObject(emptyMap())
            fun num(k: String) = (p[k] as? JsonPrimitive)?.longOrNull
            return when (method) {
                "stats.subscribe" -> {
                    val iv = num("interval_ms") ?: return null
                    if (iv < 500) return null
                    Thread {
                        Thread.sleep(50)
                        for (i in 1..2) event(ws, "stats.tick", statsSample(i))
                    }.start()
                    empty
                }
                "stats.unsubscribe" -> empty
                "alerts.get" -> alertsJson
                "alerts.set" -> {
                    alertsJson = buildJsonObject { put("rules", p["rules"] ?: return null) }
                    Thread {
                        Thread.sleep(50)
                        event(ws, "alert", buildJsonObject { put("id", "cpu"); put("firing", true); put("value", 97.5) })
                    }.start()
                    empty
                }
                "process.list" -> buildJsonObject {
                    putJsonArray("processes") {
                        addJsonObject { put("pid", 4321); put("name", "chrome.exe"); put("exe", "C:\\chrome.exe"); put("cpu_pct", 12.5); put("ram_bytes", 500_000_000) }
                        addJsonObject { put("pid", 99); put("name", "idle.exe"); put("cpu_pct", 0.1); put("ram_bytes", 1000) }
                    }
                }
                "process.kill" -> if (num("pid") == 0L || num("pid") == null) null else empty
                "process.priority" -> if (p.s("level") in listOf("idle", "below_normal", "normal", "above_normal", "high")) empty else null
                "audio.sessions" -> buildJsonObject {
                    putJsonArray("sessions") {
                        addJsonObject { put("id", "{s-1}"); put("name", "Spotify"); put("pid", 77); put("level", 0.5); put("muted", false) }
                    }
                }
                "audio.session.set" -> if (p["id"] == null) null else empty
                "windows.move" -> if (num("monitor_index") == null) null else empty
                "windows.snap" -> if (p.s("pos") in listOf("left", "right", "maximize", "restore")) empty else null
                "desktop.switch" -> if (p.s("dir") in listOf("next", "prev", "new", "close")) empty else null
                "profiles.get" -> profilesJson
                "profiles.set" -> {
                    profilesJson = buildJsonObject { put("profiles", p["profiles"] ?: return null) }
                    Thread {
                        Thread.sleep(50)
                        event(ws, "profile.active", buildJsonObject { put("id", "games") })
                    }.start()
                    empty
                }
                "fs.list" -> {
                    val path = p.s("path") ?: return null
                    val kids = fs[path] ?: return null
                    buildJsonObject {
                        putJsonArray("entries") {
                            for (k in kids) addJsonObject {
                                put("name", k.trimEnd('\\'))
                                put("is_dir", k.endsWith("\\"))
                                put("size", if (k.endsWith("\\")) 0 else 10)
                                put("modified_ms", 1_700_000_000_000)
                            }
                        }
                    }
                }
                "fs.mkdir" -> {
                    val path = p.s("path") ?: return null
                    val parent = PcPath.parent(path)
                    fs.getOrPut(parent) { mutableListOf() } += PcPath.name(path) + "\\"
                    fs[path] = mutableListOf()
                    empty
                }
                "fs.delete" -> {
                    val path = p.s("path") ?: return null
                    val removed = fs[PcPath.parent(path)]?.removeIf { it.trimEnd('\\') == PcPath.name(path) } == true
                    if (removed) empty else null
                }
                "fs.open" -> if (p.s("path").isNullOrEmpty()) null else empty
                "devices.set_tier" -> {
                    val d = p.s("device_id") ?: return null
                    val t = p.s("tier")?.takeIf { it in listOf("viewer", "remote", "admin") } ?: return null
                    tiers[d] = t
                    empty
                }
                "audit.list" -> buildJsonObject {
                    putJsonArray("lines") { repeat((num("limit") ?: 0).toInt().coerceAtMost(3)) { add(JsonPrimitive("line $it")) } }
                }
                "relay.get" -> relayJson
                "relay.set" -> {
                    relayJson = buildJsonObject { put("enabled", p["enabled"] ?: return null); put("url", p["url"] ?: return null) }
                    empty
                }
                "wol.send" -> if (p.s("mac")?.length == 17) empty else null
                else -> phase5(ws, method, p)
            }
        }

        /** Phase 5 methods (§13); null = `bad_params`. */
        private fun phase5(ws: WebSocket, method: String, p: JsonObject): JsonObject? {
            val empty = JsonObject(emptyMap())
            fun num(k: String) = (p[k] as? JsonPrimitive)?.longOrNull
            fun later(ms: Long, body: () -> Unit) = Thread { Thread.sleep(ms); body() }.start()
            return when (method) {
                "record.start" -> {
                    if (recording || p["audio"] == null) return null
                    recording = true
                    buildJsonObject { put("path", REC_PATH) }
                }
                "record.stop" -> {
                    if (!recording) return null
                    recording = false
                    buildJsonObject { put("path", REC_PATH); put("duration_ms", 4200) }
                }
                "term.open" -> {
                    if (p.s("shell") !in listOf("powershell", "cmd") || num("cols") == null || num("rows") == null) return null
                    val tid = 7L + termInput.size
                    termInput[tid] = java.io.ByteArrayOutputStream()
                    later(50) { termOut(ws, tid, PROMPT) }
                    buildJsonObject { put("term_id", tid) }
                }
                "term.write" -> {
                    val tid = num("term_id") ?: return null
                    val buf = termInput[tid] ?: return null
                    val data = Base64.getDecoder().decode(p.s("data_b64") ?: return null)
                    buf.write(data)
                    later(20) { termOut(ws, tid, String(data, Charsets.UTF_8)) }
                    empty
                }
                "term.resize" -> if (num("term_id")?.let { termInput[it] } != null && num("cols") != null && num("rows") != null) empty else null
                "term.close" -> {
                    val tid = num("term_id") ?: return null
                    termInput.remove(tid) ?: return null
                    later(20) { event(ws, "term.exit", buildJsonObject { put("term_id", tid); put("code", 0) }) }
                    empty
                }
                "notify.push" -> {
                    val key = p.s("key") ?: return null
                    if (p.s("app") == null || p.s("title") == null || p["actions"] == null) return null
                    if ((p["reply"] as? JsonPrimitive)?.content == "true") {
                        later(50) {
                            event(ws, "notify.action", buildJsonObject { put("key", key); put("action_id", "reply"); put("reply_text", "on my way") })
                        }
                    }
                    empty
                }
                "plugins.list" -> pluginsJson
                "plugins.set" -> {
                    val pid = p.s("id") ?: return null
                    val granted = p["granted"] as? JsonArray ?: return null
                    val enabled = p["enabled"] ?: return null
                    val list = (pluginsJson["plugins"] as JsonArray).map { o ->
                        val obj = o as JsonObject
                        if (obj.s("id") != pid) obj else JsonObject(obj + mapOf("granted" to granted, "enabled" to enabled))
                    }
                    pluginsJson = JsonObject(mapOf("plugins" to JsonArray(list)))
                    empty
                }
                "triggers.get" -> triggersJson
                "triggers.set" -> {
                    triggersJson = buildJsonObject { put("triggers", p["triggers"] ?: return null) }
                    empty
                }
                "ha.config.set" -> {
                    haConfig = (p.s("url") ?: return null) to (p.s("token") ?: return null)
                    empty
                }
                "ha.states" -> {
                    if (haConfig == null) return null
                    buildJsonObject {
                        putJsonArray("states") {
                            addJsonObject {
                                put("entity_id", "switch.fan"); put("state", "off")
                                putJsonObject("attributes") {}
                            }
                            addJsonObject {
                                put("entity_id", "light.desk"); put("state", "on"); put("last_changed", "2026-10-31T10:00:00Z")
                                putJsonObject("attributes") { put("friendly_name", "Desk lamp"); put("brightness", 200) }
                            }
                        }
                    }
                }
                else -> null
            }
        }

        private fun termOut(ws: WebSocket, tid: Long, text: String) {
            event(ws, "term.output", buildJsonObject {
                put("term_id", tid)
                put("data_b64", java.util.Base64.getEncoder().encodeToString(text.toByteArray(Charsets.UTF_8)))
            })
        }

        private fun reqError(ws: WebSocket, id: Long, code: String, message: String) {
            ws.send(
                buildJsonObject {
                    put("type", "res")
                    put("id", id)
                    put("ok", false)
                    putJsonObject("error") {
                        put("code", code)
                        put("message", message)
                    }
                }.toString(),
            )
        }

        private fun ok(ws: WebSocket, id: Long, result: JsonObject) {
            ws.send(
                buildJsonObject {
                    put("type", "res")
                    put("id", id)
                    put("ok", true)
                    put("result", result)
                }.toString(),
            )
        }

        private fun error(ws: WebSocket, code: String, message: String) {
            ws.send(
                buildJsonObject {
                    put("type", "error")
                    put("code", code)
                    put("message", message)
                }.toString(),
            )
            ws.close(1008, code)
        }
    }

    companion object {
        val P3_METHODS = setOf(
            "stats.subscribe", "stats.unsubscribe", "alerts.get", "alerts.set", "process.list", "process.kill",
            "process.priority", "audio.sessions", "audio.session.set", "windows.move", "windows.snap",
            "desktop.switch", "profiles.get", "profiles.set", "fs.list", "fs.delete", "fs.mkdir", "fs.open",
            "devices.set_tier", "audit.list", "relay.get", "relay.set", "wol.send",
        )

        const val REC_PATH = "C:\\Users\\test\\Videos\\Helm\\rec-1.mp4"
        const val PROMPT = "\u001b[32mPS\u001b[0m C:\\> "

        val P5_METHODS = setOf(
            "record.start", "record.stop", "term.open", "term.write", "term.resize", "term.close", "notify.push",
            "plugins.list", "plugins.set", "triggers.get", "triggers.set", "ha.config.set", "ha.states",
        )

        /** A `helm-win-sysmon` `Stats` snapshot. */
        fun statsSample(i: Int): JsonObject = buildJsonObject {
            put("timestamp_ms", 1_700_000_000_000L + i * 1000)
            put("interval_ms", 1000)
            put("cpu_total", 10.0 * i)
            putJsonArray("cpu_per_core") { add(JsonPrimitive(5.0)); add(JsonPrimitive(15.0 * i)) }
            put("cpu_temp_c", kotlinx.serialization.json.JsonNull)
            put("ram_used", 4_000_000_000L)
            put("ram_total", 16_000_000_000L)
            put("swap_used", 0)
            put("swap_total", 0)
            putJsonArray("disks") {
                addJsonObject {
                    put("mount", "C:\\"); put("label", "OS"); put("used", 750L); put("total", 1000L)
                    put("read_bps", 100); put("write_bps", 200); put("removable", false)
                }
            }
            putJsonArray("net") { addJsonObject { put("name", "Wi-Fi"); put("rx_bps", 1000); put("tx_bps", 500); put("rx_total", 1); put("tx_total", 1) } }
            putJsonArray("gpu") { addJsonObject { put("name", "GPU0"); put("load_pct", 33.0); put("vram_used", 1); put("vram_total", 2); put("temp_c", kotlinx.serialization.json.JsonNull) } }
        }

        fun JsonObject.s(k: String): String? = (this[k] as? JsonPrimitive)?.contentOrNull

        fun sha256Hex(b: ByteArray): String =
            java.security.MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it) }
    }
}
