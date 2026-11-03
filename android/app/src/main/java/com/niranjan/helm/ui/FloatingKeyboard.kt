package com.niranjan.helm.ui

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.niranjan.helm.core.InputFrame
import com.niranjan.helm.core.KbdGeometry
import com.niranjan.helm.core.KbdKey
import com.niranjan.helm.core.KbdPlacement
import com.niranjan.helm.core.PcKeyboard
import com.niranjan.helm.core.StickyMods
import com.niranjan.helm.core.Vk
import com.niranjan.helm.device.PadStore
import kotlin.math.roundToInt
import androidx.compose.foundation.layout.RowScope

/** Everything the floating keyboard remembers: geometry, sticky modifiers, opacity panel. */
class FloatingKeyboardState(initial: KbdGeometry, private val onPersist: (KbdGeometry) -> Unit) {
    var geometry by mutableStateOf(initial.sane())
    val sticky = StickyMods()

    /** Bumped whenever [sticky] changes, so keys redraw. */
    var rev by mutableIntStateOf(0)
        private set
    var opacityOpen by mutableStateOf(false)

    fun tapModifier(vk: Int) {
        sticky.tap(vk, SystemClock.uptimeMillis())
        rev++
    }

    /** Modifiers to press with the next key; clears the one-shot ones. */
    fun consumeModifiers(): List<Int> = sticky.consume().also { rev++ }

    fun persist() = onPersist(geometry)
}

/** A [FloatingKeyboardState] that loads and saves its position, size and opacity on the phone. */
@Composable
fun rememberFloatingKeyboardState(): FloatingKeyboardState {
    val store = PadStore.get(LocalContext.current)
    return remember(store) { FloatingKeyboardState(store.keyboard(), store::setKeyboard) }
}

/**
 * A draggable, resizable on-screen PC keyboard: full QWERTY with Esc/function row, arrows, and
 * sticky Ctrl / Alt / Shift / Win (tap = next key only, double-tap = lock). It draws over its
 * parent; the parent should be a box that fills the area the keyboard may move in. Key presses
 * come out as input frames through [onFrames] (send them with `RemoteViewModel.sendAll`), so
 * any screen can host it, for example the touchpad or the Screen tab.
 */
@Composable
fun FloatingKeyboard(
    state: FloatingKeyboardState,
    onFrames: (List<InputFrame>) -> Unit,
    onClose: () -> Unit,
    modifier: Modifier = Modifier,
    haptics: Boolean = true,
) {
    val c = Bp.c
    val density = LocalDensity.current
    val haptic = LocalHapticFeedback.current
    BoxWithConstraints(modifier.fillMaxSize()) {
        val cw = constraints.maxWidth.toFloat()
        val ch = constraints.maxHeight.toFloat()
        val minW = with(density) { 300.dp.toPx() }
        val g = state.geometry
        val (w, h) = KbdPlacement.size(g, cw, ch, minW)
        val (ox, oy) = KbdPlacement.origin(g, cw, ch, minW)
        val cwNow by rememberUpdatedState(cw)
        val chNow by rememberUpdatedState(ch)
        val minWNow by rememberUpdatedState(minW)
        val wDp = with(density) { w.toDp() }
        val unitSp = (wDp.value / 15.5f * 0.42f).coerceIn(7f, 14f)
        val shiftOn = state.sticky.isActive(Vk.SHIFT).also { state.rev }

        Column(
            Modifier
                .offset { IntOffset(ox.roundToInt(), oy.roundToInt()) }
                .size(wDp, with(density) { h.toDp() })
                .alpha(g.opacity)
                .background(c.paper2)
                .border(1.5.dp, c.ink)
                .semantics { contentDescription = "PC keyboard" },
        ) {
            // Title bar: drag here to move the keyboard.
            Row(
                Modifier
                    .fillMaxWidth()
                    .height(30.dp)
                    .background(c.deep)
                    .pointerInput(Unit) {
                        var x = 0f
                        var y = 0f
                        detectDragGestures(
                            onDragStart = {
                                val (sx, sy) = KbdPlacement.origin(state.geometry, cwNow, chNow, minWNow)
                                x = sx
                                y = sy
                            },
                            onDragEnd = { state.persist() },
                            onDragCancel = { state.persist() },
                        ) { change, drag ->
                            change.consume()
                            x += drag.x
                            y += drag.y
                            state.geometry = KbdPlacement.moved(state.geometry, x, y, cwNow, chNow, minWNow)
                        }
                    }
                    .padding(horizontal = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                MonoLabel("⠿ PC keyboard", color = c.ink, modifier = Modifier.weight(1f), maxLines = 1)
                MonoChip("Opacity", onClick = { state.opacityOpen = !state.opacityOpen }, selected = state.opacityOpen, height = 24.dp)
                MonoChip("Close", onClick = onClose, height = 24.dp)
            }
            if (state.opacityOpen) {
                Row(
                    Modifier.fillMaxWidth().height(30.dp).padding(horizontal = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    MonoLabel("Opacity", maxLines = 1)
                    DiamondSlider(
                        value = state.geometry.opacity,
                        onValueChange = { state.geometry = state.geometry.copy(opacity = it).sane() },
                        onValueChangeFinished = { state.persist() },
                        valueRange = KbdGeometry.MIN_OPACITY..1f,
                        modifier = Modifier.weight(1f),
                    )
                }
            }
            Column(
                Modifier.weight(1f).fillMaxWidth().padding(3.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                PcKeyboard.rows.forEach { row ->
                    Row(Modifier.weight(1f).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                        row.forEach { key ->
                            KeyCap(
                                key = key,
                                active = key.isModifier && state.sticky.isActive(key.vk).also { state.rev },
                                locked = key.isModifier && state.sticky.isLocked(key.vk).also { state.rev },
                                shiftOn = shiftOn,
                                fontSp = unitSp,
                                onTap = {
                                    if (haptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                    if (key.isModifier) {
                                        state.tapModifier(key.vk)
                                    } else {
                                        onFrames(PcKeyboard.frames(key, state.consumeModifiers()))
                                    }
                                },
                            )
                        }
                    }
                }
            }
        }

        // Resize handle: drag the bottom-right corner.
        Box(
            Modifier
                .offset { IntOffset((ox + w).roundToInt() - 30.dp.roundToPx(), (oy + h).roundToInt() - 30.dp.roundToPx()) }
                .size(30.dp)
                .pointerInput(Unit) {
                    var width = 0f
                    detectDragGestures(
                        onDragStart = { width = KbdPlacement.size(state.geometry, cwNow, chNow, minWNow).first },
                        onDragEnd = { state.persist() },
                        onDragCancel = { state.persist() },
                    ) { change, drag ->
                        change.consume()
                        width += drag.x
                        state.geometry = KbdPlacement.resized(state.geometry, width, cwNow, chNow, minWNow)
                    }
                }
                .semantics { contentDescription = "Resize keyboard" },
            contentAlignment = Alignment.BottomEnd,
        ) {
            Text("◢", color = c.pencil, fontSize = 18.sp, modifier = Modifier.padding(end = 3.dp, bottom = 1.dp))
        }
    }
}

@Composable
private fun RowScope.KeyCap(
    key: KbdKey,
    active: Boolean,
    locked: Boolean,
    shiftOn: Boolean,
    fontSp: Float,
    onTap: () -> Unit,
) {
    val c = Bp.c
    val fill = if (active) c.pencil else c.panel
    val border = when {
        locked -> Modifier.border(3.dp, c.ink)
        active -> Modifier.border(2.dp, c.deep)
        else -> Modifier.border(1.dp, c.ink.copy(alpha = 0.7f))
    }
    Box(
        Modifier
            .weight(key.weight)
            .fillMaxHeight()
            .background(fill)
            .then(border)
            .clickable(role = Role.Button, onClick = onTap)
            .semantics { contentDescription = key.label + if (locked) " locked" else if (active) " held" else "" },
        contentAlignment = Alignment.Center,
    ) {
        val label = if (shiftOn && key.shifted != null) key.shifted else key.label
        Text(
            if (label.length == 1) label.uppercase() else label,
            style = BpType.monoStrong.copy(fontSize = fontSp.sp, lineHeight = (fontSp + 2f).sp),
            color = if (active) c.deep else c.ink,
            textAlign = TextAlign.Center,
            maxLines = 1,
        )
    }
}
