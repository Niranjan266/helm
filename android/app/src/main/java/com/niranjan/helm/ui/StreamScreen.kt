package com.niranjan.helm.ui

import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilledTonalButton
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
    LaunchedEffect(settings.streamDirectTouch) {
        if (settings.streamDirectTouch) return@LaunchedEffect
        vm.cursor.collect { c ->
            if (c != null && transform.zoom > 1.01f) {
                val (x, y) = transform.toView(c.x, c.y)
                transform = transform.ensureVisible(x, y, 48f)
            }
        }
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
            )
        }
        Box(
            Modifier
                .weight(1f)
                .fillMaxWidth()
                .clipToBounds()
                .background(Color.Black)
                .onSizeChanged { s: IntSize -> transform = transform.withView(s.width.toFloat(), s.height.toFloat()) }
                .streamGestures(
                    direct = settings.streamDirectTouch,
                    speed = settings.touchpadSpeed,
                    haptics = settings.haptics,
                    remote = remote,
                    abs = absBatcher,
                    transform = { transform },
                    setTransform = { transform = it },
                    onThreeFingerTap = { showControls = !showControls },
                ),
        ) {
            VideoSurface(vm, ui, transform)
            CursorOverlay(vm, ui, transform, remoteUi.monitors.firstOrNull { it.id == ui.monitor?.id }?.width
                ?: remoteUi.monitors.firstOrNull { it.isPrimary }?.width)
            if (settings.streamShowStats) StatsOverlay(ui, Modifier.align(Alignment.TopStart).padding(8.dp))
            if (!ui.streaming) StatusOverlay(ui, onRetry = { vm.setActive(false); vm.setActive(true) }, Modifier.align(Alignment.Center))
            if (!showControls) {
                FilledTonalButton(
                    onClick = { showControls = true },
                    modifier = Modifier.align(Alignment.TopEnd).padding(6.dp),
                ) { Text("Controls") }
            }
        }
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
        modifier = modifier.background(Color(0x99000000), RoundedCornerShape(6.dp)).padding(6.dp),
    )
}

@Composable
private fun StatusOverlay(ui: StreamUi, onRetry: () -> Unit, modifier: Modifier) {
    Column(
        modifier.background(Color(0xCC000000), RoundedCornerShape(12.dp)).padding(16.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        if (!ui.supported) {
            Text("This PC's Helm agent does not offer screen streaming yet. Update Helm on the PC.", color = Color.White)
            return@Column
        }
        CircularProgressIndicator()
        Text(ui.status, color = Color.White, modifier = Modifier.padding(top = 8.dp))
        ui.error?.let { Text(it, color = Color(0xFFFFB4AB), style = MaterialTheme.typography.bodySmall) }
        Button(onClick = onRetry, modifier = Modifier.padding(top = 8.dp)) { Text("Retry now") }
    }
}
