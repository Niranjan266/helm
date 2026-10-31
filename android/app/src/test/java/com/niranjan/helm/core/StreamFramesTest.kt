package com.niranjan.helm.core

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class StreamFramesTest {
    private val au = byteArrayOf(0, 0, 0, 1, 0x41, 0x9a.toByte(), 0x22)

    @Test
    fun headerIsKindPlusBigEndianPts() {
        val b = StreamFrames.video(0x0102030405060708L, au, key = false)
        assertEquals(StreamFrames.HEADER_LEN + au.size, b.size)
        assertEquals(0x01.toByte(), b[0])
        assertArrayEquals(byteArrayOf(1, 2, 3, 4, 5, 6, 7, 8), b.copyOfRange(1, 9))
        val f = StreamFrames.parse(b) as StreamFrame.Video
        assertEquals(0x0102030405060708L, f.ptsUs)
        assertFalse(f.key)
        assertArrayEquals(au, f.data)
    }

    @Test
    fun keyframeKind() {
        val f = StreamFrames.parse(StreamFrames.video(42, au, key = true)) as StreamFrame.Video
        assertTrue(f.key)
        assertEquals(42L, f.ptsUs)
    }

    @Test
    fun audioFrame() {
        val opus = byteArrayOf(0x78, 1, 2, 3)
        val f = StreamFrames.parse(StreamFrames.audio(20_000, opus)) as StreamFrame.Audio
        assertEquals(20_000L, f.ptsUs)
        assertArrayEquals(opus, f.data)
    }

    @Test
    fun cursorFrameJson() {
        val f = StreamFrames.parse(StreamFrames.cursor(7, 100.5f, 200f, visible = true, shapeId = 3)) as StreamFrame.Cursor
        assertEquals(StreamFrame.Cursor(7, 100.5f, 200f, true, 3), f)
        val raw = StreamFrames.encode(StreamFrames.KIND_CURSOR, 1, """{"x":5,"y":6,"visible":false,"shape_id":4294967295}""".toByteArray())
        val g = StreamFrames.parse(raw) as StreamFrame.Cursor
        assertFalse(g.visible)
        assertEquals(4294967295L, g.shapeId)
        assertEquals(5f, g.x)
    }

    @Test
    fun cursorFrameBadJsonIsIgnored() {
        assertNull(StreamFrames.parse(StreamFrames.encode(StreamFrames.KIND_CURSOR, 1, "nope".toByteArray())))
        assertNull(StreamFrames.parse(StreamFrames.encode(StreamFrames.KIND_CURSOR, 1, """{"y":1}""".toByteArray())))
    }

    @Test
    fun cursorShapeHeaderIsBigEndian() {
        val png = byteArrayOf(0x89.toByte(), 'P'.code.toByte(), 'N'.code.toByte(), 'G'.code.toByte())
        val b = StreamFrames.cursorShape(9, 0xA0B0C0D0L, 32, 48, 3, 65535, png)
        // u32 shape_id right after the 9-byte header, big-endian.
        assertArrayEquals(byteArrayOf(0xA0.toByte(), 0xB0.toByte(), 0xC0.toByte(), 0xD0.toByte()), b.copyOfRange(9, 13))
        val f = StreamFrames.parse(b) as StreamFrame.CursorShape
        assertEquals(0xA0B0C0D0L, f.shapeId)
        assertEquals(32, f.width)
        assertEquals(48, f.height)
        assertEquals(3, f.hotX)
        assertEquals(65535, f.hotY)
        assertArrayEquals(png, f.png)
    }

    @Test
    fun malformedFramesAreRejected() {
        assertNull(StreamFrames.parse(ByteArray(0)))
        assertNull(StreamFrames.parse(ByteArray(8)))
        assertNull(StreamFrames.parse(StreamFrames.encode(StreamFrames.KIND_VIDEO, 0, ByteArray(0))))
        assertNull(StreamFrames.parse(StreamFrames.encode(StreamFrames.KIND_AUDIO, 0, ByteArray(0))))
        assertNull(StreamFrames.parse(StreamFrames.encode(StreamFrames.KIND_CURSOR_SHAPE, 0, ByteArray(11))))
    }

    @Test
    fun unknownKindIsIgnored() {
        assertNull(StreamFrames.parse(StreamFrames.encode(0x7f, 0, byteArrayOf(1, 2, 3))))
        assertNull(StreamFrames.parse(StreamFrames.encode(0x00, 0, byteArrayOf(1, 2, 3))))
    }
}
