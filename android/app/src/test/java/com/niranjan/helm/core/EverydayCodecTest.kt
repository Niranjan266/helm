package com.niranjan.helm.core

import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Everyday (`g6`) codecs and pure logic. */
class EverydayCodecTest {
    private fun obj(s: String) = Json.parseToJsonElement(s) as JsonObject

    @Test
    fun clipHistoryParsesTextAndImageItems() {
        val h = ClipHistory.from(obj("""{"enabled":true,"persist":false,"items":[
            {"id":5,"kind":"text","time_ms":10,"size":3000,"text":"abc","truncated":true},
            {"id":4,"kind":"image","time_ms":9,"size":100,"thumb_png":"iVBORw0KGgo=","width":800,"height":600},
            {"kind":"text"}]}"""))
        assertTrue(h.enabled)
        assertFalse(h.persist)
        assertEquals(2, h.items.size)
        val t = h.items[0]
        assertEquals(5L, t.id)
        assertFalse(t.isImage)
        assertTrue(t.truncated)
        assertEquals("abc", t.text)
        val i = h.items[1]
        assertTrue(i.isImage)
        assertEquals(800, i.width)
        assertEquals(0x89.toByte(), i.thumbPng!![0])
        assertEquals("""{"enabled":true}""", ClipHistory.setParams(true, null).toString())
        assertEquals("""{"persist":false}""", ClipHistory.setParams(null, false).toString())
    }

    @Test
    fun clipPickTextOrPng() {
        assertEquals(ClipPick.Text("full"), ClipPick.from(obj("""{"text":"full"}""")))
        val img = ClipPick.from(obj("""{"png":"AQID"}""")) as ClipPick.Image
        assertEquals(listOf<Byte>(1, 2, 3), img.png.toList())
        assertNull(ClipPick.from(obj("""{"png":"!!"}""")))
        assertNull(ClipPick.from(obj("{}")))
    }

    @Test
    fun dropModesAndDoneText() {
        assertEquals(DropMode.OPEN, DropMode.DEFAULT)
        assertEquals(DropMode.REVEAL, DropMode.fromWire("reveal"))
        assertNull(DropMode.fromWire("run"))
        assertEquals("""{"path":"C:\\x.pdf","mode":"save"}""", DropResult.params("C:\\x.pdf", DropMode.SAVE).toString())
        val r = DropResult.from(obj("""{"path":"C:\\D\\a.exe","mode":"reveal"}"""), DropMode.OPEN)
        assertEquals(DropMode.REVEAL, r.mode)
        assertEquals("a.exe", r.fileName)
        assertEquals("Opened on PC", DropResult.doneText(DropMode.OPEN, DropMode.OPEN))
        assertEquals("Shown in Explorer (programs are not opened)", DropResult.doneText(DropMode.OPEN, DropMode.REVEAL))
        assertEquals("Shown in Explorer", DropResult.doneText(DropMode.REVEAL, DropMode.REVEAL))
        assertEquals("Saved in Downloads\\Helm", DropResult.doneText(DropMode.SAVE, DropMode.SAVE))
        // Missing mode in the result: what we asked for.
        assertEquals(DropMode.SAVE, DropResult.from(obj("""{"path":"p"}"""), DropMode.SAVE).mode)
    }

    @Test
    fun transferPercent() {
        assertEquals(0, TransferPercent.of(0, 1000))
        assertEquals(50, TransferPercent.of(500, 1000))
        assertEquals(100, TransferPercent.of(1000, 1000))
        assertEquals(100, TransferPercent.of(2000, 1000))
        assertEquals(100, TransferPercent.of(0, 0))
        assertEquals(33, TransferPercent.of(1, 3))
    }

    @Test
    fun phonesAndRingEvent() {
        val list = PhoneEntry.listFrom(obj("""{"phones":[
            {"device_id":"a","device_name":"Pixel","online":true,"this":true},
            {"device_id":"b","device_name":"Galaxy","online":true,"this":false},
            {"device_id":"c","device_name":"Old","online":false,"this":false}]}"""))
        assertEquals(3, list.size)
        assertEquals(listOf("b"), PhoneEntry.ringable(list).map { it.deviceId })

        val ev = PhoneRingEvent.from(obj("""{"from":{"device_id":"b","name":"Galaxy"},"stop":false}"""))
        assertEquals(PhoneRingEvent("b", "Galaxy", false), ev)
        val tray = PhoneRingEvent.from(obj("""{"from":{"name":"DESKTOP-1"}}"""))
        assertNull(tray.fromDeviceId)
        assertFalse(tray.stop)
        assertEquals("your PC", PhoneRingEvent.from(obj("{}")).fromName)
        assertTrue(PhoneRingEvent.from(obj("""{"from":{"name":"x"},"stop":true}""")).stop)

        assertEquals("""{"device_id":"b"}""", PhoneRingEvent.ringParams("b", false).toString())
        assertEquals("""{"device_id":"b","stop":true}""", PhoneRingEvent.ringParams("b", true).toString())
        assertEquals("{}", PhoneRingEvent.ringParams(null, false).toString())
    }

    @Test
    fun ringDecisions() {
        val start = PhoneRingEvent(null, "PC", false)
        val stop = PhoneRingEvent(null, "PC", true)
        assertEquals(RingCommand.START, RingLogic.decide(start, ringing = false))
        assertEquals(RingCommand.START, RingLogic.decide(start, ringing = true))
        assertEquals(RingCommand.STOP, RingLogic.decide(stop, ringing = true))
        assertEquals(RingCommand.IGNORE, RingLogic.decide(stop, ringing = false))
    }

    private class FakeVolume(override var current: Int, override val max: Int = 7) : AlarmVolume {
        val sets = mutableListOf<Int>()
        override fun set(level: Int) {
            sets += level
            current = level
        }
    }

    @Test
    fun alarmVolumeRaisedAndRestored() {
        val v = FakeVolume(2)
        val g = AlarmVolumeGuard(v)
        g.raise()
        assertTrue(g.raised)
        assertEquals(7, v.current)
        g.raise() // a second ring while ringing must not save the max level
        g.restore()
        assertFalse(g.raised)
        assertEquals(2, v.current)
        g.restore() // no-op
        assertEquals(listOf(7, 7, 2), v.sets)

        val idle = FakeVolume(4)
        AlarmVolumeGuard(idle).restore()
        assertTrue(idle.sets.isEmpty())
    }

    @Test
    fun notifyAccessAndEvent() {
        assertEquals(NotifyAccess.ALLOWED, NotifyAccess.fromWire("allowed"))
        assertEquals(NotifyAccess.UNSPECIFIED, NotifyAccess.fromWire("unspecified"))
        assertEquals(NotifyAccess.UNSUPPORTED, NotifyAccess.fromWire("weird"))
        assertTrue(NotifyAccess.DENIED.explain.contains("Privacy"))

        val n = PcNotification.from(obj("""{"id":"77","app":"Teams","title":"Hi","body":"There","time_ms":5}"""))!!
        assertEquals(PcNotification("77", "Teams", "Hi", "There", 5), n)
        assertEquals("12", PcNotification.from(obj("""{"id":12}"""))!!.id)
        assertEquals("Windows", PcNotification.from(obj("""{"id":"1","app":""}"""))!!.app)
        assertNull(PcNotification.from(obj("""{"app":"x"}""")))
    }

    @Test
    fun pcNotifyFilterRemembersAppsOffUntilEnabled() {
        var p = PcNotifyPrefs()
        assertFalse(p.allows("Teams"))
        p = p.copy(enabled = true).seen("Teams").seen("Outlook")
        assertEquals(mapOf("Teams" to false, "Outlook" to false), p.apps)
        assertFalse(p.allows("Teams"))
        p = p.withApp("Teams", true)
        assertTrue(p.allows("Teams"))
        assertFalse(p.allows("Outlook"))
        assertEquals(p, p.seen("Teams")) // seen again keeps the choice
        assertTrue(p.copy(allApps = true).allows("Outlook"))
        assertTrue(p.copy(allApps = true).allows("Never seen"))
        assertFalse(p.copy(enabled = false, allApps = true).allows("Teams"))

        val round = PcNotifyPrefs.decode(p.encode())
        assertEquals(p, round)
        assertEquals(PcNotifyPrefs(), PcNotifyPrefs.decode("not json"))
        assertEquals(PcNotifyPrefs(), PcNotifyPrefs.decode(null))
    }

    @Test
    fun scheduleCodecRoundTripKeepsUnknownActions() {
        val src = obj("""{"schedules":[
            {"id":"a","name":"Night","enabled":true,"time":"23:30","days":[5,1,3,3],"action":{"power":"sleep"},"next_ms":99,"last_run_ms":null},
            {"id":"b","enabled":false,"time":"07:00","days":[],"date":"2026-11-02","action":{"media":"play"}},
            {"id":"c","time":"12:00","days":[6,7],"action":{"future":{"x":1}}},
            {"id":"d","time":"12:00","action":{"power":"explode"}},
            {"time":"12:00","action":{"media":"play"}}]}""")
        val list = Schedule.listFrom(src)
        assertEquals(listOf("a", "b", "c", "d"), list.map { it.id })
        val a = list[0]
        assertEquals(ScheduleAction.Power("sleep"), a.action)
        assertEquals(listOf(1, 3, 5), a.days)
        assertEquals(99L, a.nextMs)
        assertNull(a.lastRunMs)
        assertTrue(a.needsAdmin)
        assertTrue(list[1].oneOff)
        assertEquals("2026-11-02", list[1].date)
        assertFalse(list[1].enabled)
        assertEquals(ScheduleAction.Deck(DeckAction.Media("play")), list[1].action)
        assertTrue(list[2].enabled) // missing = enabled
        assertTrue((list[2].action as ScheduleAction.Deck).action is DeckAction.Unknown)
        assertTrue((list[3].action as ScheduleAction.Deck).action is DeckAction.Unknown)
        assertTrue(Schedule.needsAdmin(list))
        assertFalse(Schedule.needsAdmin(list.drop(1)))

        val enc = Schedule.encode(list)
        val arr = enc["schedules"] as JsonArray
        val ea = arr[0].jsonObject
        assertEquals("Night", ea["name"]!!.jsonPrimitive.content)
        assertEquals("[1,3,5]", ea["days"].toString())
        assertFalse("read-only fields are not sent", ea.containsKey("next_ms") || ea.containsKey("last_run_ms"))
        assertEquals("""{"power":"sleep"}""", ea["action"].toString())
        assertEquals("2026-11-02", arr[1].jsonObject["date"]!!.jsonPrimitive.content)
        assertFalse(arr[2].jsonObject.containsKey("date"))
        assertEquals("""{"future":{"x":1}}""", arr[2].jsonObject["action"].toString())
        assertEquals("""{"power":"explode"}""", arr[3].jsonObject["action"].toString())
        // Decoding what we encode gives the same schedules (minus read-only fields).
        assertEquals(list.map { it.copy(nextMs = null, lastRunMs = null) }, Schedule.listFrom(enc))
    }

    @Test
    fun scheduleValidationAndText() {
        assertTrue(Schedule.validTime("00:00"))
        assertTrue(Schedule.validTime("23:59"))
        assertFalse(Schedule.validTime("24:00"))
        assertFalse(Schedule.validTime("7:00"))
        assertFalse(Schedule.validTime("07:60"))
        assertEquals("07:05", Schedule.time(7, 5))
        assertTrue(Schedule.validDate("2026-02-28"))
        assertFalse(Schedule.validDate("2026-13-01"))
        assertTrue(Schedule.validId("a-1"))
        assertFalse(Schedule.validId(""))
        assertFalse(Schedule.validId("has space"))
        assertEquals(listOf(1, 7), Schedule.normDays(listOf(7, 1, 9, 0, 1)))
        assertEquals("Every day", Schedule.describeDays((1..7).toList(), null))
        assertEquals("Weekdays", Schedule.describeDays(listOf(5, 4, 3, 2, 1), null))
        assertEquals("Weekends", Schedule.describeDays(listOf(6, 7), null))
        assertEquals("Mon, Wed", Schedule.describeDays(listOf(3, 1), null))
        assertEquals("Once", Schedule.describeDays(emptyList(), null))
        assertEquals("Once (2026-11-02)", Schedule.describeDays(emptyList(), "2026-11-02"))
        val id = Schedule.newId(listOf("s1"), seed = 1)
        assertEquals("s2", id)
        assertEquals("Shut down", ScheduleAction.powerLabel("shutdown"))
    }

    @Test
    fun healthParsingAndFormatting() {
        val h = SystemHealth.from(obj("""{"battery":{"percent":42,"charging":false,"ac_online":true},"cpu_temp_c":null,
            "disks":[{"mount":"D:\\","label":"Data","free_bytes":1,"total_bytes":2},{"mount":"C:\\","label":"OS","free_bytes":130000000000,"total_bytes":500000000000}],
            "uptime_s":277200,"agent_uptime_s":60}"""))
        assertEquals(HealthBattery(42, false, true), h.battery)
        assertNull(h.cpuTempC)
        assertEquals("C:\\", h.systemDisk!!.mount)
        assertEquals(listOf("Battery 42 % · plugged in", "C: 121 GB free of 465 GB", "Up 3 d 5 h"), SystemHealth.lines(h))

        val desk = SystemHealth.from(obj("""{"battery":null,"cpu_temp_c":61.4,"disks":[{"mount":"E:\\","label":"","free_bytes":0,"total_bytes":0}],"uptime_s":59,"agent_uptime_s":1}"""))
        assertNull(desk.battery)
        assertEquals("E:\\", desk.systemDisk!!.mount)
        assertEquals(listOf("CPU 61 °C", "E: 0 B free of 0 B", "Up 0 min"), SystemHealth.lines(desk))

        assertEquals("4 h 12 min", SystemHealth.formatUptime(4 * 3600 + 12 * 60 + 5))
        assertEquals("12 min", SystemHealth.formatUptime(720))
        assertEquals("3 d 4 h", SystemHealth.formatUptime(3 * 86_400 + 4 * 3_600))
        assertEquals("512 MB", SystemHealth.formatBytes(512L shl 20))
        assertEquals("9.5 GB", SystemHealth.formatBytes((9.5 * (1L shl 30)).toLong()))
        assertEquals("1.8 TB", SystemHealth.formatBytes((1.8 * (1L shl 40)).toLong()))
        assertEquals("100 B", SystemHealth.formatBytes(100))
        assertEquals(SystemHealth(null, null, emptyList(), 0, 0), SystemHealth.from(obj("{}")))
        assertNull(SystemHealth.from(obj("{}")).systemDisk)
    }
}
