package com.niranjan.helm.ui

import android.app.Activity
import android.content.pm.ActivityInfo
import android.view.HapticFeedbackConstants
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.input.pointer.PointerId
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.min
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.GamepadEngine
import com.niranjan.helm.core.GamepadKeys
import com.niranjan.helm.core.GamepadMapping
import com.niranjan.helm.core.GamepadPresets
import com.niranjan.helm.core.GamepadProfile
import com.niranjan.helm.core.InputFrame
import com.niranjan.helm.core.PadButton
import com.niranjan.helm.core.PadStick
import com.niranjan.helm.core.StickMath
import com.niranjan.helm.core.SwipeDirection
import com.niranjan.helm.device.GamepadStore
import com.niranjan.helm.device.Settings
import kotlin.math.roundToInt

private val FaceA = Color(0xFF3DBE6A)
private val FaceB = Color(0xFFE5534B)
private val FaceX = Color(0xFF3F8CFF)
private val FaceY = Color(0xFFE8B931)

/**
 * On-screen controller (G1). Landscape only; everything is sent as `in` key / mouse frames
 * from the selected mapping, so no driver is needed on the PC.
 */
@Composable
fun GamepadScreen(remote: RemoteViewModel, settings: Settings) {
    val context = LocalContext.current
    val view = LocalView.current
    val hostId = remember { (context.applicationContext as HelmApp).controller.session.value?.hostId.orEmpty() }
    val store = remember { GamepadStore.get(context) }
    var profile by remember(hostId) { mutableStateOf(store.load(hostId)) }
    val engine = remember { GamepadEngine(profile.active()) }
    var editing by remember { mutableStateOf<GamepadMapping?>(null) }
    val haptics by rememberUpdatedState(settings.haptics)

    DisposableEffect(Unit) {
        val activity = context as? Activity
        activity?.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
        onDispose {
            remote.sendAll(engine.releaseAll())
            // The rotation itself recreates the activity; only unlock when really leaving.
            if (activity != null && !activity.isChangingConfigurations) {
                activity.requestedOrientation = ActivityInfo.SCREEN_ORIENTATION_UNSPECIFIED
            }
        }
    }
    LifecycleEventEffect(Lifecycle.Event.ON_STOP) { remote.sendAll(engine.releaseAll()) }

    fun setProfile(p: GamepadProfile) {
        profile = p
        if (hostId.isNotEmpty()) store.save(hostId, p)
        remote.sendAll(engine.setMapping(p.active()))
    }

    fun send(frames: List<InputFrame>) {
        if (frames.isNotEmpty()) remote.sendAll(frames)
    }

    fun press(b: PadButton) {
        val f = engine.press(b)
        if (haptics && f.isNotEmpty()) view.performHapticFeedback(HapticFeedbackConstants.VIRTUAL_KEY)
        send(f)
    }

    fun release(b: PadButton) = send(engine.release(b))

    fun stick(s: PadStick, x: Float, y: Float) = send(engine.stick(s, x, y))

    // Mouse-mapped sticks move the pointer every display frame.
    LaunchedEffect(engine) {
        var last = withFrameMillis { it }
        while (true) {
            val now = withFrameMillis { it }
            send(engine.tick((now - last).toFloat()))
            last = now
        }
    }

    Column(Modifier.fillMaxSize().padding(horizontal = 12.dp, vertical = 8.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            PadKey("LT", Modifier.width(64.dp).height(40.dp), onDown = { press(PadButton.LT) }, onUp = { release(PadButton.LT) })
            PadKey("LB", Modifier.width(64.dp).height(40.dp), onDown = { press(PadButton.LB) }, onUp = { release(PadButton.LB) })
            Row(
                Modifier.weight(1f).horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val chips = listOf(
                    GamepadPresets.KEYBOARD to "Keyboard",
                    GamepadPresets.RACING to "Racing",
                    GamepadPresets.MOUSE_LOOK to "Mouse look",
                    GamepadPresets.CUSTOM to "Custom",
                )
                chips.forEach { (id, label) ->
                    MonoChip(
                        label,
                        selected = profile.selected == id,
                        onClick = {
                            if (id == GamepadPresets.CUSTOM && profile.custom == null) editing = profile.active().copy(name = "Custom")
                            else setProfile(profile.copy(selected = id))
                        },
                        height = 36.dp,
                    )
                }
                MonoChip("Edit", dashed = true, onClick = { editing = (profile.custom ?: profile.active()).copy(name = "Custom") }, height = 36.dp)
            }
            PadKey("RB", Modifier.width(64.dp).height(40.dp), onDown = { press(PadButton.RB) }, onUp = { release(PadButton.RB) })
            PadKey("RT", Modifier.width(64.dp).height(40.dp), onDown = { press(PadButton.RT) }, onUp = { release(PadButton.RT) })
        }
        BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
            val unit: Dp = min(maxHeight, (maxWidth - 120.dp) / 3.4f)
            val stickSize = unit * 0.95f
            val clusterSize = unit * 0.8f
            Row(Modifier.fillMaxSize(), horizontalArrangement = Arrangement.SpaceBetween) {
                Thumbstick(Modifier.size(stickSize).align(Alignment.Top)) { x, y -> stick(PadStick.LEFT, x, y) }
                DPad(Modifier.size(clusterSize).align(Alignment.Bottom), onDown = ::press, onUp = ::release)
                Column(
                    Modifier.width(110.dp).fillMaxHeight(),
                    verticalArrangement = Arrangement.spacedBy(12.dp, Alignment.CenterVertically),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    MonoLabel(profile.active().name, maxLines = 1)
                    PadKey("Select", Modifier.width(96.dp).height(36.dp), onDown = { press(PadButton.SELECT) }, onUp = { release(PadButton.SELECT) })
                    PadKey("Start", Modifier.width(96.dp).height(36.dp), onDown = { press(PadButton.START) }, onUp = { release(PadButton.START) })
                }
                FaceButtons(Modifier.size(clusterSize).align(Alignment.Top), onDown = ::press, onUp = ::release)
                Thumbstick(Modifier.size(stickSize).align(Alignment.Bottom)) { x, y -> stick(PadStick.RIGHT, x, y) }
            }
        }
    }

    editing?.let { m ->
        MappingEditor(
            initial = m,
            onSave = {
                setProfile(GamepadProfile(selected = GamepadPresets.CUSTOM, custom = it))
                editing = null
            },
            onDismiss = { editing = null },
        )
    }
}

/** A held button: down on touch, up when that finger lifts (even if it slid off). */
@Composable
private fun PadKey(
    label: String,
    modifier: Modifier,
    color: Color? = null,
    shape: androidx.compose.ui.graphics.Shape = RectangleShape,
    onDown: () -> Unit,
    onUp: () -> Unit,
) {
    var pressed by remember { mutableStateOf(false) }
    val down by rememberUpdatedState(onDown)
    val up by rememberUpdatedState(onUp)
    val c = Bp.c
    // Blueprint key: an ink outline on the panel; it fills pencil-yellow while held.
    val fill = when {
        pressed -> c.pencil
        color != null -> color.copy(alpha = 0.85f)
        else -> c.panel
    }
    Box(
        modifier
            .background(fill, shape)
            .border(if (pressed) 2.dp else 1.5.dp, if (pressed) c.deep else c.ink, shape)
            .pointerInput(Unit) {
                awaitEachGesture {
                    val first = awaitFirstDown(requireUnconsumed = false)
                    first.consume()
                    pressed = true
                    down()
                    try {
                        while (true) {
                            val e = awaitPointerEvent()
                            val mine = e.changes.firstOrNull { it.id == first.id }
                            e.changes.forEach { it.consume() }
                            if (mine == null || !mine.pressed) break
                        }
                    } finally {
                        pressed = false
                        up()
                    }
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Text(
            label.uppercase(),
            style = BpType.monoStrong,
            fontWeight = FontWeight.Bold,
            color = if (pressed || color != null) c.deep else c.ink,
            maxLines = 1,
        )
    }
}

@Composable
private fun FaceButtons(modifier: Modifier, onDown: (PadButton) -> Unit, onUp: (PadButton) -> Unit) {
    BoxWithConstraints(modifier) {
        val cell = maxWidth / 3
        @Composable
        fun face(b: PadButton, color: Color, col: Int, row: Int) {
            PadKey(
                b.label,
                Modifier.padding(start = cell * col, top = cell * row).size(cell),
                color = color,
                shape = CircleShape,
                onDown = { onDown(b) },
                onUp = { onUp(b) },
            )
        }
        face(PadButton.Y, FaceY, 1, 0)
        face(PadButton.X, FaceX, 0, 1)
        face(PadButton.B, FaceB, 2, 1)
        face(PadButton.A, FaceA, 1, 2)
    }
}

/** One touch area for the whole D-pad, so sliding between directions and diagonals works. */
@Composable
private fun DPad(modifier: Modifier, onDown: (PadButton) -> Unit, onUp: (PadButton) -> Unit) {
    val down by rememberUpdatedState(onDown)
    val up by rememberUpdatedState(onUp)
    var held by remember { mutableStateOf(emptySet<SwipeDirection>()) }
    val c = Bp.c
    val fill = c.panel
    val lit = c.pencil
    val outline = c.ink
    fun button(d: SwipeDirection) = when (d) {
        SwipeDirection.UP -> PadButton.DPAD_UP
        SwipeDirection.DOWN -> PadButton.DPAD_DOWN
        SwipeDirection.LEFT -> PadButton.DPAD_LEFT
        SwipeDirection.RIGHT -> PadButton.DPAD_RIGHT
    }
    fun apply(next: Set<SwipeDirection>) {
        (held - next).forEach { up(button(it)) }
        (next - held).forEach { down(button(it)) }
        held = next
    }
    Canvas(
        modifier.pointerInput(Unit) {
            awaitEachGesture {
                val first = awaitFirstDown(requireUnconsumed = false)
                val id: PointerId = first.id
                val r = size.width / 2f
                fun at(p: Offset) {
                    val (x, y) = StickMath.fromTouch(p.x - r, p.y - r, r)
                    // A small dead centre, then 8-way with hysteresis.
                    apply(if (x * x + y * y < 0.04f) emptySet() else StickMath.directions(x, y, held, on = 0.38f, off = 0.28f))
                }
                at(first.position)
                first.consume()
                try {
                    while (true) {
                        val e = awaitPointerEvent()
                        val mine = e.changes.firstOrNull { it.id == id }
                        e.changes.forEach { it.consume() }
                        if (mine == null || !mine.pressed) break
                        at(mine.position)
                    }
                } finally {
                    apply(emptySet())
                }
            }
        },
    ) {
        val w = size.width
        val arm = w / 3f
        val stroke = 1.5.dp.toPx()
        fun rect(col: Int, row: Int, on: Boolean) {
            drawRect(if (on) lit else fill, topLeft = Offset(arm * col, arm * row), size = androidx.compose.ui.geometry.Size(arm, arm))
            drawRect(outline, topLeft = Offset(arm * col, arm * row), size = androidx.compose.ui.geometry.Size(arm, arm), style = androidx.compose.ui.graphics.drawscope.Stroke(stroke))
        }
        rect(1, 0, SwipeDirection.UP in held)
        rect(0, 1, SwipeDirection.LEFT in held)
        rect(2, 1, SwipeDirection.RIGHT in held)
        rect(1, 2, SwipeDirection.DOWN in held)
        drawRect(fill, topLeft = Offset(arm, arm), size = androidx.compose.ui.geometry.Size(arm, arm))
    }
}

/** Fixed-centre thumbstick; reports a unit-circle vector (screen axes) and 0,0 on release. */
@Composable
private fun Thumbstick(modifier: Modifier, onMove: (Float, Float) -> Unit) {
    val move by rememberUpdatedState(onMove)
    var knob by remember { mutableStateOf(Offset.Zero) }
    val c = Bp.c
    val base = c.panel
    val ring = c.ink
    val cap = c.pencil
    val deep = c.deep
    Canvas(
        modifier.pointerInput(Unit) {
            awaitEachGesture {
                val first = awaitFirstDown(requireUnconsumed = false)
                val r = size.width / 2f
                val travel = r * 0.62f
                fun at(p: Offset) {
                    val (x, y) = StickMath.fromTouch(p.x - r, p.y - r, travel)
                    knob = Offset(x, y)
                    move(x, y)
                }
                at(first.position)
                first.consume()
                try {
                    while (true) {
                        val e = awaitPointerEvent()
                        val mine = e.changes.firstOrNull { it.id == first.id }
                        e.changes.forEach { it.consume() }
                        if (mine == null || !mine.pressed) break
                        at(mine.position)
                    }
                } finally {
                    knob = Offset.Zero
                    move(0f, 0f)
                }
            }
        },
    ) {
        val r = size.minDimension / 2f
        drawCircle(base, radius = r)
        val stroke = 1.5.dp.toPx()
        drawCircle(ring, radius = r - stroke / 2, style = androidx.compose.ui.graphics.drawscope.Stroke(stroke))
        // Dashed travel ring, like the touchpad's finger marks.
        drawCircle(
            ring.copy(alpha = 0.5f),
            radius = r * 0.62f,
            style = androidx.compose.ui.graphics.drawscope.Stroke(stroke, pathEffect = androidx.compose.ui.graphics.PathEffect.dashPathEffect(floatArrayOf(5.dp.toPx(), 4.dp.toPx()))),
        )
        val knobAt = center + knob * (r * 0.62f)
        drawCircle(cap, radius = r * 0.36f, center = knobAt)
        drawCircle(deep, radius = r * 0.36f, center = knobAt, style = androidx.compose.ui.graphics.drawscope.Stroke(2.dp.toPx()))
    }
}

/** Edits every control's binding; saved as this PC's custom mapping. */
@Composable
private fun MappingEditor(initial: GamepadMapping, onSave: (GamepadMapping) -> Unit, onDismiss: () -> Unit) {
    var m by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { PencilButton(onClick = { onSave(m) }) { Text("SAVE FOR THIS PC") } },
        dismissButton = { OutlineButton(onClick = onDismiss) { Text("CANCEL") } },
        title = { DisplayText("Custom mapping") },
        text = {
            Column(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
                    MonoLabel("Start from", modifier = Modifier.align(Alignment.CenterVertically))
                    GamepadPresets.all.forEach { (_, p) ->
                        MonoChip(p.name.substringBefore(" ("), dashed = true, onClick = { m = p.copy(name = "Custom") }, height = 36.dp)
                    }
                }
                PadStick.entries.forEach { s ->
                    ChoiceRow(s.label, GamepadKeys.label(m.stick(s)), GamepadKeys.stickChoices.map { it.first }) { i ->
                        val v = GamepadKeys.stickChoices[i].second
                        m = if (s == PadStick.LEFT) m.copy(left = v) else m.copy(right = v)
                    }
                }
                PadButton.entries.forEach { b ->
                    ChoiceRow(b.label, GamepadKeys.label(m.button(b)), GamepadKeys.buttonChoices.map { it.first }) { i ->
                        m = m.copy(buttons = m.buttons + (b to GamepadKeys.buttonChoices[i].second))
                    }
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Dead zone", Modifier.width(110.dp), style = MaterialTheme.typography.bodyMedium)
                    DiamondSlider(value = m.deadzone, onValueChange = { m = m.copy(deadzone = it) }, valueRange = 0f..0.5f, modifier = Modifier.weight(1f))
                    Text("${(m.deadzone * 100).roundToInt()}%", Modifier.width(40.dp), style = BpType.monoStrong, color = Bp.c.ink)
                }
            }
        },
    )
}

@Composable
private fun ChoiceRow(label: String, value: String, options: List<String>, onPick: (Int) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(label, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium)
        Box {
            OutlineButton(onClick = { open = true }, modifier = Modifier.width(150.dp).height(40.dp), contentPadding = PaddingValues(horizontal = 8.dp), textStyle = BpType.monoStrong) {
                Text(value.uppercase(), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            DropdownMenu(expanded = open, onDismissRequest = { open = false }, modifier = Modifier.heightIn(max = 320.dp)) {
                options.forEachIndexed { i, o ->
                    DropdownMenuItem(text = { Text(o) }, onClick = {
                        onPick(i)
                        open = false
                    })
                }
            }
        }
    }
}

