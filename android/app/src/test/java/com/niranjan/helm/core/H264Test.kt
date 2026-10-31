package com.niranjan.helm.core

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class H264Test {
    /** Writes H.264 bitstream syntax (u(n), ue(v), se(v)) for building test SPS units. */
    private class BitWriter {
        private val bits = ArrayList<Int>()
        fun u(n: Int, v: Int) = apply { for (i in n - 1 downTo 0) bits += (v shr i) and 1 }
        fun ue(v: Int) = apply {
            val x = v + 1
            val len = 32 - Integer.numberOfLeadingZeros(x)
            repeat(len - 1) { bits += 0 }
            u(len, x)
        }
        fun bytes(): ByteArray {
            bits += 1 // rbsp_stop_one_bit
            while (bits.size % 8 != 0) bits += 0
            return ByteArray(bits.size / 8) { i -> (0 until 8).fold(0) { acc, j -> (acc shl 1) or bits[i * 8 + j] }.toByte() }
        }
    }

    /** Adds emulation-prevention bytes like an encoder would. */
    private fun escape(rbsp: ByteArray): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        var zeros = 0
        for (b in rbsp) {
            if (zeros >= 2 && (b.toInt() and 0xff) <= 3) {
                out.write(3)
                zeros = 0
            }
            out.write(b.toInt())
            zeros = if (b == 0.toByte()) zeros + 1 else 0
        }
        return out.toByteArray()
    }

    private fun sps(profile: Int, widthMbs: Int, heightMbs: Int, cropBottom: Int = 0, level: Int = 40): ByteArray {
        val w = BitWriter().u(8, profile).u(8, 0).u(8, level).ue(0)
        if (profile == 100) w.ue(1).ue(0).ue(0).u(1, 0).u(1, 0)
        w.ue(0) // log2_max_frame_num_minus4
        w.ue(0).ue(0) // poc type 0, log2_max_poc_lsb_minus4
        w.ue(1).u(1, 0) // max_num_ref_frames, gaps
        w.ue(widthMbs - 1).ue(heightMbs - 1)
        w.u(1, 1).u(1, 1) // frame_mbs_only, direct_8x8
        if (cropBottom > 0) w.u(1, 1).ue(0).ue(0).ue(0).ue(cropBottom / 2) else w.u(1, 0)
        w.u(1, 0) // vui
        return byteArrayOf(0x67) + escape(w.bytes())
    }

    private val pps = byteArrayOf(0x68, 0xce.toByte(), 0x3c, 0x80.toByte())
    private val idr = byteArrayOf(0x65, 0x88.toByte(), 0x84.toByte(), 0x00, 0x33)
    private val slice = byteArrayOf(0x41, 0x9a.toByte(), 0x02)
    private fun au(vararg nals: ByteArray, long: Boolean = true): ByteArray =
        nals.fold(ByteArray(0)) { acc, n -> acc + (if (long) byteArrayOf(0, 0, 0, 1) else byteArrayOf(0, 0, 1)) + n }

    @Test
    fun splitsNalUnitsWithBothStartCodeLengths() {
        val s = sps(66, 80, 45)
        val data = au(s, long = true) + au(pps, long = false) + au(idr, long = true)
        val nals = AnnexB.nalUnits(data)
        assertEquals(listOf(AnnexB.NAL_SPS, AnnexB.NAL_PPS, AnnexB.NAL_IDR), nals.map { it.type })
        assertArrayEquals(s, data.copyOfRange(nals[0].offset, nals[0].offset + nals[0].length))
        assertArrayEquals(pps, AnnexB.first(data, AnnexB.NAL_PPS))
        assertArrayEquals(idr, AnnexB.first(data, AnnexB.NAL_IDR))
        assertTrue(AnnexB.containsSps(data))
        assertTrue(AnnexB.isIdr(data))
    }

    @Test
    fun deltaFrameHasNoSpsOrIdr() {
        val data = au(byteArrayOf(0x09, 0xf0.toByte()), slice)
        assertEquals(listOf(AnnexB.NAL_AUD, AnnexB.NAL_SLICE), AnnexB.types(data))
        assertFalse(AnnexB.containsSps(data))
        assertFalse(AnnexB.isIdr(data))
        assertNull(AnnexB.first(data, AnnexB.NAL_SPS))
    }

    @Test
    fun noStartCodeMeansNoNals() {
        assertTrue(AnnexB.nalUnits(byteArrayOf(1, 2, 3, 4)).isEmpty())
        assertTrue(AnnexB.nalUnits(ByteArray(0)).isEmpty())
    }

    @Test
    fun unescapeRemovesEmulationPrevention() {
        assertArrayEquals(byteArrayOf(0, 0, 1, 0, 0, 0), AnnexB.unescape(byteArrayOf(0, 0, 3, 1, 0, 0, 3, 0)))
        assertArrayEquals(byteArrayOf(5, 3, 0, 3), AnnexB.unescape(byteArrayOf(5, 3, 0, 3)))
    }

    @Test
    fun withStartCodeForCsd() {
        assertArrayEquals(byteArrayOf(0, 0, 0, 1, 0x68, 1), AnnexB.withStartCode(byteArrayOf(0x68, 1)))
    }

    @Test
    fun parsesBaseline720p() {
        assertEquals(SpsInfo(66, 31, 1280, 720), SpsParser.parse(sps(66, 80, 45, level = 31)))
    }

    @Test
    fun parsesHigh1080pWithCropping() {
        // 1920x1088 coded, 8 rows cropped at the bottom.
        assertEquals(SpsInfo(100, 42, 1920, 1080), SpsParser.parse(sps(100, 120, 68, cropBottom = 8, level = 42)))
    }

    @Test
    fun rejectsNonSpsAndTruncated() {
        assertNull(SpsParser.parse(pps))
        assertNull(SpsParser.parse(byteArrayOf(0x67, 66)))
        assertNull(SpsParser.parse(ByteArray(0)))
    }

    @Test
    fun trackerWaitsForKeyThenReconfiguresOnNewSps() {
        val t = DecoderConfigTracker()
        val s720 = sps(66, 80, 45)
        val s1080 = sps(100, 120, 68, cropBottom = 8)
        assertEquals(DecoderConfigTracker.Action.DROP_WAIT_KEY, t.onAccessUnit(au(slice), key = false))
        assertEquals(DecoderConfigTracker.Action.CONFIGURE, t.onAccessUnit(au(s720, pps, idr), key = true))
        assertEquals(1280, t.info?.width)
        assertArrayEquals(AnnexB.withStartCode(s720), t.csd0())
        assertArrayEquals(AnnexB.withStartCode(pps), t.csd1())
        assertEquals(DecoderConfigTracker.Action.DECODE, t.onAccessUnit(au(slice), key = false))
        // Same SPS/PPS on the next keyframe: keep decoding, no reconfigure.
        assertEquals(DecoderConfigTracker.Action.DECODE, t.onAccessUnit(au(s720, pps, idr), key = true))
        // New SPS (resolution change): reconfigure.
        assertEquals(DecoderConfigTracker.Action.CONFIGURE, t.onAccessUnit(au(s1080, pps, idr), key = true))
        assertEquals(1080, t.info?.height)
        t.reset()
        assertEquals(DecoderConfigTracker.Action.DROP_WAIT_KEY, t.onAccessUnit(au(slice), key = false))
    }
}
