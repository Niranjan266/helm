package com.niranjan.helm.core

import java.nio.ByteBuffer
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/**
 * One binary frame on the stream socket (PROTOCOL.md §10):
 * `[1 byte kind][8 bytes pts µs, big-endian][payload]`.
 */
sealed interface StreamFrame {
    val ptsUs: Long

    /** `0x01` / `0x02`: one H.264 access unit in Annex-B form. [key] = `0x02` (has SPS/PPS). */
    class Video(override val ptsUs: Long, val data: ByteArray, val key: Boolean) : StreamFrame

    /** `0x03`: one Opus packet (48 kHz stereo, 20 ms). */
    class Audio(override val ptsUs: Long, val data: ByteArray) : StreamFrame

    /** `0x04`: cursor position in stream pixel coordinates. */
    data class Cursor(override val ptsUs: Long, val x: Float, val y: Float, val visible: Boolean, val shapeId: Long) : StreamFrame

    /** `0x05`: cursor image. Header fields are big-endian (see [StreamFrames.parse]). */
    class CursorShape(
        override val ptsUs: Long,
        val shapeId: Long,
        val width: Int,
        val height: Int,
        val hotX: Int,
        val hotY: Int,
        val png: ByteArray,
    ) : StreamFrame
}

object StreamFrames {
    const val KIND_VIDEO: Byte = 0x01
    const val KIND_VIDEO_KEY: Byte = 0x02
    const val KIND_AUDIO: Byte = 0x03
    const val KIND_CURSOR: Byte = 0x04
    const val KIND_CURSOR_SHAPE: Byte = 0x05
    const val HEADER_LEN = 9

    /** `[u32 shape_id][u16 w][u16 h][u16 hot_x][u16 hot_y]` */
    const val SHAPE_HEADER_LEN = 12

    /**
     * Parses one frame. Returns null for frames that are too short, malformed, or of an unknown
     * kind (unknown kinds are ignored so the agent can add new ones later).
     */
    fun parse(bytes: ByteArray): StreamFrame? {
        if (bytes.size < HEADER_LEN) return null
        val buf = ByteBuffer.wrap(bytes) // big-endian by default
        val kind = buf.get()
        val pts = buf.long
        val payloadLen = bytes.size - HEADER_LEN
        return when (kind) {
            KIND_VIDEO, KIND_VIDEO_KEY -> {
                if (payloadLen == 0) return null
                StreamFrame.Video(pts, bytes.copyOfRange(HEADER_LEN, bytes.size), kind == KIND_VIDEO_KEY)
            }
            KIND_AUDIO -> {
                if (payloadLen == 0) return null
                StreamFrame.Audio(pts, bytes.copyOfRange(HEADER_LEN, bytes.size))
            }
            KIND_CURSOR -> {
                val obj = HelmConnection.parseObject(String(bytes, HEADER_LEN, payloadLen, Charsets.UTF_8)) ?: return null
                val x = obj.optDouble("x") ?: return null
                val y = obj.optDouble("y") ?: return null
                StreamFrame.Cursor(
                    pts,
                    x.toFloat(),
                    y.toFloat(),
                    obj.optBool("visible") ?: true,
                    obj.optLong("shape_id") ?: 0L,
                )
            }
            KIND_CURSOR_SHAPE -> {
                if (payloadLen < SHAPE_HEADER_LEN) return null
                val id = buf.int.toLong() and 0xffffffffL
                val w = buf.short.toInt() and 0xffff
                val h = buf.short.toInt() and 0xffff
                val hx = buf.short.toInt() and 0xffff
                val hy = buf.short.toInt() and 0xffff
                StreamFrame.CursorShape(pts, id, w, h, hx, hy, bytes.copyOfRange(HEADER_LEN + SHAPE_HEADER_LEN, bytes.size))
            }
            else -> null
        }
    }

    // ---- Encoders (used by tests and the fake agent) --------------------------------------

    fun encode(kind: Byte, ptsUs: Long, payload: ByteArray): ByteArray =
        ByteBuffer.allocate(HEADER_LEN + payload.size).put(kind).putLong(ptsUs).put(payload).array()

    fun video(ptsUs: Long, annexB: ByteArray, key: Boolean): ByteArray =
        encode(if (key) KIND_VIDEO_KEY else KIND_VIDEO, ptsUs, annexB)

    fun audio(ptsUs: Long, opus: ByteArray): ByteArray = encode(KIND_AUDIO, ptsUs, opus)

    fun cursor(ptsUs: Long, x: Float, y: Float, visible: Boolean, shapeId: Long): ByteArray {
        val json: JsonObject = buildJsonObject {
            put("x", x)
            put("y", y)
            put("visible", visible)
            put("shape_id", shapeId)
        }
        return encode(KIND_CURSOR, ptsUs, json.toString().toByteArray(Charsets.UTF_8))
    }

    fun cursorShape(ptsUs: Long, shapeId: Long, w: Int, h: Int, hotX: Int, hotY: Int, png: ByteArray): ByteArray {
        val p = ByteBuffer.allocate(SHAPE_HEADER_LEN + png.size)
            .putInt(shapeId.toInt())
            .putShort(w.toShort())
            .putShort(h.toShort())
            .putShort(hotX.toShort())
            .putShort(hotY.toShort())
            .put(png)
            .array()
        return encode(KIND_CURSOR_SHAPE, ptsUs, p)
    }
}
