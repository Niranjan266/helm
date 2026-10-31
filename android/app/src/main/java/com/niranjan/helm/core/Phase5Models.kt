package com.niranjan.helm.core

import java.util.Base64
import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.put

// Phase 5 (§13, feature `p5`) result models and codecs. Plain JVM, unit-tested.

private fun JsonObject.strings(k: String): List<String> =
    (this[k] as? JsonArray).orEmpty().mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }

private fun stringArray(items: Collection<String>) = buildJsonArray { items.forEach { add(JsonPrimitive(it)) } }

/** `record.start` → `{path}`, `record.stop` → `{path,duration_ms}`. */
data class Recording(val path: String, val durationMs: Long?) {
    val fileName: String get() = path.substringAfterLast('\\').substringAfterLast('/')

    companion object {
        fun from(o: JsonObject): Recording? = o.optStr("path")?.let { Recording(it, o.optLong("duration_ms")) }
    }
}

/** `term.output` / `term.exit` events. */
sealed interface TermEvent {
    val termId: String

    data class Output(override val termId: String, val data: ByteArray) : TermEvent
    data class Exit(override val termId: String, val code: Long?) : TermEvent

    companion object {
        /** The `term_id` may be a string or a number on the wire; we keep its text. */
        internal fun idOf(o: JsonObject): String? = (o["term_id"] as? JsonPrimitive)?.contentOrNull

        fun from(name: String, data: JsonObject): TermEvent? = when (name) {
            "term.output" -> {
                val id = idOf(data)
                val bytes = data.optStr("data_b64")?.let {
                    try {
                        Base64.getDecoder().decode(it)
                    } catch (_: IllegalArgumentException) {
                        null
                    }
                }
                if (id != null && bytes != null) Output(id, bytes) else null
            }
            "term.exit" -> idOf(data)?.let { Exit(it, data.optLong("code")) }
            else -> null
        }
    }
}

/** Bytes the terminal input line and special keys send (`term.write`). */
object TermKeys {
    const val ENTER = "\r"
    const val TAB = "\t"
    const val ESC = "\u001b"
    const val BACKSPACE = "\u007f"
    const val UP = "\u001b[A"
    const val DOWN = "\u001b[B"
    const val RIGHT = "\u001b[C"
    const val LEFT = "\u001b[D"
    const val HOME = "\u001b[H"
    const val END = "\u001b[F"
    const val PAGE_UP = "\u001b[5~"
    const val PAGE_DOWN = "\u001b[6~"
    const val DELETE = "\u001b[3~"

    /** Ctrl+letter → control byte (Ctrl+C = 0x03). Null for anything else. */
    fun ctrl(c: Char): String? {
        val u = c.uppercaseChar()
        return when (u) {
            in 'A'..'Z' -> (u.code - 'A'.code + 1).toChar().toString()
            '[' -> ESC
            '\\' -> "\u001c"
            ']' -> "\u001d"
            else -> null
        }
    }

    fun b64(s: String): String = Base64.getEncoder().encodeToString(s.toByteArray(Charsets.UTF_8))
}

/** Terminal grid size that fits a view; at least 20×5, at most 500×200. */
object TermSize {
    fun fit(widthPx: Float, heightPx: Float, charWidthPx: Float, lineHeightPx: Float): Pair<Int, Int> {
        if (charWidthPx <= 0f || lineHeightPx <= 0f) return 80 to 24
        val cols = (widthPx / charWidthPx).toInt().coerceIn(20, 500)
        val rows = (heightPx / lineHeightPx).toInt().coerceIn(5, 200)
        return cols to rows
    }
}

/** One action button on a mirrored notification. */
data class NotifyActionButton(val id: String, val label: String)

/** `notify.push` params: a phone notification mirrored as a Windows toast. */
data class NotifyPush(
    val app: String,
    val title: String,
    val body: String,
    val actions: List<NotifyActionButton> = emptyList(),
    /** True when the toast should offer a reply box (the notification has a RemoteInput). */
    val reply: Boolean = false,
    /** Stable key the phone uses to find the notification again on `notify.action`. */
    val key: String,
    val iconPng: ByteArray? = null,
) {
    fun toJson(): JsonObject = buildJsonObject {
        put("app", app.take(MAX_APP))
        put("title", title.take(MAX_TITLE))
        put("body", body.take(MAX_BODY))
        iconPng?.takeIf { it.size <= MAX_ICON_BYTES }?.let { put("icon_png_b64", Base64.getEncoder().encodeToString(it)) }
        put("actions", buildJsonArray {
            for (a in actions.take(MAX_ACTIONS)) addJsonObject {
                put("id", a.id)
                put("label", a.label.take(MAX_LABEL))
            }
        })
        put("reply", reply)
        put("key", key)
    }

    companion object {
        const val MAX_APP = 64
        const val MAX_TITLE = 200
        const val MAX_BODY = 1000
        const val MAX_LABEL = 40
        const val MAX_ACTIONS = 5
        const val MAX_ICON_BYTES = 64 * 1024
    }
}

/** `notify.action` event: a toast click on the PC. [actionId] null = toast body clicked. */
data class NotifyActionEvent(val key: String, val actionId: String?, val replyText: String?) {
    companion object {
        fun from(o: JsonObject): NotifyActionEvent? {
            val key = o.optStr("key") ?: return null
            return NotifyActionEvent(key, o.optStr("action_id"), o.optStr("reply_text"))
        }
    }
}

/** Per-app "Mirror notifications" allow-list (package names). Empty = off for every app. */
object NotifyAllowList {
    fun encode(pkgs: Set<String>): String = pkgs.filter { it.isNotBlank() && ',' !in it }.sorted().joinToString(",")

    fun decode(s: String?): Set<String> = s.orEmpty().split(',').map { it.trim() }.filter { it.isNotEmpty() }.toSet()

    /** Whether to mirror a notification from [pkg]: only allowed apps, never Helm itself. */
    fun shouldMirror(enabled: Boolean, allowed: Set<String>, pkg: String, ownPkg: String): Boolean =
        enabled && pkg != ownPkg && pkg in allowed
}

/** `plugins.list` entry. */
data class PluginInfo(
    val id: String,
    val name: String,
    val version: String,
    val capabilities: List<String>,
    val granted: List<String>,
    val enabled: Boolean,
    val error: String?,
) {
    /** Capabilities asked for but not granted yet. */
    val missing: List<String> get() = capabilities.filterNot { it in granted }

    companion object {
        fun listFrom(o: JsonObject): List<PluginInfo> = o.objList("plugins").mapNotNull { p ->
            val id = p.optStr("id") ?: return@mapNotNull null
            PluginInfo(
                id = id,
                name = p.optStr("name") ?: id,
                version = p.optStr("version") ?: "",
                capabilities = p.strings("capabilities"),
                granted = p.strings("granted"),
                enabled = p.optBool("enabled") == true,
                error = p.optStr("error")?.takeIf { it.isNotBlank() },
            )
        }

        /** `plugins.set` params. */
        fun setParams(id: String, enabled: Boolean, granted: Collection<String>): JsonObject = buildJsonObject {
            put("id", id)
            put("enabled", enabled)
            put("granted", stringArray(granted.distinct()))
        }

        /** Human text for a capability string. */
        fun describe(cap: String): String = when {
            cap == "deck.action" -> "Add deck actions"
            cap == "notify" -> "Show notifications on the PC"
            cap.startsWith("http:") -> "Talk to ${cap.removePrefix("http:")} over the network"
            else -> cap
        }
    }
}

/** When a trigger fires (§13). Unknown shapes are kept raw so a save does not drop them. */
sealed interface TriggerWhen {
    /** `HH:MM [days]`, e.g. `07:30 weekdays` or `22:00 sat,sun`. */
    data class Time(val spec: String) : TriggerWhen
    data class AppStart(val exe: String) : TriggerWhen

    /** [deviceId] null = any phone. */
    data class PhoneConnected(val deviceId: String?) : TriggerWhen
    data class Wifi(val ssid: String) : TriggerWhen
    data class Unknown(val raw: JsonElement) : TriggerWhen

    companion object {
        private val TIME = Regex("""^([01]?\d|2[0-3]):[0-5]\d(\s+.+)?$""")

        /** True for `HH:MM` optionally followed by days (the PC validates the days part). */
        fun validTime(spec: String): Boolean = TIME.matches(spec.trim())

        fun decode(el: JsonElement?): TriggerWhen {
            val o = el as? JsonObject ?: return Unknown(el ?: JsonNull)
            if (o.size == 1) {
                o.optStr("time")?.let { return Time(it) }
                o.optStr("app_start")?.let { return AppStart(it) }
                o.optStr("wifi")?.let { return Wifi(it) }
                if (o.containsKey("phone_connected")) {
                    val v = o["phone_connected"]
                    if (v is JsonNull) return PhoneConnected(null)
                    (v as? JsonPrimitive)?.takeIf { it.isString }?.let { return PhoneConnected(it.content.ifBlank { null }) }
                }
            }
            return Unknown(o)
        }

        fun encode(w: TriggerWhen): JsonElement = when (w) {
            is Time -> buildJsonObject { put("time", w.spec.trim()) }
            is AppStart -> buildJsonObject { put("app_start", w.exe.trim()) }
            is PhoneConnected -> buildJsonObject { put("phone_connected", w.deviceId?.let { JsonPrimitive(it) } ?: JsonNull) }
            is Wifi -> buildJsonObject { put("wifi", w.ssid) }
            is Unknown -> w.raw
        }

        fun describe(w: TriggerWhen): String = when (w) {
            is Time -> "At ${w.spec}"
            is AppStart -> "When ${w.exe} starts"
            is PhoneConnected -> if (w.deviceId == null) "When any phone connects" else "When this phone connects"
            is Wifi -> "On Wi-Fi \"${w.ssid}\""
            is Unknown -> "Other: ${w.raw}"
        }
    }
}

/** `triggers.get` / `triggers.set` entry; [action] is a deck action. */
data class Trigger(val id: String, val `when`: TriggerWhen, val action: DeckAction) {
    companion object {
        fun listFrom(o: JsonObject): List<Trigger> = o.objList("triggers").mapNotNull { t ->
            val id = t.optStr("id") ?: return@mapNotNull null
            Trigger(id, TriggerWhen.decode(t["when"]), DeckCodec.decodeAction(t["action"]))
        }

        fun encode(list: List<Trigger>): JsonObject = buildJsonObject {
            put("triggers", buildJsonArray {
                for (t in list) addJsonObject {
                    put("id", t.id)
                    put("when", TriggerWhen.encode(t.`when`))
                    put("action", DeckCodec.encodeAction(t.action))
                }
            })
        }
    }
}

/** One `ha.states` entry. */
data class HaState(val entityId: String, val state: String, val friendlyName: String?, val lastChanged: String?) {
    val domain: String get() = entityId.substringBefore('.')
    val label: String get() = friendlyName ?: entityId

    /** A sensible service for a one-tap deck button on this entity. */
    val toggleService: String
        get() = when (domain) {
            "light", "switch", "fan", "input_boolean", "media_player", "cover" -> "$domain.toggle"
            "scene", "script" -> "$domain.turn_on"
            else -> "homeassistant.toggle"
        }

    companion object {
        private val ENTITY = Regex("""^[a-z0-9_]+\.[a-z0-9_]+$""")

        fun validEntityId(s: String): Boolean = ENTITY.matches(s)

        /** `domain.service` with lower-case snake names. */
        fun validService(s: String): Boolean = ENTITY.matches(s)

        fun listFrom(o: JsonObject): List<HaState> = o.objList("states").mapNotNull { s ->
            val id = s.optStr("entity_id") ?: return@mapNotNull null
            HaState(id, s.optStr("state") ?: "", (s["attributes"] as? JsonObject)?.optStr("friendly_name"), s.optStr("last_changed"))
        }.sortedWith(compareBy({ it.domain }, { it.label.lowercase() }))
    }
}
