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
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
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
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.automirrored.filled.ArrowForward
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.KeyPress
import com.niranjan.helm.core.MouseButton
import com.niranjan.helm.core.TextDiff
import com.niranjan.helm.core.PointerGain
import com.niranjan.helm.core.ScrollVelocityTracker
import com.niranjan.helm.core.ThreeFingerSwipe
import com.niranjan.helm.device.GestureSettings
import com.niranjan.helm.device.GestureStore
import com.niranjan.helm.core.Vk
import com.niranjan.helm.device.GyroSensor
import com.niranjan.helm.device.Settings
import com.niranjan.helm.device.WifiLatencyLock
import kotlin.math.abs
import kotlin.math.hypot
import kotlin.math.max
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

private enum class TwoFinger { NONE, SCROLL, PINCH, SWIPE }

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
    val gestureStore = remember { GestureStore.get(context) }
    val gestures by gestureStore.settings.collectAsStateWithLifecycle()
    var gestureDialog by remember { mutableStateOf(false) }
    if (gestureDialog) GestureOptionsDialog(gestures, onChange = { g -> gestureStore.update { g } }, onDismiss = { gestureDialog = false })

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
            precision = gestures.precision,
            onPrecision = { gestureStore.update { it.copy(precision = !it.precision) } },
            onGestures = { gestureDialog = true },
        )
    }
    val pad = @Composable { modifier: Modifier ->
        Column(modifier, verticalArrangement = Arrangement.spacedBy(ControlGap)) {
            TouchSurface(
                remote = remote,
                speed = speed,
                haptics = settings.haptics,
                gestures = gestures,
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
    Box(Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 8.dp)) {
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
    precision: Boolean = false,
    onPrecision: () -> Unit = {},
    onGestures: () -> Unit = {},
) {
    val c = Bp.c
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(
            Modifier.horizontalScroll(rememberScrollState()),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            MonoChip("Drag lock", onClick = onDragLock, selected = dragLock, height = 36.dp)
            if (airMouse != null) MonoChip("Air mouse", onClick = onAirMouse, selected = airMouse, height = 36.dp)
            MonoChip("Precision", onClick = onPrecision, selected = precision, height = 36.dp)
            MonoChip("Gestures", onClick = onGestures, dashed = true, height = 36.dp)
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            MonoLabel("Speed")
            DiamondSlider(
                value = speed,
                onValueChange = onSpeed,
                onValueChangeFinished = onSpeedDone,
                valueRange = 0.4f..3f,
                modifier = Modifier.weight(1f),
            )
            Text("%.1f×".format(speed), style = BpType.monoStrong, color = c.ink, textAlign = TextAlign.End, modifier = Modifier.width(40.dp))
        }
    }
}

@Composable
private fun TouchSurface(
    remote: RemoteViewModel,
    speed: Float,
    haptics: Boolean,
    gestures: GestureSettings = GestureSettings(),
    onLeftTap: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val currentGestures by rememberUpdatedState(gestures)
    val momentum = rememberMomentum { dx, dy ->
        remote.touchScroll(dx, dy)
        remote.flushInput()
    }
    val swipePx = with(LocalDensity.current) { 48.dp.toPx() }
    val scrollVelocity = remember { ScrollVelocityTracker() }
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
    val c = Bp.c
    val ringRadius = with(density) { 30.dp.toPx() }
    val dotRadius = with(density) { 9.dp.toPx() }
    val ringStroke = with(density) { 1.5.dp.toPx() }
    val ringDash = with(density) { floatArrayOf(5.dp.toPx(), 4.dp.toPx()) }

    Box(
        modifier
            .dotPaper(c)
            .border(1.5.dp, c.ink)
            .pointerInput(Unit) {
                awaitEachGesture {
                    val first = awaitFirstDown(requireUnconsumed = false)
                    // Any new touch stops a running fling.
                    momentum.cancel()
                    scrollVelocity.reset()
                    var swipeX = 0f
                    var swipeY = 0f
                    var swipeFired = false
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
                                val (dx, dy) = PointerGain.apply(rx, ry, dtEff, currentSpeed, currentGestures.precision)
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
                            if (mode == TwoFinger.NONE && maxPointers >= 3 && currentGestures.threeFinger) mode = TwoFinger.SWIPE
                            if (mode == TwoFinger.SWIPE) {
                                // Centroid of every finger still down (all three, or what is left while lifting).
                                val n = pressed.size
                                swipeX += pressed.fold(0f) { acc, p -> acc + (p.position.x - p.previousPosition.x) } / n
                                swipeY += pressed.fold(0f) { acc, p -> acc + (p.position.y - p.previousPosition.y) } / n
                                if (!swipeFired && pressed.size >= 3) {
                                    val dir = ThreeFingerSwipe.classify(swipeX, swipeY, swipePx)
                                    if (dir != null) {
                                        swipeFired = true
                                        remote.flushInput()
                                        remote.sendAll(ThreeFingerSwipe.frames(dir, currentGestures.sideSwipe))
                                        if (currentHaptics) haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                                    }
                                }
                            }
                            if (mode == TwoFinger.NONE && moved > slop * 1.5f) {
                                mode = if (abs(spanTravel) > centroidTravel) TwoFinger.PINCH else TwoFinger.SCROLL
                                if (mode == TwoFinger.PINCH) remote.pinch(true)
                            }
                            when (mode) {
                                // Natural scrolling: fingers down = content down = wheel up (dy > 0).
                                TwoFinger.SCROLL -> {
                                    remote.touchScroll(-dc.x / notchPx, dc.y / notchPx)
                                    scrollVelocity.add(end, -dc.x / notchPx, dc.y / notchPx)
                                }
                                TwoFinger.PINCH -> remote.touchScroll(0f, ds / pinchPx)
                                TwoFinger.NONE, TwoFinger.SWIPE -> Unit
                            }
                            if (mode == TwoFinger.SCROLL || mode == TwoFinger.PINCH) remote.flushInput()
                        }
                        event.changes.forEach { it.consume() }
                    }
                    touches.clear()
                    if (mode == TwoFinger.PINCH) remote.pinch(false)
                    val tap = moved < slop && end - start < 300
                    if (tap) {
                        if (currentHaptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        if (maxPointers >= 3) {
                            remote.click(MouseButton.MIDDLE)
                        } else if (maxPointers >= 2) {
                            remote.click(MouseButton.RIGHT)
                        } else {
                            remote.click(MouseButton.LEFT)
                            currentOnLeftTap()
                        }
                    } else {
                        remote.flushInput()
                        if (mode == TwoFinger.SCROLL && currentGestures.momentum) {
                            val (vx, vy) = scrollVelocity.velocity(end)
                            momentum.start(vx, vy)
                        }
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        MonoLabel("Fig. 2 · Touch area", modifier = Modifier.align(Alignment.TopStart).padding(start = 10.dp, top = 8.dp))
        if (!used) {
            Text(
                "1 finger · move · tap = click\n2 fingers · scroll · pinch = zoom\n2-finger tap = right click\n3 fingers · swipe = task view / desktop / switch".uppercase(),
                style = BpType.mono.copy(lineHeight = 20.sp, letterSpacing = 0.06.em),
                color = c.ink2,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(24.dp),
            )
        }
        Canvas(Modifier.fillMaxSize()) {
            touches.forEach { p ->
                drawCircle(
                    c.pencil,
                    radius = ringRadius,
                    center = p,
                    style = Stroke(width = ringStroke, pathEffect = PathEffect.dashPathEffect(ringDash)),
                )
                drawCircle(c.pencil, radius = dotRadius, center = p)
            }
        }
    }
}

@Composable
private fun ClickButtons(remote: RemoteViewModel) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(ControlGap)) {
        OutlineButton(
            onClick = { remote.click(MouseButton.LEFT) },
            filled = true,
            modifier = Modifier.weight(1f).height(56.dp),
        ) { Text("LEFT CLICK", style = BpType.button.copy(fontSize = 14.sp)) }
        OutlineButton(
            onClick = { remote.click(MouseButton.RIGHT) },
            filled = true,
            modifier = Modifier.weight(1f).height(56.dp),
        ) { Text("RIGHT CLICK", style = BpType.button.copy(fontSize = 14.sp)) }
    }
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun KeyPanel(mods: SnapshotStateList<Int>, onKey: (Int) -> Unit) {
    var expanded by remember { mutableStateOf(false) }
    Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
        // Modifiers stick until the next key, then clear.
        FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Modifiers.forEach { (label, vk) ->
                MonoChip(label, selected = vk in mods, onClick = { if (vk in mods) mods.remove(vk) else mods.add(vk) })
            }
            CommonKeys.forEach { (label, vk) -> MonoChip(label, onClick = { onKey(vk) }) }
            MonoChip(if (expanded) "Less" else "More", dashed = true, onClick = { expanded = !expanded })
        }
        if (expanded) {
            KeyRow { NavKeys.forEach { (label, vk) -> MonoChip(label, onClick = { onKey(vk) }) } }
            KeyRow { FunctionKeys.forEach { (label, vk) -> MonoChip(label, onClick = { onKey(vk) }) } }
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
private fun TypeField(
    remote: RemoteViewModel,
    mods: SnapshotStateList<Int>,
    focus: FocusRequester,
    onKey: (Int) -> Unit,
) {
    var text by remember { mutableStateOf("") }
    val c = Bp.c
    val sendEnter = {
        onKey(Vk.RETURN)
        text = ""
    }
    Row(
        Modifier.fillMaxWidth().height(52.dp).background(c.panel).border(1.5.dp, c.ink).padding(start = 14.dp, end = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        BasicTextField(
            value = text,
            onValueChange = { new ->
                if (mods.isNotEmpty() && new.length == text.length + 1 && new.startsWith(text)) {
                    // A letter typed with sticky modifiers becomes a shortcut (e.g. Ctrl+C).
                    val vk = Vk.forChar(new.last())
                    if (vk != null) {
                        onKey(vk)
                        return@BasicTextField
                    }
                }
                remote.sendAll(TextDiff.frames(text, new))
                // Keep the buffer short; reset silently.
                text = if (new.length > 80) "" else new
            },
            singleLine = true,
            textStyle = MaterialTheme.typography.bodyLarge.copy(color = c.ink, fontSize = 15.sp),
            cursorBrush = SolidColor(c.pencilInk),
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text, imeAction = ImeAction.Send, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onSend = { sendEnter() }),
            decorationBox = { inner ->
                Box(contentAlignment = Alignment.CenterStart) {
                    if (text.isEmpty()) Text("Type on the PC", style = MaterialTheme.typography.bodyLarge.copy(fontSize = 15.sp), color = c.ink2)
                    inner()
                }
            },
            modifier = Modifier.weight(1f).focusRequester(focus).semantics { contentDescription = "Type on the PC" },
        )
        Box(
            Modifier
                .size(44.dp, 40.dp)
                .background(c.pencil)
                .border(2.dp, c.deep)
                .clickable(role = Role.Button, onClick = sendEnter)
                .semantics { contentDescription = "Send Enter" },
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.AutoMirrored.Filled.ArrowForward, contentDescription = null, tint = c.deep, modifier = Modifier.size(18.dp))
        }
    }
}
