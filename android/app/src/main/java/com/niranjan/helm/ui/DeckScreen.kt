package com.niranjan.helm.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.DeckAction
import com.niranjan.helm.core.DeckButton
import com.niranjan.helm.core.DeckCodec
import com.niranjan.helm.device.Settings

val DeckColors = listOf("#3B82F6", "#22C55E", "#EF4444", "#F59E0B", "#A855F7", "#EC4899", "#14B8A6", "#64748B")

fun parseColor(hex: String): Color = try {
    Color(android.graphics.Color.parseColor(hex))
} catch (_: IllegalArgumentException) {
    Color(0xFF3B82F6)
}

private fun kindOf(a: DeckAction): String = when (a) {
    is DeckAction.Hotkey -> "hotkey"
    is DeckAction.Text -> "text"
    is DeckAction.Launch -> "launch"
    is DeckAction.Open -> "open"
    is DeckAction.Media -> "media"
    is DeckAction.VolumeStep -> "volume_step"
    is DeckAction.Delay -> "delay_ms"
    is DeckAction.Unknown -> "other"
}

private fun valueOf(a: DeckAction): String = when (a) {
    is DeckAction.Hotkey -> a.keys
    is DeckAction.Text -> a.text
    is DeckAction.Launch -> a.appId
    is DeckAction.Open -> a.target
    is DeckAction.Media -> a.action
    is DeckAction.VolumeStep -> a.step.toString()
    is DeckAction.Delay -> a.ms.toString()
    is DeckAction.Unknown -> a.raw.toString()
}

private fun kindLabel(k: String) = when (k) {
    "hotkey" -> "Hotkey"
    "text" -> "Type text"
    "launch" -> "Launch app"
    "open" -> "Open"
    "media" -> "Media"
    "volume_step" -> "Volume step"
    "delay_ms" -> "Delay"
    else -> "Other"
}

/** Builds an action from the editor fields, or null if the value is invalid. */
private fun buildAction(kind: String, value: String, original: DeckAction?): DeckAction? = when (kind) {
    "hotkey" -> value.trim().takeIf { it.isNotEmpty() }?.let { DeckAction.Hotkey(it.lowercase()) }
    "text" -> value.takeIf { it.isNotEmpty() }?.let { DeckAction.Text(it) }
    "launch" -> value.trim().takeIf { it.isNotEmpty() }?.let { DeckAction.Launch(it) }
    "open" -> value.trim().takeIf { it.isNotEmpty() }?.let { DeckAction.Open(it) }
    "media" -> value.takeIf { it in DeckAction.MEDIA_ACTIONS }?.let { DeckAction.Media(it) }
    "volume_step" -> value.trim().toDoubleOrNull()?.takeIf { it in -1.0..1.0 && it != 0.0 }?.let { DeckAction.VolumeStep(it) }
    "delay_ms" -> value.trim().toLongOrNull()?.takeIf { it in 0..60_000 }?.let { DeckAction.Delay(it) }
    else -> original as? DeckAction.Unknown
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun DeckScreen(remote: RemoteViewModel, settings: Settings) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val haptic = LocalHapticFeedback.current
    var editMode by rememberSaveable { mutableStateOf(false) }
    var editing by remember { mutableStateOf<DeckButton?>(null) }

    Column(Modifier.fillMaxSize().padding(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = editMode, onClick = { editMode = !editMode }, label = { Text("Edit") })
            if (editMode) {
                Button(onClick = {
                    editing = DeckButton(remote.newId(), "", DeckColors[ui.deck.size % DeckColors.size], action = DeckAction.Hotkey(""))
                }) { Text("Add button") }
            }
            Spacer(Modifier.weight(1f))
            if (editMode) HintText("Tap a button to edit it")
        }
        Spacer(Modifier.size(8.dp))
        if (ui.deck.isEmpty()) {
            HintText("No deck buttons yet. Turn on Edit and add hotkeys, text snippets, apps, links or media keys.")
        }
        LazyVerticalGrid(columns = GridCells.Adaptive(110.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
            items(ui.deck, key = { it.id }) { b ->
                Box(
                    Modifier
                        .aspectRatio(1f)
                        .clip(RoundedCornerShape(16.dp))
                        .background(parseColor(b.color))
                        .combinedClickable(
                            onClick = {
                                if (editMode) {
                                    editing = b
                                } else {
                                    if (settings.haptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                    remote.runDeck(b)
                                }
                            },
                            onLongClick = { editing = b },
                        )
                        .padding(8.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(b.label.ifBlank { kindLabel(kindOf(b.action)) }, color = Color.White, textAlign = TextAlign.Center, style = MaterialTheme.typography.titleSmall)
                }
            }
        }
    }

    editing?.let { b ->
        DeckEditor(
            button = b,
            isNew = ui.deck.none { it.id == b.id },
            remote = remote,
            onDismiss = { editing = null },
        )
    }
}

@Composable
private fun DeckEditor(button: DeckButton, isNew: Boolean, remote: RemoteViewModel, onDismiss: () -> Unit) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    var label by remember { mutableStateOf(button.label) }
    var color by remember { mutableStateOf(button.color) }
    var kind by remember { mutableStateOf(kindOf(button.action)) }
    var value by remember { mutableStateOf(valueOf(button.action)) }
    var picking by remember { mutableStateOf(false) }
    val action = buildAction(kind, value, button.action)

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (isNew) "New button" else "Edit button") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(label, { label = it }, label = { Text("Label") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    DeckColors.forEach { c ->
                        Box(
                            Modifier
                                .size(32.dp)
                                .clip(CircleShape)
                                .background(parseColor(c))
                                .border(if (c.equals(color, true)) 3.dp else 0.dp, Color.White, CircleShape)
                                .clickable { color = c },
                        )
                    }
                }
                Text("Action", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    DeckAction.KINDS.forEach { k ->
                        FilterChip(selected = kind == k, onClick = {
                            if (kind != k) {
                                kind = k
                                value = when (k) {
                                    "media" -> "toggle_play_pause"
                                    "volume_step" -> "0.05"
                                    "delay_ms" -> "250"
                                    else -> ""
                                }
                            }
                        }, label = { Text(kindLabel(k)) })
                    }
                }
                when (kind) {
                    "hotkey" -> OutlinedTextField(value, { value = it }, label = { Text("Keys, e.g. ctrl+shift+esc") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    "text" -> OutlinedTextField(value, { value = it }, label = { Text("Text to type") }, modifier = Modifier.fillMaxWidth())
                    "launch" -> {
                        val name = ui.apps?.firstOrNull { it.appId == value }?.name
                        Text(if (value.isEmpty()) "No app chosen" else (name ?: value))
                        OutlinedButton(onClick = { picking = true }) { Text("Choose app") }
                    }
                    "open" -> OutlinedTextField(value, { value = it }, label = { Text("URL, folder or file on the PC") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    "media" -> Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        DeckAction.MEDIA_ACTIONS.forEach { m ->
                            FilterChip(selected = value == m, onClick = { value = m }, label = { Text(m.replace('_', ' ')) })
                        }
                    }
                    "volume_step" -> OutlinedTextField(
                        value, { value = it }, label = { Text("Step (-1..1), e.g. 0.05 or -0.05") }, singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal), modifier = Modifier.fillMaxWidth(),
                    )
                    "delay_ms" -> OutlinedTextField(
                        value, { value = it }, label = { Text("Delay in ms") }, singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.fillMaxWidth(),
                    )
                    else -> HintText("This action was made elsewhere and is kept as-is: $value")
                }
                if (!isNew) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedButton(onClick = { remote.moveDeckButton(button.id, -1) }) { Text("◀ Move") }
                        OutlinedButton(onClick = { remote.moveDeckButton(button.id, +1) }) { Text("Move ▶") }
                    }
                    TextButton(onClick = { remote.deleteDeckButton(button.id); onDismiss() }) {
                        Text("Delete button", color = MaterialTheme.colorScheme.error)
                    }
                }
            }
        },
        confirmButton = {
            TextButton(enabled = action != null, onClick = {
                remote.saveDeckButton(button.copy(label = label.trim(), color = color.ifBlank { DeckCodec.DEFAULT_COLOR }, action = action!!))
                onDismiss()
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )

    if (picking) {
        AppPickerDialog(
            title = "Choose app",
            apps = ui.apps,
            loading = ui.appsLoading,
            icons = remote.icons,
            onLoad = { remote.loadApps() },
            onPick = { app ->
                value = app.appId
                if (label.isBlank()) label = app.name
                picking = false
            },
            onDismiss = { picking = false },
        )
    }
}
