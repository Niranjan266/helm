package com.niranjan.helm.device

import android.content.Context
import com.niranjan.helm.core.GamepadCodec
import com.niranjan.helm.core.GamepadProfile
import com.niranjan.helm.core.SideSwipeMode
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Touchpad gesture options (G1). Phone-local, kept apart from [AppSettings]. */
data class GestureSettings(
    /** Three-finger left/right: switch apps or virtual desktops. */
    val sideSwipe: SideSwipeMode = SideSwipeMode.APPS,
    val threeFinger: Boolean = true,
    /** Inertial two-finger scrolling after a flick. */
    val momentum: Boolean = true,
    /** Low fixed pointer gain, no acceleration. */
    val precision: Boolean = false,
)

class GestureStore(context: Context) {
    private val prefs = context.getSharedPreferences("helm_gestures", Context.MODE_PRIVATE)
    private val _settings = MutableStateFlow(load())
    val settings: StateFlow<GestureSettings> = _settings.asStateFlow()

    private fun load() = GestureSettings(
        sideSwipe = runCatching { SideSwipeMode.valueOf(prefs.getString("side", null) ?: "APPS") }.getOrDefault(SideSwipeMode.APPS),
        threeFinger = prefs.getBoolean("three", true),
        momentum = prefs.getBoolean("momentum", true),
        precision = prefs.getBoolean("precision", false),
    )

    fun update(change: (GestureSettings) -> GestureSettings) {
        val s = change(_settings.value)
        prefs.edit()
            .putString("side", s.sideSwipe.name)
            .putBoolean("three", s.threeFinger)
            .putBoolean("momentum", s.momentum)
            .putBoolean("precision", s.precision)
            .apply()
        _settings.value = s
    }

    companion object {
        @Volatile private var instance: GestureStore? = null
        fun get(context: Context): GestureStore =
            instance ?: synchronized(this) { instance ?: GestureStore(context.applicationContext).also { instance = it } }
    }
}

/** Gamepad mapping per paired PC (keyed by host id). */
class GamepadStore(context: Context) {
    private val prefs = context.getSharedPreferences("helm_gamepad", Context.MODE_PRIVATE)

    fun load(hostId: String): GamepadProfile = GamepadCodec.decodeProfile(prefs.getString(key(hostId), null))

    fun save(hostId: String, profile: GamepadProfile) {
        prefs.edit().putString(key(hostId), GamepadCodec.encodeProfile(profile)).apply()
    }

    private fun key(hostId: String) = "pc:" + hostId.lowercase()

    companion object {
        @Volatile private var instance: GamepadStore? = null
        fun get(context: Context): GamepadStore =
            instance ?: synchronized(this) { instance ?: GamepadStore(context.applicationContext).also { instance = it } }
    }
}
