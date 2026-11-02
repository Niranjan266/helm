package com.niranjan.helm.core

import com.niranjan.helm.core.FakeAgent.Companion.s
import java.util.Base64
import java.util.concurrent.CopyOnWriteArrayList
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put
import kotlinx.serialization.json.putJsonArray
import kotlinx.serialization.json.putJsonObject

/** The everyday (`g6`) side of [FakeAgent] (Docs/protocol/everyday.md). */
class FakeEveryday {
    /** Every g6 call received: method + params. */
    val calls = CopyOnWriteArrayList<Pair<String, JsonObject>>()

    class Clip(val id: Long, val text: String?, val png: ByteArray?, var timeMs: Long)

    @Volatile var historyEnabled = true
    @Volatile var historyPersist = false
    val clips = CopyOnWriteArrayList<Clip>()

    /** Clipboard text set by `clipboard.pick{to:"pc"}`. */
    @Volatile var pcClipboard: String? = null

    /** `phone.list` (the `this` flag is filled in per session). */
    @Volatile var phones: List<Triple<String, String, Boolean>> = emptyList()

    @Volatile var notifyAccess = "allowed"
    @Volatile var watching = false

    /** Whether the session may save power schedules (admin tier). */
    @Volatile var admin = true
    @Volatile var schedulesJson: JsonArray = JsonArray(emptyList())

    /** Reply of one call: result (or [error] code) plus events to send right after. */
    class Reply(val result: JsonObject?, val error: String? = null, val events: List<Pair<String, JsonObject>> = emptyList())

    private fun ok(r: JsonObject = JsonObject(emptyMap()), vararg events: Pair<String, JsonObject>) = Reply(r, null, events.toList())
    private fun err(code: String) = Reply(null, code)
    private val empty = JsonObject(emptyMap())

    private fun history() = buildJsonObject {
        put("enabled", historyEnabled)
        put("persist", historyPersist)
        putJsonArray("items") {
            for (c in clips) addJsonObject {
                put("id", c.id)
                put("time_ms", c.timeMs)
                if (c.text != null) {
                    put("kind", "text")
                    put("size", c.text.toByteArray().size)
                    put("text", c.text.take(2000))
                    if (c.text.length > 2000) put("truncated", true)
                } else {
                    put("kind", "image")
                    put("size", c.png!!.size)
                    put("thumb_png", Base64.getEncoder().encodeToString(c.png))
                    put("width", 640)
                    put("height", 480)
                }
            }
        }
    }

    fun handle(method: String, p: JsonObject, deviceId: String?, uploadedPaths: Set<String>): Reply {
        calls += method to p
        fun num(k: String) = (p[k] as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull
        val changed = "clipboard.history.changed" to empty
        return when (method) {
            "clipboard.history" -> ok(history())
            "clipboard.history.set" -> {
                (p["enabled"] as? JsonPrimitive)?.booleanOrNull?.let { historyEnabled = it; if (!it) clips.clear() }
                (p["persist"] as? JsonPrimitive)?.booleanOrNull?.let { historyPersist = it }
                ok(history())
            }
            "clipboard.history.clear" -> { clips.clear(); ok(empty, changed) }
            "clipboard.history.delete" -> {
                val c = clips.firstOrNull { it.id == num("id") } ?: return err("bad_params")
                clips.remove(c)
                ok(empty, changed)
            }
            "clipboard.pick" -> {
                val c = clips.firstOrNull { it.id == num("id") } ?: return err("bad_params")
                when (p.s("to")) {
                    "phone" -> ok(buildJsonObject {
                        if (c.text != null) put("text", c.text) else put("png", Base64.getEncoder().encodeToString(c.png))
                    })
                    "pc" -> {
                        pcClipboard = c.text
                        clips.remove(c)
                        c.timeMs += 1
                        clips.add(0, c)
                        ok(empty, changed)
                    }
                    else -> err("bad_params")
                }
            }
            "file.drop" -> {
                val path = p.s("path") ?: return err("bad_params")
                val mode = p.s("mode")?.takeIf { it in listOf("open", "save", "reveal") } ?: return err("bad_params")
                if (path !in uploadedPaths) return err("bad_params")
                val program = path.substringAfterLast('.').lowercase() in listOf("exe", "bat", "ps1", "lnk")
                ok(buildJsonObject { put("path", path); put("mode", if (program && mode == "open") "reveal" else mode) })
            }
            "phone.list" -> ok(buildJsonObject {
                putJsonArray("phones") {
                    for ((id, name, online) in phones) addJsonObject {
                        put("device_id", id); put("device_name", name); put("online", online); put("this", id == deviceId)
                    }
                }
            })
            "phone.ring" -> {
                val target = p.s("device_id")
                val stop = (p["stop"] as? JsonPrimitive)?.booleanOrNull == true
                val targets = phones.filter { (id, _, online) -> online && (if (target != null) id == target else id != deviceId) }
                val ev = buildJsonObject {
                    putJsonObject("from") { deviceId?.let { put("device_id", it) }; put("name", "Test Phone") }
                    put("stop", stop)
                }
                // Our FakeAgent has one socket per test: deliver when the target is this session.
                val events = if (targets.any { it.first == deviceId }) listOf("phone.ring" to ev) else emptyList()
                Reply(buildJsonObject { put("rang", targets.size) }, null, events)
            }
            "pc.notify.watch" -> {
                val on = (p["on"] as? JsonPrimitive)?.booleanOrNull ?: return err("bad_params")
                watching = on && notifyAccess == "allowed"
                val events = if (watching) listOf("pc.notification" to buildJsonObject {
                    put("id", "1234"); put("app", "Outlook"); put("title", "Meeting"); put("body", "Stand-up in 5 minutes")
                    put("time_ms", 1_760_000_000_000L)
                }) else emptyList()
                Reply(buildJsonObject { put("access", notifyAccess) }, null, events)
            }
            "schedule.list" -> ok(buildJsonObject { put("schedules", schedulesJson) })
            "schedule.set" -> {
                val list = (p["schedules"] as? JsonArray)?.map { it as? JsonObject ?: return err("bad_params") } ?: return err("bad_params")
                if (list.size > 64 || list.map { it.s("id") }.toSet().size != list.size) return err("bad_params")
                val time = Regex("""^([01]\d|2[0-3]):[0-5]\d$""")
                if (list.any { it.s("id").isNullOrEmpty() || !time.matches(it.s("time") ?: "") || it["action"] !is JsonObject }) return err("bad_params")
                if (list.any { (it["action"] as JsonObject).containsKey("power") } && !admin) return err("forbidden")
                if (list.any { it.containsKey("next_ms") || it.containsKey("last_run_ms") }) return err("bad_params")
                schedulesJson = buildJsonArray {
                    for (s in list) add(buildJsonObject {
                        s.forEach { (k, v) -> put(k, v) }
                        val days = (s["days"] as? JsonArray).orEmpty()
                        if (days.isEmpty() && s["date"] == null) put("date", "2026-11-02")
                        put("next_ms", if ((s["enabled"] as? JsonPrimitive)?.booleanOrNull != false) JsonPrimitive(1_800_000_000_000L) else JsonNull)
                        put("last_run_ms", JsonNull)
                    })
                }
                val res = buildJsonObject { put("schedules", schedulesJson) }
                ok(res, "schedule.changed" to res)
            }
            "system.health" -> ok(buildJsonObject {
                putJsonObject("battery") { put("percent", 87); put("charging", true); put("ac_online", true) }
                put("cpu_temp_c", 54.5)
                putJsonArray("disks") {
                    addJsonObject { put("mount", "D:\\"); put("label", "Data"); put("free_bytes", 1L shl 40); put("total_bytes", 2L shl 40) }
                    addJsonObject { put("mount", "C:\\"); put("label", "OS"); put("free_bytes", 120L shl 30); put("total_bytes", 512L shl 30) }
                }
                put("uptime_s", 3 * 86_400 + 4 * 3_600 + 125)
                put("agent_uptime_s", 3_600)
            })
            else -> err("unknown_method")
        }
    }

    companion object {
        val METHODS = setOf(
            "clipboard.history", "clipboard.history.set", "clipboard.history.clear", "clipboard.history.delete", "clipboard.pick",
            "file.drop", "phone.list", "phone.ring", "pc.notify.watch", "schedule.list", "schedule.set", "system.health",
        )
    }
}
