package com.niranjan.helm.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonElement
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.addJsonObject
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonArray
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.doubleOrNull
import kotlinx.serialization.json.longOrNull
import kotlinx.serialization.json.put

// Small, lenient readers for agent results (unknown fields ignored, wrong types -> null).
internal fun JsonObject.optStr(k: String): String? = (this[k] as? JsonPrimitive)?.takeIf { it.isString }?.contentOrNull
internal fun JsonObject.optLong(k: String): Long? = (this[k] as? JsonPrimitive)?.takeIf { !it.isString }?.longOrNull
internal fun JsonObject.optDouble(k: String): Double? = (this[k] as? JsonPrimitive)?.takeIf { !it.isString }?.doubleOrNull
internal fun JsonObject.optBool(k: String): Boolean? = (this[k] as? JsonPrimitive)?.takeIf { !it.isString }?.booleanOrNull
internal fun JsonObject.objList(k: String): List<JsonObject> = (this[k] as? JsonArray).orEmpty().mapNotNull { it as? JsonObject }

/** `apps.list` entry. */
data class AppEntry(val appId: String, val name: String, val kind: String) {
    companion object {
        fun listFrom(result: JsonObject): List<AppEntry> = result.objList("apps").mapNotNull { o ->
            val id = o.optStr("app_id") ?: return@mapNotNull null
            AppEntry(id, o.optStr("name") ?: id, o.optStr("kind") ?: "desktop")
        }
    }
}

/** `windows.list` entry. [hwnd] is kept as sent and passed back unchanged. */
data class WindowEntry(
    val hwnd: JsonPrimitive,
    val title: String,
    val pid: Long?,
    val appId: String?,
    val isMinimized: Boolean,
    val isForeground: Boolean,
) {
    companion object {
        fun listFrom(result: JsonObject): List<WindowEntry> = result.objList("windows").mapNotNull { o ->
            val hwnd = o["hwnd"] as? JsonPrimitive ?: return@mapNotNull null
            WindowEntry(
                hwnd = hwnd,
                title = o.optStr("title") ?: "",
                pid = o.optLong("pid"),
                appId = o.optStr("app_id"),
                isMinimized = o.optBool("is_minimized") == true,
                isForeground = o.optBool("is_foreground") == true,
            )
        }
    }
}

/** A dock tile (§9.3): launches an app or opens a target; G5 adds folders and icons. */
data class DockTile(
    val id: String,
    val kind: String,
    val appId: String? = null,
    val target: String? = null,
    val label: String? = null,
    /** G5 folder contents (app/open tiles). */
    val children: List<DockTile> = emptyList(),
    /** G5 icon reference (see [IconRef]). */
    val icon: String? = null,
) {
    companion object {
        const val KIND_APP = "app"
        const val KIND_OPEN = "open"
        const val KIND_FOLDER = "folder"
    }
}

/** `dock.get` / `dock.set` JSON. */
object DockCodec {
    fun decode(result: JsonObject): List<List<DockTile>> =
        (result["pages"] as? JsonArray).orEmpty().map { page ->
            (page as? JsonArray).orEmpty().mapNotNull { decodeTile(it, nested = false) }
        }

    private fun decodeTile(el: kotlinx.serialization.json.JsonElement, nested: Boolean): DockTile? {
        val o = el as? JsonObject ?: return null
        val id = o.optStr("id") ?: return null
        val kind = o.optStr("kind") ?: return null
        val children = if (nested) emptyList() else o.objList("children").mapNotNull { decodeTile(it, nested = true) }
        return DockTile(id, kind, o.optStr("app_id"), o.optStr("target"), o.optStr("label"), children, o.optStr("icon"))
    }

    private fun kotlinx.serialization.json.JsonObjectBuilder.tileFields(t: DockTile) {
        put("id", t.id)
        put("kind", t.kind)
        t.appId?.let { put("app_id", it) }
        t.target?.let { put("target", it) }
        t.label?.let { put("label", it) }
        t.icon?.let { put("icon", it) }
        if (t.kind == DockTile.KIND_FOLDER) {
            put("children", buildJsonArray { for (c in t.children) addJsonObject { tileFields(c) } })
        }
    }

    /** Params for `dock.set`. Null fields are omitted. */
    fun encode(pages: List<List<DockTile>>): JsonObject = buildJsonObject {
        put("pages", buildJsonArray {
            for (page in pages) {
                add(buildJsonArray {
                    for (t in page) addJsonObject { tileFields(t) }
                })
            }
        })
    }

    /** Adds [tile] to the last page, or a new page if it already holds [perPage] tiles. */
    fun append(pages: List<List<DockTile>>, tile: DockTile, perPage: Int): List<List<DockTile>> {
        val last = pages.lastOrNull()
        return if (last == null || last.size >= perPage) pages + listOf(listOf(tile))
        else pages.dropLast(1) + listOf(last + tile)
    }

    fun remove(pages: List<List<DockTile>>, id: String): List<List<DockTile>> =
        pages.map { p -> p.filterNot { it.id == id } }.filter { it.isNotEmpty() }

    /** Moves a tile by [delta] places in the flattened order, keeping page sizes. */
    fun move(pages: List<List<DockTile>>, id: String, delta: Int): List<List<DockTile>> {
        val flat = pages.flatten().toMutableList()
        val i = flat.indexOfFirst { it.id == id }
        if (i < 0) return pages
        val j = (i + delta).coerceIn(0, flat.lastIndex)
        if (i == j) return pages
        flat.add(j, flat.removeAt(i))
        val out = mutableListOf<List<DockTile>>()
        var k = 0
        for (p in pages) {
            out += flat.subList(k, k + p.size).toList()
            k += p.size
        }
        return out
    }

    /** Moves a tile to the end of another page (creating it if [page] == pages.size). */
    fun moveToPage(pages: List<List<DockTile>>, id: String, page: Int): List<List<DockTile>> {
        val tile = pages.flatten().firstOrNull { it.id == id } ?: return pages
        val without = pages.map { p -> p.filterNot { it.id == id } }
        val target = page.coerceIn(0, without.size)
        val out = without.toMutableList()
        if (target == out.size) out += listOf(tile) else out[target] = out[target] + tile
        return out.filter { it.isNotEmpty() }
    }
}

/** Deck button action (§9.3). Unknown kinds are kept as-is so a save does not drop them. */
sealed interface DeckAction {
    data class Hotkey(val keys: String) : DeckAction
    data class Text(val text: String) : DeckAction
    data class Launch(val appId: String) : DeckAction
    data class Open(val target: String) : DeckAction
    data class Media(val action: String) : DeckAction
    data class VolumeStep(val step: Double) : DeckAction
    data class Delay(val ms: Long) : DeckAction

    /** Phase 3 (§11): a sequence of actions (max [MAX_MACRO_STEPS]), `delay_ms` allowed inside. */
    data class Macro(val steps: List<DeckAction>) : DeckAction

    /** Phase 3: run [action] [times] times. */
    data class Repeat(val times: Int, val action: DeckAction) : DeckAction

    /** Phase 3 app remote, e.g. `powerpoint.next` or `obs.scene:<name>`. */
    data class Remote(val command: String) : DeckAction

    /** Phase 5 (§13): call a Home Assistant service, e.g. `light.toggle` on `light.desk`. */
    data class Ha(val service: String, val entityId: String) : DeckAction
    data class Unknown(val raw: JsonElement) : DeckAction

    companion object {
        val KINDS = listOf("hotkey", "text", "launch", "open", "media", "volume_step", "delay_ms")

        /** Kinds only a `p3` agent understands. */
        val P3_KINDS = listOf("macro", "repeat", "remote")

        /** Kinds only a `p5` agent understands. */
        val P5_KINDS = listOf("ha")
        val HA_SERVICES = listOf(
            "homeassistant.toggle", "light.toggle", "light.turn_on", "light.turn_off",
            "switch.toggle", "switch.turn_on", "switch.turn_off", "scene.turn_on", "script.turn_on",
        )
        val MEDIA_ACTIONS = listOf("toggle_play_pause", "play", "pause", "next", "previous", "stop")
        val REMOTE_COMMANDS = listOf(
            "powerpoint.next", "powerpoint.prev", "powerpoint.start", "powerpoint.end",
            "browser.back", "browser.forward", "browser.tab_next", "browser.tab_prev", "browser.reload",
            "obs.toggle_record", "obs.toggle_stream",
        )
        const val OBS_SCENE_PREFIX = "obs.scene:"
        const val MAX_MACRO_STEPS = 50
        val STATE_BINDS = listOf("mic_muted", "audio_muted", "media_playing")
        val WIDGETS = listOf("cpu", "ram", "clock", "now_playing")
    }
}

data class DeckButton(
    val id: String,
    val label: String,
    /** `#RRGGBB`. */
    val color: String,
    val icon: String? = null,
    val action: DeckAction,
    /** Phase 3 toggle binding (`state:{bind}`): `mic_muted`, `audio_muted` or `media_playing`. */
    val stateBind: String? = null,
    /** Phase 3 live widget: `cpu`, `ram`, `clock` or `now_playing`. */
    val widget: String? = null,
    /** G5 grid size: `1x1`, `2x1` or `2x2` ([ButtonSize]). */
    val size: String = "1x1",
)

/** `deck.get` / `deck.set` JSON. */
object DeckCodec {
    fun decodeAction(el: JsonElement?): DeckAction {
        val o = el as? JsonObject ?: return DeckAction.Unknown(el ?: JsonNull)
        if (o.size == 1) {
            o.optStr("hotkey")?.let { return DeckAction.Hotkey(it) }
            o.optStr("text")?.let { return DeckAction.Text(it) }
            o.optStr("launch")?.let { return DeckAction.Launch(it) }
            o.optStr("open")?.let { return DeckAction.Open(it) }
            o.optStr("media")?.let { return DeckAction.Media(it) }
            o.optDouble("volume_step")?.let { return DeckAction.VolumeStep(it) }
            o.optLong("delay_ms")?.let { return DeckAction.Delay(it) }
            o.optStr("remote")?.let { return DeckAction.Remote(it) }
            (o["ha"] as? JsonObject)?.let { h ->
                val svc = h.optStr("service")
                val ent = h.optStr("entity_id")
                if (svc != null && ent != null && h.size == 2) return DeckAction.Ha(svc, ent)
            }
            // Macro / repeat are decoded only when every inner action is understood; otherwise
            // the whole action is kept raw so a save never rewrites what we cannot represent.
            (o["macro"] as? JsonArray)?.let { arr ->
                val steps = arr.map { decodeAction(it) }
                if (arr.size <= DeckAction.MAX_MACRO_STEPS && steps.none { it is DeckAction.Unknown }) return DeckAction.Macro(steps)
            }
            (o["repeat"] as? JsonObject)?.let { r ->
                val times = r.optLong("times")
                val inner = decodeAction(r["action"])
                if (times != null && times in 1..1000 && inner !is DeckAction.Unknown) return DeckAction.Repeat(times.toInt(), inner)
            }
        }
        return DeckAction.Unknown(o)
    }

    fun encodeAction(a: DeckAction): JsonElement = when (a) {
        is DeckAction.Hotkey -> buildJsonObject { put("hotkey", a.keys) }
        is DeckAction.Text -> buildJsonObject { put("text", a.text) }
        is DeckAction.Launch -> buildJsonObject { put("launch", a.appId) }
        is DeckAction.Open -> buildJsonObject { put("open", a.target) }
        is DeckAction.Media -> buildJsonObject { put("media", a.action) }
        is DeckAction.VolumeStep -> buildJsonObject { put("volume_step", a.step) }
        is DeckAction.Delay -> buildJsonObject { put("delay_ms", a.ms) }
        is DeckAction.Remote -> buildJsonObject { put("remote", a.command) }
        is DeckAction.Ha -> buildJsonObject {
            put("ha", buildJsonObject {
                put("service", a.service)
                put("entity_id", a.entityId)
            })
        }
        is DeckAction.Macro -> buildJsonObject {
            put("macro", buildJsonArray { a.steps.take(DeckAction.MAX_MACRO_STEPS).forEach { add(encodeAction(it)) } })
        }
        is DeckAction.Repeat -> buildJsonObject {
            put("repeat", buildJsonObject {
                put("times", a.times)
                put("action", encodeAction(a.action))
            })
        }
        is DeckAction.Unknown -> a.raw
    }

    /** True when [a] (or a step inside it) needs a `p3` agent. */
    fun needsPhase3(a: DeckAction): Boolean = when (a) {
        is DeckAction.Macro, is DeckAction.Repeat, is DeckAction.Remote -> true
        else -> false
    }

    /** True when [a] (or a step inside it) needs a `p5` agent. */
    fun needsPhase5(a: DeckAction): Boolean = when (a) {
        is DeckAction.Ha -> true
        is DeckAction.Macro -> a.steps.any { needsPhase5(it) }
        is DeckAction.Repeat -> needsPhase5(a.action)
        else -> false
    }

    /** Lenient: `widget:"cpu"` or `widget:{"widget":"cpu"}`. */
    private fun widgetOf(o: JsonObject): String? =
        o.optStr("widget") ?: (o["widget"] as? JsonObject)?.optStr("widget")

    fun decode(result: JsonObject): List<DeckButton> = result.objList("buttons").mapNotNull { o ->
        val id = o.optStr("id") ?: return@mapNotNull null
        DeckButton(
            id = id,
            label = o.optStr("label") ?: "",
            color = o.optStr("color") ?: DEFAULT_COLOR,
            icon = o.optStr("icon"),
            action = decodeAction(o["action"]),
            stateBind = (o["state"] as? JsonObject)?.optStr("bind"),
            widget = widgetOf(o),
            size = ButtonSize.of(o.optStr("size")).wire,
        )
    }

    /** Params for `deck.set`. */
    fun encode(buttons: List<DeckButton>): JsonObject = buildJsonObject {
        put("buttons", buildJsonArray {
            for (b in buttons) addJsonObject {
                put("id", b.id)
                put("label", b.label)
                put("color", b.color)
                b.icon?.let { put("icon", it) }
                put("action", encodeAction(b.action))
                b.stateBind?.let { put("state", buildJsonObject { put("bind", it) }) }
                b.widget?.let { put("widget", it) }
                if (b.size != ButtonSize.ONE.wire) put("size", ButtonSize.of(b.size).wire)
            }
        })
    }

    const val DEFAULT_COLOR = "#3B82F6"
}

/** `media.state` (shape follows the agent's `MediaState`; missing fields default). */
data class MediaState(
    val appId: String,
    val title: String,
    val artist: String,
    val album: String,
    val status: String,
    val positionMs: Long?,
    val durationMs: Long?,
    val hasArtwork: Boolean,
    val canPlay: Boolean,
    val canPause: Boolean,
    val canNext: Boolean,
    val canPrev: Boolean,
    val canSeek: Boolean,
) {
    val isPlaying: Boolean get() = status == "playing"

    companion object {
        /** From a result/event holding `state` (object or null). */
        fun fromHolder(holder: JsonObject): MediaState? = from(holder["state"] as? JsonObject)

        fun from(o: JsonObject?): MediaState? {
            o ?: return null
            return MediaState(
                appId = o.optStr("app_id") ?: "",
                title = o.optStr("title") ?: "",
                artist = o.optStr("artist") ?: "",
                album = o.optStr("album") ?: "",
                status = o.optStr("status") ?: "unknown",
                positionMs = o.optLong("position_ms"),
                durationMs = o.optLong("duration_ms"),
                hasArtwork = o.optBool("has_artwork") == true,
                canPlay = o.optBool("can_play") ?: true,
                canPause = o.optBool("can_pause") ?: true,
                canNext = o.optBool("can_next") ?: true,
                canPrev = o.optBool("can_prev") ?: true,
                canSeek = o.optBool("can_seek") == true,
            )
        }
    }
}

/** `audio.get` / `audio.changed`: level 0..1. */
data class AudioState(val level: Float, val muted: Boolean) {
    companion object {
        fun from(o: JsonObject): AudioState? {
            val level = o.optDouble("level") ?: return null
            return AudioState(level.toFloat().coerceIn(0f, 1f), o.optBool("muted") == true)
        }
    }
}

/** `display.brightness.list` entry: level 0..100. */
data class DisplayBrightness(val id: String, val name: String, val kind: String, val level: Int, val supported: Boolean) {
    companion object {
        fun listFrom(result: JsonObject): List<DisplayBrightness> = result.objList("displays").mapNotNull { o ->
            val id = o.optStr("id") ?: return@mapNotNull null
            DisplayBrightness(id, o.optStr("name") ?: id, o.optStr("kind") ?: "", (o.optLong("level") ?: 0).toInt(), o.optBool("supported") == true)
        }
    }
}

/** A scheduled power action (`power.*`, `power.scheduled`). */
data class PowerSchedule(val action: String, val dueUnixMs: Long) {
    companion object {
        fun fromHolder(holder: JsonObject): PowerSchedule? {
            val o = holder["scheduled"] as? JsonObject ?: return null
            return PowerSchedule(o.optStr("action") ?: return null, o.optLong("due_unix_ms") ?: return null)
        }

        val ACTIONS = listOf("lock", "sleep", "hibernate", "restart", "shutdown", "sign_out")
    }
}

/** `screen.monitors` entry. [id] is passed back to `screen.capture` unchanged. */
data class MonitorInfo(
    val id: JsonPrimitive,
    val name: String,
    val width: Int,
    val height: Int,
    val isPrimary: Boolean,
) {
    companion object {
        fun listFrom(result: JsonObject): List<MonitorInfo> = result.objList("monitors").mapNotNull { o ->
            val id = o["id"] as? JsonPrimitive ?: return@mapNotNull null
            MonitorInfo(id, o.optStr("name") ?: id.content, (o.optLong("width") ?: 0).toInt(), (o.optLong("height") ?: 0).toInt(), o.optBool("is_primary") == true)
        }
    }
}

/** `system.info.macs` entry. */
data class MacEntry(val mac: String, val adapterName: String, val locallyAdministered: Boolean) {
    companion object {
        fun listFrom(info: JsonObject): List<MacEntry> = info.objList("macs").mapNotNull { o ->
            val mac = o.optStr("mac") ?: return@mapNotNull null
            MacEntry(mac, o.optStr("adapter_name") ?: "", o.optBool("locally_administered") == true)
        }
    }
}
