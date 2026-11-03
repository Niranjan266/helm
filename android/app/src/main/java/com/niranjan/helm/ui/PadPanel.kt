package com.niranjan.helm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.material3.AlertDialog
import androidx.compose.ui.graphics.Color
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.MouseButton
import com.niranjan.helm.core.PadPanel
import com.niranjan.helm.core.Vk

private val PanelModifiers = listOf("Ctrl" to Vk.CONTROL, "Alt" to Vk.ALT, "Shift" to Vk.SHIFT, "Win" to Vk.LWIN)
private val PanelKeys = listOf(
    "Esc" to Vk.ESCAPE, "Tab" to Vk.TAB, "Bksp" to Vk.BACK, "Del" to Vk.DELETE,
    "Enter" to Vk.RETURN, "Space" to Vk.SPACE, "Home" to Vk.HOME, "End" to Vk.END,
    "←" to Vk.LEFT, "→" to Vk.RIGHT, "↑" to Vk.UP, "↓" to Vk.DOWN,
    "PgUp" to Vk.PAGE_UP, "PgDn" to Vk.PAGE_DOWN,
)

/**
 * The landscape touchpad's right panel: a narrow handle on the edge (tap to collapse or open)
 * and, while open, the content the user picked for this PC.
 */
@Composable
fun PadSidePanel(
    panel: PadPanel,
    collapsed: Boolean,
    onToggle: () -> Unit,
    onChange: () -> Unit,
    remote: RemoteViewModel,
    mods: SnapshotStateList<Int>,
    onKey: (Int) -> Unit,
    modifier: Modifier = Modifier,
) {
    val c = Bp.c
    val open = !collapsed && panel != PadPanel.NONE
    Row(modifier.fillMaxHeight()) {
        // The handle.
        Box(
            Modifier
                .width(28.dp)
                .fillMaxHeight()
                .background(c.paper.copy(alpha = 0.9f))
                .border(1.5.dp, c.ink)
                .clickable(role = Role.Button, onClick = if (panel == PadPanel.NONE) onChange else onToggle)
                .semantics { contentDescription = if (open) "Hide panel" else "Show panel" },
            contentAlignment = Alignment.Center,
        ) {
            Text(if (open) "›" else "‹", color = c.pencil, fontSize = 22.sp)
        }
        if (open) {
            Column(
                Modifier
                    .width(156.dp)
                    .fillMaxHeight()
                    .background(c.paper.copy(alpha = 0.94f))
                    .border(1.5.dp, c.ink)
                    .padding(8.dp),
                verticalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                    MonoLabel(panel.label, color = c.ink, modifier = Modifier.weight(1f), maxLines = 2)
                    MonoChip("Edit", onClick = onChange, dashed = true, height = 28.dp)
                }
                Box(Modifier.weight(1f).fillMaxWidth()) {
                    when (panel) {
                        PadPanel.CLICKS -> ClicksPanel(remote)
                        PadPanel.KEYS -> KeysPanel(mods, onKey)
                        PadPanel.MEDIA -> MediaPanel(remote)
                        PadPanel.VOLUME -> VolumePanel(remote)
                        PadPanel.DECK -> DeckPanel(remote)
                        PadPanel.NONE -> Unit
                    }
                }
            }
        }
    }
}

@Composable
private fun ClicksPanel(remote: RemoteViewModel) {
    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlineButton(onClick = { remote.click(MouseButton.LEFT) }, filled = true, modifier = Modifier.weight(2f).fillMaxWidth()) {
            Text("LEFT", style = BpType.button)
        }
        OutlineButton(onClick = { remote.click(MouseButton.RIGHT) }, filled = true, modifier = Modifier.weight(2f).fillMaxWidth()) {
            Text("RIGHT", style = BpType.button)
        }
        OutlineButton(onClick = { remote.click(MouseButton.MIDDLE) }, filled = true, modifier = Modifier.weight(1f).fillMaxWidth()) {
            Text("MIDDLE", style = BpType.button.copy(fontSize = 12.sp))
        }
    }
}

@Composable
private fun KeysPanel(mods: SnapshotStateList<Int>, onKey: (Int) -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        MonoLabel("Hold for next key")
        PanelModifiers.chunked(2).forEach { pair ->
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                pair.forEach { (label, vk) ->
                    MonoChip(label, selected = vk in mods, modifier = Modifier.weight(1f), onClick = { if (vk in mods) mods.remove(vk) else mods.add(vk) })
                }
            }
        }
        MonoLabel("Keys")
        PanelKeys.chunked(2).forEach { pair ->
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                pair.forEach { (label, vk) -> MonoChip(label, modifier = Modifier.weight(1f), onClick = { onKey(vk) }) }
            }
        }
    }
}

@Composable
private fun MediaPanel(remote: RemoteViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val m = ui.media
    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            m?.title?.takeIf { it.isNotBlank() } ?: "Nothing playing",
            style = BpType.monoStrong,
            color = Bp.c.ink,
            maxLines = 3,
        )
        OutlineButton(onClick = { remote.media("toggle_play_pause") }, filled = true, modifier = Modifier.weight(2f).fillMaxWidth()) {
            Text("PLAY / PAUSE", style = BpType.button.copy(fontSize = 13.sp), textAlign = TextAlign.Center)
        }
        Row(Modifier.weight(1f).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlineButton(onClick = { remote.media("previous") }, filled = true, modifier = Modifier.weight(1f).fillMaxHeight(), contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
                Text("⏮", fontSize = 20.sp)
            }
            OutlineButton(onClick = { remote.media("next") }, filled = true, modifier = Modifier.weight(1f).fillMaxHeight(), contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
                Text("⏭", fontSize = 20.sp)
            }
        }
    }
}

@Composable
private fun VolumePanel(remote: RemoteViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val a = ui.audio
    val level = a?.level ?: 0f
    Column(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(
            if (a?.muted == true) "MUTED" else "${(level * 100).toInt()}%",
            style = BpType.title,
            color = Bp.c.ink,
        )
        DiamondSlider(
            value = level,
            onValueChange = { remote.setVolume(it, true) },
            onValueChangeFinished = { remote.setVolume(level, false) },
            modifier = Modifier.fillMaxWidth(),
        )
        Row(Modifier.weight(1f).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlineButton(onClick = { remote.setVolume((level - 0.05f).coerceAtLeast(0f), false) }, filled = true, modifier = Modifier.weight(1f).fillMaxHeight(), contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
                Text("−", fontSize = 24.sp)
            }
            OutlineButton(onClick = { remote.setVolume((level + 0.05f).coerceAtMost(1f), false) }, filled = true, modifier = Modifier.weight(1f).fillMaxHeight(), contentPadding = androidx.compose.foundation.layout.PaddingValues(0.dp)) {
                Text("+", fontSize = 24.sp)
            }
        }
        OutlineButton(onClick = remote::toggleMute, filled = a?.muted == true, modifier = Modifier.fillMaxWidth().height(48.dp)) {
            Text(if (a?.muted == true) "UNMUTE" else "MUTE", style = BpType.button.copy(fontSize = 13.sp))
        }
    }
}

@Composable
private fun DeckPanel(remote: RemoteViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    if (ui.deck.isEmpty()) {
        HintText("No deck buttons yet. Add some on the Deck screen.")
        return
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        ui.deck.chunked(2).forEach { pair ->
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                pair.forEach { b ->
                    Box(
                        Modifier
                            .weight(1f)
                            .height(56.dp)
                            .background(parseColor(b.color))
                            .border(2.dp, Bp.c.deep)
                            .clickable(role = Role.Button) { remote.runDeck(b) }
                            .padding(4.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Text(
                            b.label.ifBlank { "·" },
                            color = Color.White,
                            style = BpType.monoStrong,
                            textAlign = TextAlign.Center,
                            maxLines = 2,
                        )
                    }
                }
                if (pair.size == 1) Box(Modifier.weight(1f))
            }
        }
    }
}

/** Picks what the right panel shows for this PC. */
@Composable
fun PadPanelDialog(current: PadPanel, onPick: (PadPanel) -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        confirmButton = { PencilButton(onClick = onDismiss) { Text("DONE") } },
        title = { DisplayText("Right panel") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                MonoLabel("Saved for this PC")
                PadPanel.entries.forEach { p ->
                    BpFilterChip(
                        selected = p == current,
                        onClick = { onPick(p) },
                        label = { Text(p.label) },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            }
        },
    )
}
