package com.niranjan.helm.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.content.pm.ActivityInfo
import android.os.SystemClock
import android.view.WindowManager
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.AbsMoveBatcher
import com.niranjan.helm.core.DragAction
import com.niranjan.helm.core.DragSelect
import com.niranjan.helm.core.GamingPreset
import com.niranjan.helm.core.InputFrame
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.core.Orientation
import com.niranjan.helm.core.RotationLock
import com.niranjan.helm.core.SecondScreen
import com.niranjan.helm.core.TouchpadMath
import com.niranjan.helm.core.ViewTransform
import com.niranjan.helm.core.WindowEntry
import com.niranjan.helm.core.WindowThumb
import kotlin.math.max

/** G3 row under the stream controls: window, gaming preset, drag mode, second screen. */
@Composable
fun G3Controls(vm: StreamViewModel, g3: G3Ui, windows: List<WindowEntry>, monitors: List<MonitorInfo>) {
    var pickWindow by remember { mutableStateOf(false) }
    var pickScreen by remember { mutableStateOf(false) }
    var gamingOpen by remember { mutableStateOf(false) }
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 2.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        OutlineButton(onClick = { pickWindow = true }) {
            Text(g3.window?.let { "Window: ${it.title.take(18)}" } ?: "Window…", maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Box {
            OutlineButton(onClick = { gamingOpen = true }) { Text(g3.gaming?.label ?: "Gaming off") }
            DropdownMenu(expanded = gamingOpen, onDismissRequest = { gamingOpen = false }) {
                DropdownMenuItem(text = { Text("Gaming off") }, onClick = { gamingOpen = false; vm.setGaming(null) })
                GamingPreset.entries.forEach { p ->
                    DropdownMenuItem(
                        text = { Text("${p.label} · ${p.kbps / 1000} Mbps · sound on") },
                        onClick = { gamingOpen = false; vm.setGaming(p) },
                    )
                }
            }
        }
        BpFilterChip(selected = g3.dragMode, onClick = { vm.setDragMode(!g3.dragMode) }, label = { Text("Drag mode") })
        OutlineButton(onClick = { pickScreen = true }) { Text(if (g3.secondScreen) "Second screen: on" else "Second screen…") }
        Text(
            if (g3.inputOnStream) "input: stream" else "input: control",
            style = MaterialTheme.typography.labelSmall,
        )
    }
    if (pickWindow) {
        LaunchedEffect(windows) { vm.loadThumbs(windows) }
        WindowPickerDialog(
            windows = WindowThumb.pickable(windows),
            g3 = g3,
            onPick = { vm.selectWindow(it); pickWindow = false },
            onDismiss = { pickWindow = false },
        )
    }
    if (pickScreen) {
        SecondScreenDialog(
            monitors = monitors,
            active = g3.secondScreen,
            onStart = { m, lock -> vm.setSecondScreen(m, lock); pickScreen = false },
            onStop = { vm.setSecondScreen(null); pickScreen = false },
            onDismiss = { pickScreen = false },
        )
    }
}

@Composable
private fun WindowPickerDialog(windows: List<WindowEntry>, g3: G3Ui, onPick: (WindowEntry?) -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Stream one window") },
        text = {
            if (windows.isEmpty()) {
                Text("No windows are open on the PC.")
            } else {
                LazyVerticalGrid(
                    columns = GridCells.Adaptive(140.dp),
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                    modifier = Modifier.heightIn(max = 480.dp),
                ) {
                    items(windows, key = { it.hwnd.toString() }) { w ->
                        val key = w.hwnd.toString()
                        val thumb = g3.thumbs[key]
                        Column(
                            Modifier.clip(RoundedCornerShape(8.dp)).clickable { onPick(w) }.padding(4.dp),
                        ) {
                            Box(
                                Modifier.fillMaxWidth().aspectRatio(16f / 10f).background(Color(0xFF202020), RoundedCornerShape(6.dp)),
                                contentAlignment = Alignment.Center,
                            ) {
                                when {
                                    thumb != null -> Image(
                                        thumb.asImageBitmap(),
                                        contentDescription = w.title,
                                        contentScale = ContentScale.Fit,
                                        modifier = Modifier.fillMaxWidth(),
                                    )
                                    key in g3.thumbs -> Text(if (w.isMinimized) "Minimized" else "No preview", color = Color.LightGray)
                                    else -> Text("…", color = Color.LightGray)
                                }
                            }
                            Text(w.title, maxLines = 2, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.labelMedium)
                        }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = { onPick(null) }) { Text("Whole screen") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun SecondScreenDialog(
    monitors: List<MonitorInfo>,
    active: Boolean,
    onStart: (MonitorInfo, RotationLock) -> Unit,
    onStop: () -> Unit,
    onDismiss: () -> Unit,
) {
    var chosen by remember { mutableStateOf(SecondScreen.defaultMonitor(monitors)) }
    var lock by remember { mutableStateOf(RotationLock.MATCH_MONITOR) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Use the phone as a second screen") },
        text = {
            Column {
                Text(
                    "Shows only the chosen monitor, view only, with the screen kept on. A real extra " +
                        "monitor needs a virtual display driver on the PC (not part of Helm yet).",
                    style = MaterialTheme.typography.bodySmall,
                )
                monitors.forEachIndexed { i, m ->
                    Row(Modifier.fillMaxWidth().clickable { chosen = m }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = chosen?.id == m.id, onClick = { chosen = m })
                        Text(SecondScreen.label(i, m), style = MaterialTheme.typography.bodyMedium)
                    }
                }
                Text("Rotation", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(top = 8.dp))
                RotationLock.entries.forEach { r ->
                    Row(Modifier.fillMaxWidth().clickable { lock = r }, verticalAlignment = Alignment.CenterVertically) {
                        RadioButton(selected = lock == r, onClick = { lock = r })
                        Text(r.label, style = MaterialTheme.typography.bodyMedium)
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = chosen != null, onClick = { chosen?.let { onStart(it, lock) } }) { Text("Start") }
        },
        dismissButton = {
            if (active) TextButton(onClick = onStop) { Text("Stop second screen") } else TextButton(onClick = onDismiss) { Text("Cancel") }
        },
    )
}

private fun Context.findActivity(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

/** Applies the rotation lock (and keeps the screen on in second-screen mode) while shown. */
@Composable
fun RotationLockEffect(orientation: Orientation, keepScreenOn: Boolean) {
    val activity = LocalContext.current.findActivity() ?: return
    DisposableEffect(orientation, keepScreenOn) {
        val previous = activity.requestedOrientation
        activity.requestedOrientation = when (orientation) {
            Orientation.UNSPECIFIED -> previous
            Orientation.LANDSCAPE -> ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
            Orientation.PORTRAIT -> ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT
        }
        if (keepScreenOn) activity.window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose {
            activity.requestedOrientation = previous
            if (keepScreenOn) activity.window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }
}

/** Second-screen mode is view only: a tap shows / hides the controls. */
fun Modifier.viewOnlyGestures(onTap: () -> Unit): Modifier = pointerInput(Unit) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val t0 = down.uptimeMillis
        while (true) {
            val e = awaitPointerEvent()
            e.changes.forEach { it.consume() }
            if (e.changes.none { it.pressed }) {
                if (e.changes.maxOf { it.uptimeMillis } - t0 < 300) onTap()
                break
            }
        }
    }
}

/**
 * Drag mode (G3): long-press-then-drag holds the left button (`mb` down, `ma`/`mm` moves,
 * `mb` up), tap = left click, two-finger tap = right click, two-finger drag scrolls (pans when
 * zoomed), three-finger tap toggles the controls. See [DragSelect].
 */
fun Modifier.dragSelectGestures(
    direct: Boolean,
    speed: Float,
    haptics: Boolean,
    remote: RemoteViewModel,
    abs: AbsMoveBatcher,
    transform: () -> ViewTransform,
    setTransform: (ViewTransform) -> Unit,
    onThreeFingerTap: () -> Unit,
): Modifier = composed {
    val density = LocalDensity.current
    val haptic = LocalHapticFeedback.current
    val currentSpeed by rememberUpdatedState(speed)
    val currentHaptics by rememberUpdatedState(haptics)
    val slop = with(density) { 8.dp.toPx() }
    val notchPx = with(density) { 36.dp.toPx() }

    fun flushAbs() {
        abs.take()?.let { remote.send(it) }
        remote.flushInput()
    }

    fun panOrScroll(dx: Float, dy: Float) {
        if (transform().zoom > 1.01f) setTransform(transform().panBy(dx, dy)) else remote.touchScroll(-dx / notchPx, dy / notchPx)
    }

    fun run(actions: List<DragAction>) {
        for (a in actions) when (a) {
            is DragAction.PointAt -> {
                val n = transform().toNormalizedClamped(a.x, a.y)
                abs.set(n.first, n.second)
            }
            is DragAction.MoveBy -> {
                val (dx, dy) = TouchpadMath.accelerate(a.dx, a.dy, 16f, currentSpeed)
                remote.touchMove(dx, dy)
            }
            is DragAction.Press -> {
                flushAbs()
                remote.send(InputFrame.Button(a.button, true))
            }
            is DragAction.Release -> {
                flushAbs()
                remote.send(InputFrame.Button(a.button, false))
            }
            is DragAction.Click -> {
                flushAbs()
                remote.sendAll(InputFrame.click(a.button))
            }
            is DragAction.Pan -> panOrScroll(a.dx, a.dy)
            is DragAction.TwoFinger -> panOrScroll(a.dx, a.dy)
            DragAction.Haptic -> if (currentHaptics) haptic.performHapticFeedback(HapticFeedbackType.LongPress)
        }
    }

    pointerInput(direct) {
        awaitEachGesture {
            val ds = DragSelect(direct = direct, slopPx = slop)
            val first = awaitFirstDown(requireUnconsumed = false)
            val start = SystemClock.uptimeMillis()
            var maxPointers = 1
            run(ds.down(first.position.x, first.position.y, start))
            try {
                while (true) {
                    val event = if (ds.pending) {
                        val left = ds.deadline - SystemClock.uptimeMillis()
                        if (left <= 0) null else withTimeoutOrNull(left) { awaitPointerEvent() }
                    } else {
                        awaitPointerEvent()
                    }
                    val now = SystemClock.uptimeMillis()
                    if (event == null) {
                        run(ds.tick(now))
                        continue
                    }
                    val pressed = event.changes.filter { it.pressed }
                    if (pressed.isEmpty()) {
                        if (maxPointers >= 3 && now - start < 300) {
                            ds.cancel()
                            onThreeFingerTap()
                        } else {
                            run(ds.up(now))
                        }
                        break
                    }
                    maxPointers = max(maxPointers, pressed.size)
                    if (pressed.size >= 2) {
                        val a = pressed[0]
                        val b = pressed[1]
                        val c = (a.position + b.position) / 2f
                        val d = c - (a.previousPosition + b.previousPosition) / 2f
                        run(ds.pointers(pressed.size, d.x, d.y))
                    } else {
                        val p = pressed[0]
                        run(ds.move(p.position.x, p.position.y, now))
                    }
                    event.changes.forEach { it.consume() }
                }
            } finally {
                // Cancelled mid-drag (view gone): never leave the PC's left button held.
                if (ds.holding) run(ds.cancel())
            }
        }
    }
}
