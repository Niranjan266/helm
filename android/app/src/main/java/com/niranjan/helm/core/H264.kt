package com.niranjan.helm.core

/** One NAL unit inside an Annex-B buffer: [offset]/[length] exclude the start code. */
data class NalUnit(val offset: Int, val length: Int, val type: Int)

/** Annex-B helpers for H.264 access units (start codes `00 00 01` / `00 00 00 01`). */
object AnnexB {
    const val NAL_SLICE = 1
    const val NAL_IDR = 5
    const val NAL_SEI = 6
    const val NAL_SPS = 7
    const val NAL_PPS = 8
    const val NAL_AUD = 9

    private val START = byteArrayOf(0, 0, 0, 1)

    /** All NAL units in [data], in order. Bytes before the first start code are ignored. */
    fun nalUnits(data: ByteArray): List<NalUnit> {
        val out = ArrayList<NalUnit>(4)
        var i = findStart(data, 0) ?: return out
        while (true) {
            val payload = i.second
            val next = findStart(data, payload)
            var end = next?.first ?: data.size
            // Trailing zero bytes belong to the next start code (zero_byte), not to this NAL.
            while (end > payload && data[end - 1] == 0.toByte() && next != null) end--
            if (end > payload) out += NalUnit(payload, end - payload, data[payload].toInt() and 0x1f)
            i = next ?: break
        }
        return out
    }

    /** (index of first start-code byte, index just after it), or null. */
    private fun findStart(data: ByteArray, from: Int): Pair<Int, Int>? {
        var i = from
        while (i + 2 < data.size) {
            if (data[i] == 0.toByte() && data[i + 1] == 0.toByte() && data[i + 2] == 1.toByte()) {
                val begin = if (i > from && data[i - 1] == 0.toByte()) i - 1 else i
                return begin to i + 3
            }
            i++
        }
        return null
    }

    fun types(data: ByteArray): List<Int> = nalUnits(data).map { it.type }

    fun containsSps(data: ByteArray): Boolean = nalUnits(data).any { it.type == NAL_SPS }

    fun isIdr(data: ByteArray): Boolean = nalUnits(data).any { it.type == NAL_IDR }

    /** First NAL of [type] without its start code, or null. */
    fun first(data: ByteArray, type: Int): ByteArray? =
        nalUnits(data).firstOrNull { it.type == type }?.let { data.copyOfRange(it.offset, it.offset + it.length) }

    /** [nal] prefixed with a 4-byte start code (MediaCodec `csd-0` / `csd-1` format). */
    fun withStartCode(nal: ByteArray): ByteArray = START + nal

    /** Removes emulation-prevention bytes (`00 00 03` -> `00 00`) from a NAL payload. */
    fun unescape(nal: ByteArray): ByteArray {
        val out = java.io.ByteArrayOutputStream(nal.size)
        var zeros = 0
        for (b in nal) {
            if (zeros >= 2 && b == 3.toByte()) {
                zeros = 0
                continue
            }
            out.write(b.toInt())
            zeros = if (b == 0.toByte()) zeros + 1 else 0
        }
        return out.toByteArray()
    }
}

/** What the decoder needs to know from a sequence parameter set. */
data class SpsInfo(val profileIdc: Int, val levelIdc: Int, val width: Int, val height: Int)

/** Minimal H.264 SPS reader (ITU-T H.264 §7.3.2.1.1): profile, level and cropped size. */
object SpsParser {
    /** [nal] = SPS NAL unit without start code (header byte included). Null if malformed. */
    fun parse(nal: ByteArray): SpsInfo? = try {
        if (nal.isEmpty() || (nal[0].toInt() and 0x1f) != AnnexB.NAL_SPS) null else read(AnnexB.unescape(nal))
    } catch (_: IndexOutOfBoundsException) {
        null
    }

    private fun read(rbsp: ByteArray): SpsInfo? {
        val r = BitReader(rbsp, 8) // skip the NAL header byte
        val profile = r.bits(8)
        r.bits(8) // constraint flags + reserved
        val level = r.bits(8)
        r.ue() // seq_parameter_set_id
        var chromaFormat = 1
        var separateColourPlane = false
        if (profile in HIGH_PROFILES) {
            chromaFormat = r.ue()
            if (chromaFormat == 3) separateColourPlane = r.bit() == 1
            r.ue() // bit_depth_luma_minus8
            r.ue() // bit_depth_chroma_minus8
            r.bit() // qpprime_y_zero_transform_bypass_flag
            if (r.bit() == 1) { // seq_scaling_matrix_present_flag
                val lists = if (chromaFormat != 3) 8 else 12
                for (i in 0 until lists) {
                    if (r.bit() == 1) skipScalingList(r, if (i < 6) 16 else 64)
                }
            }
        }
        r.ue() // log2_max_frame_num_minus4
        when (r.ue()) { // pic_order_cnt_type
            0 -> r.ue()
            1 -> {
                r.bit()
                r.se()
                r.se()
                repeat(r.ue()) { r.se() }
            }
        }
        r.ue() // max_num_ref_frames
        r.bit() // gaps_in_frame_num_value_allowed_flag
        val widthMbs = r.ue() + 1
        val heightMapUnits = r.ue() + 1
        val frameMbsOnly = r.bit()
        if (frameMbsOnly == 0) r.bit() // mb_adaptive_frame_field_flag
        r.bit() // direct_8x8_inference_flag
        var width = widthMbs * 16
        var height = (2 - frameMbsOnly) * heightMapUnits * 16
        if (r.bit() == 1) { // frame_cropping_flag
            val l = r.ue()
            val rt = r.ue()
            val t = r.ue()
            val b = r.ue()
            val arrayType = if (separateColourPlane) 0 else chromaFormat
            val cropX = if (arrayType == 0) 1 else if (arrayType == 3) 1 else 2
            val cropY = (if (arrayType == 1) 2 else 1) * (2 - frameMbsOnly)
            width -= (l + rt) * cropX
            height -= (t + b) * cropY
        }
        if (width <= 0 || height <= 0) return null
        return SpsInfo(profile, level, width, height)
    }

    private fun skipScalingList(r: BitReader, size: Int) {
        var last = 8
        var next = 8
        for (j in 0 until size) {
            if (next != 0) next = (last + r.se() + 256) % 256
            if (next != 0) last = next
        }
    }

    private val HIGH_PROFILES = setOf(100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135)

    private class BitReader(private val data: ByteArray, private var pos: Int) {
        fun bit(): Int {
            val byte = data[pos ushr 3].toInt()
            val v = (byte shr (7 - (pos and 7))) and 1
            pos++
            return v
        }

        fun bits(n: Int): Int {
            var v = 0
            repeat(n) { v = (v shl 1) or bit() }
            return v
        }

        fun ue(): Int {
            var zeros = 0
            while (bit() == 0) {
                zeros++
                if (zeros > 31) throw IndexOutOfBoundsException("bad exp-golomb")
            }
            return ((1L shl zeros) - 1 + bits(zeros).toLong()).toInt()
        }

        fun se(): Int {
            val k = ue()
            return if (k and 1 == 1) (k + 1) / 2 else -(k / 2)
        }
    }
}

/**
 * Tracks the active SPS/PPS so the decoder knows when it must be (re)configured: before the
 * first keyframe, and whenever a keyframe carries a different SPS (resolution / profile change).
 */
class DecoderConfigTracker {
    var sps: ByteArray? = null
        private set
    var pps: ByteArray? = null
        private set
    var info: SpsInfo? = null
        private set

    /** What to do with one access unit. */
    enum class Action { CONFIGURE, DECODE, DROP_WAIT_KEY }

    @Synchronized
    fun onAccessUnit(data: ByteArray, key: Boolean): Action {
        val newSps = AnnexB.first(data, AnnexB.NAL_SPS)
        val newPps = AnnexB.first(data, AnnexB.NAL_PPS)
        if (newSps != null && newPps != null) {
            val changed = sps == null || !sps.contentEquals(newSps) || !pps.contentEquals(newPps)
            if (changed) {
                sps = newSps
                pps = newPps
                info = SpsParser.parse(newSps)
                return Action.CONFIGURE
            }
        }
        if (sps == null) return Action.DROP_WAIT_KEY
        return Action.DECODE
    }

    /** Forget the config (e.g. the decoder failed); the next keyframe configures again. */
    @Synchronized
    fun reset() {
        sps = null
        pps = null
        info = null
    }

    fun csd0(): ByteArray? = sps?.let { AnnexB.withStartCode(it) }

    fun csd1(): ByteArray? = pps?.let { AnnexB.withStartCode(it) }
}
