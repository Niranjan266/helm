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
import kotlinx.serialization.json.JsonObject
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

    fun listener(): WebSocketListener = object : WebSocketListener() {
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
            sockets += ws
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
        fun JsonObject.s(k: String): String? = (this[k] as? JsonPrimitive)?.contentOrNull

        fun sha256Hex(b: ByteArray): String =
            java.security.MessageDigest.getInstance("SHA-256").digest(b).joinToString("") { "%02x".format(it) }
    }
}
