package com.niranjan.helm.core

import java.util.Base64
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

// Everyday features (Docs/protocol/everyday.md, feature `g6`) models and codecs. Plain JVM,
// unit-tested.

object Everyday {
    const val FEATURE = "g6"
}

/** True when the agent advertised the everyday (`g6`) methods in `hello`. */
val HelmConnection.supportsEveryday: Boolean get() = Everyday.FEATURE in hostFeatures

private fun b64(s: String?): ByteArray? = s?.let {
    try {
        Base64.getDecoder().decode(it)
    } catch (_: IllegalArgumentException) {
        null
    }
}

// ---- 1. Clipboard history --------------------------------------------------------------

/** One `clipboard.history` item. Text items carry a preview, image items a thumbnail. */
data class ClipItem(
    val id: Long,
    val kind: String,
    val timeMs: Long,
    val size: Long,
    val text: String? = null,
    val truncated: Boolean = false,
    val thumbPng: ByteArray? = null,
    val width: Int? = null,
    val height: Int? = null,
) {
    val isImage: Boolean get() = kind == KIND_IMAGE

    override fun equals(other: Any?): Boolean =
        other is ClipItem && other.id == id && other.kind == kind && other.timeMs == timeMs && other.size == size &&
            other.text == text && other.truncated == truncated && other.width == width && other.height == height &&
            (other.thumbPng?.contentEquals(thumbPng) ?: (thumbPng == null))

    override fun hashCode(): Int = id.hashCode() * 31 + timeMs.hashCode()

    companion object {
        const val KIND_TEXT = "text"
        const val KIND_IMAGE = "image"

        fun from(o: JsonObject): ClipItem? {
            val id = o.optLong("id") ?: return null
            val kind = o.optStr("kind") ?: return null
            return ClipItem(
                id = id,
                kind = kind,
                timeMs = o.optLong("time_ms") ?: 0,
                size = o.optLong("size") ?: 0,
                text = o.optStr("text"),
                truncated = o.optBool("truncated") == true,
                thumbPng = b64(o.optStr("thumb_png")),
                width = o.optLong("width")?.toInt(),
                height = o.optLong("height")?.toInt(),
            )
        }
    }
}

/** `clipboard.history` / `clipboard.history.set` result. */
data class ClipHistory(val enabled: Boolean, val persist: Boolean, val items: List<ClipItem>) {
    companion object {
        fun from(o: JsonObject) = ClipHistory(
            enabled = o.optBool("enabled") == true,
            persist = o.optBool("persist") == true,
            items = o.objList("items").mapNotNull { ClipItem.from(it) },
        )

        fun setParams(enabled: Boolean?, persist: Boolean?): JsonObject = buildJsonObject {
            enabled?.let { put("enabled", it) }
            persist?.let { put("persist", it) }
        }
    }
}

/** `clipboard.pick{to:"phone"}` result: the full item. */
sealed interface ClipPick {
    data class Text(val text: String) : ClipPick
    class Image(val png: ByteArray) : ClipPick

    companion object {
        const val TO_PHONE = "phone"
        const val TO_PC = "pc"

        fun from(o: JsonObject): ClipPick? {
            o.optStr("text")?.let { return Text(it) }
            b64(o.optStr("png"))?.let { return Image(it) }
            return null
        }
    }
}

// ---- 2. Quick file drop ----------------------------------------------------------------

/** What the PC does with a file shared to Helm ("When a file arrives on the PC"). */
enum class DropMode(val wire: String, val label: String) {
    OPEN("open", "Open it"),
    SAVE("save", "Just save it"),
    REVEAL("reveal", "Show it in Explorer");

    companion object {
        val DEFAULT = OPEN
        fun fromWire(s: String?): DropMode? = entries.firstOrNull { it.wire == s }
    }
}

/** `file.drop` result; [mode] is what was actually done (programs are revealed, not opened). */
data class DropResult(val path: String, val mode: DropMode) {
    val fileName: String get() = path.substringAfterLast('\\').substringAfterLast('/')

    companion object {
        fun from(o: JsonObject, asked: DropMode): DropResult =
            DropResult(o.optStr("path") ?: "", DropMode.fromWire(o.optStr("mode")) ?: asked)

        fun params(path: String, mode: DropMode): JsonObject = buildJsonObject {
            put("path", path)
            put("mode", mode.wire)
        }

        /** Short status for the finished upload notification. */
        fun doneText(asked: DropMode, done: DropMode?): String = when (done) {
            null -> "Saved in Downloads\\Helm"
            DropMode.OPEN -> "Opened on PC"
            DropMode.SAVE -> "Saved in Downloads\\Helm"
            DropMode.REVEAL -> if (asked == DropMode.OPEN) "Shown in Explorer (programs are not opened)" else "Shown in Explorer"
        }
    }
}

/** Upload progress → whole percent, 0..100 (an empty file is 100). */
object TransferPercent {
    fun of(sent: Long, size: Long): Int = if (size <= 0) 100 else ((sent.coerceIn(0, size) * 100) / size).toInt()
}

// ---- 3. Find my phone ------------------------------------------------------------------

/** `phone.list` entry. */
data class PhoneEntry(val deviceId: String, val name: String, val online: Boolean, val isThis: Boolean) {
    companion object {
        fun listFrom(o: JsonObject): List<PhoneEntry> = o.objList("phones").mapNotNull { p ->
            val id = p.optStr("device_id") ?: return@mapNotNull null
            PhoneEntry(id, p.optStr("device_name") ?: id, p.optBool("online") == true, p.optBool("this") == true)
        }

        /** Phones "Ring another phone" can ring: online and not this one. */
        fun ringable(list: List<PhoneEntry>): List<PhoneEntry> = list.filter { it.online && !it.isThis }
    }
}

/** `phone.ring` event: start (or [stop]) ringing this phone. */
data class PhoneRingEvent(val fromDeviceId: String?, val fromName: String, val stop: Boolean) {
    companion object {
        const val NAME = "phone.ring"

        fun from(data: JsonObject): PhoneRingEvent {
            val from = data["from"] as? JsonObject
            return PhoneRingEvent(
                fromDeviceId = from?.optStr("device_id"),
                fromName = from?.optStr("name")?.takeIf { it.isNotBlank() } ?: "your PC",
                stop = data.optBool("stop") == true,
            )
        }

        fun ringParams(deviceId: String?, stop: Boolean): JsonObject = buildJsonObject {
            deviceId?.let { put("device_id", it) }
            if (stop) put("stop", true)
        }
    }
}

/** The phone's alarm stream volume (AudioManager on Android, a fake in tests). */
interface AlarmVolume {
    val current: Int
    val max: Int
    fun set(level: Int)
}

/**
 * Raises the alarm volume to maximum while ringing and puts back what the user had. Raising
 * twice keeps the first saved level; restoring without raising does nothing.
 */
class AlarmVolumeGuard(private val volume: AlarmVolume) {
    private var saved: Int? = null

    val raised: Boolean get() = saved != null

    @Synchronized
    fun raise() {
        if (saved == null) saved = volume.current
        volume.set(volume.max)
    }

    @Synchronized
    fun restore() {
        val s = saved ?: return
        saved = null
        volume.set(s)
    }
}

/** What to do with a `phone.ring` event given whether the phone is already ringing. */
enum class RingCommand { START, STOP, IGNORE }

object RingLogic {
    fun decide(ev: PhoneRingEvent, ringing: Boolean): RingCommand = when {
        ev.stop -> if (ringing) RingCommand.STOP else RingCommand.IGNORE
        else -> RingCommand.START
    }
}

// ---- 4. PC notifications ---------------------------------------------------------------

/** `pc.notify.watch` result. */
enum class NotifyAccess(val wire: String) {
    ALLOWED("allowed"),
    DENIED("denied"),
    UNSPECIFIED("unspecified"),
    UNSUPPORTED("unsupported");

    /** What Settings shows. */
    val explain: String
        get() = when (this) {
            ALLOWED -> "Helm can read the PC's notifications."
            DENIED -> "Windows does not let Helm read notifications. On the PC open Settings > Privacy & security > " +
                "Notifications, turn on \"Let apps access your notifications\" and allow Helm."
            UNSPECIFIED -> "Windows is asking on the PC whether Helm may read notifications. Answer the prompt there, then turn this on again."
            UNSUPPORTED -> "This version of Windows (or the Helm agent) cannot share notifications."
        }

    companion object {
        fun fromWire(s: String?): NotifyAccess = entries.firstOrNull { it.wire == s } ?: UNSUPPORTED
    }
}

/** `pc.notification` event. */
data class PcNotification(val id: String, val app: String, val title: String, val body: String, val timeMs: Long) {
    companion object {
        const val NAME = "pc.notification"

        fun from(o: JsonObject): PcNotification? {
            val id = (o["id"] as? JsonPrimitive)?.content ?: return null
            val app = o.optStr("app")?.takeIf { it.isNotBlank() } ?: "Windows"
            return PcNotification(id, app, o.optStr("title") ?: "", o.optStr("body") ?: "", o.optLong("time_ms") ?: 0)
        }
    }
}

/**
 * Phone-side filter for PC notifications. Every app seen is remembered (off until enabled);
 * [allApps] lets everything through. [enabled] = "Show PC notifications" (off by default).
 */
data class PcNotifyPrefs(
    val enabled: Boolean = false,
    val allApps: Boolean = false,
    /** App name → shown. */
    val apps: Map<String, Boolean> = emptyMap(),
) {
    /** Remembers [app] (off) if it is new. */
    fun seen(app: String): PcNotifyPrefs = if (app in apps || app.isBlank()) this else copy(apps = apps + (app to false))

    fun allows(app: String): Boolean = enabled && (allApps || apps[app] == true)

    fun withApp(app: String, on: Boolean): PcNotifyPrefs = copy(apps = apps + (app to on))

    fun encode(): String = buildJsonObject {
        put("enabled", enabled)
        put("all", allApps)
        put("apps", buildJsonObject { apps.toSortedMap().forEach { (k, v) -> put(k, v) } })
    }.toString()

    companion object {
        fun decode(s: String?): PcNotifyPrefs {
            if (s.isNullOrBlank()) return PcNotifyPrefs()
            val o = try {
                Json.parseToJsonElement(s) as? JsonObject
            } catch (_: Exception) {
                null
            } ?: return PcNotifyPrefs()
            val apps = (o["apps"] as? JsonObject).orEmpty().mapNotNull { (k, v) ->
                (v as? JsonPrimitive)?.booleanOrNull?.let { k to it }
            }.toMap()
            return PcNotifyPrefs(o.optBool("enabled") == true, o.optBool("all") == true, apps)
        }
    }
}

// ---- 5. Scheduled actions --------------------------------------------------------------

/** What a schedule runs: a power action or any deck action (unknown JSON kept raw). */
sealed interface ScheduleAction {
    data class Power(val kind: String) : ScheduleAction
    data class Deck(val action: DeckAction) : ScheduleAction

    companion object {
        val POWER_KINDS = listOf("lock", "sleep", "hibernate", "restart", "shutdown", "sign_out")

        fun powerLabel(k: String) = when (k) {
            "lock" -> "Lock"
            "sleep" -> "Sleep"
            "hibernate" -> "Hibernate"
            "restart" -> "Restart"
            "shutdown" -> "Shut down"
            "sign_out" -> "Sign out"
            else -> k
        }

        fun decode(el: JsonElement?): ScheduleAction {
            val o = el as? JsonObject
            if (o != null && o.size == 1) {
                o.optStr("power")?.takeIf { it in POWER_KINDS }?.let { return Power(it) }
            }
            return Deck(DeckCodec.decodeAction(el))
        }

        fun encode(a: ScheduleAction): JsonElement = when (a) {
            is Power -> buildJsonObject { put("power", a.kind) }
            is Deck -> DeckCodec.encodeAction(a.action)
        }
    }
}

/** `schedule.list` / `schedule.set` entry. [nextMs] / [lastRunMs] are read-only (agent-filled). */
data class Schedule(
    val id: String,
    val name: String? = null,
    val enabled: Boolean = true,
    val time: String,
    /** ISO weekdays 1 (Mon) .. 7 (Sun); empty = one-off. */
    val days: List<Int> = emptyList(),
    val date: String? = null,
    val action: ScheduleAction,
    val nextMs: Long? = null,
    val lastRunMs: Long? = null,
) {
    val oneOff: Boolean get() = days.isEmpty()
    val needsAdmin: Boolean get() = action is ScheduleAction.Power

    companion object {
        const val MAX = 64
        const val MAX_NAME = 100
        val DAY_LABELS = listOf("Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun")

        private val TIME = Regex("""^([01]\d|2[0-3]):[0-5]\d$""")
        private val DATE = Regex("""^\d{4}-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$""")

        fun validTime(t: String): Boolean = TIME.matches(t)
        fun validDate(d: String): Boolean = DATE.matches(d)
        fun validId(id: String): Boolean = id.length in 1..64 && id.all { it.code in 0x21..0x7e }

        /** `H:MM` / `HH:MM` from a picker → wire `HH:MM`. */
        fun time(hour: Int, minute: Int): String = "%02d:%02d".format(hour.coerceIn(0, 23), minute.coerceIn(0, 59))

        fun normDays(days: Collection<Int>): List<Int> = days.filter { it in 1..7 }.distinct().sorted()

        private fun dayList(el: JsonElement?): List<Int> =
            (el as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> !p.isString }?.longOrNull?.toInt() }

        fun from(o: JsonObject): Schedule? {
            val id = o.optStr("id") ?: return null
            val time = o.optStr("time") ?: return null
            return Schedule(
                id = id,
                name = o.optStr("name")?.takeIf { it.isNotBlank() },
                enabled = o.optBool("enabled") != false,
                time = time,
                days = normDays(dayList(o["days"])),
                date = o.optStr("date")?.takeIf { it.isNotBlank() },
                action = ScheduleAction.decode(o["action"]),
                nextMs = o.optLong("next_ms"),
                lastRunMs = o.optLong("last_run_ms"),
            )
        }

        fun listFrom(o: JsonObject): List<Schedule> = o.objList("schedules").mapNotNull { from(it) }

        /** `schedule.set` params (read-only fields left out). */
        fun encode(list: List<Schedule>): JsonObject = buildJsonObject {
            put("schedules", buildJsonArray {
                for (s in list) addJsonObject {
                    put("id", s.id)
                    s.name?.takeIf { it.isNotBlank() }?.let { put("name", it.take(MAX_NAME)) }
                    put("enabled", s.enabled)
                    put("time", s.time)
                    put("days", buildJsonArray { normDays(s.days).forEach { add(JsonPrimitive(it)) } })
                    if (s.days.isEmpty()) s.date?.let { put("date", it) }
                    put("action", ScheduleAction.encode(s.action))
                }
            })
        }

        /** True when saving [list] needs the admin tier (any power action). */
        fun needsAdmin(list: List<Schedule>): Boolean = list.any { it.needsAdmin }

        /** "Every day", "Weekdays", "Weekends", "Mon, Wed" or "Once (2026-11-02)". */
        fun describeDays(days: List<Int>, date: String?): String {
            val d = normDays(days)
            return when {
                d.isEmpty() -> if (date != null) "Once ($date)" else "Once"
                d.size == 7 -> "Every day"
                d == listOf(1, 2, 3, 4, 5) -> "Weekdays"
                d == listOf(6, 7) -> "Weekends"
                else -> d.joinToString(", ") { DAY_LABELS[it - 1] }
            }
        }

        /** A fresh id not used in [existing]. */
        fun newId(existing: Collection<String>, seed: Long = System.nanoTime()): String {
            var n = seed and 0xffffff
            while (true) {
                val id = "s" + n.toString(16)
                if (id !in existing) return id
                n++
            }
        }
    }
}

/** `schedule.ran` event. */
data class ScheduleRan(val id: String, val ok: Boolean, val error: String?) {
    companion object {
        const val NAME = "schedule.ran"
        fun from(o: JsonObject): ScheduleRan? = o.optStr("id")?.let { ScheduleRan(it, o.optBool("ok") == true, o.optStr("error")) }
    }
}

// ---- 6. Health card --------------------------------------------------------------------

data class HealthBattery(val percent: Int?, val charging: Boolean, val acOnline: Boolean?)
data class HealthDisk(val mount: String, val label: String, val freeBytes: Long, val totalBytes: Long)

/** `system.health` result. */
data class SystemHealth(
    val battery: HealthBattery?,
    val cpuTempC: Double?,
    val disks: List<HealthDisk>,
    val uptimeS: Long,
    val agentUptimeS: Long,
) {
    /** The system drive (`C:\`), else the first disk. */
    val systemDisk: HealthDisk?
        get() = disks.firstOrNull { it.mount.trimEnd('\\', '/').equals("C:", ignoreCase = true) } ?: disks.firstOrNull()

    companion object {
        fun from(o: JsonObject): SystemHealth {
            val b = o["battery"] as? JsonObject
            return SystemHealth(
                battery = b?.let { HealthBattery(it.optLong("percent")?.toInt() ?: it.optDouble("percent")?.toInt(), it.optBool("charging") == true, it.optBool("ac_online")) },
                cpuTempC = o.optDouble("cpu_temp_c"),
                disks = o.objList("disks").mapNotNull { d ->
                    val m = d.optStr("mount") ?: return@mapNotNull null
                    HealthDisk(m, d.optStr("label") ?: "", d.optLong("free_bytes") ?: 0, d.optLong("total_bytes") ?: 0)
                },
                uptimeS = o.optLong("uptime_s") ?: 0,
                agentUptimeS = o.optLong("agent_uptime_s") ?: 0,
            )
        }

        /** "3 d 4 h", "4 h 12 min", "12 min" (under a minute: "0 min"). */
        fun formatUptime(s: Long): String {
            val secs = s.coerceAtLeast(0)
            val d = secs / 86_400
            val h = (secs % 86_400) / 3_600
            val m = (secs % 3_600) / 60
            return when {
                d > 0 -> "$d d $h h"
                h > 0 -> "$h h $m min"
                else -> "$m min"
            }
        }

        /** Binary units with one decimal under 10: "512 MB", "9.5 GB", "120 GB", "1.8 TB". */
        fun formatBytes(b: Long): String {
            val units = listOf("B", "KB", "MB", "GB", "TB", "PB")
            var v = b.coerceAtLeast(0).toDouble()
            var i = 0
            while (v >= 1024 && i < units.lastIndex) {
                v /= 1024
                i++
            }
            return if (i == 0 || v >= 10) "%d %s".format(v.toLong(), units[i]) else "%.1f %s".format(java.util.Locale.ROOT, v, units[i])
        }

        fun formatTemp(c: Double): String = "%.0f °C".format(java.util.Locale.ROOT, c)

        /** What the PC card shows: battery, CPU temperature (if any), system drive, uptime. */
        fun lines(h: SystemHealth): List<String> = buildList {
            h.battery?.let { b ->
                val state = when {
                    b.charging -> " · charging"
                    b.acOnline == true -> " · plugged in"
                    else -> ""
                }
                add("Battery " + (b.percent?.let { "$it %" } ?: "?") + state)
            }
            h.cpuTempC?.let { add("CPU " + formatTemp(it)) }
            h.systemDisk?.let { d -> add(d.mount.trimEnd('\\') + " " + formatBytes(d.freeBytes) + " free of " + formatBytes(d.totalBytes)) }
            add("Up " + formatUptime(h.uptimeS))
        }
    }
}
