package com.niranjan.helm.core

import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.intOrNull
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Presets, stats reports, Opus setup and direct-touch input frames. */
class StreamModelsTest {
    private fun JsonObject.int(k: String) = (this[k] as JsonPrimitive).intOrNull

    @Test
    fun presetsMatchTheirNames() {
        assertEquals(listOf("720p30", "1080p30", "1080p60"), StreamPreset.entries.map { it.label })
        val p720 = StreamPreset.P720_30
        assertEquals(1280 to 720, p720.maxWidth to p720.maxHeight)
        assertEquals(30, p720.fps)
        assertEquals(60, StreamPreset.P1080_60.fps)
        assertEquals(1920, StreamPreset.P1080_60.maxWidth)
        assertTrue(StreamPreset.P1080_60.defaultKbps > StreamPreset.P1080_30.defaultKbps)
        assertTrue(StreamPreset.P1080_30.defaultKbps > StreamPreset.P720_30.defaultKbps)
        assertEquals(StreamPreset.P1080_30, StreamPreset.byName("nope"))
        assertEquals(StreamPreset.P720_30, StreamPreset.byName("P720_30"))
    }

    @Test
    fun bitrateChoicesAreSortedAndIncludeDefault() {
        for (p in StreamPreset.entries) {
            val c = p.bitrateChoices
            assertTrue(p.defaultKbps in c)
            assertEquals(c.sorted(), c)
        }
        assertEquals(StreamPreset.MIN_KBPS, StreamPreset.clampKbps(1))
        assertEquals(StreamPreset.MAX_KBPS, StreamPreset.clampKbps(1_000_000))
    }

    @Test
    fun startParamsFollowProtocol() {
        val p = StreamRequest(StreamPreset.P1080_60, bitrateKbps = 20_000, audio = true, monitorId = JsonPrimitive(65537)).toParams()
        assertEquals(1920, p.int("max_width"))
        assertEquals(1080, p.int("max_height"))
        assertEquals(60, p.int("fps"))
        assertEquals(20_000, p.int("bitrate_kbps"))
        assertEquals(true, (p["audio"] as JsonPrimitive).booleanOrNull)
        assertEquals(JsonPrimitive(65537), p["monitor_id"]) // opaque: sent back unchanged
        assertNull(p["hwnd"])

        val primary = StreamRequest(StreamPreset.P720_30, audio = false).toParams()
        assertNull("no monitor_id = primary", primary["monitor_id"])
        assertEquals(6_000, primary.int("bitrate_kbps"))
        assertEquals(false, (primary["audio"] as JsonPrimitive).booleanOrNull)

        val win = StreamRequest(StreamPreset.P720_30, monitorId = JsonPrimitive("m1"), hwnd = JsonPrimitive(1234)).toParams()
        assertEquals(JsonPrimitive(1234), win["hwnd"])
        assertNull("monitor_id|hwnd, not both", win["monitor_id"])
    }

    @Test
    fun startResultParsing() {
        val r = HelmConnection.parseObject("""{"token":"t","width":1280,"height":720,"fps":30,"codec":"h264","encoder":"hardware"}""")!!
        assertEquals(StreamStartInfo("t", 1280, 720, 30, "h264", "hardware"), StreamStartInfo.from(r))
        assertNull(StreamStartInfo.from(HelmConnection.parseObject("""{"width":1}""")!!))
    }

    @Test
    fun statsCounterReportsPerInterval() {
        val c = StreamStatsCounter(startMs = 10_000)
        c.onBytes(125_000) // 1,000,000 bits in 1 s
        repeat(60) { c.onDecoded() }
        c.onDropped(2)
        c.rttMs = 7
        val r = c.snapshot(11_000)
        assertEquals(StreamStatsReport(rxKbps = 1_000, decodedFps = 60, dropped = 2, rttMs = 7), r)
        assertEquals(r, c.last)
        // Counters reset; totals keep growing.
        c.onDropped()
        val r2 = c.snapshot(11_500)
        assertEquals(0, r2.rxKbps)
        assertEquals(0, r2.decodedFps)
        assertEquals(1, r2.dropped)
        assertEquals(3L, c.totalDropped)
    }

    @Test
    fun statsCounterScalesToShortIntervals() {
        val c = StreamStatsCounter(startMs = 0)
        repeat(30) { c.onDecoded() }
        c.onBytes(62_500)
        val r = c.snapshot(500)
        assertEquals(60, r.decodedFps)
        assertEquals(1_000, r.rxKbps)
    }

    @Test
    fun statsParamsUseProtocolNames() {
        val p = StreamStatsReport(1500, 59, 3, 12).toParams()
        assertEquals(setOf("rx_kbps", "decoded_fps", "dropped", "rtt_ms"), p.keys)
        assertEquals(1500, p.int("rx_kbps"))
        assertEquals(12, p.int("rtt_ms"))
    }

    @Test
    fun opusHeadIsRfc7845() {
        val h = OpusConfig.opusHead()
        assertEquals(19, h.size)
        assertEquals("OpusHead", String(h, 0, 8, Charsets.US_ASCII))
        val b = ByteBuffer.wrap(h).order(ByteOrder.LITTLE_ENDIAN)
        assertEquals(1, b.get(8).toInt())
        assertEquals(2, b.get(9).toInt())
        assertEquals(312, b.getShort(10).toInt())
        assertEquals(48_000, b.getInt(12))
        assertEquals(0, b.get(18).toInt())
        assertEquals(6_500_000L, OpusConfig.preSkipNs())
        assertEquals(80_000_000L, ByteBuffer.wrap(OpusConfig.nanosLe(80_000_000L)).order(ByteOrder.LITTLE_ENDIAN).long)
    }

    @Test
    fun absoluteMoveFrame() {
        val j = InputFrame.AbsMove(0.123456f, 2f).toJson()
        assertEquals("""{"type":"in","k":"ma","x":0.1235,"y":1.0}""", j.toString())
    }

    @Test
    fun absBatcherKeepsNewestAndSkipsRepeats() {
        val b = AbsMoveBatcher()
        assertNull(b.drain())
        b.set(0.1f, 0.1f)
        b.set(0.2f, 0.3f)
        assertEquals(InputFrame.AbsMove(0.2f, 0.3f), b.drain())
        assertNull(b.drain())
        b.set(0.2f, 0.3f)
        assertNull("same position is not resent", b.drain())
        b.set(-1f, 5f)
        assertEquals(InputFrame.AbsMove(0f, 1f), b.take())
        b.set(Float.NaN, 0f)
        assertFalse(b.drain() != null)
    }
}
