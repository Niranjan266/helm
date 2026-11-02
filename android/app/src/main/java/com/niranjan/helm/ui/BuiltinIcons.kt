package com.niranjan.helm.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import com.niranjan.helm.core.IconRef

/** A picture for a deck button or dock tile: real app icon, uploaded image, built-in or text. */
@Composable
fun IconView(ref: IconRef, label: String, apps: IconLoader, custom: IconLoader, size: Dp, tint: Color = MaterialTheme.colorScheme.onSurface) {
    when (ref) {
        is IconRef.App -> AppIcon(ref.appId, label, apps, size)
        is IconRef.Custom -> {
            val bmp by produceState<Bitmap?>(custom.peek(ref.id), ref.id) {
                if (value == null) value = custom.load(ref.id)
            }
            val b = bmp
            if (b != null) {
                Image(b.asImageBitmap(), contentDescription = null, modifier = Modifier.size(size).clip(RoundedCornerShape(size / 6)))
            } else {
                LetterIcon(label, size)
            }
        }
        is IconRef.Builtin -> BuiltinIcon(ref.name, tint, size)
        is IconRef.Legacy -> Box(Modifier.size(size), contentAlignment = Alignment.Center) {
            Text(ref.text.take(2), color = tint, style = MaterialTheme.typography.headlineSmall)
        }
        IconRef.None -> LetterIcon(label, size)
    }
}

@Composable
fun BuiltinIcon(name: String, color: Color, size: Dp, modifier: Modifier = Modifier) {
    Canvas(modifier.size(size)) { drawBuiltin(name, color) }
}

/** Draws one of [com.niranjan.helm.core.BUILTIN_ICONS] as strokes in a 24-unit box. */
fun DrawScope.drawBuiltin(name: String, color: Color) {
    val u = size.minDimension / 24f
    val stroke = Stroke(width = 2f * u, cap = StrokeCap.Round, join = StrokeJoin.Round)
    fun p(x: Float, y: Float) = Offset(x * u, y * u)
    fun line(x1: Float, y1: Float, x2: Float, y2: Float) = drawLine(color, p(x1, y1), p(x2, y2), stroke.width, StrokeCap.Round)
    fun circle(cx: Float, cy: Float, r: Float) = drawCircle(color, r * u, p(cx, cy), style = stroke)
    fun dot(cx: Float, cy: Float, r: Float) = drawCircle(color, r * u, p(cx, cy))
    fun rect(x: Float, y: Float, w: Float, h: Float, r: Float = 2f) =
        drawRoundRect(color, p(x, y), Size(w * u, h * u), androidx.compose.ui.geometry.CornerRadius(r * u), style = stroke)
    fun path(fill: Boolean = false, build: Path.() -> Unit) {
        val path = Path().apply(build)
        if (fill) drawPath(path, color) else drawPath(path, color, style = stroke)
    }
    fun Path.m(x: Float, y: Float) = moveTo(x * u, y * u)
    fun Path.l(x: Float, y: Float) = lineTo(x * u, y * u)
    fun Path.q(cx: Float, cy: Float, x: Float, y: Float) = quadraticTo(cx * u, cy * u, x * u, y * u)
    fun arc(cx: Float, cy: Float, r: Float, start: Float, sweep: Float) =
        drawArc(color, start, sweep, false, p(cx - r, cy - r), Size(2 * r * u, 2 * r * u), style = stroke)
    fun speaker() = path { m(4f, 9f); l(8f, 9f); l(13f, 5f); l(13f, 19f); l(8f, 15f); l(4f, 15f); close() }
    fun mic() {
        rect(9f, 3f, 6f, 11f, 3f)
        arc(12f, 11f, 6f, 0f, 180f)
        line(12f, 17f, 12f, 21f)
    }

    when (name) {
        "play" -> path { m(7f, 5f); l(19f, 12f); l(7f, 19f); close() }
        "pause" -> { line(8f, 5f, 8f, 19f); line(16f, 5f, 16f, 19f) }
        "next" -> { path { m(5f, 5f); l(15f, 12f); l(5f, 19f); close() }; line(19f, 5f, 19f, 19f) }
        "prev" -> { path { m(19f, 5f); l(9f, 12f); l(19f, 19f); close() }; line(5f, 5f, 5f, 19f) }
        "volume_up" -> { speaker(); arc(13f, 12f, 4f, -50f, 100f); arc(13f, 12f, 7.5f, -50f, 100f) }
        "volume_down" -> { speaker(); arc(13f, 12f, 4f, -50f, 100f) }
        "mute" -> { speaker(); line(16f, 9f, 22f, 15f); line(22f, 9f, 16f, 15f) }
        "mic" -> mic()
        "mic_off" -> { mic(); line(4f, 4f, 20f, 20f) }
        "power" -> { arc(12f, 13f, 8f, -60f, 300f); line(12f, 2f, 12f, 12f) }
        "lock" -> { rect(5f, 11f, 14f, 10f); arc(12f, 11f, 4f, 180f, 180f); line(8f, 11f, 8f, 9f); line(16f, 11f, 16f, 9f) }
        "globe" -> {
            circle(12f, 12f, 9f)
            line(3f, 12f, 21f, 12f)
            path { m(12f, 3f); q(17f, 12f, 12f, 21f) }
            path { m(12f, 3f); q(7f, 12f, 12f, 21f) }
        }
        "folder" -> path { m(3f, 6f); l(9f, 6f); l(11f, 8f); l(21f, 8f); l(21f, 19f); l(3f, 19f); close() }
        "file" -> { path { m(6f, 3f); l(14f, 3f); l(19f, 8f); l(19f, 21f); l(6f, 21f); close() }; path { m(14f, 3f); l(14f, 8f); l(19f, 8f) } }
        "terminal" -> { rect(3f, 4f, 18f, 16f); path { m(7f, 9f); l(10f, 12f); l(7f, 15f) }; line(12f, 15f, 16f, 15f) }
        "gear" -> {
            circle(12f, 12f, 3f)
            circle(12f, 12f, 7f)
            for (i in 0 until 8) {
                val a = Math.toRadians(i * 45.0)
                val c = kotlin.math.cos(a).toFloat()
                val s = kotlin.math.sin(a).toFloat()
                line(12f + 7f * c, 12f + 7f * s, 12f + 10f * c, 12f + 10f * s)
            }
        }
        "star" -> path {
            for (i in 0 until 10) {
                val r = if (i % 2 == 0) 9.5f else 4f
                val a = Math.toRadians(-90.0 + i * 36.0)
                val x = 12f + r * kotlin.math.cos(a).toFloat()
                val y = 12.5f + r * kotlin.math.sin(a).toFloat()
                if (i == 0) m(x, y) else l(x, y)
            }
            close()
        }
        "heart" -> path { m(12f, 20f); q(2f, 13f, 4f, 7.5f); q(7f, 3f, 12f, 8f); q(17f, 3f, 20f, 7.5f); q(22f, 13f, 12f, 20f); close() }
        "home" -> { path { m(3f, 11f); l(12f, 3f); l(21f, 11f) }; path { m(5f, 10f); l(5f, 21f); l(19f, 21f); l(19f, 10f) }; rect(10f, 14f, 4f, 7f, 0.5f) }
        "mail" -> { rect(3f, 5f, 18f, 14f); path { m(3f, 7f); l(12f, 13f); l(21f, 7f) } }
        "camera" -> { path { m(3f, 8f); l(7f, 8f); l(9f, 5f); l(15f, 5f); l(17f, 8f); l(21f, 8f); l(21f, 19f); l(3f, 19f); close() }; circle(12f, 13f, 3.5f) }
        "music" -> { line(9f, 17f, 9f, 5f); line(9f, 5f, 19f, 3f); line(19f, 3f, 19f, 15f); dot(7f, 17f, 2.5f); dot(17f, 15f, 2.5f) }
        "keyboard" -> {
            rect(2f, 6f, 20f, 12f)
            for (x in listOf(6f, 10f, 14f, 18f)) { dot(x, 10f, 0.9f) }
            line(7f, 14f, 17f, 14f)
        }
        "monitor" -> { rect(3f, 4f, 18f, 12f); line(12f, 16f, 12f, 20f); line(8f, 20f, 16f, 20f) }
        "bulb" -> { path { m(9f, 17f); q(9f, 14f, 7f, 12f); q(4f, 7f, 9f, 4f); q(12f, 2.5f, 15f, 4f); q(20f, 7f, 17f, 12f); q(15f, 14f, 15f, 17f); close() }; line(9.5f, 20f, 14.5f, 20f) }
        "code" -> { path { m(8f, 7f); l(3f, 12f); l(8f, 17f) }; path { m(16f, 7f); l(21f, 12f); l(16f, 17f) }; line(14f, 5f, 10f, 19f) }
        "chat" -> path { m(4f, 5f); l(20f, 5f); l(20f, 16f); l(10f, 16f); l(5f, 20f); l(6f, 16f); l(4f, 16f); close() }
        "game" -> {
            path { m(7f, 8f); l(17f, 8f); q(22f, 8f, 21f, 15f); q(20f, 19f, 16f, 16f); l(8f, 16f); q(4f, 19f, 3f, 15f); q(2f, 8f, 7f, 8f); close() }
            line(6f, 12f, 10f, 12f); line(8f, 10f, 8f, 14f); dot(15f, 11f, 1f); dot(17f, 13f, 1f)
        }
        "bolt" -> path { m(13f, 2f); l(4f, 14f); l(11f, 14f); l(10f, 22f); l(20f, 9f); l(13f, 9f); close() }
        "text" -> { line(5f, 5f, 19f, 5f); line(12f, 5f, 12f, 19f); line(9f, 19f, 15f, 19f) }
        else -> circle(12f, 12f, 8f)
    }
}
