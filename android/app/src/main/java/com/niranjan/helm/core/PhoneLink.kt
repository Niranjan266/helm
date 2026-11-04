package com.niranjan.helm.core

import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// Typed g9 calls (Docs/protocol/phone-control.md). Gated on the hello feature: against an older
// agent they throw `unsupported` without sending anything.

private suspend fun HelmApi.g9(method: String, params: JsonObject, timeoutMs: Long = 10_000) =
    gated(PhoneControl.FEATURE, method, params, timeoutMs)

/** Tells the PC what this phone currently allows. */
suspend fun HelmApi.phoneCaps(enabled: Boolean, accessibility: Boolean) {
    g9("phone.caps", buildJsonObject { put("enabled", enabled); put("accessibility", accessibility) })
}

suspend fun HelmApi.phoneDecline(requestId: Long, reason: String) {
    g9("phone.decline", buildJsonObject { put("request_id", requestId); put("reason", reason) })
}

suspend fun HelmApi.phoneStop(requestId: Long, reason: String) {
    g9("phone.stop", buildJsonObject { put("request_id", requestId); put("reason", reason) })
}

suspend fun HelmApi.phoneState(requestId: Long, change: PhoneStateChange) {
    g9("phone.state", change.toParams(requestId))
}

/** `phone.state`: only what changed. `secureChanged` with a null [secure] clears the notice. */
data class PhoneStateChange(
    val width: Int? = null,
    val height: Int? = null,
    val rotation: Int? = null,
    val control: Boolean? = null,
    val accessibility: Boolean? = null,
    val secureChanged: Boolean = false,
    val secure: String? = null,
) {
    fun toParams(requestId: Long): JsonObject = buildJsonObject {
        put("request_id", requestId)
        width?.let { put("width", it) }
        height?.let { put("height", it) }
        rotation?.let { put("rotation", it) }
        control?.let { put("control", it) }
        accessibility?.let { put("accessibility", it) }
        if (secureChanged) {
            if (secure != null) put("secure", secure) else put("secure", JsonNull)
        }
    }
}

/**
 * The live link of one session, on top of the control socket and a `/stream` socket: announce the
 * picture (`phone.accept`), open the socket, then send frames and receive input. Plain JVM code.
 */
class PhoneLink(
    private val control: HelmConnection,
    private val openStreamSocket: () -> HelmConnection,
) {
    @Volatile
    private var socket: HelmConnection? = null

    /** `phone.accept`, then `/stream` + `phone.open`. Throws [HelmException] on failure. */
    suspend fun open(requestId: Long, size: PhoneSize, rotation: Int, controlGranted: Boolean): HelmConnection {
        val res = HelmApi(control).g9(
            "phone.accept",
            buildJsonObject {
                put("request_id", requestId)
                put("width", size.width)
                put("height", size.height)
                put("rotation", rotation)
                put("control", controlGranted)
            },
            15_000,
        )
        val token = res.optStr("token")
            ?: throw HelmException(LocalCodes.BAD_STREAM, "PC did not return a token.")
        val sc = openStreamSocket()
        socket = sc
        try {
            sc.connect()
        } catch (e: HelmException) {
            socket = null
            throw e
        }
        val opened = sc.sendText(
            buildJsonObject {
                put("type", "phone.open")
                put("token", token)
            },
        )
        if (!opened) {
            close()
            throw HelmException(LocalCodes.CONNECTION_LOST, "Stream socket closed before phone.open.")
        }
        return sc
    }

    /** PC input arriving on the stream socket. */
    fun input(sc: HelmConnection): Flow<PhoneIn> =
        sc.events(PhoneControl.EV_IN).mapNotNull { PhoneIn.parse(it.data) }

    /** One access unit, split if it has to be. False when the socket is gone. */
    fun send(au: QueuedAu): Boolean {
        val sc = socket ?: return false
        for (frame in PhoneFrames.fragments(au.ptsUs, au.data, au.key)) {
            if (!sc.sendBytes(frame)) return false
        }
        return true
    }

    /** Bytes the socket has not written yet (network slower than the encoder). */
    val queuedBytes: Long get() = socket?.queuedBytes ?: 0L

    fun close() {
        socket?.close()
        socket = null
    }
}
