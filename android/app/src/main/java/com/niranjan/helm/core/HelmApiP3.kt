package com.niranjan.helm.core

import kotlinx.serialization.json.JsonArray
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// Typed calls for Phase 3 (§11, feature `p3`) and Phase 4 (§12, feature `p4`). Each is gated on
// the hello feature: against an older agent it throws `unsupported` without sending anything.

private suspend fun HelmApi.p3(method: String, params: JsonObject = JsonObject(emptyMap()), timeoutMs: Long = 10_000) =
    gated(Features.P3, method, params, timeoutMs)

private suspend fun HelmApi.p4(method: String, params: JsonObject = JsonObject(emptyMap()), timeoutMs: Long = 10_000) =
    gated(Features.P4, method, params, timeoutMs)

// Monitor & alerts
suspend fun HelmApi.statsSubscribe(intervalMs: Long) {
    p3("stats.subscribe", buildJsonObject { put("interval_ms", intervalMs.coerceAtLeast(500)) })
}

suspend fun HelmApi.statsUnsubscribe() {
    p3("stats.unsubscribe")
}

suspend fun HelmApi.alerts(): List<AlertRule> = AlertRule.listFrom(p3("alerts.get"))

suspend fun HelmApi.setAlerts(rules: List<AlertRule>) {
    p3("alerts.set", AlertRule.encode(rules))
}

// Task manager
suspend fun HelmApi.processes(): List<ProcessEntry> = ProcessEntry.listFrom(p3("process.list", timeoutMs = 20_000))

suspend fun HelmApi.killProcess(pid: Long) {
    p3("process.kill", buildJsonObject { put("pid", pid) })
}

suspend fun HelmApi.setPriority(pid: Long, level: String) {
    require(level in ProcessEntry.PRIORITIES) { "bad priority $level" }
    p3("process.priority", buildJsonObject { put("pid", pid); put("level", level) })
}

// Mixer
suspend fun HelmApi.audioSessions(): List<AudioSession> = AudioSession.listFrom(p3("audio.sessions"))

suspend fun HelmApi.setAudioSession(id: JsonPrimitive, level: Float? = null, muted: Boolean? = null) {
    p3("audio.session.set", buildJsonObject {
        put("id", id)
        level?.let { put("level", it.coerceIn(0f, 1f)) }
        muted?.let { put("muted", it) }
    })
}

// Windows & virtual desktops
suspend fun HelmApi.moveWindow(hwnd: JsonPrimitive, monitorIndex: Int) {
    p3("windows.move", buildJsonObject { put("hwnd", hwnd); put("monitor_index", monitorIndex) })
}

/** [pos]: `left`, `right`, `maximize` or `restore`. */
suspend fun HelmApi.snapWindow(hwnd: JsonPrimitive, pos: String) {
    p3("windows.snap", buildJsonObject { put("hwnd", hwnd); put("pos", pos) })
}

/** [dir]: `next`, `prev`, `new` or `close`. */
suspend fun HelmApi.switchDesktop(dir: String) {
    p3("desktop.switch", buildJsonObject { put("dir", dir) })
}

// Profiles
suspend fun HelmApi.profiles(): List<Profile> = Profile.listFrom(p3("profiles.get"))

suspend fun HelmApi.setProfiles(list: List<Profile>) {
    p3("profiles.set", Profile.encode(list))
}

// File browser
suspend fun HelmApi.fsList(path: String): List<FsEntry> =
    FsEntry.sorted(FsEntry.listFrom(p3("fs.list", buildJsonObject { put("path", path) }, timeoutMs = 20_000)))

suspend fun HelmApi.fsDelete(path: String) {
    p3("fs.delete", buildJsonObject { put("path", path) }, timeoutMs = 30_000)
}

suspend fun HelmApi.fsMkdir(path: String) {
    p3("fs.mkdir", buildJsonObject { put("path", path) })
}

suspend fun HelmApi.fsOpen(path: String) {
    p3("fs.open", buildJsonObject { put("path", path) })
}

// Security
suspend fun HelmApi.setTier(deviceId: String, tier: String) {
    require(tier in Tiers.ALL) { "bad tier $tier" }
    p3("devices.set_tier", buildJsonObject { put("device_id", deviceId); put("tier", tier) })
}

suspend fun HelmApi.auditLines(limit: Int): List<String> =
    (p3("audit.list", buildJsonObject { put("limit", limit) })["lines"] as? JsonArray).orEmpty()
        .mapNotNull { (it as? JsonPrimitive)?.takeIf { p -> p.isString }?.content }

// Phase 4
suspend fun HelmApi.relayConfig(): RelayConfig = RelayConfig.from(p4("relay.get")) ?: RelayConfig(false, null)

suspend fun HelmApi.setRelayConfig(enabled: Boolean, url: String) {
    p4("relay.set", buildJsonObject { put("enabled", enabled); put("url", url) })
}

/** Asks this (awake) agent to send a Wake-on-LAN packet for [mac] on its LAN. */
suspend fun HelmApi.wolSend(mac: String) {
    p4("wol.send", buildJsonObject { put("mac", mac) })
}
