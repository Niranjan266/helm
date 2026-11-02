package com.niranjan.helm.core

import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Quality presets for `stream.start` (§10). Bitrates are starting points; the agent adapts. */
enum class StreamPreset(val label: String, val maxWidth: Int, val maxHeight: Int, val fps: Int, val defaultKbps: Int) {
    P720_30("720p30", 1280, 720, 30, 6_000),
    P1080_30("1080p30", 1920, 1080, 30, 10_000),
    P1080_60("1080p60", 1920, 1080, 60, 16_000),
    ;

    /** Bitrate choices offered for this preset (kbps), lowest first, always containing the default. */
    val bitrateChoices: List<Int>
        get() = listOf(defaultKbps / 2, defaultKbps * 3 / 4, defaultKbps, defaultKbps * 3 / 2, defaultKbps * 2)

    companion object {
        const val MIN_KBPS = 500
        const val MAX_KBPS = 50_000

        fun clampKbps(kbps: Int): Int = kbps.coerceIn(MIN_KBPS, MAX_KBPS)

        fun byName(name: String?): StreamPreset = entries.firstOrNull { it.name == name } ?: P1080_30
    }
}

/** What to stream: a monitor (opaque id from `screen.monitors`) or the whole primary display. */
data class StreamRequest(
    val preset: StreamPreset,
    val bitrateKbps: Int = preset.defaultKbps,
    val audio: Boolean = true,
    val monitorId: JsonPrimitive? = null,
    val hwnd: JsonPrimitive? = null,
    /** G3: the gaming preset replaces size, fps and bitrate (agent `preset:"gaming"`). */
    val gaming: GamingPreset? = null,
    /** G3 second screen: cap the stream size (e.g. to the phone's screen). */
    val maxSize: Pair<Int, Int>? = null,
) {
    fun toParams(): JsonObject = gaming?.toParams(monitorId, hwnd) ?: buildJsonObject {
        if (hwnd != null) put("hwnd", hwnd) else if (monitorId != null) put("monitor_id", monitorId)
        put("max_width", maxSize?.first?.coerceIn(64, preset.maxWidth) ?: preset.maxWidth)
        put("max_height", maxSize?.second?.coerceIn(64, preset.maxHeight) ?: preset.maxHeight)
        put("fps", preset.fps)
        put("bitrate_kbps", StreamPreset.clampKbps(bitrateKbps))
        put("audio", audio)
    }
}

/** `stream.start` result. */
data class StreamStartInfo(
    val token: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val codec: String,
    val encoder: String,
    /** G3: `"gaming"` or `"default"` (older agents: `"default"`). */
    val preset: String = "default",
    /** G3: the agent accepts `in` frames on the `/stream` socket. */
    val inputOnStream: Boolean = false,
) {
    companion object {
        fun from(r: JsonObject): StreamStartInfo? {
            val token = r.optStr("token") ?: return null
            return StreamStartInfo(
                token = token,
                width = (r.optLong("width") ?: 0).toInt(),
                height = (r.optLong("height") ?: 0).toInt(),
                fps = (r.optLong("fps") ?: 0).toInt(),
                codec = r.optStr("codec") ?: "h264",
                encoder = r.optStr("encoder") ?: "unknown",
                preset = r.optStr("preset") ?: "default",
                inputOnStream = r.optBool("input_on_stream") == true,
            )
        }
    }
}

/** One `stream.stats` report (phone -> PC every second). */
data class StreamStatsReport(val rxKbps: Int, val decodedFps: Int, val dropped: Int, val rttMs: Int) {
    fun toParams(): JsonObject = buildJsonObject {
        put("rx_kbps", rxKbps)
        put("decoded_fps", decodedFps)
        put("dropped", dropped)
        put("rtt_ms", rttMs)
    }
}

/**
 * Counts what happened on the stream and turns it into one [StreamStatsReport] per interval.
 * Thread-safe; time is passed in so it can be tested.
 */
class StreamStatsCounter(startMs: Long) {
    private var bytes = 0L
    private var decoded = 0
    private var dropped = 0
    private var lastMs = startMs

    @Volatile
    var rttMs: Int = 0

    /** Last report, for the on-screen overlay. */
    @Volatile
    var last: StreamStatsReport = StreamStatsReport(0, 0, 0, 0)
        private set

    @Volatile
    var totalDropped: Long = 0
        private set

    @Synchronized
    fun onBytes(n: Int) {
        bytes += n
    }

    @Synchronized
    fun onDecoded(n: Int = 1) {
        decoded += n
    }

    @Synchronized
    fun onDropped(n: Int = 1) {
        dropped += n
        totalDropped += n
    }

    /** Report for the time since the previous call; resets the interval counters. */
    @Synchronized
    fun snapshot(nowMs: Long): StreamStatsReport {
        val dt = (nowMs - lastMs).coerceAtLeast(1)
        val r = StreamStatsReport(
            rxKbps = (bytes * 8 / dt).toInt(), // bits per ms == kbit/s
            decodedFps = ((decoded * 1000L + dt / 2) / dt).toInt(),
            dropped = dropped,
            rttMs = rttMs,
        )
        bytes = 0
        decoded = 0
        dropped = 0
        lastMs = nowMs
        last = r
        return r
    }
}

/** Opus decoder setup for MediaCodec `audio/opus` (RFC 7845 identification header). */
object OpusConfig {
    const val SAMPLE_RATE = 48_000
    const val CHANNELS = 2

    /** Typical libopus encoder look-ahead at 48 kHz. The agent does not announce it (§10). */
    const val DEFAULT_PRE_SKIP = 312

    /** `csd-0`: the 19-byte `OpusHead` (version 1, mapping family 0). */
    fun opusHead(channels: Int = CHANNELS, preSkip: Int = DEFAULT_PRE_SKIP, inputRate: Int = SAMPLE_RATE): ByteArray =
        ByteBuffer.allocate(19).order(ByteOrder.LITTLE_ENDIAN)
            .put("OpusHead".toByteArray(Charsets.US_ASCII))
            .put(1)
            .put(channels.toByte())
            .putShort(preSkip.toShort())
            .putInt(inputRate)
            .putShort(0) // output gain
            .put(0) // channel mapping family
            .array()

    /** `csd-1` / `csd-2`: 64-bit little-endian nanoseconds (codec delay, seek pre-roll). */
    fun nanosLe(ns: Long): ByteArray = ByteBuffer.allocate(8).order(ByteOrder.LITTLE_ENDIAN).putLong(ns).array()

    fun preSkipNs(preSkip: Int = DEFAULT_PRE_SKIP): Long = preSkip * 1_000_000_000L / SAMPLE_RATE

    const val SEEK_PREROLL_NS = 80_000_000L
}
