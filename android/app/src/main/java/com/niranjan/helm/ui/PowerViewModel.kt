package com.niranjan.helm.ui

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.AlertRule
import com.niranjan.helm.core.AudioSession
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.FsEntry
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.PcPath
import com.niranjan.helm.core.ProcessEntry
import com.niranjan.helm.core.ProcessSort
import com.niranjan.helm.core.Profile
import com.niranjan.helm.core.RelayConfig
import com.niranjan.helm.core.Stats
import com.niranjan.helm.core.StatsHistory
import com.niranjan.helm.core.WindowEntry
import com.niranjan.helm.core.alerts
import com.niranjan.helm.core.audioSessions
import com.niranjan.helm.core.auditLines
import com.niranjan.helm.core.fsDelete
import com.niranjan.helm.core.fsList
import com.niranjan.helm.core.fsMkdir
import com.niranjan.helm.core.fsOpen
import com.niranjan.helm.core.killProcess
import com.niranjan.helm.core.moveWindow
import com.niranjan.helm.core.processes
import com.niranjan.helm.core.profiles
import com.niranjan.helm.core.relayConfig
import com.niranjan.helm.core.setAlerts
import com.niranjan.helm.core.setAudioSession
import com.niranjan.helm.core.setPriority
import com.niranjan.helm.core.setProfiles
import com.niranjan.helm.core.setRelayConfig
import com.niranjan.helm.core.setTier
import com.niranjan.helm.core.snapWindow
import com.niranjan.helm.core.statsSubscribe
import com.niranjan.helm.core.statsUnsubscribe
import com.niranjan.helm.core.switchDesktop
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterIsInstance
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.serialization.json.JsonPrimitive

data class PowerUi(
    val phase3: Boolean = false,
    val phase4: Boolean = false,
    val stats: List<Stats> = emptyList(),
    val alerts: List<AlertRule>? = null,
    /** Rule ids currently firing. */
    val firing: Set<String> = emptySet(),
    val processes: List<ProcessEntry>? = null,
    val processSort: ProcessSort = ProcessSort.CPU,
    val processFilter: String = "",
    val sessions: List<AudioSession>? = null,
    val profiles: List<Profile>? = null,
    val activeProfile: String? = null,
    val fsPath: String = "",
    val fsEntries: List<FsEntry>? = null,
    val fsLoading: Boolean = false,
    val audit: List<String>? = null,
    val relay: RelayConfig? = null,
)

/** Phase 3 (§11) screens: monitor, tasks, mixer, windows, profiles, files, security. */
@OptIn(ExperimentalCoroutinesApi::class)
class PowerViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    private val _ui = MutableStateFlow(PowerUi())
    val ui: StateFlow<PowerUi> = _ui.asStateFlow()
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages = _messages.asSharedFlow()

    private val history = StatsHistory(120)
    private var monitorUsers = 0
    private var processJob: Job? = null
    private val sessionTarget = MutableStateFlow<Pair<JsonPrimitive, Float>?>(null)

    private val conn: HelmConnection? get() = controller.readyConnection()
    private val api: HelmApi? get() = conn?.let { HelmApi(it) }

    init {
        viewModelScope.launch {
            controller.session.map { it?.connection }.distinctUntilChanged().collect {
                _ui.value = PowerUi()
                history.clear()
            }
        }
        viewModelScope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.filterIsInstance<ConnectionState.Ready>()?.map { s.connection } ?: flowOf() }
                .collect { c ->
                    _ui.update { it.copy(phase3 = c.supportsPhase3, phase4 = c.supportsPhase4) }
                    if (monitorUsers > 0) quiet { it.statsSubscribe(INTERVAL_MS) }
                    c.events.collect { ev ->
                        when (ev.name) {
                            "stats.tick" -> _ui.update { it.copy(stats = history.add(Stats.from(ev.data))) }
                            "alert" -> com.niranjan.helm.core.AlertEvent.from(ev.data)?.let { a ->
                                _ui.update { u -> u.copy(firing = if (a.firing) u.firing + a.id else u.firing - a.id) }
                            }
                            "profile.active" -> _ui.update {
                                it.copy(activeProfile = (ev.data["id"] as? JsonPrimitive)?.takeIf { p -> p.isString }?.content)
                            }
                        }
                    }
                }
        }
        viewModelScope.launch {
            sessionTarget.filterNotNull().collect { (id, level) ->
                act("Volume") { it.setAudioSession(id, level = level) }
                delay(50)
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

    private suspend fun <T> quiet(block: suspend (HelmApi) -> T): T? = try {
        api?.let { block(it) }
    } catch (_: HelmException) {
        null
    }

    // ---- Monitor & alerts ---------------------------------------------------------------

    /** Call while the Monitor screen is visible; [visible] false unsubscribes. */
    fun monitorVisible(visible: Boolean) {
        monitorUsers = (monitorUsers + if (visible) 1 else -1).coerceAtLeast(0)
        viewModelScope.launch {
            if (visible && monitorUsers == 1) {
                act("Live stats") { it.statsSubscribe(INTERVAL_MS) }
                loadAlerts()
            } else if (!visible && monitorUsers == 0) {
                quiet { it.statsUnsubscribe() }
            }
        }
    }

    fun loadAlerts() {
        viewModelScope.launch { quiet { it.alerts() }?.let { r -> _ui.update { it.copy(alerts = r) } } }
    }

    fun saveAlerts(rules: List<AlertRule>) {
        val before = _ui.value.alerts
        _ui.update { it.copy(alerts = rules) }
        viewModelScope.launch {
            if (act("Saving alerts") { it.setAlerts(rules) } == null) _ui.update { it.copy(alerts = before) } else message("Alerts saved on the PC.")
        }
    }

    // ---- Task manager -------------------------------------------------------------------

    fun tasksVisible(visible: Boolean) {
        processJob?.cancel()
        if (!visible) return
        processJob = viewModelScope.launch {
            while (true) {
                quiet { it.processes() }?.let { p -> _ui.update { it.copy(processes = p) } }
                delay(3_000)
            }
        }
    }

    fun setProcessSort(s: ProcessSort) = _ui.update { it.copy(processSort = s) }

    fun setProcessFilter(f: String) = _ui.update { it.copy(processFilter = f) }

    fun kill(p: ProcessEntry) {
        viewModelScope.launch {
            if (act("End task") { it.killProcess(p.pid) } != null) {
                message("Ended ${p.name}.")
                _ui.update { u -> u.copy(processes = u.processes?.filterNot { it.pid == p.pid }) }
            }
        }
    }

    fun priority(p: ProcessEntry, level: String) {
        viewModelScope.launch { if (act("Priority") { it.setPriority(p.pid, level) } != null) message("${p.name}: ${level.replace('_', ' ')} priority.") }
    }

    // ---- Mixer --------------------------------------------------------------------------

    fun loadSessions() {
        viewModelScope.launch { act("Mixer") { it.audioSessions() }?.let { s -> _ui.update { it.copy(sessions = s) } } }
    }

    fun setSessionLevel(s: AudioSession, level: Float) {
        _ui.update { u -> u.copy(sessions = u.sessions?.map { if (it.id == s.id) it.copy(level = level) else it }) }
        sessionTarget.value = s.id to level
    }

    fun toggleSessionMute(s: AudioSession) {
        _ui.update { u -> u.copy(sessions = u.sessions?.map { if (it.id == s.id) it.copy(muted = !s.muted) else it }) }
        viewModelScope.launch { act("Mute") { it.setAudioSession(s.id, muted = !s.muted) } }
    }

    // ---- Windows & desktops -------------------------------------------------------------

    fun snap(w: WindowEntry, pos: String) {
        viewModelScope.launch { act("Snap") { it.snapWindow(w.hwnd, pos) } }
    }

    fun moveToMonitor(w: WindowEntry, index: Int) {
        viewModelScope.launch { act("Move window") { it.moveWindow(w.hwnd, index) } }
    }

    fun desktop(dir: String) {
        viewModelScope.launch { act("Virtual desktop") { it.switchDesktop(dir) } }
    }

    // ---- Profiles -----------------------------------------------------------------------

    fun loadProfiles() {
        viewModelScope.launch { act("Profiles") { it.profiles() }?.let { p -> _ui.update { it.copy(profiles = p) } } }
    }

    fun saveProfiles(list: List<Profile>) {
        val before = _ui.value.profiles
        _ui.update { it.copy(profiles = list) }
        viewModelScope.launch { if (act("Saving profiles") { it.setProfiles(list) } == null) _ui.update { it.copy(profiles = before) } }
    }

    // ---- File browser -------------------------------------------------------------------

    fun browse(path: String) {
        _ui.update { it.copy(fsLoading = true) }
        viewModelScope.launch {
            val entries = act("Folder") { it.fsList(path) }
            _ui.update { if (entries != null) it.copy(fsPath = path, fsEntries = entries, fsLoading = false) else it.copy(fsLoading = false) }
        }
    }

    fun up() = browse(PcPath.parent(_ui.value.fsPath))

    fun refreshFolder() = browse(_ui.value.fsPath)

    fun mkdir(name: String) {
        val dir = _ui.value.fsPath
        if (dir.isEmpty() || !PcPath.validName(name)) return message("That folder name is not allowed.")
        viewModelScope.launch { if (act("New folder") { it.fsMkdir(PcPath.join(dir, name.trim())) } != null) refreshFolder() }
    }

    fun delete(e: FsEntry) {
        viewModelScope.launch {
            if (act("Delete") { it.fsDelete(PcPath.join(_ui.value.fsPath, e.name)) } != null) {
                message("Deleted ${e.name}.")
                refreshFolder()
            }
        }
    }

    fun openOnPc(e: FsEntry) {
        viewModelScope.launch { if (act("Open") { it.fsOpen(PcPath.join(_ui.value.fsPath, e.name)) } != null) message("Opened ${e.name} on the PC.") }
    }

    fun pathOf(e: FsEntry): String = PcPath.join(_ui.value.fsPath, e.name)

    // ---- Security -----------------------------------------------------------------------

    fun setTier(deviceId: String, tier: String, after: () -> Unit) {
        viewModelScope.launch { if (act("Access level") { it.setTier(deviceId, tier) } != null) after() }
    }

    fun loadAudit() {
        viewModelScope.launch { act("Audit log") { it.auditLines(200) }?.let { l -> _ui.update { it.copy(audit = l.reversed()) } } }
    }

    fun loadRelay() {
        viewModelScope.launch { quiet { it.relayConfig() }?.let { r -> _ui.update { it.copy(relay = r) } } }
    }

    fun saveRelay(enabled: Boolean, url: String) {
        viewModelScope.launch {
            if (act("Relay") { it.setRelayConfig(enabled, url.trim()) } != null) {
                _ui.update { it.copy(relay = RelayConfig(enabled, url.trim().ifBlank { null })) }
                message(if (enabled) "Relay turned on. Reconnect once on Wi-Fi so this phone learns it." else "Relay turned off.")
            }
        }
    }

    private companion object {
        const val INTERVAL_MS = 1_000L
    }
}
