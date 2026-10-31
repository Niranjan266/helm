package com.niranjan.helm.core

import java.util.Base64
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** Typed calls for the Phase 1 methods (PROTOCOL.md §9.3) over one [HelmConnection]. */
class HelmApi(val conn: HelmConnection) {
    private suspend fun call(method: String, params: JsonObject = JsonObject(emptyMap()), timeoutMs: Long = 10_000) =
        conn.request(method, params, timeoutMs)

    private fun pngOf(r: JsonObject): ByteArray? = r.optStr("png")?.let {
        try {
            Base64.getDecoder().decode(it)
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    // Apps & windows
    suspend fun apps(): List<AppEntry> = AppEntry.listFrom(call("apps.list", timeoutMs = 30_000))

    suspend fun appIcon(appId: String, size: Int): ByteArray? =
        pngOf(call("apps.icon", buildJsonObject { put("app_id", appId); put("size", size) }))

    suspend fun launch(appId: String) {
        call("apps.launch", buildJsonObject { put("app_id", appId) })
    }

    suspend fun open(target: String) {
        call("open", buildJsonObject { put("target", target) })
    }

    suspend fun windows(): List<WindowEntry> = WindowEntry.listFrom(call("windows.list"))

    suspend fun focus(hwnd: JsonPrimitive): JsonObject = call("windows.focus", buildJsonObject { put("hwnd", hwnd) })

    /** [op] is `minimize`, `maximize`, `restore` or `close`. */
    suspend fun windowOp(op: String, hwnd: JsonPrimitive) {
        call("windows.$op", buildJsonObject { put("hwnd", hwnd) })
    }

    // Dock & deck
    suspend fun dock(): List<List<DockTile>> = DockCodec.decode(call("dock.get"))

    suspend fun setDock(pages: List<List<DockTile>>) {
        call("dock.set", DockCodec.encode(pages))
    }

    suspend fun deck(): List<DeckButton> = DeckCodec.decode(call("deck.get"))

    suspend fun setDeck(buttons: List<DeckButton>) {
        call("deck.set", DeckCodec.encode(buttons))
    }

    suspend fun runDeck(id: String) {
        call("deck.run", buildJsonObject { put("id", id) }, timeoutMs = 30_000)
    }

    // Media & controls
    suspend fun mediaState(): MediaState? = MediaState.fromHolder(call("media.state"))

    suspend fun mediaArtwork(): ByteArray? = pngOf(call("media.artwork"))

    suspend fun mediaControl(action: String) {
        call("media.control", buildJsonObject { put("action", action) })
    }

    suspend fun mediaSeek(ms: Long) {
        call("media.control", buildJsonObject { put("action", buildJsonObject { put("seek_ms", ms) }) })
    }

    suspend fun audio(): AudioState? = AudioState.from(call("audio.get"))

    suspend fun setAudio(level: Float? = null, muted: Boolean? = null) {
        call("audio.set", buildJsonObject {
            level?.let { put("level", it.coerceIn(0f, 1f)) }
            muted?.let { put("muted", it) }
        })
    }

    suspend fun brightness(): List<DisplayBrightness> = DisplayBrightness.listFrom(call("display.brightness.list"))

    suspend fun setBrightness(id: String, level: Int) {
        call("display.brightness.set", buildJsonObject { put("id", id); put("level", level.coerceIn(0, 100)) })
    }

    suspend fun powerAction(action: String, delayS: Int?): PowerSchedule? =
        PowerSchedule.fromHolder(call("power.action", buildJsonObject {
            put("action", action)
            if (delayS != null && delayS > 0) put("delay_s", delayS)
        }))

    suspend fun powerCancel(): PowerSchedule? = PowerSchedule.fromHolder(call("power.cancel"))

    suspend fun powerStatus(): PowerSchedule? = PowerSchedule.fromHolder(call("power.status"))

    // Clipboard & screen
    data class Clip(val text: String?, val png: ByteArray?)

    suspend fun clipboardGet(): Clip {
        val r = call("clipboard.get")
        return Clip(r.optStr("text"), pngOf(r))
    }

    suspend fun clipboardSetText(text: String) {
        call("clipboard.set", buildJsonObject { put("text", text) })
    }

    suspend fun clipboardWatch(on: Boolean) {
        call("clipboard.watch", buildJsonObject { put("on", on) })
    }

    suspend fun monitors(): List<MonitorInfo> = MonitorInfo.listFrom(call("screen.monitors"))

    suspend fun capture(monitorId: JsonPrimitive?): ByteArray? =
        pngOf(call("screen.capture", buildJsonObject { monitorId?.let { put("monitor_id", it) } }, timeoutMs = 30_000))

    suspend fun systemInfo(): JsonObject = call("system.info")
}
