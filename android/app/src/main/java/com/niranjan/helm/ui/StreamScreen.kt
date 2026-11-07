package com.niranjan.helm.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.niranjan.helm.core.AbsMoveBatcher
import com.niranjan.helm.core.Orientation
import com.niranjan.helm.core.SecondScreen
import com.niranjan.helm.core.StreamFullscreen
import com.niranjan.helm.core.ViewTransform
import com.niranjan.helm.device.Settings
import kotlin.math.roundToInt
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

/** Phase 2 "Screen": live remote desktop with trackpad / direct-touch input. */
@Composable
fun StreamScreen(remote: RemoteViewModel, settings: Settings) {
    val vm: StreamViewModel = viewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val remoteUi by remote.ui.collectAsStateWithLifecycle()
    var transform by remember { mutableStateOf(ViewTransform(0f, 0f, 0f, 0f)) }
    var showKeyboard by remember { mutableStateOf(false) }
    var showControls by remember { mutableStateOf(true) }
    val absBatcher = remember { AbsMoveBatcher() }
    val extras: ExtrasViewModel = viewModel()
    val extrasUi by extras.ui.collectAsStateWithLifecycle()
    var penMode by remember { mutableStateOf(false) }
    val g3 by vm.g3.collectAsStateWithLifecycle()
    val metrics = androidx.compose.ui.platform.LocalContext.current.resources.displayMetrics
    val config = LocalConfiguration.current
    val landscape = config.screenWidthDp > config.screenHeightDp
    val fill by vm.fill.collectAsStateWithLifecycle()
    val landscapeLock by vm.landscapeLock.collectAsStateWithLifecycle()
    val exited by vm.fullscreenExited.collectAsStateWithLifecycle()
    val fullscreen = StreamFullscreen.active(landscape, exited)
    val toolbar = rememberToolbarState()
    // Set before the first stream.start so it already asks for the phone's landscape size.
    vm.phoneScreen = metrics.widthPixels to metrics.heightPixels
    vm.displayLandscape = landscape
    LaunchedEffect(landscape, metrics.widthPixels, metrics.heightPixels) {
        vm.onDisplay(landscape, metrics.widthPixels, metrics.heightPixels)
        if (!landscape) vm.resetFullscreenExit()
    }
    // G3 second screen / rotation lock: orientation follows the mirrored monitor; otherwise the
    // landscape lock keeps the phone sideways so the tab is full screen.
    val shownMonitor = ui.monitor ?: remoteUi.monitors.firstOrNull { it.isPrimary }
    val g3Orientation = SecondScreen.orientation(g3.rotation, shownMonitor?.width ?: 0, shownMonitor?.height ?: 0)
    RotationLockEffect(
        if (g3Orientation == Orientation.UNSPECIFIED && landscapeLock) Orientation.LANDSCAPE else g3Orientation,
        keepScreenOn = g3.secondScreen,
    )
    ImmersiveEffect(fullscreen)
    BackHandler(enabled = fullscreen) { vm.exitFullscreen() }
    val toggleControls: () -> Unit = { if (fullscreen) toolbar.toggle() else showControls = !showControls }
    LaunchedEffect(g3.secondScreen) { if (g3.secondScreen) showControls = false }

    // Stream only while this screen is visible and the app is in the foreground.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val obs = LifecycleEventObserver { _, e ->
            when (e) {
                Lifecycle.Event.ON_START -> vm.setActive(true)
                Lifecycle.Event.ON_STOP -> vm.setActive(false)
                else -> Unit
            }
        }
        lifecycle.addObserver(obs)
        if (lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)) vm.setActive(true)
        onDispose {
            lifecycle.removeObserver(obs)
            vm.setActive(false)
            remote.flushInput()
        }
    }

    // One combined move / scroll / absolute-move frame per ~16 ms.
    LaunchedEffect(Unit) {
        while (isActive) {
            delay(16)
            remote.flushInput()
            absBatcher.drain()?.let { remote.send(it) }
        }
    }

    LaunchedEffect(ui.videoWidth, ui.videoHeight) {
        if (ui.videoWidth > 0 && ui.videoHeight > 0) {
            transform = transform.withContent(ui.videoWidth.toFloat(), ui.videoHeight.toFloat())
        }
    }

    // Trackpad mode while zoomed: keep the PC cursor in view.
    val directInput = settings.streamDirectTouch || settings.streamSwipe
    LaunchedEffect(directInput) {
        if (directInput) return@LaunchedEffect
        vm.cursor.collect { c ->
            if (c != null && transform.canPan) {
                val (x, y) = transform.toView(c.x, c.y)
                transform = transform.ensureVisible(x, y, 48f)
            }
        }
    }

    LaunchedEffect(Unit) { vm.loadFill() }
    LaunchedEffect(fullscreen, fill) { transform = transform.withFill(fullscreen && fill) }

    // The video stage: size tracking and the gesture layer for the current mode.
    val stageModifier = Modifier
        .clipToBounds()
        .background(Color.Black)
        .onSizeChanged { s: IntSize -> transform = transform.withView(s.width.toFloat(), s.height.toFloat()) }
        .then(
            // Pen mode: the stylus overlay owns input; finger gestures are off.
            if (penMode && remoteUi.phase5) {
                Modifier
            } else if (g3.secondScreen) {
                Modifier.viewOnlyGestures { toggleControls() }
            } else if (g3.dragMode) {
                Modifier.dragSelectGestures(
                    direct = directInput,
                    speed = settings.touchpadSpeed,
                    scrollSpeed = settings.streamScrollSpeed,
                    reverseScroll = settings.streamReverseScroll,
                    haptics = settings.haptics,
                    remote = remote,
                    abs = absBatcher,
                    transform = { transform },
                    setTransform = { transform = it },
                    onThreeFingerTap = toggleControls,
                )
            } else Modifier.streamGestures(
                direct = directInput,
                swipe = settings.streamSwipe,
                speed = settings.touchpadSpeed,
                scrollSpeed = settings.streamScrollSpeed,
                reverseScroll = settings.streamReverseScroll,
                haptics = settings.haptics,
                remote = remote,
                abs = absBatcher,
                transform = { transform },
                setTransform = { transform = it },
                onThreeFingerTap = toggleControls,
            ),
        )
    val stage: @Composable BoxScope.() -> Unit = {
        VideoSurface(vm, ui, transform)
        CursorOverlay(vm, ui, transform, remoteUi.monitors.firstOrNull { it.id == ui.monitor?.id }?.width
            ?: remoteUi.monitors.firstOrNull { it.isPrimary }?.width)
        if (penMode && remoteUi.phase5) PenOverlay({ transform }, remote::send, Modifier.fillMaxSize())
        if (settings.streamShowStats) StatsOverlay(ui, Modifier.align(Alignment.TopStart).padding(8.dp))
        val saver = ui.saver
        if (saver != null && ui.relayChoice?.loweredForRelay == true && ui.streaming) {
            MonoLabel(
                com.niranjan.helm.core.DataSaver.notice(saver),
                color = Color.White,
                modifier = Modifier.align(Alignment.BottomCenter).background(Color.Black.copy(alpha = 0.6f)).padding(6.dp),
            )
        }
        if (ui.streaming && !settings.streamShowStats) StreamQualityBadge(ui, Modifier.align(Alignment.BottomEnd).padding(6.dp))
        if (!ui.streaming) StatusOverlay(ui, onRetry = { vm.setActive(false); vm.setActive(true) }, Modifier.align(Alignment.Center))
        if ((fullscreen || !showControls) && extrasUi.recording != null) RecordingDot(extrasUi, Modifier.align(Alignment.TopStart).padding(8.dp))
        if (!fullscreen && !showControls) {
            OutlineButton(filled = true,
                onClick = { showControls = true },
                modifier = Modifier.align(Alignment.TopEnd).padding(6.dp),
            ) { Text("Controls") }
        }
    }

    if (fullscreen) {
        // Landscape: the picture fills the display (padded clear of any cutout).
        Box(Modifier.fillMaxSize().background(Color.Black)) {
            Box(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.displayCutout).then(stageModifier), content = stage)
            EdgeRevealStrip(toolbar, Modifier.align(Alignment.TopCenter).windowInsetsPadding(WindowInsets.displayCutout))
            FullscreenToolbar(
                state = toolbar,
                vm = vm,
                ui = ui,
                settings = settings,
                g3 = g3,
                monitors = remoteUi.monitors,
                windows = remoteUi.windows,
                fill = fill,
                fillUseful = transform.fillUseful,
                landscapeLock = landscapeLock,
                keyboardOn = showKeyboard,
                onKeyboard = { showKeyboard = it },
                penMode = penMode,
                penAvailable = remoteUi.phase5,
                onPen = { penMode = it },
                onExit = { vm.exitFullscreen() },
                modifier = Modifier.align(Alignment.TopCenter).windowInsetsPadding(WindowInsets.displayCutout),
            )
            if (settings.streamQuickKeys && !g3.secondScreen) {
                androidx.compose.animation.AnimatedVisibility(
                    visible = toolbar.visible && !showKeyboard,
                    enter = androidx.compose.animation.fadeIn(),
                    exit = androidx.compose.animation.fadeOut(),
                    modifier = Modifier.align(Alignment.BottomCenter).windowInsetsPadding(WindowInsets.displayCutout),
                ) { StreamQuickKeys(remote, settings.haptics, toolbar = toolbar) }
            }
            if (showKeyboard) FullscreenKeyboardSlot(remote, onClose = { showKeyboard = false }, Modifier.fillMaxSize())
        }
        return
    }

    Column(Modifier.fillMaxSize()) {
        if (showControls) {
            StreamControls(
                vm = vm,
                ui = ui,
                settings = settings,
                monitors = remoteUi.monitors,
                keyboardOn = showKeyboard,
                onKeyboard = { showKeyboard = it },
                onResetView = { transform = transform.reset() },
                onHide = { showControls = false },
                landscapeLock = landscapeLock,
                onLandscapeLock = vm::setLandscapeLock,
            )
            if (g3.supported) G3Controls(vm, g3, remoteUi.windows, remoteUi.monitors)
            if (settings.streamQuickKeys && !g3.secondScreen) StreamQuickKeys(remote, settings.haptics)
            if (remoteUi.phase5) {
                ExtrasBar(
                    ui = extrasUi,
                    audio = settings.streamAudio,
                    penMode = penMode,
                    onPen = { penMode = it },
                    onRecord = { extras.toggleRecording(settings.streamAudio) },
                )
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth().then(stageModifier), content = stage)
        if (showKeyboard) StreamKeyboard(remote)
    }
}

/** The SurfaceView, laid out at the zoomed/panned content rectangle. */
@Composable
private fun VideoSurface(vm: StreamViewModel, ui: StreamUi, transform: ViewTransform) {
    AndroidView(
        factory = { ctx ->
            SurfaceView(ctx).apply {
                holder.addCallback(object : SurfaceHolder.Callback {
                    override fun surfaceCreated(h: SurfaceHolder) = vm.setSurface(h.surface)
                    override fun surfaceChanged(h: SurfaceHolder, format: Int, w: Int, hgt: Int) = Unit
                    override fun surfaceDestroyed(h: SurfaceHolder) = vm.setSurface(null)
                })
            }
        },
        update = { v ->
            // Decode at the stream's size; the compositor scales it to the view.
            if (ui.videoWidth > 0 && ui.videoHeight > 0) v.holder.setFixedSize(ui.videoWidth, ui.videoHeight)
        },
        modifier = Modifier.layout { measurable, constraints ->
            val r = transform.contentRect
            val w = if (transform.isValid) r.width.roundToInt().coerceIn(1, MAX_PX) else constraints.maxWidth
            val h = if (transform.isValid) r.height.roundToInt().coerceIn(1, MAX_PX) else constraints.maxHeight
            val p = measurable.measure(Constraints.fixed(w, h))
            layout(constraints.maxWidth, constraints.maxHeight) {
                if (transform.isValid) p.place(IntOffset(r.left.roundToInt(), r.top.roundToInt())) else p.place(0, 0)
            }
        },
    )
}

private const val MAX_PX = 16_000

/** The PC cursor (`0x04` position + `0x05` image), drawn over the video. */
@Composable
private fun CursorOverlay(vm: StreamViewModel, ui: StreamUi, transform: ViewTransform, monitorWidth: Int?) {
    val cursor by vm.cursor.collectAsStateWithLifecycle()
    val version by vm.cursorShapeVersion.collectAsStateWithLifecycle()
    val c = cursor ?: return
    if (!c.visible || !transform.isValid) return
    // Cursor images come in monitor pixels; the stream may be scaled down from the monitor.
    val monitorScale = if (monitorWidth != null && monitorWidth > 0 && ui.videoWidth > 0) ui.videoWidth.toFloat() / monitorWidth else 1f
    val img = remember(version, c.shapeId) { vm.cursorImages[c.shapeId] }
    Canvas(Modifier.fillMaxSize()) {
        val (x, y) = transform.toView(c.x, c.y)
        if (img != null) {
            val s = transform.scale * monitorScale
            val bw = (img.bitmap.width * s).roundToInt().coerceAtLeast(1)
            val bh = (img.bitmap.height * s).roundToInt().coerceAtLeast(1)
            drawImage(
                image = img.bitmap.asImageBitmap(),
                dstOffset = IntOffset((x - img.hotX * s).roundToInt(), (y - img.hotY * s).roundToInt()),
                dstSize = IntSize(bw, bh),
            )
        } else {
            drawCircle(Color.White, radius = 6.dp.toPx(), center = Offset(x, y))
            drawCircle(Color.Black, radius = 6.dp.toPx(), center = Offset(x, y), style = Stroke(1.5f.dp.toPx()))
        }
    }
}

@Composable
private fun StatsOverlay(ui: StreamUi, modifier: Modifier) {
    val s = ui.stats
    val info = ui.info
    val lines = buildList {
        add("${ui.videoWidth}×${ui.videoHeight} · ${info?.encoder ?: "?"} encoder")
        if (s != null) {
            add("${s.decodedFps} fps · ${"%.1f".format(s.rxKbps / 1000f)} Mbps · rtt ${s.rttMs} ms")
            add("dropped ${s.dropped}/s · ${ui.totalDropped} total")
        }
        add("decoder ${ui.codecName ?: "—"}${if (ui.lowLatency) " · low-latency" else ""}")
        if (ui.reconnects > 0) add("reconnects ${ui.reconnects}")
    }
    Text(
        lines.joinToString("\n"),
        color = Color.White,
        style = MaterialTheme.typography.labelSmall,
        modifier = modifier.background(Color(0x99000000), RoundedCornerShape(2.dp)).padding(6.dp),
    )
}

/** Small signal badge over the stream: quality, fps and bitrate (hidden while Stats is on). */
@Composable
private fun StreamQualityBadge(ui: StreamUi, modifier: Modifier) {
    val s = ui.stats ?: return
    val q = com.niranjan.helm.core.LinkQuality.of(s.rttMs.toLong(), s.dropped, s.decodedFps) ?: return
    val c = Bp.c
    val dot = when (q) {
        com.niranjan.helm.core.LinkQuality.GOOD -> c.ok
        com.niranjan.helm.core.LinkQuality.OK -> c.pencil
        com.niranjan.helm.core.LinkQuality.WEAK -> c.red
    }
    androidx.compose.foundation.layout.Row(
        modifier.background(Color(0x99000000), RoundedCornerShape(2.dp)).padding(horizontal = 6.dp, vertical = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        MonoLabel("● ", color = dot)
        MonoLabel(
            "${q.label} · ${com.niranjan.helm.core.LinkQuality.streamText(s.decodedFps, s.rxKbps)}" + (if (ui.saver != null) " · saver" else ""),
            color = Color.White.copy(alpha = 0.85f),
            maxLines = 1,
        )
    }
}

@Composable
private fun StatusOverlay(ui: StreamUi, onRetry: () -> Unit, modifier: Modifier) {
    Column(
        modifier.background(Color(0xCC000000), RoundedCornerShape(2.dp)).padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (!ui.supported) {
            Text("This PC's Helm agent does not offer screen streaming yet. Update Helm on the PC.", color = Color.White)
            return@Column
        }
        CircularProgressIndicator()
        Text(ui.status, color = Color.White, modifier = Modifier.padding(top = 8.dp))
        ui.error?.let { Text(it, color = Color(0xFFFFB4AB), style = MaterialTheme.typography.bodySmall) }
        PencilButton(onClick = onRetry, modifier = Modifier.padding(top = 8.dp)) { Text("Retry now") }
    }
}

/** Phase 5 controls under the stream controls: PC screen recording and pen mode. */
@Composable
private fun ExtrasBar(ui: ExtrasUi, audio: Boolean, penMode: Boolean, onPen: (Boolean) -> Unit, onRecord: () -> Unit) {
    androidx.compose.foundation.layout.Row(
        Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 2.dp),
        horizontalArrangement = androidx.compose.foundation.layout.Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val rec = ui.recording != null
        androidx.compose.material3.FilterChip(
            selected = rec,
            enabled = !ui.recordBusy,
            onClick = onRecord,
            label = { Text(if (rec) "Stop recording" else if (audio) "Record (with sound)" else "Record") },
        )
        if (rec) RecordingDot(ui, Modifier)
        androidx.compose.material3.FilterChip(
            selected = penMode,
            onClick = { onPen(!penMode) },
            label = { Text(if (penMode) "Pen mode (fingers off)" else "Pen") },
        )
    }
}

/** Red dot + elapsed time while the PC records. */
@Composable
private fun RecordingDot(ui: ExtrasUi, modifier: Modifier) {
    var now by remember { mutableStateOf(android.os.SystemClock.elapsedRealtime()) }
    LaunchedEffect(ui.recordingSince) {
        while (isActive) {
            now = android.os.SystemClock.elapsedRealtime()
            delay(500)
        }
    }
    val secs = ((now - ui.recordingSince) / 1000).coerceAtLeast(0)
    Text(
        "● REC ${secs / 60}:${"%02d".format(secs % 60)}",
        color = Color(0xFFFF5252),
        style = MaterialTheme.typography.labelMedium,
        modifier = modifier.background(Color(0x99000000), RoundedCornerShape(2.dp)).padding(horizontal = 6.dp, vertical = 2.dp),
    )
}
