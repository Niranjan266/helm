package com.niranjan.helm.core

import java.io.ByteArrayOutputStream
import java.util.ArrayDeque
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock
import kotlin.concurrent.withLock
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.roundToInt
import kotlinx.serialization.json.JsonObject

// g9: the PC views and controls this phone (Docs/protocol/phone-control.md). Plain JVM code: the
// wire forms, the consent state machine, and the gesture and text plans the Android services run.

object PhoneControl {
    const val FEATURE = "g9"
    const val EV_REQUEST = "phone.request"
    const val EV_CANCEL = "phone.cancel"
    const val EV_IDR = "phone.idr"
    const val EV_IN = "phone.in"

    const val REASON_DECLINED = "declined"
    const val REASON_CAPTURE_DENIED = "capture_denied"
    const val REASON_DISABLED = "disabled"
    const val REASON_BUSY = "busy"
    const val REASON_STOPPED = "stopped_on_phone"
    const val REASON_LOCKED = "screen_locked"
    const val REASON_IDLE = "idle"
    const val REASON_CAPTURE_STOPPED = "capture_stopped"
    const val REASON_ENCODER = "encoder_failed"

    /** No input from the PC for this long ends the session. */
    const val IDLE_TIMEOUT_MS = 10 * 60_000L
}

val HelmConnection.supportsPhoneControl: Boolean get() = PhoneControl.FEATURE in hostFeatures

/** `phone.request`: a PC asks to see this phone. */
data class PhoneRequest(
    val requestId: Long,
    val pcName: String,
    val relayed: Boolean,
    val maxLong: Int,
    val maxShort: Int,
    val fps: Int,
    val bitrateKbps: Int,
) {
    companion object {
        fun from(d: JsonObject): PhoneRequest? {
            val id = d.optLong("request_id") ?: return null
            return PhoneRequest(
                requestId = id,
                pcName = d.optStr("pc_name")?.takeIf { it.isNotBlank() } ?: "Your PC",
                relayed = d.optBool("relayed") ?: false,
                maxLong = (d.optLong("max_long") ?: 1920L).toInt().coerceIn(320, 4096),
                maxShort = (d.optLong("max_short") ?: 1080L).toInt().coerceIn(240, 4096),
                fps = (d.optLong("fps") ?: 30L).toInt().coerceIn(1, 60),
                bitrateKbps = (d.optLong("bitrate_kbps") ?: 6000L).toInt().coerceIn(200, 50_000),
            )
        }
    }
}

// ---------------------------------------------------------------- input events (PC -> phone)

/** One `phone.in` event. Coordinates are normalised 0..1 to the picture being sent. */
sealed interface PhoneIn {
    data class Tap(val x: Double, val y: Double) : PhoneIn
    data class LongPress(val x: Double, val y: Double) : PhoneIn
    data class Swipe(val x1: Double, val y1: Double, val x2: Double, val y2: Double, val ms: Long) : PhoneIn
    data class Down(val x: Double, val y: Double) : PhoneIn
    data class Move(val x: Double, val y: Double) : PhoneIn
    data class Up(val x: Double, val y: Double) : PhoneIn
    data class Scroll(val x: Double, val y: Double, val dy: Double, val dx: Double) : PhoneIn
    data class Global(val action: GlobalAction) : PhoneIn
    data class Text(val text: String) : PhoneIn
    data class Key(val key: EditKey) : PhoneIn

    enum class GlobalAction { BACK, HOME, RECENTS, NOTIFICATIONS }
    enum class EditKey { BACKSPACE, ENTER }

    companion object {
        private fun JsonObject.unit(k: String): Double? =
            optDouble(k)?.takeIf { it.isFinite() }?.coerceIn(0.0, 1.0)

        private fun JsonObject.notches(k: String): Double? =
            (optDouble(k) ?: 0.0).takeIf { it.isFinite() }?.coerceIn(-20.0, 20.0)

        /** Null for anything unknown or malformed (it is then simply not performed). */
        fun parse(d: JsonObject): PhoneIn? {
            val k = d.optStr("k") ?: return null
            if (k in POINT_KINDS) {
                val x = d.unit("x") ?: return null
                val y = d.unit("y") ?: return null
                return when (k) {
                    "tap" -> Tap(x, y)
                    "long" -> LongPress(x, y)
                    "down" -> Down(x, y)
                    "move" -> Move(x, y)
                    else -> Up(x, y)
                }
            }
            return when (k) {
                "swipe" -> Swipe(
                    d.unit("x1") ?: return null,
                    d.unit("y1") ?: return null,
                    d.unit("x2") ?: return null,
                    d.unit("y2") ?: return null,
                    (d.optLong("ms") ?: 300L).coerceIn(1L, 5_000L),
                )
                "scroll" -> Scroll(
                    d.unit("x") ?: return null,
                    d.unit("y") ?: return null,
                    d.notches("dy") ?: return null,
                    d.notches("dx") ?: return null,
                )
                "global" -> when (d.optStr("action")) {
                    "back" -> Global(GlobalAction.BACK)
                    "home" -> Global(GlobalAction.HOME)
                    "recents" -> Global(GlobalAction.RECENTS)
                    "notifications" -> Global(GlobalAction.NOTIFICATIONS)
                    else -> null
                }
                "text" -> d.optStr("text")?.takeIf { it.isNotEmpty() }?.let { Text(it) }
                "key" -> when (d.optStr("key")) {
                    "backspace" -> Key(EditKey.BACKSPACE)
                    "enter" -> Key(EditKey.ENTER)
                    else -> null
                }
                else -> null
            }
        }

        private val POINT_KINDS = setOf("tap", "long", "down", "move", "up")
    }
}

// ---------------------------------------------------------------- consent state machine

sealed interface PhoneStage {
    data object Idle : PhoneStage

    /** The consent screen is up. */
    data class Asking(val request: PhoneRequest) : PhoneStage

    /** The person said yes; Android's capture prompt, encoder and socket are coming up. */
    data class Capturing(val request: PhoneRequest, val wantControl: Boolean) : PhoneStage

    /** Frames flow. [control] = granted at consent and the accessibility service is on. */
    data class Live(val request: PhoneRequest, val wantControl: Boolean, val control: Boolean) : PhoneStage
}

enum class RequestDecision { ASK, DECLINE_DISABLED, DECLINE_BUSY }

/**
 * The rules. Each session goes Idle -> Asking -> Capturing -> Live -> Idle and nothing survives
 * the return to Idle, so a later request is always asked again: there is no way to auto-accept or
 * to remember a yes. Input may be performed only in [PhoneStage.Live] with control granted.
 */
class PhoneSessionLogic {
    @get:Synchronized
    var stage: PhoneStage = PhoneStage.Idle
        private set

    @Synchronized
    fun onRequest(request: PhoneRequest, enabled: Boolean): RequestDecision {
        if (!enabled) return RequestDecision.DECLINE_DISABLED
        if (stage !is PhoneStage.Idle) return RequestDecision.DECLINE_BUSY
        stage = PhoneStage.Asking(request)
        return RequestDecision.ASK
    }

    /** The person accepted [requestId]. False when that request is no longer the one asked. */
    @Synchronized
    fun onAccepted(requestId: Long, wantControl: Boolean): Boolean {
        val s = stage as? PhoneStage.Asking ?: return false
        if (s.request.requestId != requestId) return false
        stage = PhoneStage.Capturing(s.request, wantControl)
        return true
    }

    @Synchronized
    fun onLive(requestId: Long, accessibilityOn: Boolean): Boolean {
        val s = stage as? PhoneStage.Capturing ?: return false
        if (s.request.requestId != requestId) return false
        stage = PhoneStage.Live(s.request, s.wantControl, s.wantControl && accessibilityOn)
        return true
    }

    /** The accessibility service came or went. Returns the new control flag when it changed. */
    @Synchronized
    fun onAccessibility(on: Boolean): Boolean? {
        val s = stage as? PhoneStage.Live ?: return null
        val control = s.wantControl && on
        if (control == s.control) return null
        stage = s.copy(control = control)
        return control
    }

    /** Back to nothing. The request it returns is what just ended (null if nothing did). */
    @Synchronized
    fun end(requestId: Long? = null): PhoneRequest? {
        val current = when (val s = stage) {
            is PhoneStage.Asking -> s.request
            is PhoneStage.Capturing -> s.request
            is PhoneStage.Live -> s.request
            PhoneStage.Idle -> return null
        }
        if (requestId != null && current.requestId != requestId) return null
        stage = PhoneStage.Idle
        return current
    }

    /** The gate for every PC input event. */
    @Synchronized
    fun mayInject(): Boolean = (stage as? PhoneStage.Live)?.control == true
}

/** Ends the session after [timeoutMs] without PC input. Time is passed in (tests). */
class IdleWatchdog(private val timeoutMs: Long, now: Long) {
    private var last = now

    @Synchronized
    fun touch(now: Long) {
        last = now
    }

    @Synchronized
    fun expired(now: Long): Boolean = now - last >= timeoutMs
}

// ---------------------------------------------------------------- picture size

data class PhoneSize(val width: Int, val height: Int)

object PhoneScale {
    private fun even(v: Double): Int = max(16, v.roundToInt() and 1.inv())

    /**
     * Fits the screen into what the PC asked for, in either orientation, without upscaling.
     * Encoders need even sides.
     */
    fun fit(width: Int, height: Int, maxLong: Int, maxShort: Int): PhoneSize {
        val long = max(width, height).toDouble()
        val short = min(width, height).toDouble()
        if (long <= 0 || short <= 0) return PhoneSize(16, 16)
        val scale = minOf(1.0, maxLong / long, maxShort / short)
        return PhoneSize(even(width * scale), even(height * scale))
    }

    /** Fallback for encoders that only take multiples of 16. */
    fun align16(s: PhoneSize): PhoneSize = PhoneSize(max(16, s.width and 15.inv()), max(16, s.height and 15.inv()))
}

// ---------------------------------------------------------------- frames (phone -> PC)

object PhoneFrames {
    /** Part of an access unit too big for one WebSocket message; the last piece is a normal frame. */
    const val KIND_PART: Byte = 0x06

    /** The agent accepts 1 MiB messages from phones; stay clear of it. */
    const val MAX_PAYLOAD = 900 * 1024

    /** One access unit as the frames to send, in order (`[kind][pts u64 BE][payload]`, §10). */
    fun fragments(ptsUs: Long, au: ByteArray, key: Boolean, maxPayload: Int = MAX_PAYLOAD): List<ByteArray> {
        if (au.size <= maxPayload) return listOf(StreamFrames.video(ptsUs, au, key))
        val out = ArrayList<ByteArray>(au.size / maxPayload + 1)
        var at = 0
        while (au.size - at > maxPayload) {
            out += StreamFrames.encode(KIND_PART, ptsUs, au.copyOfRange(at, at + maxPayload))
            at += maxPayload
        }
        out += StreamFrames.video(ptsUs, au.copyOfRange(at, au.size), key)
        return out
    }

    /** §10: a keyframe carries its SPS / PPS. MediaCodec hands them over separately. */
    fun keyUnit(config: ByteArray?, au: ByteArray): ByteArray {
        if (config == null || AnnexB.containsSps(au)) return au
        return ByteArrayOutputStream(config.size + au.size).apply {
            write(config)
            write(au)
        }.toByteArray()
    }
}

/** One access unit waiting for the socket. */
class QueuedAu(val ptsUs: Long, val data: ByteArray, val key: Boolean)

/** What [PhoneSendQueue.pop] returns: the unit, and whether to ask the encoder for a keyframe. */
class PoppedAu(val au: QueuedAu, val requestKeyframe: Boolean)

/**
 * Between encoder and socket. At most [MAX_QUEUED] units wait. A delta that does not fit is
 * dropped, and so is every delta after it until a keyframe (they reference the lost picture). A
 * keyframe always gets in, replacing what waits when full. When the queue drains after a drop,
 * the pop says to request a keyframe (once).
 */
class PhoneSendQueue {
    private val lock = ReentrantLock()
    private val changed = lock.newCondition()
    private val q = ArrayDeque<QueuedAu>()
    private var waitingKey = false
    private var keyWanted = false
    private var closed = false

    var dropped = 0L
        get() = lock.withLock { field }
        private set

    fun push(au: QueuedAu): Boolean = lock.withLock {
        if (closed) return false
        if (au.key) {
            if (q.size >= MAX_QUEUED) {
                dropped += q.size
                q.clear()
            }
            q.addLast(au)
            waitingKey = false
            keyWanted = false
            changed.signalAll()
            return true
        }
        if (waitingKey || q.size >= MAX_QUEUED) {
            dropped++
            if (!waitingKey) {
                waitingKey = true
                keyWanted = true
            }
            return false
        }
        q.addLast(au)
        changed.signalAll()
        true
    }

    fun pop(): PoppedAu? = lock.withLock {
        val au = q.pollFirst() ?: return null
        val ask = q.isEmpty() && keyWanted
        if (ask) keyWanted = false
        PoppedAu(au, ask)
    }

    /** Blocks until a unit is there or the queue was closed (null). */
    fun take(): PoppedAu? = lock.withLock {
        while (!closed) {
            pop()?.let { return it }
            changed.await(250, TimeUnit.MILLISECONDS)
        }
        null
    }

    fun close() = lock.withLock {
        closed = true
        q.clear()
        changed.signalAll()
    }

    companion object {
        const val MAX_QUEUED = 3
    }
}

/** Keyframes are expensive; several requests in a row get one. */
class KeyframeLimiter(private val minGapMs: Long = 500) {
    private var last = Long.MIN_VALUE / 2

    @Synchronized
    fun allow(now: Long): Boolean {
        if (now - last < minGapMs) return false
        last = now
        return true
    }
}

// ---------------------------------------------------------------- gestures

data class Pt(val x: Float, val y: Float)

/** One `StrokeDescription`: pixel path, duration, and how it chains to the next / previous one. */
data class Stroke(val points: List<Pt>, val durationMs: Long, val willContinue: Boolean, val continues: Boolean)

/** The real display right now. Normalised 0..1 maps onto it, whatever its rotation. */
data class ScreenBox(val width: Int, val height: Int) {
    fun px(nx: Double, ny: Double): Pt = Pt(
        (nx.coerceIn(0.0, 1.0) * (width - 1)).toFloat(),
        (ny.coerceIn(0.0, 1.0) * (height - 1)).toFloat(),
    )
}

object GesturePlanner {
    const val TAP_MS = 50L
    const val LONG_PRESS_MS = 700L

    /** One wheel notch moves the finger this share of the screen. */
    const val NOTCH_SHARE = 0.12

    fun tap(b: ScreenBox, x: Double, y: Double) = Stroke(listOf(b.px(x, y)), TAP_MS, willContinue = false, continues = false)

    fun longPress(b: ScreenBox, x: Double, y: Double) =
        Stroke(listOf(b.px(x, y)), LONG_PRESS_MS, willContinue = false, continues = false)

    fun swipe(b: ScreenBox, s: PhoneIn.Swipe) =
        Stroke(listOf(b.px(s.x1, s.y1), b.px(s.x2, s.y2)), s.ms, willContinue = false, continues = false)

    /** How far a finger moves for [notches], at most most of the screen. */
    private fun travel(notches: Double, size: Int): Double =
        (notches * NOTCH_SHARE * size).coerceIn(-0.8 * size, 0.8 * size)

    /** The finger goes from the result to the result minus [travel]; both have to be on screen. */
    private fun startFor(at: Float, travel: Double, size: Int): Double {
        val low = max(1.0, 1.0 + travel)
        val high = min(size - 2.0, size - 2.0 + travel)
        return at.toDouble().coerceIn(low, max(low, high))
    }

    /**
     * Wheel down (dy > 0) shows what is below, so the finger moves up. The start point is moved
     * away from the edge when the swipe would not fit on the screen. Null when nothing moves.
     */
    fun scroll(b: ScreenBox, s: PhoneIn.Scroll): Stroke? {
        val mx = travel(s.dx, b.width)
        val my = travel(s.dy, b.height)
        if (abs(mx) < 1 && abs(my) < 1) return null
        val start = b.px(s.x, s.y)
        val sx = startFor(start.x, mx, b.width)
        val sy = startFor(start.y, my, b.height)
        val ms = 150L + (160 * max(abs(mx) / b.width, abs(my) / b.height)).toLong()
        return Stroke(
            listOf(Pt(sx.toFloat(), sy.toFloat()), Pt((sx - mx).toFloat(), (sy - my).toFloat())),
            ms,
            willContinue = false,
            continues = false,
        )
    }
}

/**
 * A live drag (`down`, `move`s, `up`) as chained strokes. Android runs one gesture at a time, so
 * moves that arrive during a stroke are joined into the next one. [next] gives the stroke to
 * dispatch whenever none is running; a continued stroke starts where the last one ended.
 */
class DragPlanner {
    private var last: Pt? = null
    private var begin = false
    private var ending = false
    private val pending = ArrayList<Pt>()
    private var lastAt = 0L

    val active: Boolean get() = last != null

    fun down(p: Pt, now: Long) {
        pending.clear()
        last = p
        begin = true
        ending = false
        lastAt = now
    }

    fun move(p: Pt) {
        if (last != null && !ending) pending += p
    }

    fun up(p: Pt) {
        if (last == null) return
        pending += p
        ending = true
    }

    /** Lift the finger where it is (session over, or no word from the PC for too long). */
    fun release() {
        if (last != null) ending = true
    }

    fun forget() {
        last = null
        begin = false
        ending = false
        pending.clear()
    }

    /** No stroke has been planned for [ms]: the PC may have lost track of this finger. */
    fun heldLongerThan(ms: Long, now: Long): Boolean = last != null && now - lastAt >= ms

    fun next(now: Long): Stroke? {
        val from = last ?: return null
        if (begin) {
            begin = false
            lastAt = now
            val endsNow = ending && pending.isEmpty()
            if (endsNow) forget()
            return Stroke(listOf(from), MIN_STROKE_MS, willContinue = !endsNow, continues = false)
        }
        if (pending.isEmpty() && !ending) return null
        val points = ArrayList<Pt>(pending.size + 1)
        points += from
        points += pending
        val end = pending.lastOrNull() ?: from
        val finishing = ending
        val ms = (now - lastAt).coerceIn(MIN_STROKE_MS, MAX_STROKE_MS)
        lastAt = now
        pending.clear()
        if (finishing) forget() else last = end
        return Stroke(points, ms, willContinue = !finishing, continues = true)
    }

    companion object {
        const val MIN_STROKE_MS = 16L
        const val MAX_STROKE_MS = 120L
    }
}

// ---------------------------------------------------------------- text

/**
 * What the accessibility service may look at on the focused node: flags and offsets, never its
 * text. [nodeKey] tells one field from another.
 */
data class FieldSnapshot(
    val nodeKey: Int,
    val editable: Boolean,
    val password: Boolean,
    val selectionStart: Int,
    val selectionEnd: Int,
    val showingHint: Boolean,
)

/** What the PC typed into a field so far (the only field text the phone keeps, in memory). */
data class TypedText(val nodeKey: Int, val value: String)

sealed interface TextPlan {
    data object NoField : TextPlan

    /** A password field: not typed into; the PC is told. */
    data object Blocked : TextPlan

    /** `ACTION_SET_TEXT` with [value]: the whole field becomes this. */
    data class SetText(val value: String) : TextPlan

    /** Clipboard + `ACTION_PASTE`: goes in at the cursor, so it cannot overwrite anything. */
    data class Paste(val text: String) : TextPlan

    /** Backspace with nothing of ours to edit: select one character before the cursor, cut it. */
    data class CutBefore(val selectionEnd: Int) : TextPlan

    data object Skip : TextPlan
}

/**
 * Helm never reads what is in a field, and `ACTION_SET_TEXT` replaces all of it. So it is used
 * when the field is known to be empty (showing its hint) or holds exactly what the PC typed; in
 * every other case the text is pasted at the cursor.
 */
object TextPlanner {
    private fun ours(f: FieldSnapshot, typed: TypedText?): TypedText? =
        typed?.takeIf { it.nodeKey == f.nodeKey && (f.selectionEnd < 0 || f.selectionEnd == it.value.length) }

    fun text(f: FieldSnapshot?, text: String, typed: TypedText?): TextPlan {
        if (f == null || !f.editable) return TextPlan.NoField
        if (f.password) return TextPlan.Blocked
        ours(f, typed)?.let { return TextPlan.SetText(it.value + text) }
        if (f.showingHint) return TextPlan.SetText(text)
        return TextPlan.Paste(text)
    }

    fun backspace(f: FieldSnapshot?, typed: TypedText?): TextPlan {
        if (f == null || !f.editable) return TextPlan.NoField
        if (f.password) return TextPlan.Blocked
        ours(f, typed)?.let { if (it.value.isNotEmpty()) return TextPlan.SetText(dropLastChar(it.value)) }
        if (f.selectionEnd > 0) return TextPlan.CutBefore(f.selectionEnd)
        return TextPlan.Skip
    }

    /** Whole code points: never leaves half of a surrogate pair behind. */
    fun dropLastChar(s: String): String = if (s.isEmpty()) s else s.substring(0, s.offsetByCodePoints(s.length, -1))
}
