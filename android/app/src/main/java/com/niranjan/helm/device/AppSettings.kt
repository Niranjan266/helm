package com.niranjan.helm.device

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

enum class ThemeMode { DARK, AMOLED }

data class Settings(
    val keepScreenOnInDock: Boolean = true,
    val theme: ThemeMode = ThemeMode.DARK,
    val haptics: Boolean = true,
    /** Dock tiles per page: 4, 6 or 8. */
    val dockPageSize: Int = 6,
    /** Touchpad speed multiplier, 0.4..3. */
    val touchpadSpeed: Float = 1.2f,
    /** Screen stream (Phase 2): [com.niranjan.helm.core.StreamPreset] name. */
    val streamPreset: String = "P1080_30",
    /** 0 = the preset's default bitrate. */
    val streamBitrateKbps: Int = 0,
    val streamAudio: Boolean = true,
    /** true = direct touch (tap where you see), false = trackpad. */
    val streamDirectTouch: Boolean = false,
    val streamShowStats: Boolean = false,
    /** Phase 5: mirror this phone's notifications to the PC (off by default). */
    val mirrorNotifications: Boolean = false,
    /** Package names allowed to mirror; empty = none. */
    val mirrorApps: Set<String> = emptySet(),
    /** G5: order dock tiles and the app list by how much each app is used on the PC. */
    val smartOrder: Boolean = false,
)

/** Phone-local preferences (not synced to the PC). */
class AppSettings(context: Context) {
    private val prefs = context.getSharedPreferences("helm_settings", Context.MODE_PRIVATE)
    private val _settings = MutableStateFlow(load())
    val settings: StateFlow<Settings> = _settings.asStateFlow()

    private fun load() = Settings(
        keepScreenOnInDock = prefs.getBoolean("keep_on", true),
        theme = runCatching { ThemeMode.valueOf(prefs.getString("theme", null) ?: "DARK") }.getOrDefault(ThemeMode.DARK),
        haptics = prefs.getBoolean("haptics", true),
        dockPageSize = prefs.getInt("dock_page", 6).let { if (it in listOf(4, 6, 8)) it else 6 },
        touchpadSpeed = prefs.getFloat("tp_speed", 1.2f).coerceIn(0.4f, 3f),
        streamPreset = prefs.getString("st_preset", null) ?: "P1080_30",
        streamBitrateKbps = prefs.getInt("st_kbps", 0).coerceAtLeast(0),
        streamAudio = prefs.getBoolean("st_audio", true),
        streamDirectTouch = prefs.getBoolean("st_direct", false),
        streamShowStats = prefs.getBoolean("st_stats", false),
        mirrorNotifications = prefs.getBoolean("nm_on", false),
        mirrorApps = com.niranjan.helm.core.NotifyAllowList.decode(prefs.getString("nm_apps", null)),
        smartOrder = prefs.getBoolean("smart_order", false),
    )

    fun update(change: (Settings) -> Settings) {
        val s = change(_settings.value)
        prefs.edit()
            .putBoolean("keep_on", s.keepScreenOnInDock)
            .putString("theme", s.theme.name)
            .putBoolean("haptics", s.haptics)
            .putInt("dock_page", s.dockPageSize)
            .putFloat("tp_speed", s.touchpadSpeed)
            .putString("st_preset", s.streamPreset)
            .putInt("st_kbps", s.streamBitrateKbps)
            .putBoolean("st_audio", s.streamAudio)
            .putBoolean("st_direct", s.streamDirectTouch)
            .putBoolean("st_stats", s.streamShowStats)
            .putBoolean("nm_on", s.mirrorNotifications)
            .putString("nm_apps", com.niranjan.helm.core.NotifyAllowList.encode(s.mirrorApps))
            .putBoolean("smart_order", s.smartOrder)
            .apply()
        _settings.value = s
    }
}
