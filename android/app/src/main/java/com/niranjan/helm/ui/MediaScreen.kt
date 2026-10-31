package com.niranjan.helm.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.PowerSchedule
import kotlinx.coroutines.delay

private fun fmt(ms: Long): String {
    val s = (ms / 1000).coerceAtLeast(0)
    return if (s >= 3600) "%d:%02d:%02d".format(s / 3600, (s % 3600) / 60, s % 60) else "%d:%02d".format(s / 60, s % 60)
}

private fun powerLabel(a: String) = when (a) {
    "sign_out" -> "Sign out"
    "shutdown" -> "Shut down"
    else -> a.replaceFirstChar { it.uppercase() }
}

@Composable
fun MediaScreen(remote: RemoteViewModel, landscape: Boolean) {
    if (landscape) {
        Row(Modifier.fillMaxSize().padding(12.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) { NowPlaying(remote) }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Controls(remote)
            }
        }
    } else {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            NowPlaying(remote)
            Controls(remote)
        }
    }
}

@Composable
private fun NowPlaying(remote: RemoteViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val m = ui.media
    SectionCard("Now playing") {
        if (m == null) {
            HintText("Nothing is playing on the PC.")
            return@SectionCard
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            val art = ui.artwork
            if (art != null) Image(art.asImageBitmap(), contentDescription = "Artwork", modifier = Modifier.size(96.dp))
            else LetterIcon(m.title.ifBlank { "♪" }, 96.dp)
            Spacer(Modifier.width(16.dp))
            Column(Modifier.weight(1f)) {
                Text(m.title.ifBlank { "Unknown title" }, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(m.artist, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                if (m.album.isNotBlank()) Text(m.album, style = MaterialTheme.typography.bodySmall, maxLines = 1)
            }
        }
        val dur = m.durationMs
        if (dur != null && dur > 0) {
            // Extrapolate the position while playing.
            val pos by produceState(m.positionMs ?: 0L, m, ui.mediaAtMs) {
                while (true) {
                    val base = m.positionMs ?: 0L
                    value = if (m.isPlaying) (base + System.currentTimeMillis() - ui.mediaAtMs).coerceAtMost(dur) else base
                    delay(500)
                }
            }
            var seeking by remember { mutableStateOf<Float?>(null) }
            Slider(
                value = seeking ?: (pos.toFloat() / dur).coerceIn(0f, 1f),
                onValueChange = { if (m.canSeek) seeking = it },
                onValueChangeFinished = { seeking?.let { remote.seek((it * dur).toLong()) }; seeking = null },
                enabled = m.canSeek,
            )
            Row {
                Text(fmt(pos), style = MaterialTheme.typography.bodySmall)
                Spacer(Modifier.weight(1f))
                Text(fmt(dur), style = MaterialTheme.typography.bodySmall)
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceEvenly) {
            FilledTonalButton(onClick = { remote.media("previous") }, enabled = m.canPrev) { Text("⏮") }
            FilledTonalButton(onClick = { remote.media("toggle_play_pause") }) { Text(if (m.isPlaying) "⏸" else "▶") }
            FilledTonalButton(onClick = { remote.media("next") }, enabled = m.canNext) { Text("⏭") }
        }
    }
}

@Composable
private fun Controls(remote: RemoteViewModel) {
    val gate = LocalAdminGate.current
    val ui by remote.ui.collectAsStateWithLifecycle()
    var confirm by remember { mutableStateOf<String?>(null) }

    SectionCard("Volume") {
        val a = ui.audio
        if (a == null) {
            HintText("Volume not available.")
        } else {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Slider(
                    value = a.level,
                    onValueChange = { remote.setVolume(it, dragging = true) },
                    onValueChangeFinished = { remote.setVolume(a.level, dragging = false) },
                    modifier = Modifier.weight(1f),
                )
                Text("${(a.level * 100).toInt()}%", modifier = Modifier.width(48.dp))
                TextButton(onClick = remote::toggleMute) { Text(if (a.muted) "Unmute" else "Mute") }
            }
        }
    }
    val supported = ui.displays.filter { it.supported }
    if (supported.isNotEmpty()) {
        SectionCard("Brightness") {
            supported.forEach { d ->
                Text(d.name, style = MaterialTheme.typography.bodySmall)
                Slider(value = d.level / 100f, onValueChange = { remote.setBrightness(d.id, (it * 100).toInt()) })
            }
        }
    }
    SectionCard("Power") {
        val p = ui.power
        if (p != null) {
            val left by produceState(p.dueUnixMs - System.currentTimeMillis(), p) {
                while (true) {
                    value = p.dueUnixMs - System.currentTimeMillis()
                    delay(1000)
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("${powerLabel(p.action)} in ${fmt(left)}", modifier = Modifier.weight(1f), color = MaterialTheme.colorScheme.error)
                OutlinedButton(onClick = remote::cancelPower) { Text("Cancel") }
            }
            Spacer(Modifier.size(8.dp))
        }
        PowerSchedule.ACTIONS.chunked(3).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                row.forEach { a ->
                    OutlinedButton(onClick = { confirm = a }, modifier = Modifier.weight(1f)) { Text(powerLabel(a), maxLines = 1) }
                }
            }
        }
    }

    confirm?.let { action ->
        val delays = listOf(0 to "Now", 60 to "In 1 minute", 300 to "In 5 minutes", 900 to "In 15 minutes", 1800 to "In 30 minutes", 3600 to "In 1 hour")
        var choice by remember { mutableIntStateOf(0) }
        AlertDialog(
            onDismissRequest = { confirm = null },
            title = { Text("${powerLabel(action)} the PC?") },
            text = {
                Column {
                    delays.forEach { (s, label) ->
                        Row(
                            Modifier.fillMaxWidth().selectable(selected = choice == s, onClick = { choice = s }),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            RadioButton(selected = choice == s, onClick = { choice = s })
                            Text(label)
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { confirm = null; gate("${powerLabel(action)} the PC") { remote.power(action, choice.takeIf { it > 0 }) } }) { Text(powerLabel(action)) }
            },
            dismissButton = { TextButton(onClick = { confirm = null }) { Text("Cancel") } },
        )
    }
}

