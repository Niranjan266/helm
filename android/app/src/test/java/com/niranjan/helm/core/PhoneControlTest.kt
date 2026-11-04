package com.niranjan.helm.core

import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** g9 pure logic: consent rules, wire forms, gesture and text plans (phone-control.md). */
class PhoneControlTest {
    private fun request(id: Long = 7) = PhoneRequest(id, "DESKTOP", false, 1920, 1080, 30, 6000)

    // ---------------------------------------------------------------- consent

    @Test
    fun `off by default declines without asking`() {
        val l = PhoneSessionLogic()
        assertEquals(RequestDecision.DECLINE_DISABLED, l.onRequest(request(), enabled = false))
        assertEquals(PhoneStage.Idle, l.stage)
    }

    @Test
    fun `every request is asked and nothing is remembered`() {
        val l = PhoneSessionLogic()
        for (id in 1L..3L) {
            assertEquals(RequestDecision.ASK, l.onRequest(request(id), enabled = true))
            assertTrue(l.stage is PhoneStage.Asking)
            assertFalse("no input while asking", l.mayInject())
            assertTrue(l.onAccepted(id, wantControl = true))
            assertFalse("no input during Android's prompt", l.mayInject())
            assertTrue(l.onLive(id, accessibilityOn = true))
            assertTrue(l.mayInject())
            assertEquals(id, l.end(id)?.requestId)
            assertEquals("back to nothing after each session", PhoneStage.Idle, l.stage)
            assertFalse(l.mayInject())
        }
    }

    @Test
    fun `a second request while busy is declined and does not disturb the first`() {
        val l = PhoneSessionLogic()
        l.onRequest(request(1), true)
        assertEquals(RequestDecision.DECLINE_BUSY, l.onRequest(request(2), true))
        assertEquals(1L, (l.stage as PhoneStage.Asking).request.requestId)
        l.onAccepted(1, false)
        l.onLive(1, true)
        assertEquals(RequestDecision.DECLINE_BUSY, l.onRequest(request(3), true))
    }

    @Test
    fun `answers for the wrong request do nothing`() {
        val l = PhoneSessionLogic()
        assertFalse("nothing asked yet", l.onAccepted(1, true))
        assertFalse(l.onLive(1, true))
        assertNull(l.end(1))
        l.onRequest(request(1), true)
        assertFalse(l.onAccepted(2, true))
        assertNull(l.end(2))
        assertTrue(l.stage is PhoneStage.Asking)
    }

    @Test
    fun `input needs control granted and the service on`() {
        val l = PhoneSessionLogic()
        l.onRequest(request(), true)
        l.onAccepted(7, wantControl = false)
        l.onLive(7, accessibilityOn = true)
        assertFalse("view only was chosen", l.mayInject())
        assertNull("the service coming on does not grant control", l.onAccessibility(true))
        l.end()

        l.onRequest(request(), true)
        l.onAccepted(7, wantControl = true)
        l.onLive(7, accessibilityOn = false)
        assertFalse("service off: view only", l.mayInject())
        assertEquals(true, l.onAccessibility(true))
        assertTrue(l.mayInject())
        assertEquals(false, l.onAccessibility(false))
        assertFalse(l.mayInject())
        assertNull("no change", l.onAccessibility(false))
    }

    @Test
    fun `idle watchdog`() {
        val w = IdleWatchdog(1000, 0)
        assertFalse(w.expired(999))
        w.touch(900)
        assertFalse(w.expired(1899))
        assertTrue(w.expired(1900))
    }

    // ---------------------------------------------------------------- wire forms

    @Test
    fun `request event parses and clamps`() {
        val r = PhoneRequest.from(
            buildJsonObject {
                put("request_id", 3)
                put("pc_name", "PC")
                put("relayed", true)
                put("max_long", 100000)
                put("max_short", 1)
                put("fps", 500)
                put("bitrate_kbps", 1)
            },
        )!!
        assertEquals(PhoneRequest(3, "PC", true, 4096, 240, 60, 200), r)
        assertNull(PhoneRequest.from(buildJsonObject { put("pc_name", "PC") }))
        assertEquals("Your PC", PhoneRequest.from(buildJsonObject { put("request_id", 1) })!!.pcName)
    }

    @Test
    fun `input events parse, clamp and refuse junk`() {
        fun parse(f: kotlinx.serialization.json.JsonObjectBuilder.() -> Unit) = PhoneIn.parse(buildJsonObject(f))
        assertEquals(PhoneIn.Tap(0.5, 1.0), parse { put("k", "tap"); put("x", 0.5); put("y", 3.0) })
        assertEquals(PhoneIn.LongPress(0.0, 0.25), parse { put("k", "long"); put("x", -1.0); put("y", 0.25) })
        assertEquals(PhoneIn.Down(0.1, 0.2), parse { put("k", "down"); put("x", 0.1); put("y", 0.2) })
        assertEquals(PhoneIn.Move(0.1, 0.2), parse { put("k", "move"); put("x", 0.1); put("y", 0.2) })
        assertEquals(PhoneIn.Up(0.1, 0.2), parse { put("k", "up"); put("x", 0.1); put("y", 0.2) })
        assertEquals(
            PhoneIn.Swipe(0.0, 0.1, 0.2, 0.3, 5000),
            parse { put("k", "swipe"); put("x1", 0.0); put("y1", 0.1); put("x2", 0.2); put("y2", 0.3); put("ms", 99999) },
        )
        assertEquals(PhoneIn.Scroll(0.5, 0.5, 20.0, 0.0), parse { put("k", "scroll"); put("x", 0.5); put("y", 0.5); put("dy", 99.0) })
        assertEquals(PhoneIn.Global(PhoneIn.GlobalAction.NOTIFICATIONS), parse { put("k", "global"); put("action", "notifications") })
        assertEquals(PhoneIn.Text("hi"), parse { put("k", "text"); put("text", "hi") })
        assertEquals(PhoneIn.Key(PhoneIn.EditKey.ENTER), parse { put("k", "key"); put("key", "enter") })
        assertNull(parse { put("k", "tap"); put("x", 0.5) })
        assertNull(parse { put("k", "global"); put("action", "power") })
        assertNull(parse { put("k", "text"); put("text", "") })
        assertNull(parse { put("k", "wipe") })
        assertNull(parse { })
    }

    @Test
    fun `state change sends only what changed`() {
        val p = PhoneStateChange(width = 100, secureChanged = true).toParams(5)
        assertEquals("""{"request_id":5,"width":100,"secure":null}""", p.toString())
        assertEquals("""{"request_id":5,"secure":"password"}""", PhoneStateChange(secureChanged = true, secure = "password").toParams(5).toString())
        assertEquals("""{"request_id":5,"control":false}""", PhoneStateChange(control = false).toParams(5).toString())
    }

    @Test
    fun `scale fits either orientation, keeps aspect and never upscales`() {
        assertEquals(PhoneSize(1080, 2400).let { PhoneSize(486, 1080) }, PhoneScale.fit(1080, 2400, 1080, 1080))
        assertEquals(PhoneSize(864, 1920), PhoneScale.fit(1080, 2400, 1920, 1080))
        assertEquals(PhoneSize(1920, 864), PhoneScale.fit(2400, 1080, 1920, 1080))
        assertEquals(PhoneSize(720, 1600), PhoneScale.fit(720, 1600, 1920, 1080))
        val odd = PhoneScale.fit(1081, 2401, 4096, 4096)
        assertEquals(0, odd.width % 2)
        assertEquals(0, odd.height % 2)
        assertEquals(PhoneSize(16, 16), PhoneScale.fit(0, 0, 100, 100))
        assertEquals(PhoneSize(720, 1600), PhoneScale.align16(PhoneSize(726, 1606)))
    }

    @Test
    fun `small access units are one section-10 frame`() {
        val au = byteArrayOf(0, 0, 1, 0x41)
        val frames = PhoneFrames.fragments(0x0102030405060708, au, key = false)
        assertEquals(1, frames.size)
        assertArrayEquals(byteArrayOf(1, 1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 1, 0x41), frames[0])
        val parsed = StreamFrames.parse(frames[0]) as StreamFrame.Video
        assertEquals(0x0102030405060708, parsed.ptsUs)
        assertFalse(parsed.key)
        assertArrayEquals(au, parsed.data)
    }

    @Test
    fun `large access units split and the last piece carries the kind`() {
        val au = ByteArray(25) { it.toByte() }
        val frames = PhoneFrames.fragments(9, au, key = true, maxPayload = 10)
        assertEquals(listOf<Byte>(0x06, 0x06, 0x02), frames.map { it[0] })
        val joined = frames.flatMap { it.drop(StreamFrames.HEADER_LEN) }.toByteArray()
        assertArrayEquals(au, joined)
        frames.forEach { assertEquals(9L, java.nio.ByteBuffer.wrap(it, 1, 8).long) }
        // Exactly the limit needs no split.
        assertEquals(1, PhoneFrames.fragments(9, ByteArray(10), key = false, maxPayload = 10).size)
        // Exactly twice: two frames, the last one a real frame.
        assertEquals(listOf<Byte>(0x06, 0x01), PhoneFrames.fragments(9, ByteArray(20), key = false, maxPayload = 10).map { it[0] })
    }

    @Test
    fun `keyframes get the codec config once`() {
        val sps = byteArrayOf(0, 0, 0, 1, 0x67, 1, 0, 0, 0, 1, 0x68, 2)
        val idr = byteArrayOf(0, 0, 0, 1, 0x65, 3)
        val key = PhoneFrames.keyUnit(sps, idr)
        assertArrayEquals(sps + idr, key)
        assertArrayEquals("already has SPS", key, PhoneFrames.keyUnit(sps, key))
        assertArrayEquals("no config known yet", idr, PhoneFrames.keyUnit(null, idr))
    }

    // ---------------------------------------------------------------- backpressure

    private fun au(n: Byte, key: Boolean = false) = QueuedAu(n.toLong(), byteArrayOf(n), key)

    @Test
    fun `queue drops deltas until a keyframe and asks for one when drained`() {
        val q = PhoneSendQueue()
        assertTrue(q.push(au(0, key = true)))
        assertTrue(q.push(au(1)))
        assertTrue(q.push(au(2)))
        assertFalse("full", q.push(au(3)))
        assertEquals(0L, q.pop()!!.au.ptsUs)
        assertFalse("references the lost picture", q.push(au(4)))
        assertEquals(2L, q.dropped)
        assertFalse(q.pop()!!.requestKeyframe)
        assertTrue("drained after drops", q.pop()!!.requestKeyframe)
        assertNull(q.pop())
        assertFalse("still no keyframe", q.push(au(5)))
        assertTrue(q.push(au(6, key = true)))
        assertTrue(q.push(au(7)))
        assertFalse(q.pop()!!.requestKeyframe)
        assertFalse(q.pop()!!.requestKeyframe)
    }

    @Test
    fun `keyframe replaces a full queue and close wakes the reader`() {
        val q = PhoneSendQueue()
        q.push(au(0, key = true))
        q.push(au(1))
        q.push(au(2))
        assertTrue(q.push(au(9, key = true)))
        assertEquals(3L, q.dropped)
        assertEquals(9L, q.take()!!.au.ptsUs)
        q.close()
        assertNull(q.take())
        assertFalse(q.push(au(1, key = true)))
    }

    @Test
    fun `keyframe requests are limited`() {
        val k = KeyframeLimiter(500)
        assertTrue(k.allow(1000))
        assertFalse(k.allow(1499))
        assertTrue(k.allow(1500))
    }

    // ---------------------------------------------------------------- coordinates and gestures

    private val portrait = ScreenBox(1081, 2401)
    private val landscape = ScreenBox(2401, 1081)

    @Test
    fun `normalised coordinates follow the rotation`() {
        assertEquals(Pt(0f, 0f), portrait.px(0.0, 0.0))
        assertEquals(Pt(1080f, 2400f), portrait.px(1.0, 1.0))
        assertEquals(Pt(540f, 1200f), portrait.px(0.5, 0.5))
        assertEquals(Pt(1200f, 540f), landscape.px(0.5, 0.5))
        assertEquals(Pt(2400f, 0f), landscape.px(2.0, -1.0))
    }

    @Test
    fun `tap and long press are single points`() {
        val t = GesturePlanner.tap(portrait, 0.5, 0.5)
        assertEquals(listOf(Pt(540f, 1200f)), t.points)
        assertEquals(GesturePlanner.TAP_MS, t.durationMs)
        assertFalse(t.willContinue || t.continues)
        assertTrue(GesturePlanner.longPress(portrait, 0.5, 0.5).durationMs >= 500)
    }

    @Test
    fun `swipe goes corner to corner in the given time`() {
        val s = GesturePlanner.swipe(portrait, PhoneIn.Swipe(0.0, 0.0, 1.0, 1.0, 300))
        assertEquals(listOf(Pt(0f, 0f), Pt(1080f, 2400f)), s.points)
        assertEquals(300L, s.durationMs)
    }

    @Test
    fun `wheel down moves the finger up and stays on screen`() {
        val box = ScreenBox(1000, 2000)
        val down = GesturePlanner.scroll(box, PhoneIn.Scroll(0.5, 0.5, 1.0, 0.0))!!
        assertEquals(1000f, down.points[0].y, 1.5f)
        assertEquals(down.points[0].y - 240f, down.points[1].y, 0.01f)
        assertEquals(down.points[0].x, down.points[1].x, 0.01f)
        val up = GesturePlanner.scroll(box, PhoneIn.Scroll(0.5, 0.5, -1.0, 0.0))!!
        assertTrue(up.points[1].y > up.points[0].y)
        val sideways = GesturePlanner.scroll(box, PhoneIn.Scroll(0.5, 0.5, 0.0, 1.0))!!
        assertTrue(sideways.points[1].x < sideways.points[0].x)
        // At the top edge there is no room to move up: the start moves down instead.
        val edge = GesturePlanner.scroll(box, PhoneIn.Scroll(0.5, 0.0, 2.0, 0.0))!!
        assertTrue(edge.points[1].y >= 1f)
        assertEquals(480f, edge.points[0].y - edge.points[1].y, 0.01f)
        // Huge scrolls fit too.
        for (dy in listOf(20.0, -20.0)) {
            val big = GesturePlanner.scroll(box, PhoneIn.Scroll(0.5, 0.9, dy, 0.0))!!
            for (p in big.points) assertTrue("$p", p.y in 0f..1999f)
        }
        assertNull(GesturePlanner.scroll(box, PhoneIn.Scroll(0.5, 0.5, 0.0, 0.0)))
    }

    @Test
    fun `a drag is chained strokes`() {
        val d = DragPlanner()
        assertNull(d.next(0))
        val a = Pt(10f, 10f)
        val b = Pt(20f, 20f)
        val c = Pt(30f, 30f)
        val e = Pt(40f, 40f)
        d.down(a, 100)
        assertTrue(d.active)
        val first = d.next(100)!!
        assertEquals(listOf(a), first.points)
        assertTrue(first.willContinue)
        assertFalse(first.continues)
        assertNull("finger held, nothing new", d.next(110))
        // Moves that arrive during a stroke are joined.
        d.move(b)
        d.move(c)
        val second = d.next(150)!!
        assertEquals(listOf(a, b, c), second.points)
        assertEquals(50L, second.durationMs)
        assertTrue(second.continues && second.willContinue)
        d.up(e)
        val last = d.next(160)!!
        assertEquals("starts where the last ended", listOf(c, e), last.points)
        assertTrue(last.continues)
        assertFalse(last.willContinue)
        assertFalse(d.active)
        assertNull(d.next(170))
    }

    @Test
    fun `a stuck finger can be lifted`() {
        val d = DragPlanner()
        val a = Pt(1f, 1f)
        d.down(a, 0)
        d.next(0)
        assertFalse(d.heldLongerThan(1000, 999))
        assertTrue(d.heldLongerThan(1000, 1000))
        d.release()
        val s = d.next(1000)!!
        assertEquals(listOf(a), s.points)
        assertFalse(s.willContinue)
        assertFalse(d.active)
    }

    @Test
    fun `up before the first stroke still presses and lifts`() {
        val d = DragPlanner()
        val a = Pt(1f, 1f)
        val b = Pt(2f, 2f)
        d.down(a, 0)
        d.up(b)
        val s1 = d.next(0)!!
        assertTrue(s1.willContinue)
        val s2 = d.next(20)!!
        assertEquals(listOf(a, b), s2.points)
        assertFalse(s2.willContinue)
        // Stroke durations stay in range.
        d.down(a, 0)
        d.next(0)
        d.move(b)
        assertEquals(DragPlanner.MAX_STROKE_MS, d.next(100_000)!!.durationMs)
    }

    // ---------------------------------------------------------------- text

    private fun field(key: Int = 1, editable: Boolean = true, password: Boolean = false, start: Int = 0, end: Int = 0, hint: Boolean = false) =
        FieldSnapshot(key, editable, password, start, end, hint)

    @Test
    fun `password fields are refused, whatever we typed`() {
        assertEquals(TextPlan.Blocked, TextPlanner.text(field(password = true), "x", null))
        assertEquals(TextPlan.Blocked, TextPlanner.text(field(password = true, end = 3), "x", TypedText(1, "abc")))
        assertEquals(TextPlan.Blocked, TextPlanner.backspace(field(password = true), null))
    }

    @Test
    fun `no editable field means nothing is sent anywhere`() {
        assertEquals(TextPlan.NoField, TextPlanner.text(null, "x", null))
        assertEquals(TextPlan.NoField, TextPlanner.text(field(editable = false), "x", null))
        assertEquals(TextPlan.NoField, TextPlanner.backspace(null, null))
    }

    @Test
    fun `set text only over an empty field or our own text`() {
        assertEquals(TextPlan.SetText("hi"), TextPlanner.text(field(hint = true), "hi", null))
        assertEquals(TextPlan.SetText("hi there"), TextPlanner.text(field(end = 2), " there", TypedText(1, "hi")))
        assertEquals(TextPlan.SetText("hi!"), TextPlanner.text(field(end = -1), "!", TypedText(1, "hi")))
    }

    @Test
    fun `anything else is pasted so nothing is overwritten`() {
        assertEquals("field with text", TextPlan.Paste("x"), TextPlanner.text(field(start = 5, end = 5), "x", null))
        assertEquals("cursor moved away", TextPlan.Paste("x"), TextPlanner.text(field(end = 1), "x", TypedText(1, "hi")))
        assertEquals("another field", TextPlan.Paste("x"), TextPlanner.text(field(key = 2, end = 2), "x", TypedText(1, "hi")))
        assertEquals("unknown contents", TextPlan.Paste("x"), TextPlanner.text(field(), "x", null))
    }

    @Test
    fun `backspace edits our text or cuts one character at the cursor`() {
        assertEquals(TextPlan.SetText("h"), TextPlanner.backspace(field(end = 2), TypedText(1, "hi")))
        assertEquals(TextPlan.CutBefore(5), TextPlanner.backspace(field(start = 5, end = 5), null))
        assertEquals(TextPlan.Skip, TextPlanner.backspace(field(), null))
        assertEquals(TextPlan.Skip, TextPlanner.backspace(field(end = -1), null))
        assertEquals("a", TextPlanner.dropLastChar("a😀"))
        assertEquals("", TextPlanner.dropLastChar(""))
        assertNotNull(TextPlanner.backspace(field(end = 1), TypedText(1, "h")))
    }
}
