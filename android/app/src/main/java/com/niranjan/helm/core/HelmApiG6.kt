package com.niranjan.helm.core

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// Typed calls for the everyday features (Docs/protocol/everyday.md, feature `g6`). Each is gated
// on the hello feature: against an older agent it throws `unsupported` without sending anything.

private suspend fun HelmApi.g6(method: String, params: JsonObject = JsonObject(emptyMap()), timeoutMs: Long = 10_000) =
    gated(Everyday.FEATURE, method, params, timeoutMs)

// 1. Clipboard history
suspend fun HelmApi.clipboardHistory(): ClipHistory = ClipHistory.from(g6("clipboard.history"))

suspend fun HelmApi.setClipboardHistory(enabled: Boolean? = null, persist: Boolean? = null): ClipHistory =
    ClipHistory.from(g6("clipboard.history.set", ClipHistory.setParams(enabled, persist)))

suspend fun HelmApi.clearClipboardHistory() {
    g6("clipboard.history.clear")
}

suspend fun HelmApi.deleteClipboardItem(id: Long) {
    g6("clipboard.history.delete", buildJsonObject { put("id", id) })
}

/** Fetches the full item (text or PNG) to put on the phone. */
suspend fun HelmApi.pickClipboardToPhone(id: Long): ClipPick =
    ClipPick.from(g6("clipboard.pick", buildJsonObject { put("id", id); put("to", ClipPick.TO_PHONE) }, timeoutMs = 30_000))
        ?: throw HelmException("bad_result", "clipboard.pick returned nothing")

/** Puts the item back on the PC clipboard (it moves to the top of the history). */
suspend fun HelmApi.pickClipboardToPc(id: Long) {
    g6("clipboard.pick", buildJsonObject { put("id", id); put("to", ClipPick.TO_PC) })
}

// 2. Quick file drop
suspend fun HelmApi.fileDrop(path: String, mode: DropMode): DropResult =
    DropResult.from(g6("file.drop", DropResult.params(path, mode), timeoutMs = 20_000), mode)

// 3. Find my phone
suspend fun HelmApi.phones(): List<PhoneEntry> = PhoneEntry.listFrom(g6("phone.list"))

/** Rings ([stop] = false) or silences a phone; null [deviceId] = every other paired phone. Returns sessions told. */
suspend fun HelmApi.ringPhone(deviceId: String?, stop: Boolean = false): Int =
    (g6("phone.ring", PhoneRingEvent.ringParams(deviceId, stop)).optLong("rang") ?: 0).toInt()

// 4. PC notifications
suspend fun HelmApi.pcNotifyWatch(on: Boolean): NotifyAccess =
    NotifyAccess.fromWire(g6("pc.notify.watch", buildJsonObject { put("on", on) }).optStr("access"))

// 5. Schedules
suspend fun HelmApi.schedules(): List<Schedule> = Schedule.listFrom(g6("schedule.list"))

/** Replaces every schedule; returns the agent's normalised list (with `next_ms`). */
suspend fun HelmApi.setSchedules(list: List<Schedule>): List<Schedule> = Schedule.listFrom(g6("schedule.set", Schedule.encode(list)))

// 6. Health
suspend fun HelmApi.systemHealth(): SystemHealth = SystemHealth.from(g6("system.health", timeoutMs = 15_000))
