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
) {
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
                }.toString(),
            )
        }

        override fun onMessage(webSocket: WebSocket, text: String) {
            val msg = Json.parseToJsonElement(text) as JsonObject
            received += msg
            when (msg.s("type")) {
                "pair" -> onPair(webSocket, msg)
                "auth" -> onAuth(webSocket, msg)
                "req" -> if (authed) onReq(webSocket, msg) else error(webSocket, "bad_message", "not authenticated")
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
    }
}
