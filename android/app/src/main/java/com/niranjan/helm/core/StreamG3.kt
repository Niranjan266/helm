package com.niranjan.helm.core

import java.util.Base64
import kotlin.math.abs
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// G3 (Docs/protocol/streaming-2.md): window streaming, gaming preset, drag-select input and
// second-screen groundwork. Plain JVM code, unit-tested in G3Test.

object G3 {
    /** `hello.features` name. */
    const val FEATURE = "g3"

    /** Thumbnail size asked for in the window picker (longer side, px). */
    const val THUMB_SIZE = 256
}

val HelmConnection.supportsG3: Boolean get() = G3.FEATURE in hostFeatures

/**
 * Gaming preset (`stream.start{preset:"gaming"}`): lowest-latency encoder settings on the PC,
 * audio always on. The agent falls back to 60 fps when only a software encoder exists.
 */
enum class GamingPreset(val label: String, val maxWidth: Int, val maxHeight: Int, val fps: Int, val kbps: Int) {
    G720_60("Game 720p60", 1280, 720, 60, 12_000),
    G720_120("Game 720p120", 1280, 720, 120, 20_000),
    G1080_60("Game 1080p60", 1920, 1080, 60, 20_000),
    G1080_120("Game 1080p120", 1920, 1080, 120, 30_000),
    ;

    companion object {
        fun byName(name: String?): GamingPreset? = entries.firstOrNull { it.name == name }
    }
}

/** `stream.start` params for the gaming preset (overrides size, fps, bitrate; audio on). */
fun GamingPreset.toParams(monitorId: JsonPrimitive?, hwnd: JsonPrimitive?): JsonObject = buildJsonObject {
    if (hwnd != null) put("hwnd", hwnd) else if (monitorId != null) put("monitor_id", monitorId)
    put("preset", "gaming")
    put("max_width", maxWidth)
    put("max_height", maxHeight)
    put("fps", fps)
    put("bitrate_kbps", StreamPreset.clampKbps(kbps))
    put("audio", true)
}

/** `windows.thumb{hwnd,size}` → PNG bytes, or null for a minimized / hidden window. */
suspend fun HelmApi.windowThumb(hwnd: JsonPrimitive, size: Int = G3.THUMB_SIZE): ByteArray? {
    val r = gated(G3.FEATURE, "windows.thumb", buildJsonObject {
        put("hwnd", hwnd)
        put("size", size.coerceIn(32, 512))
    }, timeoutMs = 10_000)
    return WindowThumb.pngOf(r)
}

object WindowThumb {
    fun pngOf(r: JsonObject): ByteArray? = r.optStr("png")?.let {
        try {
            Base64.getDecoder().decode(it)
        } catch (_: IllegalArgumentException) {
            null
        }
    }

    /** Windows worth offering in the picker: titled, not Helm itself; foreground first. */
    fun pickable(all: List<WindowEntry>): List<WindowEntry> =
        all.filter { it.title.isNotBlank() }
            .sortedWith(compareByDescending<WindowEntry> { it.isForeground }.thenBy { it.isMinimized }.thenBy { it.title.lowercase() })
}

// ---- drag-select (long-press-then-drag) ---------------------------------------------------

/** What the drag-select gesture asks the UI to do. Coordinates are view pixels. */
sealed interface DragAction {
    /** Absolute pointer (direct touch): move the PC cursor under the finger. */
    data class PointAt(val x: Float, val y: Float) : DragAction

    /** Relative pointer (trackpad): move the PC cursor by this many view pixels. */
    data class MoveBy(val dx: Float, val dy: Float) : DragAction

    data class Press(val button: MouseButton) : DragAction
    data class Release(val button: MouseButton) : DragAction
    data class Click(val button: MouseButton) : DragAction

    /** One-finger drag before the long press (direct mode): pan the view / scroll. */
    data class Pan(val dx: Float, val dy: Float) : DragAction

    /** Two-finger movement: scroll (or pan the zoomed view). */
    data class TwoFinger(val dx: Float, val dy: Float) : DragAction

    data object Haptic : DragAction
}

/**
 * Drag mode gesture state machine (pure; time is passed in).
 *
 * * Long press (no movement for [longPressMs]) → the left button goes **down** where the finger
 *   is (direct: `ma` there first); dragging keeps it held (direct: `ma` moves; trackpad: relative
 *   moves); lifting the finger releases it. This is how text and files are drag-selected.
 * * Quick tap → left click. Two-finger tap → right click. Two-finger drag → [DragAction.TwoFinger].
 * * One-finger drag before the long press → [DragAction.Pan] (direct) or cursor move (trackpad).
 */
class DragSelect(
    private val direct: Boolean,
    private val slopPx: Float,
    val longPressMs: Long = 450,
    private val tapMs: Long = 300,
) {
    private enum class State { IDLE, PENDING, HOLDING, MOVING, MULTI }

    private var state = State.IDLE
    private var downT = 0L
    private var downX = 0f
    private var downY = 0f
    private var lastX = 0f
    private var lastY = 0f
    private var travel = 0f
    private var maxPointers = 0

    val holding: Boolean get() = state == State.HOLDING

    /** True while waiting for the long press (the UI should call [tick] when [deadline] passes). */
    val pending: Boolean get() = state == State.PENDING

    val deadline: Long get() = downT + longPressMs

    fun down(x: Float, y: Float, t: Long): List<DragAction> {
        state = State.PENDING
        downT = t
        downX = x
        downY = y
        lastX = x
        lastY = y
        travel = 0f
        maxPointers = 1
        return emptyList()
    }

    fun tick(t: Long): List<DragAction> {
        if (state != State.PENDING || t - downT < longPressMs) return emptyList()
        state = State.HOLDING
        val at = if (direct) listOf(DragAction.PointAt(downX, downY)) else emptyList()
        return at + listOf(DragAction.Press(MouseButton.LEFT), DragAction.Haptic)
    }

    /** One finger moved to (x, y). */
    fun move(x: Float, y: Float, t: Long): List<DragAction> {
        val out = mutableListOf<DragAction>()
        if (state == State.PENDING) out += tick(t)
        val dx = x - lastX
        val dy = y - lastY
        lastX = x
        lastY = y
        travel += abs(dx) + abs(dy)
        when (state) {
            State.PENDING -> if (travel > slopPx) {
                state = State.MOVING
                val tx = x - downX
                val ty = y - downY
                out += if (direct) DragAction.Pan(tx, ty) else DragAction.MoveBy(tx, ty)
            }
            State.MOVING -> out += if (direct) DragAction.Pan(dx, dy) else DragAction.MoveBy(dx, dy)
            State.HOLDING -> out += if (direct) DragAction.PointAt(x, y) else DragAction.MoveBy(dx, dy)
            State.MULTI, State.IDLE -> Unit
        }
        return out
    }

    /** [n] fingers are down now; [dx]/[dy] is the centroid movement since the last call. */
    fun pointers(n: Int, dx: Float = 0f, dy: Float = 0f): List<DragAction> {
        if (n > maxPointers) maxPointers = n
        if (n < 2) return emptyList()
        return when (state) {
            State.PENDING, State.MOVING -> {
                state = State.MULTI
                travel += abs(dx) + abs(dy)
                emptyList()
            }
            State.MULTI -> {
                travel += abs(dx) + abs(dy)
                if (travel > slopPx * 1.5f) listOf(DragAction.TwoFinger(dx, dy)) else emptyList()
            }
            else -> emptyList() // extra fingers while holding are ignored
        }
    }

    /** All fingers lifted. */
    fun up(t: Long): List<DragAction> {
        val s = state
        state = State.IDLE
        return when (s) {
            State.HOLDING -> (if (direct) listOf(DragAction.PointAt(lastX, lastY)) else emptyList()) +
                DragAction.Release(MouseButton.LEFT)
            State.PENDING -> if (t - downT < tapMs || travel <= slopPx) {
                (if (direct) listOf(DragAction.PointAt(downX, downY)) else emptyList()) + DragAction.Click(MouseButton.LEFT)
            } else {
                emptyList()
            }
            State.MULTI -> if (maxPointers == 2 && t - downT < tapMs && travel <= slopPx * 1.5f) {
                (if (direct) listOf(DragAction.PointAt(downX, downY)) else emptyList()) +
                    listOf(DragAction.Haptic, DragAction.Click(MouseButton.RIGHT))
            } else {
                emptyList()
            }
            State.MOVING, State.IDLE -> emptyList()
        }
    }

    /** Gesture cancelled (e.g. the view went away): never leave the button stuck down. */
    fun cancel(): List<DragAction> {
        val s = state
        state = State.IDLE
        return if (s == State.HOLDING) listOf(DragAction.Release(MouseButton.LEFT)) else emptyList()
    }
}

// ---- second screen (mirror one monitor) ---------------------------------------------------

/** Screen rotation while streaming. */
enum class RotationLock(val label: String) {
    AUTO("Rotate freely"),
    MATCH_MONITOR("Match the monitor"),
    LANDSCAPE("Landscape"),
    PORTRAIT("Portrait"),
}

/** What the activity orientation should be set to. */
enum class Orientation { UNSPECIFIED, LANDSCAPE, PORTRAIT }

object SecondScreen {
    /** Resolve [lock] for a monitor of [w]×[h] (unknown size = no lock for MATCH_MONITOR). */
    fun orientation(lock: RotationLock, w: Int, h: Int): Orientation = when (lock) {
        RotationLock.AUTO -> Orientation.UNSPECIFIED
        RotationLock.LANDSCAPE -> Orientation.LANDSCAPE
        RotationLock.PORTRAIT -> Orientation.PORTRAIT
        RotationLock.MATCH_MONITOR -> when {
            w <= 0 || h <= 0 -> Orientation.UNSPECIFIED
            h > w -> Orientation.PORTRAIT
            else -> Orientation.LANDSCAPE
        }
    }

    /** "Monitor 2 · DELL U2723QE 2560×1440" (N is 1-based, in agent order: primary first). */
    fun label(index: Int, m: MonitorInfo): String =
        "Monitor ${index + 1} · ${m.name} ${m.width}×${m.height}${if (m.isPrimary) " (primary)" else ""}"

    /** The monitor a phone mirror should show by default: the first non-primary one. */
    fun defaultMonitor(monitors: List<MonitorInfo>): MonitorInfo? = monitors.firstOrNull { !it.isPrimary } ?: monitors.firstOrNull()

    /**
     * Stream size for mirroring a monitor on a phone screen of [phoneW]×[phoneH] (any
     * orientation): never more than the monitor, and no more than the phone can show once
     * rotated to match (saves bandwidth and encoder time). Even sizes, at least 64.
     */
    fun mirrorSize(monitorW: Int, monitorH: Int, phoneW: Int, phoneH: Int): Pair<Int, Int> {
        val long = maxOf(phoneW, phoneH)
        val short = minOf(phoneW, phoneH)
        val (pw, ph) = if (monitorW >= monitorH) long to short else short to long
        val w = minOf(monitorW, pw).coerceAtLeast(64) and 1.inv()
        val h = minOf(monitorH, ph).coerceAtLeast(64) and 1.inv()
        return w to h
    }
}
