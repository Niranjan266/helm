package com.niranjan.helm.core

import kotlin.math.PI
import kotlin.math.atan
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.tan
import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// Typed calls for Phase 5 (§13, feature `p5`). Each is gated on the hello feature: against an
// older agent it throws `unsupported` without sending anything.

private suspend fun HelmApi.p5(method: String, params: JsonObject = JsonObject(emptyMap()), timeoutMs: Long = 10_000) =
    gated(Features.P5, method, params, timeoutMs)

// Recording
suspend fun HelmApi.recordStart(audio: Boolean): Recording =
    Recording.from(p5("record.start", buildJsonObject { put("audio", audio) }, timeoutMs = 20_000))
        ?: throw HelmException("bad_result", "record.start returned no path")

suspend fun HelmApi.recordStop(): Recording =
    Recording.from(p5("record.stop", timeoutMs = 30_000)) ?: throw HelmException("bad_result", "record.stop returned no path")

// Terminal
object Shells {
    const val POWERSHELL = "powershell"
    const val CMD = "cmd"
    val ALL = listOf(POWERSHELL, CMD)
}

/** Opens a ConPTY session; returns its `term_id` (as text). */
suspend fun HelmApi.termOpen(shell: String, cols: Int, rows: Int): String {
    require(shell in Shells.ALL) { "bad shell $shell" }
    val r = p5("term.open", buildJsonObject { put("shell", shell); put("cols", cols); put("rows", rows) }, timeoutMs = 20_000)
    return TermEvent.idOf(r) ?: throw HelmException("bad_result", "term.open returned no term_id")
}

/** Wire value for a term id: numbers go back as numbers. */
private fun termIdJson(id: String): JsonPrimitive = id.toLongOrNull()?.let { JsonPrimitive(it) } ?: JsonPrimitive(id)

suspend fun HelmApi.termWrite(termId: String, text: String) {
    if (text.isEmpty()) return
    p5("term.write", buildJsonObject { put("term_id", termIdJson(termId)); put("data_b64", TermKeys.b64(text)) })
}

suspend fun HelmApi.termResize(termId: String, cols: Int, rows: Int) {
    p5("term.resize", buildJsonObject { put("term_id", termIdJson(termId)); put("cols", cols); put("rows", rows) })
}

suspend fun HelmApi.termClose(termId: String) {
    p5("term.close", buildJsonObject { put("term_id", termIdJson(termId)) })
}

// Notifications
suspend fun HelmApi.notifyPush(n: NotifyPush) {
    p5("notify.push", n.toJson())
}

// Plugins
suspend fun HelmApi.plugins(): List<PluginInfo> = PluginInfo.listFrom(p5("plugins.list"))

suspend fun HelmApi.setPlugin(id: String, enabled: Boolean, granted: Collection<String>) {
    p5("plugins.set", PluginInfo.setParams(id, enabled, granted))
}

// Triggers
suspend fun HelmApi.triggers(): List<Trigger> = Trigger.listFrom(p5("triggers.get"))

suspend fun HelmApi.setTriggers(list: List<Trigger>) {
    p5("triggers.set", Trigger.encode(list))
}

// Home Assistant
suspend fun HelmApi.haConfigSet(url: String, token: String) {
    p5("ha.config.set", buildJsonObject { put("url", url.trim().trimEnd('/')); put("token", token.trim()) })
}

suspend fun HelmApi.haStates(): List<HaState> = HaState.listFrom(p5("ha.states", timeoutMs = 20_000))

/** Stylus maths for `pn` frames (§13). */
object PenMath {
    /**
     * Android stylus axes → Windows tilt in degrees. [tiltRad] is the angle from perpendicular
     * (`AXIS_TILT`, 0..π/2), [orientationRad] the direction the pen points (`AXIS_ORIENTATION`,
     * 0 = up/away from the user, +π/2 = right). Returns (tilt_x, tilt_y): x positive when the
     * pen top leans right, y positive when it leans towards the user.
     */
    fun tilt(tiltRad: Float, orientationRad: Float): Pair<Float, Float> {
        if (!tiltRad.isFinite() || !orientationRad.isFinite() || tiltRad <= 0f) return 0f to 0f
        val t = tan(tiltRad.coerceAtMost((PI / 2 - 1e-3).toFloat()).toDouble())
        val tx = Math.toDegrees(atan(t * sin(orientationRad.toDouble()))).toFloat()
        val ty = Math.toDegrees(atan(-t * cos(orientationRad.toDouble()))).toFloat()
        return tx.coerceIn(-90f, 90f) to ty.coerceIn(-90f, 90f)
    }

    fun frame(
        x: Float,
        y: Float,
        pressure: Float,
        tiltRad: Float,
        orientationRad: Float,
        state: PenState,
        eraser: Boolean,
    ): InputFrame.Pen {
        val (tx, ty) = tilt(tiltRad, orientationRad)
        val p = if (pressure.isFinite()) pressure else 0f
        return InputFrame.Pen(x, y, p, tx, ty, state, eraser)
    }
}
