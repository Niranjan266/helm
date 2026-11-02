package com.niranjan.helm.core

import kotlin.math.PI
import kotlin.math.abs
import kotlin.math.asin
import kotlin.math.atan2
import kotlin.math.sqrt
import kotlinx.serialization.json.JsonNull
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// Presentation mode (Docs/protocol/presentation.md, hello feature `g2`): slide keys, PowerPoint
// slide + notes, laser pointer (`lp` input frames), talk timer. Plain Kotlin, unit-tested.

object PresentFeature {
    const val G2 = "g2"
}

/** `present.go` actions. */
enum class PresentAction(val wire: String) {
    NEXT("next"), PREV("prev"), FIRST("first"), LAST("last"), BLACK("black"), START("start"), END("end"),
}

/** PowerPoint's current slide (automation on the PC). */
data class SlideInfo(val index: Int, val count: Int, val notes: String, val showing: Boolean) {
    /** "3 / 12", or "– / 12" when PowerPoint shows a black/end screen. */
    val label: String get() = (if (index > 0) "$index" else "–") + " / $count"
}

/** `present.state` result. */
data class PresentState(
    /** "powerpoint", "slides", "pdf" or null (nothing known in front). */
    val app: String?,
    val title: String?,
    val slide: SlideInfo?,
    /** "ok", "no_presentation", "unsupported" or "busy". */
    val slideStatus: String,
    val laserVisible: Boolean,
) {
    val appLabel: String get() = when (app) {
        "powerpoint" -> "PowerPoint"
        "slides" -> "Google Slides"
        "pdf" -> "PDF viewer"
        else -> "No presentation app in front"
    }

    companion object {
        fun from(o: JsonObject): PresentState {
            val s = o["slide"] as? JsonObject
            val slide = s?.let {
                SlideInfo(
                    (it.optLong("index") ?: 0).toInt(),
                    (it.optLong("count") ?: 0).toInt(),
                    it.optStr("notes") ?: "",
                    it.optBool("showing") == true,
                )
            }
            val laser = o["laser"] as? JsonObject
            return PresentState(
                app = o.optStr("app"),
                title = o.optStr("title"),
                slide = slide,
                slideStatus = o.optStr("slide_status") ?: "unsupported",
                laserVisible = laser?.optBool("visible") == true,
            )
        }
    }
}

/** `lp` laser frame: [x], [y] in 0..1 of the laser monitor; `on=false` hides the dot. */
data class LaserFrame(val x: Float, val y: Float, val on: Boolean) : InputFrame {
    override val kind get() = "lp"
    override fun toJson(): JsonObject = buildJsonObject {
        put("type", "in")
        put("k", kind)
        if (on) {
            put("x", InputFrame.round4(x.coerceIn(0f, 1f)))
            put("y", InputFrame.round4(y.coerceIn(0f, 1f)))
        }
        put("on", on)
    }

    companion object {
        val OFF = LaserFrame(0f, 0f, false)
    }
}

private suspend fun HelmApi.g2(method: String, params: JsonObject = JsonObject(emptyMap())) =
    gated(PresentFeature.G2, method, params)

suspend fun HelmApi.presentState(): PresentState = PresentState.from(g2("present.state"))

/** Returns the key combination the PC pressed. */
suspend fun HelmApi.presentGo(action: PresentAction): String? =
    g2("present.go", buildJsonObject { put("action", action.wire) }).optStr("keys")

/** Choose the laser monitor (`null` = primary / streamed) and the trail. */
suspend fun HelmApi.presentLaser(monitorId: JsonPrimitive?, trail: Boolean) {
    g2("present.laser", buildJsonObject {
        put("monitor_id", monitorId ?: JsonNull)
        put("trail", trail)
    })
}

/**
 * Laser by tilt: the direction the phone's top edge points (from the rotation-vector sensor)
 * relative to where it pointed when the laser button was pressed. Turning right moves the dot
 * right, tilting the top up moves it up. [spanYawDeg] / [spanPitchDeg] = turn for the full
 * width / height of the screen.
 */
class TiltPointer(private val spanYawDeg: Float = 40f, private val spanPitchDeg: Float = 25f) {
    private var yaw0 = 0.0
    private var pitch0 = 0.0
    private var calibrated = false

    val isCalibrated: Boolean get() = calibrated

    /** The current direction becomes the centre of the screen. */
    fun calibrate(yaw: Double, pitch: Double) {
        yaw0 = yaw
        pitch0 = pitch
        calibrated = true
    }

    fun reset() {
        calibrated = false
    }

    /** Normalized (x, y) for the direction (radians); centre until calibrated. */
    fun position(yaw: Double, pitch: Double): Pair<Float, Float> {
        if (!calibrated) return 0.5f to 0.5f
        val dYaw = Math.toDegrees(wrap(yaw - yaw0))
        val dPitch = Math.toDegrees(pitch - pitch0)
        val x = 0.5 + dYaw / spanYawDeg
        val y = 0.5 - dPitch / spanPitchDeg
        return x.toFloat().coerceIn(0f, 1f) to y.toFloat().coerceIn(0f, 1f)
    }

    companion object {
        /** Angle into -π..π. */
        fun wrap(a: Double): Double {
            var v = a % (2 * PI)
            if (v > PI) v -= 2 * PI
            if (v < -PI) v += 2 * PI
            return v
        }

        /**
         * Rotation-vector sensor values (x, y, z[, w]) → 3×3 row-major device→world matrix
         * (world: x east, y north, z up), like SensorManager.getRotationMatrixFromVector.
         */
        fun matrixFromRotationVector(v: FloatArray): DoubleArray {
            val x = v[0].toDouble()
            val y = v[1].toDouble()
            val z = v[2].toDouble()
            val w = if (v.size >= 4) v[3].toDouble() else sqrt((1 - x * x - y * y - z * z).coerceAtLeast(0.0))
            return doubleArrayOf(
                1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w),
                2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w),
                2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y),
            )
        }

        /**
         * Where the phone's top edge (device +y) points: (yaw, pitch) in radians. Yaw is the
         * compass heading (clockwise from north), pitch the elevation above the horizon.
         */
        fun pointing(r: DoubleArray): Pair<Double, Double> {
            val ex = r[1]
            val ny = r[4]
            val up = r[7]
            return atan2(ex, ny) to asin(up.coerceIn(-1.0, 1.0))
        }
    }
}

/** Exponential smoothing for the laser position (tilt jitter). */
class LaserSmoother(private val alpha: Float = 0.45f) {
    private var x = Float.NaN
    private var y = Float.NaN

    fun reset() {
        x = Float.NaN
        y = Float.NaN
    }

    fun push(nx: Float, ny: Float): Pair<Float, Float> {
        if (x.isNaN()) {
            x = nx
            y = ny
        } else {
            x += alpha * (nx - x)
            y += alpha * (ny - y)
        }
        return x to y
    }
}

/** Drops laser frames that would not move the dot visibly (≈ 0.1 % of the screen). */
class LaserThrottle(private val minDelta: Float = 0.001f) {
    private var lastX = Float.NaN
    private var lastY = Float.NaN

    fun reset() {
        lastX = Float.NaN
        lastY = Float.NaN
    }

    fun shouldSend(x: Float, y: Float): Boolean {
        if (!lastX.isNaN() && abs(x - lastX) < minDelta && abs(y - lastY) < minDelta) return false
        lastX = x
        lastY = y
        return true
    }
}

/** Touch-drag laser: a finger at (px, py) on a pad of size (w, h) → normalized position. */
fun padToLaser(px: Float, py: Float, w: Float, h: Float): Pair<Float, Float> {
    if (w <= 0f || h <= 0f) return 0.5f to 0.5f
    return (px / w).coerceIn(0f, 1f) to (py / h).coerceIn(0f, 1f)
}

/**
 * Talk timer: elapsed time since start (pausable) and an optional countdown. [alertsAtS] are
 * remaining-time marks (seconds) that should buzz once each as the countdown passes them.
 */
class PresentTimer(var countdownS: Int? = null, val alertsAtS: List<Int> = listOf(300, 60, 0)) {
    private var startedAt: Long? = null
    private var accumulatedMs = 0L
    private val fired = mutableSetOf<Int>()

    val running: Boolean get() = startedAt != null

    fun start(nowMs: Long) {
        if (startedAt == null) startedAt = nowMs
    }

    fun pause(nowMs: Long) {
        startedAt?.let { accumulatedMs += nowMs - it }
        startedAt = null
    }

    fun reset() {
        startedAt = null
        accumulatedMs = 0
        fired.clear()
    }

    fun elapsedMs(nowMs: Long): Long = accumulatedMs + (startedAt?.let { nowMs - it } ?: 0L)

    /** Remaining countdown (negative = overtime), or null without a countdown. */
    fun remainingMs(nowMs: Long): Long? = countdownS?.let { it * 1000L - elapsedMs(nowMs) }

    /**
     * Marks (seconds) crossed since the last call, each reported once. Marks above the
     * countdown length never fire.
     */
    fun dueAlerts(nowMs: Long): List<Int> {
        val total = countdownS ?: return emptyList()
        val remaining = remainingMs(nowMs) ?: return emptyList()
        val due = alertsAtS.filter { it < total && it !in fired && remaining <= it * 1000L }
        fired += due
        return due.sortedDescending()
    }

    companion object {
        /** "m:ss" or "h:mm:ss"; negative values get a leading "-". */
        fun format(ms: Long): String {
            val neg = ms < 0
            val s = abs(ms) / 1000
            val body = if (s >= 3600) "%d:%02d:%02d".format(s / 3600, (s % 3600) / 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
            return if (neg) "-$body" else body
        }

        /** Vibration pattern for a mark: longer for the final one. */
        fun vibration(markS: Int): LongArray = when {
            markS <= 0 -> longArrayOf(0, 600, 200, 600, 200, 600)
            markS <= 60 -> longArrayOf(0, 300, 150, 300)
            else -> longArrayOf(0, 250)
        }
    }
}
