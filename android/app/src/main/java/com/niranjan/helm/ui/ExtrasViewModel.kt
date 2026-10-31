package com.niranjan.helm.ui

import android.app.Application
import android.os.SystemClock
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.HaState
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.PluginInfo
import com.niranjan.helm.core.Recording
import com.niranjan.helm.core.TermEvent
import com.niranjan.helm.core.TermRun
import com.niranjan.helm.core.TermScreen
import com.niranjan.helm.core.Trigger
import com.niranjan.helm.core.haConfigSet
import com.niranjan.helm.core.haStates
import com.niranjan.helm.core.plugins
import com.niranjan.helm.core.recordStart
import com.niranjan.helm.core.recordStop
import com.niranjan.helm.core.setPlugin
import com.niranjan.helm.core.setTriggers
import com.niranjan.helm.core.termClose
import com.niranjan.helm.core.termOpen
import com.niranjan.helm.core.termResize
import com.niranjan.helm.core.termWrite
import com.niranjan.helm.core.triggers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.channels.Channel
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

data class TermUi(
    val termId: String? = null,
    val shell: String? = null,
    val opening: Boolean = false,
    val lines: List<List<TermRun>> = emptyList(),
    val cursorRow: Int = 0,
    val cursorCol: Int = 0,
    val cursorVisible: Boolean = true,
    /** Index in [lines] where the screen (not scrollback) starts. */
    val screenStart: Int = 0,
    val exitCode: Long? = null,
    val exited: Boolean = false,
)

data class ExtrasUi(
    val phase5: Boolean = false,
    val recording: Recording? = null,
    /** `elapsedRealtime` when the current recording started. */
    val recordingSince: Long = 0,
    val recordBusy: Boolean = false,
    val term: TermUi = TermUi(),
    val plugins: List<PluginInfo>? = null,
    val triggers: List<Trigger>? = null,
    val haStates: List<HaState>? = null,
    val haLoading: Boolean = false,
)

/** Phase 5 (§13) screens: recording, terminal, plugins, triggers, Home Assistant. */
@OptIn(ExperimentalCoroutinesApi::class)
class ExtrasViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    private val _ui = MutableStateFlow(ExtrasUi())
    val ui: StateFlow<ExtrasUi> = _ui.asStateFlow()
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages = _messages.asSharedFlow()

    /** This phone's device id (for the "this phone connects" trigger). */
    val myDeviceId: String get() = controller.deviceInfo.deviceId

    private val screen = TermScreen(80, 24)

    /** Keystrokes in order; one writer so they never reorder on the wire. */
    private val termOut = Channel<String>(Channel.UNLIMITED)

    private val conn: HelmConnection? get() = controller.readyConnection()
    private val api: HelmApi? get() = conn?.let { HelmApi(it) }

    init {
        viewModelScope.launch {
            controller.session.map { it?.connection }.distinctUntilChanged().collect {
                synchronized(screen) { screen.feed("\u001bc") }
                _ui.value = ExtrasUi()
            }
        }
        viewModelScope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.filterIsInstance<ConnectionState.Ready>()?.map { s.connection } ?: flowOf() }
                .collect { c ->
                    _ui.update { it.copy(phase5 = c.supportsPhase5) }
                    c.events.collect { ev ->
                        when (val t = TermEvent.from(ev.name, ev.data)) {
                            // While term.open is in flight the first output can beat its reply.
                            is TermEvent.Output -> if (t.termId == _ui.value.term.termId || (_ui.value.term.opening && _ui.value.term.termId == null)) {
                                synchronized(screen) { screen.feed(t.data) }
                                publishTerm()
                            }
                            is TermEvent.Exit -> if (t.termId == _ui.value.term.termId) {
                                _ui.update { it.copy(term = it.term.copy(exited = true, exitCode = t.code)) }
                            }
                            null -> Unit
                        }
                    }
                }
        }
        viewModelScope.launch {
            for (text in termOut) {
                val id = _ui.value.term.termId ?: continue
                if (_ui.value.term.exited) continue
                act("Terminal") { it.termWrite(id, text) }
            }
        }
    }

    private fun message(t: String) {
        _messages.tryEmit(t)
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

    // ---- Recording ----------------------------------------------------------------------

    fun toggleRecording(audio: Boolean) {
        if (_ui.value.recordBusy) return
        _ui.update { it.copy(recordBusy = true) }
        viewModelScope.launch {
            if (_ui.value.recording == null) {
                val r = act("Start recording") { it.recordStart(audio) }
                _ui.update { it.copy(recordBusy = false, recording = r, recordingSince = SystemClock.elapsedRealtime()) }
                if (r != null) message("Recording on the PC.")
            } else {
                val r = act("Stop recording") { it.recordStop() }
                _ui.update { it.copy(recordBusy = false, recording = if (r != null) null else it.recording) }
                if (r != null) {
                    val secs = (r.durationMs ?: 0) / 1000
                    message("Saved ${r.fileName} (${secs / 60}:${"%02d".format(secs % 60)}) in Videos\\Helm on the PC.")
                }
            }
        }
    }

    // ---- Terminal -----------------------------------------------------------------------

    private fun publishTerm() {
        val snap = synchronized(screen) {
            val lines = screen.lines()
            TermSnap(lines, lines.size - screen.rows, screen.cursorRow, screen.cursorCol, screen.cursorVisible)
        }
        _ui.update {
            it.copy(term = it.term.copy(
                lines = snap.lines, screenStart = snap.screenStart.coerceAtLeast(0),
                cursorRow = snap.row, cursorCol = snap.col, cursorVisible = snap.visible,
            ))
        }
    }

    private class TermSnap(val lines: List<List<TermRun>>, val screenStart: Int, val row: Int, val col: Int, val visible: Boolean)

    /** Opens a new session (closing any previous one). Call after the admin check. */
    fun openTerminal(shell: String, cols: Int, rows: Int) {
        if (_ui.value.term.opening) return
        val old = _ui.value.term.termId
        synchronized(screen) {
            screen.feed("\u001bc")
            screen.resize(cols, rows)
        }
        _ui.update { it.copy(term = TermUi(shell = shell, opening = true)) }
        viewModelScope.launch {
            if (old != null) runCatching { api?.termClose(old) }
            val id = act("Terminal") { it.termOpen(shell, cols, rows) }
            _ui.update { it.copy(term = it.term.copy(termId = id, opening = false)) }
            publishTerm()
        }
    }

    fun termSend(text: String) {
        if (text.isNotEmpty()) termOut.trySend(text)
    }

    /** The view's grid changed size (rotation, keyboard). */
    fun termResize(cols: Int, rows: Int) {
        val changed = synchronized(screen) {
            val c = screen.cols != cols || screen.rows != rows
            screen.resize(cols, rows)
            c
        }
        if (!changed) return
        publishTerm()
        val id = _ui.value.term.termId ?: return
        if (_ui.value.term.exited) return
        viewModelScope.launch { runCatching { api?.termResize(id, cols, rows) } }
    }

    fun closeTerminal() {
        val id = _ui.value.term.termId ?: return
        _ui.update { it.copy(term = it.term.copy(exited = true)) }
        viewModelScope.launch { act("Close terminal") { it.termClose(id) } }
    }

    // ---- Plugins ------------------------------------------------------------------------

    fun loadPlugins() {
        viewModelScope.launch { act("Plugins") { it.plugins() }?.let { p -> _ui.update { it.copy(plugins = p) } } }
    }

    /** Admin: enable/disable and grant capabilities (call after the admin check). */
    fun setPlugin(p: PluginInfo, enabled: Boolean, granted: List<String>) {
        viewModelScope.launch {
            if (act("Plugin ${p.name}") { it.setPlugin(p.id, enabled, granted) } != null) loadPlugins()
        }
    }

    // ---- Triggers -----------------------------------------------------------------------

    fun loadTriggers() {
        viewModelScope.launch { act("Triggers") { it.triggers() }?.let { t -> _ui.update { it.copy(triggers = t) } } }
    }

    fun saveTriggers(list: List<Trigger>) {
        val before = _ui.value.triggers
        _ui.update { it.copy(triggers = list) }
        viewModelScope.launch {
            if (act("Saving triggers") { it.setTriggers(list) } == null) _ui.update { it.copy(triggers = before) } else message("Triggers saved on the PC.")
        }
    }

    // ---- Home Assistant -----------------------------------------------------------------

    /** Admin: stores URL + long-lived token on the PC (encrypted there). Call after the admin check. */
    fun saveHaConfig(url: String, token: String) {
        viewModelScope.launch {
            if (act("Home Assistant") { it.haConfigSet(url, token) } != null) {
                message("Home Assistant connection saved on the PC.")
                loadHaStates()
            }
        }
    }

    fun loadHaStates() {
        _ui.update { it.copy(haLoading = true) }
        viewModelScope.launch {
            val s = act("Home Assistant states") { it.haStates() }
            _ui.update { if (s != null) it.copy(haStates = s, haLoading = false) else it.copy(haLoading = false) }
        }
    }

    override fun onCleared() {
        termOut.close()
        super.onCleared()
    }
}
