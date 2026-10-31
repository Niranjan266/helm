package com.niranjan.helm.ui

import android.content.res.Configuration
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.Send
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.KeyboardArrowUp
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.KeyPress
import com.niranjan.helm.core.MouseButton
import com.niranjan.helm.core.TextDiff
import com.niranjan.helm.core.TouchpadMath
import com.niranjan.helm.core.Vk
import com.niranjan.helm.device.GyroSensor
import com.niranjan.helm.device.Settings
import com.niranjan.helm.device.WifiLatencyLock
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

private enum class TwoFinger { NONE, SCROLL, PINCH }

private val PadShape = RoundedCornerShape(20.dp)
private val ControlGap = 8.dp

private val Modifiers = listOf("Ctrl" to Vk.CONTROL, "Alt" to Vk.ALT, "Shift" to Vk.SHIFT, "Win" to Vk.LWIN)
private val CommonKeys = listOf("Esc" to Vk.ESCAPE, "Tab" to Vk.TAB, "Bksp" to Vk.BACK, "Del" to Vk.DELETE, "Enter" to Vk.RETURN)
private val NavKeys = listOf(
    "←" to Vk.LEFT, "↑" to Vk.UP, "↓" to Vk.DOWN, "→" to Vk.RIGHT,
    "Home" to Vk.HOME, "End" to Vk.END, "PgUp" to Vk.PAGE_UP, "PgDn" to Vk.PAGE_DOWN,
)
private val FunctionKeys = (1..12).map { "F$it" to Vk.f(it) }

@Composable
fun TouchpadScreen(remote: RemoteViewModel, settings: Settings) {
    val keyboard = LocalSoftwareKeyboardController.current
    val focusManager = LocalFocusManager.current
    val focus = remember { FocusRequester() }
    var dragLock by remember { mutableStateOf(remote.isDragLocked) }
    var airMouse by remember { mutableStateOf(false) }
    var speed by remember { mutableStateOf(settings.touchpadSpeed) }
    val context = LocalContext.current
    DisposableEffect(airMouse) {
        val sensor = if (airMouse) GyroSensor(context) { x, z, dt -> remote.gyroSample(x, z, dt, speed) } else null
        sensor?.start()
        onDispose {
            sensor?.stop()
            remote.gyroReset()
        }
    }
    val mods = remember { mutableStateListOf<Int>() }
    var autoShown by remember { mutableStateOf(false) }
    val rui by remote.ui.collectAsStateWithLifecycle()
    val showAirMouse = rui.phase4 && GyroSensor.available(context)

    // Wi-Fi power-save adds delay between packets; keep the radio awake while the pad is open.
    DisposableEffect(Unit) {
        val wifi = WifiLatencyLock(context)
        wifi.acquire()
        onDispose { wifi.release() }
    }

    // Safety net: moves are sent as each touch event arrives; this catches anything left over.
    LaunchedEffect(Unit) {
        while (isActive) {
            delay(16)
            remote.flushInput()
        }
    }
    DisposableEffect(Unit) { onDispose { remote.flushInput() } }

    // Auto-open the phone keyboard when a text field gets focus on the PC.
    LaunchedEffect(Unit) {
        remote.textFocus.collect { focused ->
            if (focused) {
                runCatching { focus.requestFocus() }
                keyboard?.show()
                autoShown = true
            } else if (autoShown) {
                keyboard?.hide()
                focusManager.clearFocus()
                autoShown = false
            }
        }
    }

    fun press(vk: Int) {
        remote.sendAll(KeyPress.frames(vk, mods.toList()))
        mods.clear()
    }

    val toolbar = @Composable {
        PadToolbar(
            dragLock = dragLock,
            onDragLock = { dragLock = remote.setDragLock(!dragLock) },
            airMouse = if (showAirMouse) airMouse else null,
            onAirMouse = { airMouse = !airMouse },
            speed = speed,
            onSpeed = { speed = it },
            onSpeedDone = { remote.setTouchpadSpeed(speed) },
        )
    }
    val pad = @Composable { modifier: Modifier ->
        Column(modifier, verticalArrangement = Arrangement.spacedBy(ControlGap)) {
            TouchSurface(
                remote = remote,
                speed = speed,
                haptics = settings.haptics,
                onLeftTap = { dragLock = remote.isDragLocked },
                modifier = Modifier.weight(1f).fillMaxWidth(),
            )
            ClickButtons(remote)
        }
    }
    val keys = @Composable {
        KeyPanel(mods = mods, onKey = ::press)
        TypeField(remote = remote, mods = mods, focus = focus, onKey = ::press)
    }

    // Orientation, not window size: the soft keyboard shrinks the window and must not flip the layout.
    val landscape = LocalConfiguration.current.orientation == Configuration.ORIENTATION_LANDSCAPE
    Box(Modifier.fillMaxSize().padding(12.dp)) {
        if (landscape) {
            // Landscape: pad on the left, controls in a column on the right.
            Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                pad(Modifier.weight(1f).fillMaxHeight())
                Column(
                    Modifier.width(320.dp).fillMaxHeight().verticalScroll(rememberScrollState()),
                    verticalArrangement = Arrangement.spacedBy(ControlGap),
                ) {
                    toolbar()
                    keys()
                }
            }
        } else {
            Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(ControlGap)) {
                toolbar()
                pad(Modifier.weight(1f).fillMaxWidth())
                keys()
            }
        }
    }
}

@Composable
private fun PadToolbar(
    dragLock: Boolean,
    onDragLock: () -> Unit,
    airMouse: Boolean?,
    onAirMouse: () -> Unit,
    speed: Float,
    onSpeed: (Float) -> Unit,
    onSpeedDone: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(ControlGap)) {
            FilterChip(selected = dragLock, onClick = onDragLock, label = { Text("Drag lock") })
            if (airMouse != null) {
                FilterChip(selected = airMouse, onClick = onAirMouse, label = { Text("Air mouse") })
            }
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(ControlGap)) {
            Text("Speed", style = MaterialTheme.typography.labelLarge)
            Slider(
                value = speed,
                onValueChange = onSpeed,
                onValueChangeFinished = onSpeedDone,
                valueRange = 0.4f..3f,
                modifier = Modifier.weight(1f),
            )
            Text("%.1f×".format(speed), style = MaterialTheme.typography.labelLarge, modifier = Modifier.width(40.dp))
        }
    }
}

@Composable
private fun TouchSurface(
    remote: RemoteViewModel,
    speed: Float,
    haptics: Boolean,
    onLeftTap: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current
    val currentSpeed by rememberUpdatedState(speed)
    val currentHaptics by rememberUpdatedState(haptics)
    val currentOnLeftTap by rememberUpdatedState(onLeftTap)
    val touches: SnapshotStateList<Offset> = remember { mutableStateListOf() }
    var used by remember { mutableStateOf(false) }
    val slop = with(density) { 8.dp.toPx() }
    val notchPx = with(density) { 36.dp.toPx() }
    val pinchPx = with(density) { 60.dp.toPx() }
    val glow = MaterialTheme.colorScheme.primary
    val glowRadius = with(density) { 28.dp.toPx() }

    Box(
        modifier
            .background(MaterialTheme.colorScheme.surfaceContainerHigh, PadShape)
            .border(1.dp, MaterialTheme.colorScheme.outlineVariant, PadShape)
            .pointerInput(Unit) {
                awaitEachGesture {
                    val first = awaitFirstDown(requireUnconsumed = false)
                    used = true
                    val start = first.uptimeMillis
                    var last = start
                    var maxPointers = 1
                    var moved = 0f
                    var mode = TwoFinger.NONE
                    var centroidTravel = 0f
                    var spanTravel = 0f
                    var end = start
                    // Movement inside the tap slop is held back (so a tap does not nudge the
                    // cursor) and released in one piece once the finger clearly moves.
                    var heldX = 0f
                    var heldY = 0f
                    // Smoothed finger speed (px/ms): raw per-event timing is noisy and made
                    // the pointer boost jump between frames.
                    var velocity = 0f
                    while (true) {
                        val event = awaitPointerEvent()
                        end = event.changes.maxOf { it.uptimeMillis }
                        val pressed = event.changes.filter { it.pressed }
                        touches.clear()
                        touches.addAll(pressed.map { it.position })
                        if (pressed.isEmpty()) break
                        maxPointers = max(maxPointers, pressed.size)
                        if (pressed.size == 1 && maxPointers == 1) {
                            val c = pressed[0]
                            val d = c.position - c.previousPosition
                            val dist = d.getDistance()
                            moved += dist
                            val dt = (c.uptimeMillis - last).coerceAtLeast(1).toFloat()
                            velocity = if (velocity == 0f) dist / dt else velocity * 0.6f + (dist / dt) * 0.4f
                            if (moved > slop) {
                                val rx = d.x + heldX
                                val ry = d.y + heldY
                                heldX = 0f
                                heldY = 0f
                                val rd = hypot(rx, ry)
                                // Feed the smoothed speed in as an equivalent time step.
                                val dtEff = if (velocity > 0f) rd / velocity else dt
                                val (dx, dy) = TouchpadMath.accelerate(rx, ry, dtEff, currentSpeed)
                                remote.touchMove(dx, dy)
                                remote.flushInput()
                            } else {
                                heldX += d.x
                                heldY += d.y
                            }
                            last = c.uptimeMillis
                        } else if (pressed.size >= 2) {
                            val a = pressed[0]
                            val b = pressed[1]
                            val dc = ((a.position + b.position) - (a.previousPosition + b.previousPosition)) / 2f
                            val ds = (a.position - b.position).getDistance() - (a.previousPosition - b.previousPosition).getDistance()
                            centroidTravel += dc.getDistance()
                            spanTravel += ds
                            moved += dc.getDistance() + abs(ds)
                            if (mode == TwoFinger.NONE && moved > slop * 1.5f) {
                                mode = if (abs(spanTravel) > centroidTravel) TwoFinger.PINCH else TwoFinger.SCROLL
                                if (mode == TwoFinger.PINCH) remote.pinch(true)
                            }
                            when (mode) {
                                // Natural scrolling: fingers down = content down = wheel up (dy > 0).
                                TwoFinger.SCROLL -> remote.touchScroll(-dc.x / notchPx, dc.y / notchPx)
                                TwoFinger.PINCH -> remote.touchScroll(0f, ds / pinchPx)
                                TwoFinger.NONE -> Unit
                            }
                            if (mode != TwoFinger.NONE) remote.flushInput()
                        }
                        event.changes.forEach { it.consume() }
                    }
                    touches.clear()
                    if (mode == TwoFinger.PINCH) remote.pinch(false)
                    val tap = moved < slop && end - start < 300
                    if (tap) {
                        if (currentHaptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        if (maxPointers >= 2) {
                            remote.click(MouseButton.RIGHT)
                        } else {
                            remote.click(MouseButton.LEFT)
                            currentOnLeftTap()
                        }
                    } else {
                        remote.flushInput()
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        if (!used) {
            HintText(
                "Move with one finger · tap to click\nTwo fingers to scroll · two-finger tap for right-click · pinch to zoom",
                modifier = Modifier.padding(24.dp),
            )
        }
        Canvas(Modifier.fillMaxSize()) {
            touches.forEach { p ->
                drawCircle(glow.copy(alpha = 0.18f), radius = glowRadius, center = p)
                drawCircle(glow.copy(alpha = 0.35f), radius = glowRadius * 0.45f, center = p)
            }
        }
    }
}

@Composable
private fun ClickButtons(remote: RemoteViewModel) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(ControlGap)) {
        FilledTonalButton(
            onClick = { remote.click(MouseButton.LEFT) },
            shape = RoundedCornerShape(topStart = 12.dp, bottomStart = 20.dp, topEnd = 6.dp, bottomEnd = 6.dp),
            modifier = Modifier.weight(1f).height(52.dp),
        ) { Text("Left click") }
        FilledTonalButton(
            onClick = { remote.click(MouseButton.RIGHT) },
            shape = RoundedCornerShape(topStart = 6.dp, bottomStart = 6.dp, topEnd = 12.dp, bottomEnd = 20.dp),
            modifier = Modifier.weight(1f).height(52.dp),
        ) { Text("Right click") }
    }
}

@Composable
private fun KeyPanel(mods: SnapshotStateList<Int>, onKey: (Int) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        // Modifiers stick until the next key, then clear.
        KeyRow {
            Modifiers.forEach { (label, vk) ->
                FilterChip(
                    selected = vk in mods,
                    onClick = { if (vk in mods) mods.remove(vk) else mods.add(vk) },
                    label = { Text(label) },
                )
            }
        }
        KeyRow { CommonKeys.forEach { (label, vk) -> KeyButton(label) { onKey(vk) } } }
        if (expanded) {
            KeyRow { NavKeys.forEach { (label, vk) -> KeyButton(label) { onKey(vk) } } }
            KeyRow { FunctionKeys.forEach { (label, vk) -> KeyButton(label) { onKey(vk) } } }
        }
        TextButton(onClick = { expanded = !expanded }) {
            Icon(if (expanded) Icons.Filled.KeyboardArrowUp else Icons.Filled.KeyboardArrowDown, contentDescription = null)
            Text(if (expanded) "Fewer keys" else "More keys (arrows, F1–F12)")
        }
    }
}

@Composable
private fun KeyRow(content: @Composable () -> Unit) {
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) { content() }
}

@Composable
private fun KeyButton(label: String, onClick: () -> Unit) {
    OutlinedButton(
        onClick = onClick,
        contentPadding = PaddingValues(horizontal = 10.dp),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.height(40.dp).width(if (label.length > 3) 64.dp else 48.dp),
    ) { Text(label, style = MaterialTheme.typography.labelLarge) }
}

@Composable
private fun TypeField(
    remote: RemoteViewModel,
    mods: SnapshotStateList<Int>,
    focus: FocusRequester,
    onKey: (Int) -> Unit,
) {
    var text by remember { mutableStateOf("") }
    OutlinedTextField(
        value = text,
        onValueChange = { new ->
            if (mods.isNotEmpty() && new.length == text.length + 1 && new.startsWith(text)) {
                // A letter typed with sticky modifiers becomes a shortcut (e.g. Ctrl+C).
                val vk = Vk.forChar(new.last())
                if (vk != null) {
                    onKey(vk)
                    return@OutlinedTextField
                }
            }
            remote.sendAll(TextDiff.frames(text, new))
            // Keep the buffer short; reset silently.
            text = if (new.length > 80) "" else new
        },
        placeholder = { Text("Type on the PC") },
        singleLine = true,
        shape = RoundedCornerShape(12.dp),
        trailingIcon = {
            IconButton(onClick = {
                onKey(Vk.RETURN)
                text = ""
            }) { Icon(Icons.AutoMirrored.Filled.Send, contentDescription = "Send Enter") }
        },
        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text, imeAction = ImeAction.Send, autoCorrectEnabled = false),
        keyboardActions = KeyboardActions(onSend = {
            onKey(Vk.RETURN)
            text = ""
        }),
        modifier = Modifier.fillMaxWidth().focusRequester(focus),
    )
}
