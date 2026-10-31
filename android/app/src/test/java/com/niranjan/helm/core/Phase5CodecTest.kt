package com.niranjan.helm.core

import java.util.Base64
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.jsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Phase 5 (§13) codecs: VT screen, terminal keys, pen frames, notifications, plugins, triggers, HA. */
class Phase5CodecTest {
    private fun obj(s: String) = Json.parseToJsonElement(s) as JsonObject

    // ---- Terminal screen ----------------------------------------------------------------

    @Test
    fun plainTextWrapsAndScrollsIntoScrollback() {
        val t = TermScreen(5, 2)
        t.feed("abcdefg\r\nxy\r\nz")
        assertEquals(listOf("xy", "z"), t.screenText())
        assertEquals(listOf("abcde", "fg"), t.scrollbackText())
        assertEquals(1, t.cursorCol)
    }

    @Test
    fun sgrColoursBecomeRuns() {
        val t = TermScreen(20, 2)
        t.feed("\u001b[32mPS\u001b[0m C:\\> \u001b[1;91mERR\u001b[38;5;21mX\u001b[48;2;1;2;3mY")
        val runs = t.lines().last { it.isNotEmpty() }
        assertEquals("PS", runs[0].text)
        assertEquals(TermPalette.ansi(2), runs[0].style.fg)
        assertEquals(" C:\\> ", runs[1].text)
        assertEquals(TermStyle.DEFAULT, runs[1].style)
        assertEquals("ERR", runs[2].text)
        assertTrue(runs[2].style.bold)
        assertEquals(TermPalette.ansi(9), runs[2].style.fg)
        assertEquals(TermPalette.rgb(0, 0, 255), runs[3].style.fg)
        assertEquals(TermPalette.rgb(1, 2, 3), runs[4].style.bg)
    }

    @Test
    fun cursorPositionEraseAndOscAreHandled() {
        val t = TermScreen(10, 3)
        t.feed("\u001b]0;Windows PowerShell\u0007hello\r\nworld")
        t.feed("\u001b[1;3HXY\u001b[2;1H\u001b[K")
        assertEquals(listOf("heXYo", "", ""), t.screenText())
        t.feed("\u001b[2J\u001b[Hnew")
        assertEquals(listOf("new", "", ""), t.screenText())
        t.feed("\u001b[?25l")
        assertFalse(t.cursorVisible)
    }

    @Test
    fun splitUtf8AndEscapesAcrossChunks() {
        val t = TermScreen(10, 1)
        val bytes = "é\u001b[31mR".toByteArray(Charsets.UTF_8)
        for (b in bytes) t.feed(byteArrayOf(b))
        assertEquals(listOf("éR"), t.screenText())
        assertEquals(TermPalette.ansi(1), t.lines().last()[1].style.fg)
    }

    @Test
    fun insertDeleteAndBackspace() {
        val t = TermScreen(10, 1)
        t.feed("abcdef\r\u001b[2C\u001b[2P")
        assertEquals(listOf("abef"), t.screenText())
        t.feed("\u001b[1@Z")
        assertEquals(listOf("abZef"), t.screenText())
        t.feed("\b\u001b[K")
        assertEquals(listOf("ab"), t.screenText())
    }

    @Test
    fun alternateScreenRestoresMain() {
        val t = TermScreen(10, 2)
        t.feed("main")
        t.feed("\u001b[?1049h")
        assertEquals(listOf("", ""), t.screenText())
        t.feed("alt")
        t.feed("\u001b[?1049l")
        assertEquals(listOf("main", ""), t.screenText())
    }

    @Test
    fun resizeKeepsPromptRow() {
        val t = TermScreen(10, 4)
        t.feed("1\r\n2\r\n3\r\nPS>")
        t.resize(6, 2)
        assertEquals(listOf("3", "PS>"), t.screenText())
        assertEquals(listOf("1", "2"), t.scrollbackText())
        assertEquals(1, t.cursorRow)
        t.resize(6, 3)
        assertEquals(listOf("3", "PS>", ""), t.screenText())
    }

    @Test
    fun scrollRegionOnlyScrollsInside() {
        val t = TermScreen(5, 3)
        t.feed("top\u001b[2;3r\u001b[2;1Ha\r\nb\r\nc")
        assertEquals(listOf("top", "b", "c"), t.screenText())
        assertTrue(t.scrollbackText().isEmpty())
    }

    @Test
    fun termKeysAndSize() {
        assertEquals("\u0003", TermKeys.ctrl('c'))
        assertEquals("\u001a", TermKeys.ctrl('Z'))
        assertNull(TermKeys.ctrl('1'))
        assertEquals("ls\r", String(Base64.getDecoder().decode(TermKeys.b64("ls" + TermKeys.ENTER))))
        assertEquals(100 to 40, TermSize.fit(1000f, 800f, 10f, 20f))
        assertEquals(20 to 5, TermSize.fit(10f, 10f, 10f, 20f))
        assertEquals(80 to 24, TermSize.fit(10f, 10f, 0f, 20f))
    }

    @Test
    fun termEvents() {
        val out = TermEvent.from("term.output", obj("""{"term_id":7,"data_b64":"${TermKeys.b64("hi")}"}""")) as TermEvent.Output
        assertEquals("7", out.termId)
        assertEquals("hi", String(out.data))
        assertEquals(TermEvent.Exit("t1", 3), TermEvent.from("term.exit", obj("""{"term_id":"t1","code":3}""")))
        assertNull(TermEvent.from("term.output", obj("""{"term_id":7,"data_b64":"%%%"}""")))
        assertNull(TermEvent.from("other", obj("{}")))
    }

    // ---- Pen ----------------------------------------------------------------------------

    @Test
    fun penFrameShapeAndTilt() {
        val f = PenMath.frame(0.25f, 1.5f, 0.6f, (Math.PI / 4).toFloat(), (Math.PI / 2).toFloat(), PenState.DOWN, eraser = true).toJson()
        assertEquals("in", f["type"]!!.jsonPrimitive.content)
        assertEquals("pn", f["k"]!!.jsonPrimitive.content)
        assertEquals(0.25, f["x"]!!.jsonPrimitive.content.toDouble(), 1e-6)
        assertEquals(1.0, f["y"]!!.jsonPrimitive.content.toDouble(), 1e-6)
        assertEquals(0.6, f["pressure"]!!.jsonPrimitive.content.toDouble(), 1e-4)
        assertEquals(45.0, f["tilt_x"]!!.jsonPrimitive.content.toDouble(), 0.05)
        assertEquals(0.0, f["tilt_y"]!!.jsonPrimitive.content.toDouble(), 0.05)
        assertEquals("down", f["state"]!!.jsonPrimitive.content)
        assertEquals("true", f["eraser"]!!.jsonPrimitive.content)
        assertTrue(f["id"] == null)

        val hover = PenMath.frame(0.5f, 0.5f, 0.9f, 0f, 0f, PenState.HOVER, eraser = false).toJson()
        assertEquals(0.0, hover["pressure"]!!.jsonPrimitive.content.toDouble(), 0.0)
        assertEquals("hover", hover["state"]!!.jsonPrimitive.content)

        // Pen pointing up (away from the user): top leans away, so tilt_y is negative.
        val (tx, ty) = PenMath.tilt((Math.PI / 6).toFloat(), 0f)
        assertEquals(0f, tx, 0.01f)
        assertEquals(-30f, ty, 0.05f)
        assertEquals(0f to 0f, PenMath.tilt(Float.NaN, 0f))
    }

    // ---- Notifications ------------------------------------------------------------------

    @Test
    fun notifyPushJsonAndActionEvent() {
        val n = NotifyPush(
            app = "Messages", title = "Sam", body = "x".repeat(2000),
            actions = (1..7).map { NotifyActionButton("a$it", "Action $it") }, reply = true, key = "0|pkg|1|null|10",
            iconPng = byteArrayOf(1, 2, 3),
        ).toJson()
        assertEquals("Messages", n["app"]!!.jsonPrimitive.content)
        assertEquals(NotifyPush.MAX_BODY, n["body"]!!.jsonPrimitive.content.length)
        assertEquals(NotifyPush.MAX_ACTIONS, n["actions"]!!.jsonArray.size)
        assertEquals("a1", n["actions"]!!.jsonArray[0].jsonObject["id"]!!.jsonPrimitive.content)
        assertEquals("true", n["reply"]!!.jsonPrimitive.content)
        assertEquals("AQID", n["icon_png_b64"]!!.jsonPrimitive.content)
        assertTrue(NotifyPush("a", "t", "b", key = "k").toJson()["icon_png_b64"] == null)

        assertEquals(NotifyActionEvent("k", "reply", "ok"), NotifyActionEvent.from(obj("""{"key":"k","action_id":"reply","reply_text":"ok"}""")))
        assertEquals(NotifyActionEvent("k", null, null), NotifyActionEvent.from(obj("""{"key":"k"}""")))
        assertNull(NotifyActionEvent.from(obj("""{"action_id":"x"}""")))
    }

    @Test
    fun allowListIsOffByDefault() {
        assertEquals(emptySet<String>(), NotifyAllowList.decode(null))
        val set = NotifyAllowList.decode(NotifyAllowList.encode(setOf("com.b", "com.a", "bad,pkg", " ")))
        assertEquals(setOf("com.a", "com.b"), set)
        assertEquals("com.a,com.b", NotifyAllowList.encode(set))
        assertFalse(NotifyAllowList.shouldMirror(false, set, "com.a", "com.niranjan.helm"))
        assertTrue(NotifyAllowList.shouldMirror(true, set, "com.a", "com.niranjan.helm"))
        assertFalse(NotifyAllowList.shouldMirror(true, set, "com.c", "com.niranjan.helm"))
        assertFalse(NotifyAllowList.shouldMirror(true, set + "com.niranjan.helm", "com.niranjan.helm", "com.niranjan.helm"))
    }

    // ---- Plugins, triggers, Home Assistant ----------------------------------------------

    @Test
    fun pluginsCodec() {
        val list = PluginInfo.listFrom(obj("""{"plugins":[
            {"id":"w","name":"Weather","version":"1.0","capabilities":["deck.action","http:api.x.com"],"granted":["deck.action"],"enabled":true},
            {"id":"e","capabilities":[],"granted":[],"enabled":false,"error":"trap"},
            {"name":"no id"}]}"""))
        assertEquals(2, list.size)
        assertEquals(listOf("http:api.x.com"), list[0].missing)
        assertEquals("e", list[1].name)
        assertEquals("trap", list[1].error)
        val p = PluginInfo.setParams("w", true, listOf("notify", "notify"))
        assertEquals("""{"id":"w","enabled":true,"granted":["notify"]}""", p.toString())
        assertEquals("Talk to api.x.com over the network", PluginInfo.describe("http:api.x.com"))
    }

    @Test
    fun triggersRoundTripKeepsUnknown() {
        val src = obj("""{"triggers":[
            {"id":"t1","when":{"time":"07:30 weekdays"},"action":{"media":"play"}},
            {"id":"t2","when":{"app_start":"obs64.exe"},"action":{"ha":{"service":"light.turn_on","entity_id":"light.desk"}}},
            {"id":"t3","when":{"phone_connected":null},"action":{"hotkey":"Win+D"}},
            {"id":"t4","when":{"wifi":"Home"},"action":{"macro":[{"text":"hi"},{"delay_ms":100}]}},
            {"id":"t5","when":{"moon_phase":"full"},"action":{"future":1}}]}""")
        val list = Trigger.listFrom(src)
        assertEquals(TriggerWhen.Time("07:30 weekdays"), list[0].`when`)
        assertEquals(DeckAction.Ha("light.turn_on", "light.desk"), list[1].action)
        assertEquals(TriggerWhen.PhoneConnected(null), list[2].`when`)
        assertEquals(TriggerWhen.Wifi("Home"), list[3].`when`)
        assertTrue(list[4].`when` is TriggerWhen.Unknown)
        assertEquals(src, Trigger.encode(list))
        assertEquals(TriggerWhen.PhoneConnected("dev-1"), TriggerWhen.decode(obj("""{"phone_connected":"dev-1"}""")))
        assertTrue(TriggerWhen.validTime("7:05"))
        assertTrue(TriggerWhen.validTime("23:59 sat,sun"))
        assertFalse(TriggerWhen.validTime("24:00"))
        assertFalse(TriggerWhen.validTime("noon"))
        assertEquals(JsonNull, Trigger.encode(listOf(Trigger("x", TriggerWhen.PhoneConnected(null), DeckAction.Media("play"))))["triggers"]!!
            .jsonArray[0].jsonObject["when"]!!.jsonObject["phone_connected"])
    }

    @Test
    fun haStatesAndDeckAction() {
        val states = HaState.listFrom(obj("""{"states":[
            {"entity_id":"switch.fan","state":"off","attributes":{}},
            {"entity_id":"light.desk","state":"on","attributes":{"friendly_name":"Desk lamp"}},
            {"state":"broken"}]}"""))
        assertEquals(listOf("light.desk", "switch.fan"), states.map { it.entityId })
        assertEquals("Desk lamp", states[0].label)
        assertEquals("light.toggle", states[0].toggleService)
        assertEquals("homeassistant.toggle", HaState("sensor.t", "1", null, null).toggleService)
        assertTrue(HaState.validEntityId("light.desk_2"))
        assertFalse(HaState.validEntityId("Light.Desk"))
        assertFalse(HaState.validService("light"))

        val raw = obj("""{"ha":{"service":"light.toggle","entity_id":"light.desk"}}""")
        val a = DeckCodec.decodeAction(raw)
        assertEquals(DeckAction.Ha("light.toggle", "light.desk"), a)
        assertEquals(raw, DeckCodec.encodeAction(a))
        assertTrue(DeckCodec.needsPhase5(a))
        assertTrue(DeckCodec.needsPhase5(DeckAction.Macro(listOf(DeckAction.Text("x"), a))))
        assertFalse(DeckCodec.needsPhase5(DeckAction.Text("x")))
        // Extra fields: kept raw.
        assertTrue(DeckCodec.decodeAction(obj("""{"ha":{"service":"x.y","entity_id":"a.b","data":{}}}""")) is DeckAction.Unknown)
    }

    @Test
    fun recordingResult() {
        val r = Recording.from(obj("""{"path":"C:\\Users\\a\\Videos\\Helm\\rec.mp4","duration_ms":1500}"""))!!
        assertEquals("rec.mp4", r.fileName)
        assertEquals(1500L, r.durationMs)
        assertNull(Recording.from(obj("{}")))
        assertEquals(JsonPrimitive("p5"), JsonPrimitive(Features.P5))
    }
}
