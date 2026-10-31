package com.niranjan.helm.wearlink

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class WearLinkTest {
    @Test
    fun commandRoundTripsEveryAction() {
        for (a in WearAction.entries) {
            val cmd = WearCommand("abc-1", a, if (a == WearAction.DECK) 3 else null)
            assertEquals(cmd, WearCommand.decode(cmd.encode()))
        }
    }

    @Test
    fun commandWireFormatIsStable() {
        assertEquals("helm-wear/1\n7\ndeck\n2", WearCommand("7", WearAction.DECK, 2).encode().toString(Charsets.UTF_8))
        assertEquals("helm-wear/1\n8\nlock\n", WearCommand("8", WearAction.LOCK).encode().toString(Charsets.UTF_8))
    }

    @Test
    fun commandRejectsBadInput() {
        fun d(s: String) = WearCommand.decode(s.toByteArray())
        assertNull(WearCommand.decode(null))
        assertNull(WearCommand.decode(ByteArray(0)))
        assertNull(d("helm-wear/2\n1\nlock\n")) // wrong version
        assertNull(d("helm-wear/1\n1\nshutdown\n")) // not in the closed set
        assertNull(d("helm-wear/1\n1\nlock")) // missing field
        assertNull(d("helm-wear/1\n1\nlock\n\nextra")) // extra field
        assertNull(d("helm-wear/1\n1\ndeck\n")) // deck without slot
        assertNull(d("helm-wear/1\n1\ndeck\n4")) // slot out of range
        assertNull(d("helm-wear/1\n1\ndeck\n-1"))
        assertNull(d("helm-wear/1\n1\ndeck\nx"))
        assertNull(d("helm-wear/1\n1\nmute\n0")) // slot on a non-deck action
        assertNull(d("helm-wear/1\n\nmute\n")) // empty id
        assertNull(d("helm-wear/1\nbad id!\nmute\n")) // id charset
        assertNull(d("helm-wear/1\n1\\x\nmute\n")) // bad escape
        assertNull(WearCommand.decode(byteArrayOf(0xC3.toByte(), 0x28))) // invalid UTF-8
        assertNull(WearCommand.decode(ByteArray(WearLink.MAX_PAYLOAD + 1) { 'a'.code.toByte() }))
    }

    @Test
    fun replyEscapesNewlinesAndBackslashes() {
        val r = WearReply("id1", false, "line1\nline2 \\ end\r")
        assertEquals(4, r.encode().toString(Charsets.UTF_8).split('\n').size)
        assertEquals(r, WearReply.decode(r.encode()))
        assertEquals(WearReply("x", true, "OK"), WearReply.decode(WearReply("x", true, "OK").encode()))
        assertNull(WearReply.decode("helm-wear/1\nx\nmaybe\nhi".toByteArray()))
    }

    @Test
    fun longFieldsAreTruncatedOnEncode() {
        val back = WearReply.decode(WearReply("x", true, "é".repeat(500)).encode())!!
        assertEquals(WearLink.MAX_FIELD, back.message.length)
    }

    @Test
    fun statusRoundTrips() {
        val s = WearStatus(true, "Desk PC\nevil", muted = true, nowPlaying = "Song - Artist", deck = listOf("OBS", "", "Mic"))
        val back = WearStatus.decode(s.encode())!!
        assertTrue(back.connected)
        assertEquals("Desk PC\nevil", back.pcName)
        assertEquals(true, back.muted)
        assertEquals("Song - Artist", back.nowPlaying)
        assertEquals(listOf("OBS", "", "Mic", ""), back.deck)

        val off = WearStatus.decode(WearStatus.DISCONNECTED.encode())!!
        assertEquals(false, off.connected)
        assertNull(off.pcName)
        assertNull(off.muted)
        assertEquals(List(4) { "" }, off.deck)
    }

    @Test
    fun statusRejectsBadFlags() {
        fun s(vararg f: String) = (listOf(WearLink.MAGIC) + f).joinToString("\n").toByteArray()
        assertTrue(WearStatus.decode(s("1", "", "", "", "", "", "", "")) != null)
        assertNull(WearStatus.decode(s("2", "", "", "", "", "", "", ""))) // connected flag
        assertNull(WearStatus.decode(s("1", "", "x", "", "", "", "", ""))) // muted flag
        assertNull(WearStatus.decode(s("1", "", ""))) // too few fields
    }

    @Test
    fun escapeUnescapeAreInverse() {
        for (s in listOf("", "\\", "\\n", "a\nb", "\r\n\\\\", "plain")) {
            assertEquals(s, WearLink.unescape(WearLink.escape(s)))
        }
        assertNull(WearLink.unescape("trailing\\"))
    }
}
