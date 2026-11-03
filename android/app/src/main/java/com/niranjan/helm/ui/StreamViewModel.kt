package com.niranjan.helm.ui

import android.app.Application
import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.ConnectivityManager
import android.net.Network
import android.view.Surface
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.core.StreamFrame
import com.niranjan.helm.core.StreamFrameSink
import com.niranjan.helm.core.StreamPreset
import com.niranjan.helm.core.StreamRequest
import com.niranjan.helm.core.StreamSession
import com.niranjan.helm.core.StreamStartInfo
import com.niranjan.helm.core.StreamState
import com.niranjan.helm.core.StreamStatsCounter
import com.niranjan.helm.core.StreamStatsReport
import com.niranjan.helm.core.GamingPreset
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.RotationLock
import com.niranjan.helm.core.SecondScreen
import com.niranjan.helm.core.WindowEntry
import com.niranjan.helm.core.WindowThumb
import com.niranjan.helm.core.supportsG3
import com.niranjan.helm.core.windowThumb
import com.niranjan.helm.device.AudioPlayer
import com.niranjan.helm.device.Settings
import com.niranjan.helm.device.VideoDecoder
import java.util.concurrent.ConcurrentHashMap
import kotlin.math.min
import kotlinx.coroutines.Job
import kotlinx.coroutines.NonCancellable
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** Cursor image from a `0x05` frame. */
class CursorImage(val bitmap: Bitmap, val hotX: Int, val hotY: Int)

/** Latest cursor position (`0x04`), in stream pixels. */
data class CursorPos(val x: Float, val y: Float, val visible: Boolean, val shapeId: Long)

data class StreamUi(
    val supported: Boolean = true,
    val status: String = "Idle",
    val streaming: Boolean = false,
    val error: String? = null,
    val info: StreamStartInfo? = null,
    /** Decoded picture size (follows SPS changes). */
    val videoWidth: Int = 0,
    val videoHeight: Int = 0,
    val stats: StreamStatsReport? = null,
    val totalDropped: Long = 0,
    val lowLatency: Boolean = false,
    val codecName: String? = null,
    val monitor: MonitorInfo? = null,
    val reconnects: Int = 0,
    /** This stream runs over the relay (§12). */
    val viaRelay: Boolean = false,
    /** What the relay policy picked; the controls show this while on the relay. */
    val relayChoice: com.niranjan.helm.core.StreamChoice? = null,
)

/** G3 stream options (Docs/protocol/streaming-2.md). Not persisted: each visit starts plain. */
data class G3Ui(
    /** The PC agent speaks `g3`. */
    val supported: Boolean = false,
    /** Streaming one window instead of a monitor. */
    val window: WindowEntry? = null,
    val gaming: GamingPreset? = null,
    /** Long-press-then-drag holds the left button. */
    val dragMode: Boolean = false,
    val rotation: RotationLock = RotationLock.AUTO,
    /** Second-screen mirror: one monitor, controls hidden, view only, screen kept on. */
    val secondScreen: Boolean = false,
    /** Input currently travels on the `/stream` socket. */
    val inputOnStream: Boolean = false,
    /** Window picker thumbnails by hwnd text (null = none available). */
    val thumbs: Map<String, Bitmap?> = emptyMap(),
)

/** Phase 2 screen stream: one [StreamSession] at a time, decoders, auto-reconnect. */
class StreamViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    val settings: StateFlow<Settings> = controller.settings.settings

    private val _ui = MutableStateFlow(StreamUi())
    val ui: StateFlow<StreamUi> = _ui.asStateFlow()

    private val _g3 = MutableStateFlow(G3Ui())
    val g3: StateFlow<G3Ui> = _g3.asStateFlow()

    /** Phone screen size in pixels (second-screen stream size). */
    var phoneScreen: Pair<Int, Int> = 0 to 0

    private val _cursor = MutableStateFlow<CursorPos?>(null)
    val cursor: StateFlow<CursorPos?> = _cursor.asStateFlow()
    val cursorImages = ConcurrentHashMap<Long, CursorImage>()
    private val _cursorShapeVersion = MutableStateFlow(0)
    val cursorShapeVersion: StateFlow<Int> = _cursorShapeVersion.asStateFlow()

    @Volatile
    private var session: StreamSession? = null
    private var keeper: Job? = null
    private val restart = MutableSharedFlow<String>(extraBufferCapacity = 4)

    /** Shared by the decoder (decoded / dropped) and each session (bytes, RTT, reports). */
    private val decoderStats = StreamStatsCounter(System.currentTimeMillis())

    private val decoder = VideoDecoder(
        stats = decoderStats,
        requestKeyframe = { session?.let { s -> viewModelScope.launch { s.requestKeyframe() } } },
        onVideoSize = { w, h -> _ui.update { it.copy(videoWidth = w, videoHeight = h) } },
    )
    private val audio = AudioPlayer()

    private val sink = StreamFrameSink { f ->
        when (f) {
            is StreamFrame.Video -> decoder.submit(f)
            is StreamFrame.Audio -> audio.submit(f)
            is StreamFrame.Cursor -> _cursor.value = CursorPos(f.x, f.y, f.visible, f.shapeId)
            is StreamFrame.CursorShape -> {
                val bmp = runCatching { BitmapFactory.decodeByteArray(f.png, 0, f.png.size) }.getOrNull()
                if (bmp != null) {
                    cursorImages[f.shapeId] = CursorImage(bmp, f.hotX, f.hotY)
                    _cursorShapeVersion.update { it + 1 }
                }
            }
        }
    }

    private val connectivity = app.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private var lastNetwork: Network? = null
    private val netCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            val changed = lastNetwork != null && lastNetwork != network
            lastNetwork = network
            if (changed) restart.tryEmit("network")
        }

        override fun onLost(network: Network) {
            if (network == lastNetwork) _ui.update { it.copy(status = "Network lost — waiting…") }
        }
    }

    init {
        runCatching { connectivity.registerDefaultNetworkCallback(netCallback) }
        audio.enabled = settings.value.streamAudio
    }

    private val control: HelmConnection?
        get() = controller.session.value?.connection?.takeIf { it.state.value is ConnectionState.Ready }

    /** The Screen tab became visible (true) or hidden (false). */
    fun setActive(active: Boolean) {
        if (active) {
            if (keeper?.isActive == true) return
            keeper = viewModelScope.launch { keepStreaming() }
        } else {
            keeper?.cancel()
            keeper = null
            stopCurrent()
        }
    }

    fun setSurface(s: Surface?) = decoder.setSurface(s)

    fun selectMonitor(m: MonitorInfo?) {
        _ui.update { it.copy(monitor = m) }
        _g3.update { it.copy(window = null) }
        restart.tryEmit("monitor")
    }

    // ---- G3 ----

    /** Stream one window (null = back to the monitor). */
    fun selectWindow(w: WindowEntry?) {
        _g3.update { it.copy(window = w, secondScreen = if (w != null) false else it.secondScreen) }
        restart.tryEmit("window")
    }

    fun setGaming(p: GamingPreset?) {
        _g3.update { it.copy(gaming = p) }
        if (p != null) {
            audio.enabled = true
            controller.settings.update { it.copy(streamAudio = true) }
        }
        restart.tryEmit("gaming")
    }

    fun setDragMode(on: Boolean) = _g3.update { it.copy(dragMode = on) }

    fun setRotation(r: RotationLock) = _g3.update { it.copy(rotation = r) }

    /** Mirror monitor [m] (null = leave second-screen mode). */
    fun setSecondScreen(m: MonitorInfo?, rotation: RotationLock = RotationLock.MATCH_MONITOR) {
        if (m == null) {
            _g3.update { it.copy(secondScreen = false, rotation = RotationLock.AUTO) }
        } else {
            _ui.update { it.copy(monitor = m) }
            _g3.update { it.copy(secondScreen = true, window = null, gaming = null, rotation = rotation) }
        }
        restart.tryEmit("second-screen")
    }

    /** Loads picker thumbnails (`windows.thumb`) for [windows], a few at a time. */
    fun loadThumbs(windows: List<WindowEntry>) {
        val c = control ?: return
        if (!c.supportsG3) return
        val api = HelmApi(c)
        viewModelScope.launch {
            for (w in WindowThumb.pickable(windows)) {
                val key = w.hwnd.toString()
                if (key in _g3.value.thumbs) continue
                val png = try {
                    api.windowThumb(w.hwnd)
                } catch (_: HelmException) {
                    null
                }
                val bmp = png?.let { runCatching { BitmapFactory.decodeByteArray(it, 0, it.size) }.getOrNull() }
                _g3.update { it.copy(thumbs = it.thumbs + (key to bmp)) }
            }
        }
    }

    /** The user's quality picks while on the relay; they don't touch the LAN settings. */
    private var relayPick: StreamPreset? = null
    private var relayKbps = 0

    private val onRelay: Boolean get() = control?.readyInfo?.viaRelay == true

    fun setPreset(p: StreamPreset) {
        if (onRelay) {
            relayPick = p
            relayKbps = 0
        } else {
            controller.settings.update { it.copy(streamPreset = p.name, streamBitrateKbps = 0) }
        }
        restart.tryEmit("preset")
    }

    /** Bitrate changes go through `stream.config` (no restart). */
    fun setBitrate(kbps: Int) {
        if (onRelay) {
            relayKbps = kbps
            val c = _ui.value.relayChoice
            if (c != null) _ui.update { it.copy(relayChoice = c.copy(kbps = kbps, loweredForRelay = false)) }
        } else {
            controller.settings.update { it.copy(streamBitrateKbps = kbps) }
        }
        val s = session ?: return
        viewModelScope.launch {
            try {
                s.configure(bitrateKbps = kbps)
            } catch (e: HelmException) {
                _ui.update { it.copy(error = "Bitrate change failed: ${e.message}") }
            }
        }
    }

    fun setAudio(on: Boolean) {
        controller.settings.update { it.copy(streamAudio = on) }
        audio.enabled = on
        restart.tryEmit("audio") // the PC only captures audio when asked in stream.start
    }

    fun setDirectTouch(on: Boolean) = controller.settings.update { it.copy(streamDirectTouch = on) }

    fun setShowStats(on: Boolean) = controller.settings.update { it.copy(streamShowStats = on) }

    fun requestKeyframe() {
        session?.let { s -> viewModelScope.launch { s.requestKeyframe() } }
    }

    private fun request(): StreamRequest {
        val st = settings.value
        val perPc = controller.prefsFor(controller.session.value?.hostId).streamPreset
        val viaRelay = onRelay
        val choice = com.niranjan.helm.core.RelayStreamPolicy.choose(viaRelay, perPc, st.streamPreset, st.streamBitrateKbps, relayPick, relayKbps)
        val preset = choice.preset
        _ui.update { it.copy(viaRelay = viaRelay, relayChoice = choice.takeIf { viaRelay }) }
        val g = _g3.value
        val g3 = control?.supportsG3 == true
        val m = _ui.value.monitor
        return StreamRequest(
            preset = preset,
            bitrateKbps = choice.kbps,
            audio = st.streamAudio,
            monitorId = m?.id,
            hwnd = g.window?.hwnd,
            gaming = if (g3) g.gaming else null,
            maxSize = if (g.secondScreen && m != null && phoneScreen.first > 0) {
                SecondScreen.mirrorSize(m.width, m.height, phoneScreen.first, phoneScreen.second)
            } else {
                null
            },
        )
    }

    /** Runs while the Screen tab is visible: (re)starts the stream until cancelled. */
    private suspend fun keepStreaming() {
        var failures = 0
        while (viewModelScope.isActive) {
            val c = control
            if (c == null) {
                _ui.update { it.copy(streaming = false, status = "Waiting for the PC…") }
                if (controller.reconnectIfDropped()) _ui.update { it.copy(reconnects = it.reconnects + 1) }
                delay(backoff(failures++))
                continue
            }
            if (!c.supportsPhase2) {
                _ui.update { it.copy(supported = false, streaming = false, status = "Not supported") }
                // Wait until the connection changes (e.g. the agent was updated and we reconnected).
                controller.session.first { it?.connection !== c }
                continue
            }
            _ui.update { it.copy(supported = true, status = "Starting…", error = null) }
            _g3.update { it.copy(supported = c.supportsG3) }
            val s = StreamSession(c, controller::streamSocket, sink, stats = decoderStats)
            session = s
            val started = try {
                val info = s.start(request())
                _ui.update { it.copy(info = info, streaming = true, status = "Live", videoWidth = info.width, videoHeight = info.height) }
                _g3.update { it.copy(inputOnStream = s.inputOnStream) }
                failures = 0
                true
            } catch (e: HelmException) {
                _ui.update { it.copy(streaming = false, status = "Stream failed", error = e.message) }
                false
            }
            if (started) {
                val statsJob = viewModelScope.launch {
                    while (isActive) {
                        delay(1_000)
                        _ui.update {
                            it.copy(
                                stats = decoderStats.last,
                                totalDropped = decoderStats.totalDropped,
                                lowLatency = decoder.lowLatency,
                                codecName = decoder.codecName,
                            )
                        }
                    }
                }
                val reason = merge(
                    s.state.filter { it is StreamState.Failed || it is StreamState.Stopped }.map { "ended" },
                    restart.map { it },
                    c.state.filter { it !is ConnectionState.Ready }.map { "control" },
                ).first()
                statsJob.cancel()
                _g3.update { it.copy(inputOnStream = false) }
                val ended = s.state.value
                if (ended is StreamState.Failed) _ui.update { it.copy(error = ended.message) }
                withContext(NonCancellable) { if (reason == "control") s.abandon() else s.stop() }
                session = null
                _ui.update { it.copy(streaming = false, status = if (reason == "ended") "Reconnecting…" else "Restarting…") }
                if (reason != "ended") continue
                _ui.update { it.copy(reconnects = it.reconnects + 1) }
            } else {
                session = null
                withContext(NonCancellable) { s.abandon() }
            }
            delay(backoff(failures++))
        }
    }

    private fun backoff(n: Int): Long = min(8_000L, 500L shl min(n, 4))

    private fun stopCurrent() {
        val s = session ?: return
        session = null
        _ui.update { it.copy(streaming = false, status = "Stopped") }
        viewModelScope.launch(NonCancellable) { s.stop() }
    }

    override fun onCleared() {
        runCatching { connectivity.unregisterNetworkCallback(netCallback) }
        session?.abandon()
        decoder.release()
        audio.release()
        super.onCleared()
    }
}
