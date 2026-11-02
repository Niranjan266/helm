package com.niranjan.helm.core

import com.niranjan.helm.core.FakeAgent.Companion.s
import java.io.ByteArrayInputStream
import java.net.InetAddress
import java.util.UUID
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.CoroutineStart
import kotlinx.coroutines.async
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.jsonObject
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.OkHttpClient
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

/** Everyday (`g6`) methods against the FakeAgent, events, and feature gating. */
class EverydayConnectionTest {
    private val server = MockWebServer()
    private var agent = FakeAgent(features = listOf("p1", "p2", "p3", "p4", "p5", "g6"))
    private val identity = DeviceIdentity(UUID.randomUUID().toString(), "Test Phone", SoftwareDeviceSigner())

    private val plain = object : HelmTransport {
        override val scheme = "ws"
        override fun client(fingerprintHex: String): OkHttpClient = OkHttpClient.Builder().connectTimeout(4, TimeUnit.SECONDS).build()
    }

    @After
    fun tearDown() {
        server.shutdown()
    }

    private suspend fun connected(): HelmConnection {
        server.enqueue(MockResponse().withWebSocketUpgrade(agent.listener()))
        server.start(InetAddress.getByName("127.0.0.1"), 0)
        val conn = HelmConnection(
            agent.hostId, "ab".repeat(32), listOf(Endpoint("127.0.0.1", server.port)), identity,
            HandshakeMode.Pair(agent.validToken), plain, 5_000, 5_000, 5_000,
        )
        conn.connect()
        return conn
    }

    private fun last(method: String) = agent.everyday.calls.last { it.first == method }.second

    private suspend fun expectCode(code: String, block: suspend () -> Unit) {
        try {
            block()
            fail("expected $code")
        } catch (e: HelmException) {
            assertEquals(code, e.code)
        }
    }

    @Test
    fun clipboardHistoryListSetPickDeleteClear() = runBlocking {
        agent.everyday.clips += FakeEveryday.Clip(2, "x".repeat(2500), null, 20)
        agent.everyday.clips += FakeEveryday.Clip(1, null, byteArrayOf(0x89.toByte(), 0x50, 0x4E, 0x47), 10)
        val conn = connected()
        assertTrue(conn.supportsEveryday)
        val api = HelmApi(conn)

        val h = api.clipboardHistory()
        assertTrue(h.enabled)
        assertEquals(listOf(2L, 1L), h.items.map { it.id })
        assertTrue(h.items[0].truncated)
        assertEquals(2000, h.items[0].text!!.length)
        assertTrue(h.items[1].isImage)
        assertEquals(4, h.items[1].thumbPng!!.size)

        val full = api.pickClipboardToPhone(2) as ClipPick.Text
        assertEquals(2500, full.text.length)
        assertEquals("phone", last("clipboard.pick").s("to"))
        val png = api.pickClipboardToPhone(1) as ClipPick.Image
        assertEquals(0x50.toByte(), png.png[1])

        val changed = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("clipboard.history.changed").first() } }
        api.pickClipboardToPc(2)
        changed.await()
        assertEquals("pc", last("clipboard.pick").s("to"))
        assertEquals(2500, agent.everyday.pcClipboard!!.length)

        api.deleteClipboardItem(1)
        assertEquals("1", last("clipboard.history.delete")["id"]!!.jsonPrimitive.content)
        assertEquals(listOf(2L), api.clipboardHistory().items.map { it.id })
        expectCode("bad_params") { api.deleteClipboardItem(99) }
        expectCode("bad_params") { api.pickClipboardToPhone(99) }

        val p = api.setClipboardHistory(persist = true)
        assertTrue(p.persist)
        assertNull(last("clipboard.history.set")["enabled"])
        api.clearClipboardHistory()
        assertTrue(api.clipboardHistory().items.isEmpty())
        val off = api.setClipboardHistory(enabled = false)
        assertFalse(off.enabled)
        conn.close()
    }

    @Test
    fun fileDropAfterUpload() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val data = "hello".toByteArray()
        val path = conn.uploadFile("notes.txt", data.size.toLong(), FakeAgent.sha256Hex(data), { ByteArrayInputStream(data) })
        val r = api.fileDrop(path, DropMode.OPEN)
        assertEquals(DropMode.OPEN, r.mode)
        assertEquals(path, last("file.drop").s("path"))
        assertEquals("open", last("file.drop").s("mode"))
        assertEquals(DropMode.SAVE, api.fileDrop(path, DropMode.SAVE).mode)

        val exe = "MZ".toByteArray()
        val exePath = conn.uploadFile("setup.exe", exe.size.toLong(), FakeAgent.sha256Hex(exe), { ByteArrayInputStream(exe) })
        val shown = api.fileDrop(exePath, DropMode.OPEN)
        assertEquals("programs are revealed, never opened", DropMode.REVEAL, shown.mode)
        expectCode("bad_params") { api.fileDrop("C:\\Windows\\notepad.exe", DropMode.OPEN) }
        conn.close()
    }

    @Test
    fun phoneListRingAndRingEvent() = runBlocking {
        agent.everyday.phones = listOf(
            Triple(identity.deviceId, "Test Phone", true),
            Triple("other", "Galaxy", true),
            Triple("off", "Old phone", false),
        )
        val conn = connected()
        val api = HelmApi(conn)
        val phones = api.phones()
        assertEquals(3, phones.size)
        assertTrue(phones.first { it.deviceId == identity.deviceId }.isThis)
        assertEquals(listOf("other"), PhoneEntry.ringable(phones).map { it.deviceId })

        assertEquals(1, api.ringPhone("other"))
        assertEquals("other", last("phone.ring").s("device_id"))
        assertNull(last("phone.ring")["stop"])
        assertEquals(0, api.ringPhone("off"))
        assertEquals(1, api.ringPhone("other", stop = true))
        assertEquals("true", last("phone.ring")["stop"]!!.jsonPrimitive.content)

        // Ringing ourselves delivers the `phone.ring` event to this session.
        val ring = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events(PhoneRingEvent.NAME).first() } }
        assertEquals(1, api.ringPhone(identity.deviceId))
        val ev = PhoneRingEvent.from(ring.await().data)
        assertEquals("Test Phone", ev.fromName)
        assertFalse(ev.stop)
        assertEquals(RingCommand.START, RingLogic.decide(ev, ringing = false))

        val stop = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events(PhoneRingEvent.NAME).first() } }
        api.ringPhone(identity.deviceId, stop = true)
        assertEquals(RingCommand.STOP, RingLogic.decide(PhoneRingEvent.from(stop.await().data), ringing = true))
        conn.close()
    }

    @Test
    fun pcNotifyWatchAccessAndEvents() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        val note = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events(PcNotification.NAME).first() } }
        assertEquals(NotifyAccess.ALLOWED, api.pcNotifyWatch(true))
        assertTrue(agent.everyday.watching)
        val n = PcNotification.from(note.await().data)!!
        assertEquals("Outlook", n.app)
        assertEquals("Stand-up in 5 minutes", n.body)
        var prefs = PcNotifyPrefs(enabled = true).seen(n.app)
        assertFalse("new apps are off until enabled", prefs.allows(n.app))
        prefs = prefs.withApp(n.app, true)
        assertTrue(prefs.allows(n.app))

        assertEquals(NotifyAccess.ALLOWED, api.pcNotifyWatch(false))
        assertFalse(agent.everyday.watching)
        assertEquals("false", last("pc.notify.watch")["on"]!!.jsonPrimitive.content)

        agent.everyday.notifyAccess = "denied"
        assertEquals(NotifyAccess.DENIED, api.pcNotifyWatch(true))
        assertFalse(agent.everyday.watching)
        agent.everyday.notifyAccess = "unsupported"
        assertEquals(NotifyAccess.UNSUPPORTED, api.pcNotifyWatch(true))
        conn.close()
    }

    @Test
    fun schedulesSetListChangedRanAndForbidden() = runBlocking {
        val conn = connected()
        val api = HelmApi(conn)
        assertTrue(api.schedules().isEmpty())
        val list = listOf(
            Schedule(id = "night", name = "Night", time = "23:30", days = listOf(1, 2, 3, 4, 5), action = ScheduleAction.Power("sleep")),
            Schedule(id = "once", time = "07:00", action = ScheduleAction.Deck(DeckAction.Media("play"))),
            Schedule(id = "raw", enabled = false, time = "12:00", days = listOf(6, 7),
                action = ScheduleAction.Deck(DeckCodec.decodeAction(kotlinx.serialization.json.Json.parseToJsonElement("""{"future":{"x":1}}""")))),
        )
        val changed = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events("schedule.changed").first() } }
        val saved = api.setSchedules(list)
        assertEquals(listOf("night", "once", "raw"), saved.map { it.id })
        assertEquals(1_800_000_000_000L, saved[0].nextMs)
        assertNull(saved[2].nextMs)
        assertEquals("2026-11-02", saved[1].date) // the agent fills in a one-off's date
        assertEquals(ScheduleAction.Power("sleep"), saved[0].action)
        assertEquals(list[2].action, saved[2].action) // unknown action survives the round trip
        val sent = (last("schedule.set")["schedules"] as JsonArray).map { it.jsonObject }
        assertEquals("""{"power":"sleep"}""", sent[0]["action"].toString())
        assertFalse(sent.any { it.containsKey("next_ms") })
        assertEquals(saved, Schedule.listFrom(changed.await().data))
        assertEquals(saved, api.schedules())

        // Saving the list again (with the agent's read-only fields) sends none of them back.
        api.setSchedules(saved)

        agent.everyday.admin = false
        expectCode("forbidden") { api.setSchedules(list) }
        api.setSchedules(list.drop(1)) // no power action: allowed without admin
        expectCode("bad_params") { api.setSchedules(listOf(list[1].copy(time = "25:00"))) }

        // schedule.ran is a plain event.
        val ran = async(start = CoroutineStart.UNDISPATCHED) { withTimeout(5_000) { conn.events(ScheduleRan.NAME).first() } }
        agent.broadcast(ScheduleRan.NAME, kotlinx.serialization.json.buildJsonObject {
            put("id", kotlinx.serialization.json.JsonPrimitive("once")); put("ok", kotlinx.serialization.json.JsonPrimitive(false))
            put("error", kotlinx.serialization.json.JsonPrimitive("no such app"))
        })
        assertEquals(ScheduleRan("once", false, "no such app"), ScheduleRan.from(ran.await().data))
        conn.close()
    }

    @Test
    fun systemHealth() = runBlocking {
        val conn = connected()
        val h = HelmApi(conn).systemHealth()
        assertEquals(87, h.battery!!.percent)
        assertTrue(h.battery!!.charging)
        assertEquals(54.5, h.cpuTempC!!, 0.01)
        assertEquals("C:\\", h.systemDisk!!.mount)
        assertEquals(listOf("Battery 87 % · charging", "CPU 55 °C", "C: 120 GB free of 512 GB", "Up 3 d 4 h"), SystemHealth.lines(h))
        assertEquals(3_600L, h.agentUptimeS)
        conn.close()
    }

    @Test
    fun everyCallIsGatedOnG6() = runBlocking {
        agent = FakeAgent(features = listOf("p1", "p2", "p3", "p4", "p5"))
        val conn = connected()
        assertFalse(conn.supportsEveryday)
        val api = HelmApi(conn)
        val calls: List<suspend () -> Unit> = listOf(
            { api.clipboardHistory() }, { api.setClipboardHistory(enabled = true) }, { api.clearClipboardHistory() },
            { api.deleteClipboardItem(1) }, { api.pickClipboardToPhone(1) }, { api.pickClipboardToPc(1) },
            { api.fileDrop("C:\\x", DropMode.OPEN) }, { api.phones() }, { api.ringPhone(null) }, { api.pcNotifyWatch(true) },
            { api.schedules() }, { api.setSchedules(emptyList()) }, { api.systemHealth() },
        )
        for (c in calls) expectCode(LocalCodes.UNSUPPORTED) { c() }
        assertTrue("nothing was sent", agent.everyday.calls.isEmpty())
        conn.close()
    }
}
