package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.MomentumScroller
import com.niranjan.helm.core.SideSwipeMode
import com.niranjan.helm.device.GestureSettings
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/** Runs a [MomentumScroller] once per display frame after a two-finger flick. */
class MomentumRunner internal constructor(
    private val scope: CoroutineScope,
    private val onStep: () -> (Float, Float) -> Unit,
) {
    private val scroller = MomentumScroller()
    private var job: Job? = null

    fun start(vx: Float, vy: Float) {
        cancel()
        if (!scroller.start(vx, vy)) return
        job = scope.launch {
            var last = withFrameMillis { it }
            while (isActive) {
                val now = withFrameMillis { it }
                val d = scroller.step((now - last).toFloat()) ?: break
                last = now
                onStep()(d.first, d.second)
            }
        }
    }

    fun cancel() {
        job?.cancel()
        job = null
        scroller.stop()
    }
}

@Composable
fun rememberMomentum(onStep: (Float, Float) -> Unit): MomentumRunner {
    val scope = rememberCoroutineScope()
    val step by rememberUpdatedState(onStep)
    val runner = remember(scope) { MomentumRunner(scope) { step } }
    DisposableEffect(runner) { onDispose { runner.cancel() } }
    return runner
}

@Composable
fun GestureOptionsDialog(g: GestureSettings, onChange: (GestureSettings) -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { PencilButton(onClick = onDismiss) { Text("DONE") } },
        title = { DisplayText("Touchpad gestures") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                ToggleRow("Three-finger swipes", "Up = Task View · down = show desktop", g.threeFinger) {
                    onChange(g.copy(threeFinger = it))
                }
                Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                    MonoLabel("Swipe left / right", color = Bp.c.ink)
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        BpFilterChip(
                            selected = g.sideSwipe == SideSwipeMode.APPS,
                            onClick = { onChange(g.copy(sideSwipe = SideSwipeMode.APPS)) },
                            label = { Text("Switch apps") },
                            enabled = g.threeFinger,
                        )
                        BpFilterChip(
                            selected = g.sideSwipe == SideSwipeMode.DESKTOPS,
                            onClick = { onChange(g.copy(sideSwipe = SideSwipeMode.DESKTOPS)) },
                            label = { Text("Switch desktops") },
                            enabled = g.threeFinger,
                        )
                    }
                }
                ToggleRow("Momentum scrolling", "Keep scrolling after a two-finger flick; touch to stop", g.momentum) {
                    onChange(g.copy(momentum = it))
                }
                ToggleRow("Precision mode", "Slow, steady pointer with no acceleration", g.precision) {
                    onChange(g.copy(precision = it))
                }
            }
        },
    )
}

@Composable
private fun ToggleRow(title: String, subtitle: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.titleMedium, color = Bp.c.ink)
            Text(subtitle, style = MaterialTheme.typography.bodySmall, color = Bp.c.ink2)
        }
        Switch(checked = checked, onCheckedChange = onChange)
    }
}
