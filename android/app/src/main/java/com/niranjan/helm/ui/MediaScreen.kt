package com.niranjan.helm.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.RadioButton
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.MediaState
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

/** Restart, shut down and sign out end the session on the PC: drawn red. */
private fun isDestructive(a: String) = a == "restart" || a == "shutdown" || a == "sign_out"

private fun glyph(name: String, path: String) = ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f)
    .addPath(addPathNodes(path), fill = SolidColor(androidx.compose.ui.graphics.Color.Black))
    .build()

private val PrevGlyph by lazy { glyph("Prev", "M6 6h2v12H6zM20 6v12L9 12z") }
private val NextGlyph by lazy { glyph("Next", "M16 6h2v12h-2zM4 6v12l11-6z") }
private val PauseGlyph by lazy { glyph("Pause", "M7 5h4v14H7zM13 5h4v14h-4z") }

/** "Spotify.exe" / "…!Spotify" / "C:\…\vlc.exe" -> "Spotify" / "vlc". */
private fun sourceName(appId: String): String =
    appId.substringAfterLast('!').substringAfterLast('\\').substringAfterLast('/').removeSuffix(".exe").removeSuffix(".EXE")

@Composable
fun MediaScreen(remote: RemoteViewModel, landscape: Boolean) {
    if (landscape) {
        Row(Modifier.fillMaxSize().padding(16.dp), horizontalArrangement = Arrangement.spacedBy(14.dp)) {
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState())) { NowPlaying(remote) }
            Column(Modifier.weight(1f).verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(14.dp)) {
                Controls(remote)
            }
        }
    } else {
        Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(16.dp), verticalArrangement = Arrangement.spacedBy(14.dp)) {
            NowPlaying(remote)
            Controls(remote)
        }
    }
}

@Composable
private fun NowPlaying(remote: RemoteViewModel) {
    val c = Bp.c
    val ui by remote.ui.collectAsStateWithLifecycle()
    val m: MediaState? = ui.media
    FigBox(fig = "3", title = "Now playing", trailing = m?.appId?.let(::sourceName)?.takeIf { it.isNotBlank() }, modifier = Modifier.fillMaxWidth()) {
        if (m == null) {
            HintText("Nothing is playing on the PC.")
            return@FigBox
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            val art = ui.artwork
            if (art != null) {
                Image(
                    art.asImageBitmap(),
                    contentDescription = "Album art",
                    contentScale = ContentScale.Crop,
                    modifier = Modifier.size(68.dp).border(1.5.dp, c.ink),
                )
            } else {
                LetterIcon(m.title.ifBlank { "♪" }, 68.dp)
            }
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
                DisplayText(m.title.ifBlank { "Unknown title" }, style = BpType.title.copy(fontSize = 17.sp))
                Text(m.artist, style = MaterialTheme.typography.bodyMedium.copy(fontSize = 13.sp), color = c.ink2, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (m.album.isNotBlank()) Text(m.album, style = MaterialTheme.typography.bodySmall, color = c.ink2, maxLines = 1, overflow = TextOverflow.Ellipsis)
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
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(fmt(seeking?.let { (it * dur).toLong() } ?: pos), style = BpType.mono, color = c.ink)
                DiamondSlider(
                    value = seeking ?: (pos.toFloat() / dur).coerceIn(0f, 1f),
                    onValueChange = { if (m.canSeek) seeking = it },
                    onValueChangeFinished = { seeking?.let { remote.seek((it * dur).toLong()) }; seeking = null },
                    enabled = m.canSeek,
                    modifier = Modifier.weight(1f),
                )
                Text(fmt(dur), style = BpType.mono, color = c.ink)
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(16.dp, Alignment.CenterHorizontally), verticalAlignment = Alignment.Top) {
            OutlineButton(
                onClick = { remote.media("previous") },
                enabled = m.canPrev,
                contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp),
                modifier = Modifier.size(52.dp).semantics { contentDescription = "Previous" },
            ) { Icon(PrevGlyph, contentDescription = null, modifier = Modifier.size(18.dp)) }
            PencilButton(
                onClick = { remote.media("toggle_play_pause") },
                shadow = 4.dp,
                contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp),
                modifier = Modifier.size(68.dp, 56.dp).semantics { contentDescription = if (m.isPlaying) "Pause" else "Play" },
            ) { Icon(if (m.isPlaying) PauseGlyph else Icons.Filled.PlayArrow, contentDescription = null, modifier = Modifier.size(22.dp)) }
            OutlineButton(
                onClick = { remote.media("next") },
                enabled = m.canNext,
                contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp),
                modifier = Modifier.size(52.dp).semantics { contentDescription = "Next" },
            ) { Icon(NextGlyph, contentDescription = null, modifier = Modifier.size(18.dp)) }
        }
    }
}

/** One "LABEL ——◆—— 62" row. */
@Composable
private fun LevelRow(label: String, value: Float, onChange: (Float) -> Unit, onDone: (() -> Unit)? = null, thumb: androidx.compose.ui.graphics.Color, trailing: @Composable () -> Unit = {}) {
    val c = Bp.c
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Text(label.uppercase(), style = BpType.mono.copy(fontSize = 12.sp), color = c.ink2, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.width(86.dp))
        DiamondSlider(value = value, onValueChange = onChange, onValueChangeFinished = onDone, thumbColor = thumb, modifier = Modifier.weight(1f))
        Text("${(value * 100).toInt()}", style = BpType.monoStrong, color = c.ink, textAlign = TextAlign.End, modifier = Modifier.width(30.dp))
        trailing()
    }
}

@Composable
private fun Controls(remote: RemoteViewModel) {
    val c = Bp.c
    val gate = LocalAdminGate.current
    val ui by remote.ui.collectAsStateWithLifecycle()
    var confirm by remember { mutableStateOf<String?>(null) }

    FigBox(modifier = Modifier.fillMaxWidth(), contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 12.dp, vertical = 14.dp)) {
        val a = ui.audio
        if (a == null) {
            MonoLabel("Volume not available")
        } else {
            LevelRow(
                "Volume",
                a.level,
                onChange = { remote.setVolume(it, dragging = true) },
                onDone = { remote.setVolume(a.level, dragging = false) },
                thumb = c.pencil,
            ) {
                MonoChip(if (a.muted) "Unmute" else "Mute", onClick = remote::toggleMute, selected = a.muted, height = 34.dp)
            }
        }
        val supported = ui.displays.filter { it.supported }
        supported.forEach { d ->
            LevelRow(
                if (supported.size == 1) "Brightness" else d.name,
                d.level / 100f,
                onChange = { remote.setBrightness(d.id, (it * 100).toInt()) },
                thumb = c.ink,
            )
        }
    }

    val p = ui.power
    if (p != null) {
        val left by produceState(p.dueUnixMs - System.currentTimeMillis(), p) {
            while (true) {
                value = p.dueUnixMs - System.currentTimeMillis()
                delay(1000)
            }
        }
        Row(
            Modifier.fillMaxWidth().border(2.dp, c.red).padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            MonoLabel("${powerLabel(p.action)} in ${fmt(left)}", color = c.redInk, strong = true, modifier = Modifier.weight(1f))
            MonoChip("Cancel", onClick = remote::cancelPower, height = 36.dp)
        }
    }
    PowerSchedule.ACTIONS.chunked(3).forEach { row ->
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            row.forEach { a ->
                OutlineButton(
                    onClick = { confirm = a },
                    danger = isDestructive(a),
                    filled = true,
                    textStyle = BpType.button.copy(fontSize = 13.sp),
                    contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 4.dp),
                    modifier = Modifier.weight(1f).height(70.dp),
                ) { Text(powerLabel(a).uppercase(), maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
        }
    }
    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.Center) {
        MonoLabel("Power asks first · optional timer · red ends the session")
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
