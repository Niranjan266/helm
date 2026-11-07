package com.niranjan.helm.ui

import android.app.Activity
import android.content.Context
import android.content.ContextWrapper
import android.os.Build
import android.view.WindowManager
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.core.StreamPreset
import com.niranjan.helm.core.WindowEntry
import com.niranjan.helm.core.WindowThumb
import com.niranjan.helm.device.Settings
import kotlinx.coroutines.delay

private const val AUTO_HIDE_MS = 3_000L

/** Visibility of the full-screen toolbar: shown on an edge tap, hidden again after 3 s idle. */
@Stable
class ToolbarState {
    var visible by mutableStateOf(true)
    /** Open menus / dialogs; the toolbar stays while this is above zero. */
    var holds by mutableIntStateOf(0)
    /** Bumped on every touch of the toolbar so the idle timer restarts. */
    var touches by mutableIntStateOf(0)

    fun toggle() {
        visible = !visible
        touches++
    }

    fun touch() {
        touches++
    }
}

@Composable
fun rememberToolbarState(): ToolbarState {
    val state = remember { ToolbarState() }
    LaunchedEffect(state.visible, state.holds, state.touches) {
        if (state.visible && state.holds == 0) {
            delay(AUTO_HIDE_MS)
            state.visible = false
        }
    }
    return state
}

private fun Context.activityOrNull(): Activity? {
    var c: Context? = this
    while (c is ContextWrapper) {
        if (c is Activity) return c
        c = c.baseContext
    }
    return null
}

/**
 * Immersive full screen while [active]: system bars hidden (a swipe from the edge reveals them
 * briefly), content allowed into the display cutout area (callers pad by the cutout inset) and
 * the screen kept on.
 */
@Composable
fun ImmersiveEffect(active: Boolean) {
    val view = LocalView.current
    DisposableEffect(active) {
        val window = view.context.activityOrNull()?.window
        if (!active || window == null) return@DisposableEffect onDispose { }
        val controller = WindowCompat.getInsetsController(window, view)
        val oldBehavior = controller.systemBarsBehavior
        controller.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        controller.hide(WindowInsetsCompat.Type.systemBars())
        var oldCutout = 0
        if (Build.VERSION.SDK_INT >= 28) {
            val attrs = window.attributes
            oldCutout = attrs.layoutInDisplayCutoutMode
            attrs.layoutInDisplayCutoutMode = WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
            window.attributes = attrs
        }
        val hadKeepOn = window.attributes.flags and WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON != 0
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        onDispose {
            if (!hadKeepOn) window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
            if (Build.VERSION.SDK_INT >= 28) {
                val attrs = window.attributes
                attrs.layoutInDisplayCutoutMode = oldCutout
                window.attributes = attrs
            }
            controller.systemBarsBehavior = oldBehavior
            controller.show(WindowInsetsCompat.Type.systemBars())
        }
    }
}

/** A thin strip along the top edge: a tap shows / hides the toolbar. */
@Composable
fun EdgeRevealStrip(state: ToolbarState, modifier: Modifier = Modifier) {
    Box(
        modifier
            .fillMaxWidth()
            .height(28.dp)
            .pointerInput(Unit) { detectTapGestures(onTap = { state.toggle() }) },
        contentAlignment = Alignment.TopCenter,
    ) {
        if (!state.visible) {
            Box(
                Modifier
                    .padding(top = 6.dp)
                    .width(44.dp)
                    .height(4.dp)
                    .background(Bp.c.ink.copy(alpha = 0.45f), RoundedCornerShape(2.dp)),
            )
        }
    }
}

/**
 * The auto-hiding, translucent full-screen toolbar: quality, monitor / window, sound,
 * trackpad / touch, Fit / Fill, landscape lock, keyboard, exit.
 */
@Composable
fun FullscreenToolbar(
    state: ToolbarState,
    vm: StreamViewModel,
    ui: StreamUi,
    settings: Settings,
    g3: G3Ui,
    monitors: List<MonitorInfo>,
    windows: List<WindowEntry>,
    fill: Boolean,
    fillUseful: Boolean,
    landscapeLock: Boolean,
    keyboardOn: Boolean,
    onKeyboard: (Boolean) -> Unit,
    penMode: Boolean,
    penAvailable: Boolean,
    onPen: (Boolean) -> Unit,
    onExit: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = Bp.c
    val preset = StreamPreset.byName(settings.streamPreset)
    var pickWindow by remember { mutableStateOf(false) }
    if (pickWindow) {
        DisposableEffect(Unit) {
            state.holds++
            onDispose { state.holds-- }
        }
        LaunchedEffect(windows) { vm.loadThumbs(windows) }
        WindowPickerDialog(
            windows = WindowThumb.pickable(windows),
            g3 = g3,
            onPick = { vm.selectWindow(it); pickWindow = false },
            onDismiss = { pickWindow = false },
        )
    }
    AnimatedVisibility(visible = state.visible, enter = fadeIn(), exit = fadeOut(), modifier = modifier) {
        Row(
            Modifier
                .fillMaxWidth()
                .background(c.panel.copy(alpha = 0.86f))
                .border(1.5.dp, c.ink.copy(alpha = 0.7f))
                .pointerInput(Unit) {
                    // Any touch on the toolbar restarts the 3 s idle timer (never consumed).
                    awaitPointerEventScope {
                        while (true) {
                            awaitPointerEvent(PointerEventPass.Initial)
                            state.touch()
                        }
                    }
                }
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 8.dp, vertical = 4.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ToolbarPicker(
                state = state,
                label = preset.label,
                items = StreamPreset.entries.toList(),
                itemLabel = { "${it.label} · ${it.defaultKbps / 1000} Mbps" },
                onPick = vm::setPreset,
            )
            ToolbarPicker(
                state = state,
                label = ui.monitor?.name ?: "Primary screen",
                items = listOf<MonitorInfo?>(null) + monitors,
                itemLabel = { m -> m?.let { "${it.name} ${it.width}×${it.height}${if (it.isPrimary) " (primary)" else ""}" } ?: "Primary screen" },
                onPick = vm::selectMonitor,
            )
            if (g3.supported) {
                OutlineButton(onClick = { pickWindow = true }) { Text(if (g3.window != null) "Window ✓" else "Window…") }
            }
            if (g3.supported) {
                BpFilterChip(selected = g3.dragMode, onClick = { vm.setDragMode(!g3.dragMode) }, label = { Text("Drag mode") })
            }
            if (penAvailable) {
                BpFilterChip(selected = penMode, onClick = { onPen(!penMode) }, label = { Text("Pen mode") })
            }
            BpFilterChip(selected = settings.streamAudio, onClick = { vm.setAudio(!settings.streamAudio) }, label = { Text("Sound") })
            BpFilterChip(selected = !settings.streamDirectTouch && !settings.streamSwipe, onClick = { vm.setDirectTouch(false) }, label = { Text("Trackpad") })
            BpFilterChip(selected = settings.streamDirectTouch && !settings.streamSwipe, onClick = { vm.setDirectTouch(true) }, label = { Text("Touch") })
        BpFilterChip(selected = settings.streamSwipe, onClick = { vm.setSwipe() }, label = { Text("Swipe") })
            if (fillUseful) {
                BpFilterChip(selected = !fill, onClick = { vm.setFill(false) }, label = { Text("Fit") })
                BpFilterChip(selected = fill, onClick = { vm.setFill(true) }, label = { Text("Fill") })
            }
            BpFilterChip(selected = landscapeLock, onClick = { vm.setLandscapeLock(!landscapeLock) }, label = { Text("Lock landscape") })
            BpFilterChip(selected = keyboardOn, onClick = { onKeyboard(!keyboardOn) }, label = { Text("Keyboard") })
            PencilButton(onClick = onExit) { Text("Exit full screen") }
        }
    }
}

@Composable
private fun <T> ToolbarPicker(state: ToolbarState, label: String, items: List<T>, itemLabel: (T) -> String, onPick: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    if (open) {
        DisposableEffect(Unit) {
            state.holds++
            onDispose { state.holds-- }
        }
    }
    Box {
        OutlineButton(onClick = { open = true }) { Text(label) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            items.forEach { item ->
                DropdownMenuItem(text = { Text(itemLabel(item)) }, onClick = {
                    open = false
                    onPick(item)
                })
            }
        }
    }
}

/**
 * The keyboard in full screen. SWAP POINT: this is the single place that draws the on-screen
 * keyboard in full screen; replace the [StreamKeyboard] call below with the reusable
 * FloatingKeyboard composable when it lands.
 */
@Composable
fun FullscreenKeyboardSlot(remote: RemoteViewModel, onClose: () -> Unit, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxSize()) {
        FloatingKeyboard(rememberFloatingKeyboardState(), remote::sendAll, onClose)
    }
}
