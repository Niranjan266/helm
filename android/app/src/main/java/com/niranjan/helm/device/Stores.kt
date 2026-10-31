package com.niranjan.helm.device

import android.content.Context
import android.os.Build
import android.provider.Settings
import com.niranjan.helm.core.SavedPc
import com.niranjan.helm.core.SavedPcCodec
import java.util.UUID
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** This phone's stable device id and a friendly name. */
class DeviceInfoStore(private val context: Context) {
    private val prefs = context.getSharedPreferences("helm_device", Context.MODE_PRIVATE)

    val deviceId: String
        get() = synchronized(this) {
            prefs.getString(KEY_ID, null) ?: UUID.randomUUID().toString().also {
                prefs.edit().putString(KEY_ID, it).apply()
            }
        }

    val deviceName: String
        get() {
            val name = try {
                Settings.Global.getString(context.contentResolver, Settings.Global.DEVICE_NAME)
            } catch (_: Exception) {
                null
            }
            return name?.takeIf { it.isNotBlank() } ?: Build.MODEL ?: "Android phone"
        }

    private companion object {
        const val KEY_ID = "device_id"
    }
}

/** Paired PCs, as JSON in SharedPreferences. */
class SavedPcStore(context: Context) {
    private val prefs = context.getSharedPreferences("helm_pcs", Context.MODE_PRIVATE)
    private val _pcs = MutableStateFlow(SavedPcCodec.decode(prefs.getString(KEY, null)))
    val pcs: StateFlow<List<SavedPc>> = _pcs.asStateFlow()

    fun get(hostId: String): SavedPc? = _pcs.value.firstOrNull { it.hostId.equals(hostId, ignoreCase = true) }

    @Synchronized
    fun upsert(pc: SavedPc) {
        val list = _pcs.value.filterNot { it.hostId.equals(pc.hostId, ignoreCase = true) } + pc
        save(list.sortedByDescending { it.lastUsedMs })
    }

    @Synchronized
    fun remove(hostId: String) {
        save(_pcs.value.filterNot { it.hostId.equals(hostId, ignoreCase = true) })
    }

    private fun save(list: List<SavedPc>) {
        prefs.edit().putString(KEY, SavedPcCodec.encode(list)).apply()
        _pcs.value = list
    }

    private companion object {
        const val KEY = "pcs"
    }
}
