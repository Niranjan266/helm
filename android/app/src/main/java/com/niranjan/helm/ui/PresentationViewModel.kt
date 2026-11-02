package com.niranjan.helm.ui

import android.app.Application
import android.content.Context
import android.os.Build
import android.os.SystemClock
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.LaserFrame
import com.niranjan.helm.core.LaserSmoother
import com.niranjan.helm.core.LaserThrottle
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.core.PresentAction
import com.niranjan.helm.core.PresentFeature
import com.niranjan.helm.core.PresentState
import com.niranjan.helm.core.PresentTimer
import com.niranjan.helm.core.TiltPointer
import com.niranjan.helm.core.presentGo
import com.niranjan.helm.core.presentLaser
import com.niranjan.helm.core.presentState
import com.niranjan.helm.device.PointingSensor
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

enum class LaserMode { TILT, TOUCH }

data class PresentUi(
    val supported: Boolean = false,
    val state: PresentState? = null,
    val monitors: List<MonitorInfo> = emptyList(),
    /** Index into [monitors]; -1 = primary / streamed monitor. */
    val laserMonitor: Int = -1,
    val trail: Boolean = true,
    val laserMode: LaserMode = LaserMode.TILT,
    val pointing: Boolean = false,
    val volumeKeys: Boolean = true,
    val elapsedMs: Long = 0,
    val remainingMs: Long? = null,
    val countdownMin: Int? = null,
    val timerRunning: Boolean = false,
)

/** Presentation screen: polls `present.state`, sends slide keys and laser frames, runs the timer. */
class PresentationViewModel(app: Application) : AndroidViewModel(app) {
    private val controller = (app as HelmApp).controller
    private val _ui = MutableStateFlow(PresentUi())
    val ui: StateFlow<PresentUi> = _ui.asStateFlow()
    private val _messages = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val messages = _messages.asSharedFlow()

    private val conn: HelmConnection? get() = controller.readyConnection()
    private val api: HelmApi? get() = conn?.let { HelmApi(it) }

    private val timer = PresentTimer()
    private val tilt = TiltPointer()
    private val smoother = LaserSmoother()
    private val throttle = LaserThrottle()
    private val sensor = PointingSensor(app) { yaw, pitch -> onTilt(yaw, pitch) }
    private var calibrateNext = false
    private var poll: Job? = null

    val tiltAvailable: Boolean get() = sensor.available

    /** Screen became visible: poll state and tick the timer until [stop]. */
    fun start() {
        val c = conn
        _ui.update { it.copy(supported = c != null && PresentFeature.G2 in c.hostFeatures) }
        if (poll?.isActive == true) return
        poll = viewModelScope.launch {
            loadMonitors()
            var n = 0
            while (isActive) {
                if (n % 4 == 0) refresh()
                tick()
                n++
                delay(250)
            }
        }
    }

    fun stop() {
        poll?.cancel()
        poll = null
        laserUp()
    }

    private suspend fun loadMonitors() {
        val a = api ?: return
        try {
            val list = a.monitors()
            _ui.update { it.copy(monitors = list) }
        } catch (_: HelmException) {
        }
    }

    private suspend fun refresh() {
        val a = api ?: return
        if (!_ui.value.supported) return
        try {
            val s = a.presentState()
            _ui.update { it.copy(state = s) }
        } catch (_: HelmException) {
        }
    }

    private fun tick() {
        val now = SystemClock.elapsedRealtime()
        val alerts = timer.dueAlerts(now)
        _ui.update { it.copy(elapsedMs = timer.elapsedMs(now), remainingMs = timer.remainingMs(now), timerRunning = timer.running) }
        alerts.firstOrNull()?.let { mark ->
            vibrate(PresentTimer.vibration(mark))
            _messages.tryEmit(if (mark <= 0) "Time is up" else "${mark / 60} min left")
        }
    }

    fun go(action: PresentAction) {
        val a = api ?: return
        viewModelScope.launch {
            try {
                a.presentGo(action)
                delay(150)
                refresh()
            } catch (e: HelmException) {
                _messages.tryEmit(e.message ?: "Could not change the slide")
            }
        }
    }

    /** Volume key while the screen is open: up = previous, down = next. */
    fun onVolumeKey(up: Boolean): Boolean {
        if (!_ui.value.volumeKeys || !_ui.value.supported) return false
        go(if (up) PresentAction.PREV else PresentAction.NEXT)
        return true
    }

    fun setVolumeKeys(on: Boolean) = _ui.update { it.copy(volumeKeys = on) }

    // ---- timer ----

    fun timerToggle() {
        val now = SystemClock.elapsedRealtime()
        if (timer.running) timer.pause(now) else timer.start(now)
        tick()
    }

    fun timerReset() {
        timer.reset()
        tick()
    }

    fun setCountdown(minutes: Int?) {
        timer.countdownS = minutes?.let { it * 60 }
        _ui.update { it.copy(countdownMin = minutes) }
        tick()
    }

    private fun vibrate(pattern: LongArray) {
        val ctx = getApplication<Application>()
        val v: Vibrator? = if (Build.VERSION.SDK_INT >= 31) {
            (ctx.getSystemService(Context.VIBRATOR_MANAGER_SERVICE) as? VibratorManager)?.defaultVibrator
        } else {
            @Suppress("DEPRECATION")
            ctx.getSystemService(Context.VIBRATOR_SERVICE) as? Vibrator
        }
        try {
            v?.vibrate(VibrationEffect.createWaveform(pattern, -1))
        } catch (_: SecurityException) {
        }
    }

    // ---- laser ----

    fun setLaserMode(mode: LaserMode) {
        laserUp()
        _ui.update { it.copy(laserMode = mode) }
    }

    fun setTrail(on: Boolean) {
        _ui.update { it.copy(trail = on) }
        pushLaserConfig()
    }

    fun setLaserMonitor(index: Int) {
        _ui.update { it.copy(laserMonitor = index) }
        pushLaserConfig()
    }

    private fun pushLaserConfig() {
        val a = api ?: return
        val u = _ui.value
        val id = u.monitors.getOrNull(u.laserMonitor)?.id
        viewModelScope.launch {
            try {
                a.presentLaser(id, u.trail)
            } catch (e: HelmException) {
                _messages.tryEmit(e.message ?: "Could not set the laser monitor")
            }
        }
    }

    /** Laser button pressed (tilt mode): the current direction becomes the screen centre. */
    fun laserDown() {
        if (_ui.value.pointing) return
        smoother.reset()
        throttle.reset()
        tilt.reset()
        calibrateNext = true
        _ui.update { it.copy(pointing = true) }
        if (_ui.value.laserMode == LaserMode.TILT) {
            sensor.start()
            send(0.5f, 0.5f)
        }
    }

    fun laserUp() {
        sensor.stop()
        if (_ui.value.pointing) conn?.sendInput(LaserFrame.OFF)
        _ui.update { it.copy(pointing = false) }
    }

    /** Touch mode: finger at normalized (x, y) on the pad. */
    fun laserTouch(x: Float, y: Float) {
        if (!_ui.value.pointing) laserDown()
        send(x, y)
    }

    private fun onTilt(yaw: Double, pitch: Double) {
        if (!_ui.value.pointing) return
        if (calibrateNext) {
            tilt.calibrate(yaw, pitch)
            calibrateNext = false
        }
        val (x, y) = tilt.position(yaw, pitch)
        val (sx, sy) = smoother.push(x, y)
        send(sx, sy)
    }

    private fun send(x: Float, y: Float) {
        if (throttle.shouldSend(x, y)) conn?.sendInput(LaserFrame(x, y, true))
    }

    override fun onCleared() {
        stop()
        super.onCleared()
    }
}
