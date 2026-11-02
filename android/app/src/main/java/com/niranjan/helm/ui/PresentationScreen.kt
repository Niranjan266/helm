package com.niranjan.helm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.PresentAction
import com.niranjan.helm.core.PresentTimer
import com.niranjan.helm.core.padToLaser

/** Volume keys go to the Presentation screen while it is open (set by the screen). */
object VolumeKeys {
    @Volatile
    var handler: ((up: Boolean) -> Boolean)? = null
    private var consumed = false

    /** From the activity: true = the key was used (do not change the volume). */
    fun dispatch(keyCode: Int, down: Boolean, repeat: Int): Boolean {
        val up = keyCode == android.view.KeyEvent.KEYCODE_VOLUME_UP
        if (!up && keyCode != android.view.KeyEvent.KEYCODE_VOLUME_DOWN) return false
        val h = handler ?: return false
        if (down && repeat == 0) consumed = h(up)
        return consumed
    }
}

@Composable
fun PresentationScreen(snackbar: androidx.compose.material3.SnackbarHostState) {
    val vm: PresentationViewModel = androidx.lifecycle.viewmodel.compose.viewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val view = LocalView.current

    DisposableEffect(Unit) {
        vm.start()
        view.keepScreenOn = true
        VolumeKeys.handler = { up -> vm.onVolumeKey(up) }
        onDispose {
            VolumeKeys.handler = null
            view.keepScreenOn = false
            vm.stop()
        }
    }
    LaunchedEffect(Unit) { vm.messages.collect { snackbar.showSnackbar(it) } }

    if (!ui.supported) {
        Column(Modifier.fillMaxSize().padding(16.dp)) {
            HintText("This PC's Helm agent does not offer presentation mode yet. Update Helm on the PC.")
        }
        return
    }

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        val s = ui.state
        SectionCard(s?.appLabel ?: "Presentation") {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    s?.slide?.label ?: "– / –",
                    style = MaterialTheme.typography.displaySmall,
                    fontWeight = FontWeight.Bold,
                    modifier = Modifier.weight(1f),
                )
                Column(horizontalAlignment = Alignment.End) {
                    Text(
                        PresentTimer.format(ui.elapsedMs),
                        style = MaterialTheme.typography.headlineMedium,
                    )
                    ui.remainingMs?.let {
                        Text(
                            PresentTimer.format(it) + " left",
                            color = if (it <= 60_000) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                }
            }
            if (s?.slide == null && s?.app == "powerpoint") {
                HintText(
                    when (s.slideStatus) {
                        "busy" -> "PowerPoint is busy; slide number will come back."
                        "no_presentation" -> "PowerPoint has no presentation open."
                        else -> "Slide number and notes are not available."
                    },
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                OutlineButton(onClick = vm::timerToggle) { Text(if (ui.timerRunning) "Pause" else "Start timer") }
                OutlineButton(onClick = vm::timerReset) { Text("Reset") }
            }
            Row(
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
                modifier = Modifier.horizontalScroll(rememberScrollState()),
            ) {
                Text("Countdown", style = MaterialTheme.typography.labelLarge)
                listOf(null, 5, 10, 15, 20, 30, 45).forEach { m ->
                    BpFilterChip(
                        selected = ui.countdownMin == m,
                        onClick = { vm.setCountdown(m) },
                        label = { Text(m?.toString() ?: "Off") },
                    )
                }
            }
        }

        Row(horizontalArrangement = Arrangement.spacedBy(12.dp), modifier = Modifier.fillMaxWidth()) {
            OutlineButton(
                onClick = { vm.go(PresentAction.PREV) },
                modifier = Modifier.weight(1f).height(120.dp),
            ) { Text("Prev", fontSize = 26.sp) }
            PencilButton(
                onClick = { vm.go(PresentAction.NEXT) },
                modifier = Modifier.weight(2f).height(120.dp),
            ) { Text("Next", fontSize = 30.sp) }
        }
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp), modifier = Modifier.fillMaxWidth()) {
            listOf(
                PresentAction.START to "Start",
                PresentAction.FIRST to "First",
                PresentAction.BLACK to "Black",
                PresentAction.LAST to "Last",
                PresentAction.END to "End",
            ).forEach { (a, label) ->
                TextButton(onClick = { vm.go(a) }, modifier = Modifier.weight(1f)) { Text(label) }
            }
        }

        LaserCard(vm, ui)

        SectionCard("Speaker notes") {
            val notes = s?.slide?.notes.orEmpty()
            if (notes.isBlank()) {
                HintText(if (s?.app == "powerpoint") "No notes on this slide." else "Notes are read from PowerPoint.")
            } else {
                Text(notes, style = MaterialTheme.typography.bodyLarge, modifier = Modifier.heightIn(max = 400.dp).verticalScroll(rememberScrollState()))
            }
        }

        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
            Text("Volume keys change slides", modifier = Modifier.weight(1f))
            Switch(checked = ui.volumeKeys, onCheckedChange = vm::setVolumeKeys)
        }
    }
}

@Composable
private fun LaserCard(vm: PresentationViewModel, ui: PresentUi) {
    SectionCard("Laser pointer") {
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
            if (vm.tiltAvailable) {
                BpFilterChip(selected = ui.laserMode == LaserMode.TILT, onClick = { vm.setLaserMode(LaserMode.TILT) }, label = { Text("Tilt") })
            }
            BpFilterChip(selected = ui.laserMode == LaserMode.TOUCH, onClick = { vm.setLaserMode(LaserMode.TOUCH) }, label = { Text("Touch") })
            Text("Trail", modifier = Modifier.padding(start = 8.dp))
            Switch(checked = ui.trail, onCheckedChange = vm::setTrail)
        }
        if (ui.monitors.size > 1) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp), modifier = Modifier.horizontalScroll(rememberScrollState())) {
                BpFilterChip(selected = ui.laserMonitor < 0, onClick = { vm.setLaserMonitor(-1) }, label = { Text("Main") })
                ui.monitors.forEachIndexed { i, m ->
                    BpFilterChip(
                        selected = ui.laserMonitor == i,
                        onClick = { vm.setLaserMonitor(i) },
                        label = { Text(m.name.ifBlank { "Monitor ${i + 1}" }) },
                    )
                }
            }
        }
        val pressed = ui.pointing
        if (ui.laserMode == LaserMode.TILT && vm.tiltAvailable) {
            HintText("Hold the button and point the top of the phone at the screen. Where it points when you press becomes the centre.")
            Box(Modifier.fillMaxWidth().padding(top = 8.dp), contentAlignment = Alignment.Center) {
                Box(
                    Modifier
                        .size(140.dp)
                        .clip(CircleShape)
                        .background(if (pressed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.errorContainer)
                        .pointerInput(Unit) {
                            awaitEachGesture {
                                awaitFirstDown(requireUnconsumed = false)
                                vm.laserDown()
                                do {
                                    val ev = awaitPointerEvent()
                                } while (ev.changes.any { it.pressed })
                                vm.laserUp()
                            }
                        },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (pressed) "Pointing" else "Hold to point",
                        textAlign = TextAlign.Center,
                        color = if (pressed) MaterialTheme.colorScheme.onError else MaterialTheme.colorScheme.onErrorContainer,
                    )
                }
            }
        } else {
            HintText("Drag on the pad; the dot follows your finger.")
            Box(
                Modifier
                    .fillMaxWidth()
                    .padding(top = 8.dp)
                    .aspectRatio(16f / 9f)
                    .clip(RoundedCornerShape(12.dp))
                    .background(MaterialTheme.colorScheme.surfaceVariant)
                    .pointerInput(Unit) {
                        awaitEachGesture {
                            val down = awaitFirstDown(requireUnconsumed = false)
                            val w = size.width.toFloat()
                            val h = size.height.toFloat()
                            padToLaser(down.position.x, down.position.y, w, h).let { (x, y) -> vm.laserTouch(x, y) }
                            do {
                                val ev = awaitPointerEvent()
                                val c = ev.changes.firstOrNull { it.pressed } ?: break
                                c.consume()
                                padToLaser(c.position.x, c.position.y, w, h).let { (x, y) -> vm.laserTouch(x, y) }
                            } while (true)
                            vm.laserUp()
                        }
                    },
            )
        }
    }
}
