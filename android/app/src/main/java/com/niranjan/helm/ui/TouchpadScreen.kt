package com.niranjan.helm.ui

import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.device.GyroSensor

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.KeyPress
import com.niranjan.helm.core.MouseButton
import com.niranjan.helm.core.TextDiff
import com.niranjan.helm.core.TouchpadMath
import com.niranjan.helm.core.Vk
import com.niranjan.helm.device.Settings
import kotlin.math.abs
import kotlin.math.max
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

private enum class TwoFinger { NONE, SCROLL, PINCH }

@Composable
fun TouchpadScreen(remote: RemoteViewModel, settings: Settings) {
    val haptic = LocalHapticFeedback.current
    val density = LocalDensity.current
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
    var text by remember { mutableStateOf("") }
    var autoShown by remember { mutableStateOf(false) }
    val currentSpeed by rememberUpdatedState(speed)
    val haptics by rememberUpdatedState(settings.haptics)

    // One combined move/scroll frame per ~16 ms (about 60 Hz).
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

    val slop = with(density) { 8.dp.toPx() }
    val notchPx = with(density) { 36.dp.toPx() }
    val pinchPx = with(density) { 60.dp.toPx() }

    Column(Modifier.fillMaxSize().padding(8.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = dragLock, onClick = { dragLock = remote.setDragLock(!dragLock) }, label = { Text("Drag lock") })
            val rui by remote.ui.collectAsStateWithLifecycle()
            if (rui.phase4 && GyroSensor.available(LocalContext.current)) {
                FilterChip(selected = airMouse, onClick = { airMouse = !airMouse }, label = { Text("Air mouse") })
            }
            Text("Speed", style = MaterialTheme.typography.bodySmall)
            Slider(
                value = speed,
                onValueChange = { speed = it },
                onValueChangeFinished = { remote.setTouchpadSpeed(speed) },
                valueRange = 0.4f..3f,
                modifier = Modifier.weight(1f),
            )
        }

        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .background(MaterialTheme.colorScheme.surfaceContainerHigh, RoundedCornerShape(16.dp))
                .pointerInput(Unit) {
                    awaitEachGesture {
                        val first = awaitFirstDown(requireUnconsumed = false)
                        val start = first.uptimeMillis
                        var last = start
                        var maxPointers = 1
                        var moved = 0f
                        var mode = TwoFinger.NONE
                        var centroidTravel = 0f
                        var spanTravel = 0f
                        var end = start
                        while (true) {
                            val event = awaitPointerEvent()
                            end = event.changes.maxOf { it.uptimeMillis }
                            val pressed = event.changes.filter { it.pressed }
                            if (pressed.isEmpty()) break
                            maxPointers = max(maxPointers, pressed.size)
                            if (pressed.size == 1 && maxPointers == 1) {
                                val c = pressed[0]
                                val d = c.position - c.previousPosition
                                moved += d.getDistance()
                                if (moved > slop) {
                                    val (dx, dy) = TouchpadMath.accelerate(d.x, d.y, (c.uptimeMillis - last).toFloat(), currentSpeed)
                                    remote.touchMove(dx, dy)
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
                            }
                            event.changes.forEach { it.consume() }
                        }
                        if (mode == TwoFinger.PINCH) remote.pinch(false)
                        val tap = moved < slop && end - start < 300
                        if (tap) {
                            if (haptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                            if (maxPointers >= 2) {
                                remote.click(MouseButton.RIGHT)
                            } else {
                                remote.click(MouseButton.LEFT)
                                dragLock = remote.isDragLocked
                            }
                        } else {
                            remote.flushInput()
                        }
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            HintText("1 finger: move · tap: click · 2 fingers: scroll / tap: right-click · pinch: zoom")
        }

        // Special keys: modifiers stick until the next key.
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("Ctrl" to Vk.CONTROL, "Alt" to Vk.ALT, "Shift" to Vk.SHIFT, "Win" to Vk.LWIN).forEach { (label, vk) ->
                FilterChip(selected = vk in mods, onClick = { if (vk in mods) mods.remove(vk) else mods.add(vk) }, label = { Text(label) })
            }
            val keys = listOf(
                "Esc" to Vk.ESCAPE, "Tab" to Vk.TAB, "←" to Vk.LEFT, "↑" to Vk.UP, "↓" to Vk.DOWN, "→" to Vk.RIGHT,
                "Enter" to Vk.RETURN, "Bksp" to Vk.BACK, "Del" to Vk.DELETE, "Home" to Vk.HOME, "End" to Vk.END,
                "PgUp" to Vk.PAGE_UP, "PgDn" to Vk.PAGE_DOWN,
            ) + (1..12).map { "F$it" to Vk.f(it) }
            keys.forEach { (label, vk) ->
                OutlinedButton(onClick = { press(vk) }, modifier = Modifier.width(if (label.length > 3) 72.dp else 56.dp)) { Text(label) }
            }
        }

        OutlinedTextField(
            value = text,
            onValueChange = { new ->
                if (mods.isNotEmpty() && new.length == text.length + 1 && new.startsWith(text)) {
                    // A letter typed with sticky modifiers becomes a shortcut (e.g. Ctrl+C).
                    val vk = Vk.forChar(new.last())
                    if (vk != null) {
                        press(vk)
                        return@OutlinedTextField
                    }
                }
                remote.sendAll(TextDiff.frames(text, new))
                // Keep the buffer short; reset silently.
                text = if (new.length > 80) "" else new
            },
            placeholder = { Text("Type on the PC") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text, imeAction = ImeAction.Send, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onSend = {
                press(Vk.RETURN)
                text = ""
            }),
            modifier = Modifier.fillMaxWidth().focusRequester(focus),
        )
    }
}
