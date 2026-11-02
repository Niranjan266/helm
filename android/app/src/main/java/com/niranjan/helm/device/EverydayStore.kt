package com.niranjan.helm.device

import android.content.Context
import com.niranjan.helm.core.DropMode
import com.niranjan.helm.core.NotifyAccess
import com.niranjan.helm.core.PcNotifyPrefs
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** Phone-local settings of the everyday (`g6`) features. */
data class EverydaySettings(
    /** "When a file arrives on the PC". */
    val dropMode: DropMode = DropMode.DEFAULT,
    /** "Show PC notifications" + per-app filter (off by default). */
    val pcNotify: PcNotifyPrefs = PcNotifyPrefs(),
)

/** Separate preferences file so these settings never touch [AppSettings]. */
class EverydayStore private constructor(context: Context) {
    private val prefs = context.getSharedPreferences("helm_everyday", Context.MODE_PRIVATE)
    private val _settings = MutableStateFlow(load())
    val settings: StateFlow<EverydaySettings> = _settings.asStateFlow()

    /** Last `pc.notify.watch` answer for the connected PC (null = not asked yet). */
    val notifyAccess = MutableStateFlow<NotifyAccess?>(null)

    private fun load() = EverydaySettings(
        dropMode = DropMode.fromWire(prefs.getString("drop_mode", null)) ?: DropMode.DEFAULT,
        pcNotify = PcNotifyPrefs.decode(prefs.getString("pc_notify", null)),
    )

    @Synchronized
    fun update(change: (EverydaySettings) -> EverydaySettings) {
        val s = change(_settings.value)
        if (s == _settings.value) return
        prefs.edit()
            .putString("drop_mode", s.dropMode.wire)
            .putString("pc_notify", s.pcNotify.encode())
            .apply()
        _settings.value = s
    }

    companion object {
        @Volatile
        private var instance: EverydayStore? = null

        fun get(context: Context): EverydayStore =
            instance ?: synchronized(this) { instance ?: EverydayStore(context.applicationContext).also { instance = it } }
    }
}
