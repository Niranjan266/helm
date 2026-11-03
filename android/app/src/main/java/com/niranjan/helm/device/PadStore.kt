package com.niranjan.helm.device

import android.content.Context
import com.niranjan.helm.core.KbdGeometry
import com.niranjan.helm.core.PadPanel

/**
 * Landscape touchpad choices: the right panel per PC (keyed by host id, like the gamepad
 * mapping) and the floating keyboard's position, size and opacity (one per phone, remembered
 * across the touchpad and the Screen tab).
 */
class PadStore(context: Context) {
    private val prefs = context.getSharedPreferences("helm_pad", Context.MODE_PRIVATE)

    fun panel(hostId: String?): PadPanel = PadPanel.parse(prefs.getString(key(hostId, "panel"), null))

    fun setPanel(hostId: String?, panel: PadPanel) {
        prefs.edit().putString(key(hostId, "panel"), panel.name).apply()
    }

    fun panelCollapsed(hostId: String?): Boolean = prefs.getBoolean(key(hostId, "collapsed"), false)

    fun setPanelCollapsed(hostId: String?, collapsed: Boolean) {
        prefs.edit().putBoolean(key(hostId, "collapsed"), collapsed).apply()
    }

    fun keyboard(): KbdGeometry = KbdGeometry(
        xFrac = prefs.getFloat("kbd_x", 0.5f),
        yFrac = prefs.getFloat("kbd_y", 1f),
        widthFrac = prefs.getFloat("kbd_w", 0.62f),
        opacity = prefs.getFloat("kbd_opacity", 0.92f),
    ).sane()

    fun setKeyboard(g: KbdGeometry) {
        val s = g.sane()
        prefs.edit()
            .putFloat("kbd_x", s.xFrac)
            .putFloat("kbd_y", s.yFrac)
            .putFloat("kbd_w", s.widthFrac)
            .putFloat("kbd_opacity", s.opacity)
            .apply()
    }

    fun keyboardOpen(): Boolean = prefs.getBoolean("kbd_open", false)

    fun setKeyboardOpen(open: Boolean) {
        prefs.edit().putBoolean("kbd_open", open).apply()
    }

    private fun key(hostId: String?, field: String) = "pc:${hostId?.lowercase().orEmpty()}:$field"

    companion object {
        @Volatile private var instance: PadStore? = null
        fun get(context: Context): PadStore =
            instance ?: synchronized(this) { instance ?: PadStore(context.applicationContext).also { instance = it } }
    }
}
