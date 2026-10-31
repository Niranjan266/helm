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
    )

    fun update(change: (Settings) -> Settings) {
        val s = change(_settings.value)
        prefs.edit()
            .putBoolean("keep_on", s.keepScreenOnInDock)
            .putString("theme", s.theme.name)
            .putBoolean("haptics", s.haptics)
            .putInt("dock_page", s.dockPageSize)
            .putFloat("tp_speed", s.touchpadSpeed)
            .apply()
        _settings.value = s
    }
}
