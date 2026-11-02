package com.niranjan.helm.ui

import android.app.Application
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.core.content.FileProvider
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.ClipHistory
import com.niranjan.helm.core.ClipItem
import com.niranjan.helm.core.ClipPick
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.PhoneEntry
import com.niranjan.helm.core.Schedule
import com.niranjan.helm.core.ScheduleRan
import com.niranjan.helm.core.SystemHealth
import com.niranjan.helm.core.clearClipboardHistory
import com.niranjan.helm.core.clipboardHistory
import com.niranjan.helm.core.deleteClipboardItem
import com.niranjan.helm.core.phones
import com.niranjan.helm.core.pickClipboardToPc
import com.niranjan.helm.core.pickClipboardToPhone
import com.niranjan.helm.core.ringPhone
import com.niranjan.helm.core.schedules
import com.niranjan.helm.core.setClipboardHistory
import com.niranjan.helm.core.setSchedules
import com.niranjan.helm.core.supportsEveryday
import com.niranjan.helm.core.systemHealth
import java.io.File
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

data class EverydayUi(
    val g6: Boolean = false,
    val clips: ClipHistory? = null,
    /** Decoded thumbnails by item id. */
    val thumbs: Map<Long, Bitmap> = emptyMap(),
    val phones: List<PhoneEntry>? = null,
    val schedules: List<Schedule>? = null,
    val health: SystemHealth? = null,
)

/** Everyday (`g6`) screens: clipboard history, ring another phone, schedules, health card. */
@OptIn(ExperimentalCoroutinesApi::class)
class EverydayViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    private val _ui = MutableStateFlow(EverydayUi())
    val ui: StateFlow<EverydayUi> = _ui.asStateFlow()
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages = _messages.asSharedFlow()

    private val api: HelmApi? get() = controller.readyConnection()?.let { HelmApi(it) }

    init {
        viewModelScope.launch {
            controller.session.map { it?.connection }.distinctUntilChanged().collect { _ui.value = EverydayUi() }
        }
        viewModelScope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.filterIsInstance<ConnectionState.Ready>()?.map { s.connection } ?: flowOf() }
                .collect { c ->
                    _ui.update { it.copy(g6 = c.supportsEveryday) }
                    if (!c.supportsEveryday) return@collect
                    c.events.collect { ev ->
                        when (ev.name) {
                            "clipboard.history.changed" -> if (_ui.value.clips != null) loadClips()
                            "schedule.changed" -> _ui.update { it.copy(schedules = Schedule.listFrom(ev.data)) }
                            ScheduleRan.NAME -> ScheduleRan.from(ev.data)?.let { r -> onRan(r) }
                        }
                    }
                }
        }
    }

    private fun message(t: String) {
        _messages.tryEmit(t)
    }

    private fun onRan(r: ScheduleRan) {
        val name = _ui.value.schedules?.firstOrNull { it.id == r.id }?.let { it.name ?: it.time } ?: r.id
        message(if (r.ok) "Schedule \"$name\" ran on the PC." else "Schedule \"$name\" failed: ${r.error ?: "unknown error"}")
        loadSchedules()
    }

    private suspend fun <T> act(what: String, block: suspend (HelmApi) -> T): T? {
        val a = api ?: run { message("Not connected."); return null }
        return try {
            block(a)
        } catch (e: HelmException) {
            message("$what failed: " + when (e.code) {
                "forbidden" -> "this phone's access level does not allow it."
                "unknown_method", "unsupported" -> "the PC agent does not support this yet."
                else -> e.message ?: e.code
            })
            null
        }
    }

    // ---- Clipboard history ----------------------------------------------------------------

    fun loadClips() {
        viewModelScope.launch {
            val h = act("Clipboard history") { it.clipboardHistory() } ?: return@launch
            publishClips(h)
        }
    }

    private suspend fun publishClips(h: ClipHistory) {
        val known = _ui.value.thumbs
        val thumbs = withContext(Dispatchers.Default) {
            h.items.filter { it.isImage }.mapNotNull { item ->
                known[item.id]?.let { item.id to it }
                    ?: item.thumbPng?.let { png -> BitmapFactory.decodeByteArray(png, 0, png.size)?.let { item.id to it } }
            }.toMap()
        }
        _ui.update { it.copy(clips = h, thumbs = thumbs) }
    }

    fun setClipHistory(enabled: Boolean? = null, persist: Boolean? = null) {
        viewModelScope.launch {
            act("Clipboard history") { it.setClipboardHistory(enabled, persist) }?.let { publishClips(it) }
        }
    }

    fun clearClips() {
        viewModelScope.launch { if (act("Clear history") { it.clearClipboardHistory() } != null) loadClips() }
    }

    fun deleteClip(item: ClipItem) {
        viewModelScope.launch { if (act("Delete") { it.deleteClipboardItem(item.id) } != null) loadClips() }
    }

    fun copyOnPc(item: ClipItem) {
        viewModelScope.launch { if (act("Copy on PC") { it.pickClipboardToPc(item.id) } != null) message("Copied on the PC.") }
    }

    fun copyToPhone(item: ClipItem) {
        viewModelScope.launch {
            val pick = act("Copy to phone") { it.pickClipboardToPhone(item.id) } ?: return@launch
            val app = getApplication<Application>()
            val cm = app.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
            when (pick) {
                is ClipPick.Text -> {
                    cm.setPrimaryClip(ClipData.newPlainText("From PC", pick.text))
                    message("Copied to the phone.")
                }
                is ClipPick.Image -> {
                    val uri = withContext(Dispatchers.IO) { cacheImage(item.id, pick.png) }
                    if (uri == null) return@launch message("Could not save the image.")
                    cm.setPrimaryClip(ClipData.newUri(app.contentResolver, "From PC", uri))
                    message("Image copied to the phone.")
                }
            }
        }
    }

    /** Fetches the full item and hands it to the Android share sheet via [onShare]. */
    fun share(item: ClipItem, onShare: (Intent) -> Unit) {
        viewModelScope.launch {
            val pick = act("Share") { it.pickClipboardToPhone(item.id) } ?: return@launch
            val send = when (pick) {
                is ClipPick.Text -> Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, pick.text)
                is ClipPick.Image -> {
                    val uri = withContext(Dispatchers.IO) { cacheImage(item.id, pick.png) } ?: return@launch message("Could not save the image.")
                    Intent(Intent.ACTION_SEND).setType("image/png").putExtra(Intent.EXTRA_STREAM, uri).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
            }
            onShare(Intent.createChooser(send, "Share"))
        }
    }

    private fun cacheImage(id: Long, png: ByteArray): android.net.Uri? = try {
        val app = getApplication<Application>()
        val dir = File(app.cacheDir, "clips").apply { mkdirs() }
        dir.listFiles()?.filter { System.currentTimeMillis() - it.lastModified() > 3_600_000 }?.forEach { it.delete() }
        val f = File(dir, "pc-clip-$id.png")
        f.writeBytes(png)
        FileProvider.getUriForFile(app, app.packageName + ".everydayfiles", f)
    } catch (_: Exception) {
        null
    }

    // ---- Ring another phone ---------------------------------------------------------------

    fun loadPhones() {
        viewModelScope.launch { act("Phones") { it.phones() }?.let { p -> _ui.update { it.copy(phones = p) } } }
    }

    fun ring(p: PhoneEntry, stop: Boolean) {
        viewModelScope.launch {
            val n = act(if (stop) "Stop ringing" else "Ring") { it.ringPhone(p.deviceId, stop) } ?: return@launch
            message(
                when {
                    n == 0 -> "${p.name} is not connected to the PC right now."
                    stop -> "Told ${p.name} to stop ringing."
                    else -> "Ringing ${p.name}…"
                },
            )
        }
    }

    // ---- Schedules ------------------------------------------------------------------------

    fun loadSchedules() {
        viewModelScope.launch { act("Schedules") { it.schedules() }?.let { s -> _ui.update { it.copy(schedules = s) } } }
    }

    fun saveSchedules(list: List<Schedule>) {
        val before = _ui.value.schedules
        _ui.update { it.copy(schedules = list) }
        viewModelScope.launch {
            val a = api ?: run { message("Not connected."); _ui.update { it.copy(schedules = before) }; return@launch }
            try {
                val saved = a.setSchedules(list)
                _ui.update { it.copy(schedules = saved) }
                message("Schedules saved on the PC.")
            } catch (e: HelmException) {
                _ui.update { it.copy(schedules = before) }
                message(
                    when {
                        e.code == "forbidden" && Schedule.needsAdmin(list) ->
                            "Power actions (sleep, shut down, …) need the admin access level. Raise this phone to admin on the PC under Devices & security."
                        e.code == "forbidden" -> "This phone's access level does not allow changing schedules."
                        e.code == "unsupported" || e.code == "unknown_method" -> "The PC agent does not support schedules yet."
                        else -> "Saving schedules failed: ${e.message ?: e.code}"
                    },
                )
            }
        }
    }

    // ---- Health card ----------------------------------------------------------------------

    /** Quiet refresh (no error messages: the card just keeps the last value). */
    fun refreshHealth() {
        val a = api ?: return
        if (!_ui.value.g6) return
        viewModelScope.launch {
            try {
                val h = a.systemHealth()
                _ui.update { it.copy(health = h) }
            } catch (_: HelmException) {
            }
        }
    }
}
