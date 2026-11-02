package com.niranjan.helm.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
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
import com.niranjan.helm.core.ButtonSize
import com.niranjan.helm.core.DeckButton
import com.niranjan.helm.core.DeckGrid
import com.niranjan.helm.core.DeckLayout
import com.niranjan.helm.core.DeckPage
import com.niranjan.helm.core.DeckPages
import com.niranjan.helm.core.IconRef
import com.niranjan.helm.core.DeckCodec
import com.niranjan.helm.core.HaState
import com.niranjan.helm.device.Settings
import kotlinx.coroutines.launch

val DeckColors = listOf("#3B82F6", "#22C55E", "#EF4444", "#F59E0B", "#A855F7", "#EC4899", "#14B8A6", "#64748B")

fun parseColor(hex: String): Color = try {
    Color(android.graphics.Color.parseColor(hex))
} catch (_: IllegalArgumentException) {
    Color(0xFF3B82F6)
}

internal fun kindOf(a: DeckAction): String = when (a) {
    is DeckAction.Hotkey -> "hotkey"
    is DeckAction.Text -> "text"
    is DeckAction.Launch -> "launch"
    is DeckAction.Open -> "open"
    is DeckAction.Media -> "media"
    is DeckAction.VolumeStep -> "volume_step"
    is DeckAction.Delay -> "delay_ms"
    is DeckAction.Macro -> "macro"
    is DeckAction.Repeat -> "repeat"
    is DeckAction.Remote -> "remote"
    is DeckAction.Ha -> "ha"
    is DeckAction.Unknown -> "other"
}

/** Editable text for a simple action (macro/repeat are edited structurally). */
internal fun valueOf(a: DeckAction): String = when (a) {
    is DeckAction.Hotkey -> a.keys
    is DeckAction.Text -> a.text
    is DeckAction.Launch -> a.appId
    is DeckAction.Open -> a.target
    is DeckAction.Media -> a.action
    is DeckAction.VolumeStep -> a.step.toString()
    is DeckAction.Delay -> a.ms.toString()
    is DeckAction.Remote -> a.command
    is DeckAction.Ha -> "${a.service} ${a.entityId}"
    is DeckAction.Macro, is DeckAction.Repeat -> ""
    is DeckAction.Unknown -> a.raw.toString()
}

internal fun kindLabel(k: String) = when (k) {
    "hotkey" -> "Hotkey"
    "text" -> "Type text"
    "launch" -> "Launch app"
    "open" -> "Open"
    "media" -> "Media"
    "volume_step" -> "Volume step"
    "delay_ms" -> "Delay"
    "macro" -> "Macro"
    "repeat" -> "Repeat"
    "remote" -> "App remote"
    "ha" -> "Home Assistant"
    else -> "Other"
}

internal fun defaultValue(k: String) = when (k) {
    "media" -> "toggle_play_pause"
    "volume_step" -> "0.05"
    "delay_ms" -> "250"
    "remote" -> "powerpoint.next"
    "ha" -> "homeassistant.toggle "
    else -> ""
}

/** Builds a simple action from the editor fields, or null if the value is invalid. */
internal fun buildAction(kind: String, value: String, original: DeckAction?): DeckAction? = when (kind) {
    "hotkey" -> value.trim().takeIf { it.isNotEmpty() }?.let { DeckAction.Hotkey(it.lowercase()) }
    "text" -> value.takeIf { it.isNotEmpty() }?.let { DeckAction.Text(it) }
    "launch" -> value.trim().takeIf { it.isNotEmpty() }?.let { DeckAction.Launch(it) }
    "open" -> value.trim().takeIf { it.isNotEmpty() }?.let { DeckAction.Open(it) }
    "media" -> value.takeIf { it in DeckAction.MEDIA_ACTIONS }?.let { DeckAction.Media(it) }
    "volume_step" -> value.trim().toDoubleOrNull()?.takeIf { it in -1.0..1.0 && it != 0.0 }?.let { DeckAction.VolumeStep(it) }
    "delay_ms" -> value.trim().toLongOrNull()?.takeIf { it in 0..60_000 }?.let { DeckAction.Delay(it) }
    "remote" -> value.trim().takeIf {
        it in DeckAction.REMOTE_COMMANDS || (it.startsWith(DeckAction.OBS_SCENE_PREFIX) && it.length > DeckAction.OBS_SCENE_PREFIX.length)
    }?.let { DeckAction.Remote(it) }
    "ha" -> {
        val parts = value.trim().split(Regex("\\s+"))
        if (parts.size == 2 && HaState.validService(parts[0]) && HaState.validEntityId(parts[1])) DeckAction.Ha(parts[0], parts[1]) else null
    }
    else -> original as? DeckAction.Unknown
}

/** One step of a macro, or the action inside a repeat. */
private data class StepDraft(val kind: String, val value: String) {
    fun build(): DeckAction? = buildAction(kind, value, null)

    companion object {
        fun of(a: DeckAction) = StepDraft(kindOf(a), valueOf(a))
    }
}

/** Kinds allowed inside a macro / repeat. */
private val STEP_KINDS = listOf("hotkey", "text", "open", "media", "volume_step", "delay_ms", "remote")

private fun widgetLabel(w: String) = when (w) {
    "cpu" -> "CPU %"
    "ram" -> "RAM %"
    "clock" -> "Clock"
    "now_playing" -> "Now playing"
    else -> w
}

private fun bindLabel(b: String) = when (b) {
    "mic_muted" -> "Mic muted"
    "audio_muted" -> "Sound muted"
    "media_playing" -> "Media playing"
    else -> b
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun DeckScreen(remote: RemoteViewModel, settings: Settings) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val haptic = LocalHapticFeedback.current
    var editMode by rememberSaveable { mutableStateOf(false) }
    var editing by remember { mutableStateOf<DeckButton?>(null) }
    var renamingPage by remember { mutableStateOf<DeckPage?>(null) }
    val pages = ui.deckPages
    val pager = rememberPagerState { pages.size }
    val scope = rememberCoroutineScope()
    val currentPage = pages.getOrElse(pager.currentPage.coerceAtMost(pages.lastIndex)) { pages.first() }

    // A profile became active and names a deck page: show it.
    LaunchedEffect(ui.deckPageWanted, pages) {
        val want = ui.deckPageWanted ?: return@LaunchedEffect
        val i = pages.indexOfFirst { it.id == want }
        if (i >= 0) pager.animateScrollToPage(i)
        remote.deckPageShown()
    }

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
        if (pages.size > 1 || (editMode && ui.g5)) {
            Row(
                Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(top = 6.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                pages.forEachIndexed { i, p ->
                    FilterChip(
                        selected = i == pager.currentPage,
                        onClick = { if (editMode && i == pager.currentPage) renamingPage = p else scope.launch { pager.animateScrollToPage(i) } },
                        label = { Text(p.name.ifBlank { "Page ${i + 1}" }) },
                    )
                }
                if (editMode && ui.g5 && pages.size < DeckPages.MAX_PAGES) {
                    TextButton(onClick = {
                        val id = remote.addDeckPage("Page ${pages.size + 1}")
                        if (id != null) scope.launch { pager.animateScrollToPage(pages.size) }
                    }) { Text("+ Page") }
                }
            }
            if (editMode && ui.g5) HintText("Tap the selected page again to rename or delete it. Profiles can pick a page by its name.")
        }
        Spacer(Modifier.size(8.dp))
        if (ui.deck.isEmpty()) {
            HintText("No deck buttons yet. Turn on Edit and add hotkeys, text snippets, apps, links, media keys" + if (ui.phase3) ", macros or app remotes." else ".")
        }
        HorizontalPager(state = pager, modifier = Modifier.weight(1f).fillMaxWidth(), verticalAlignment = Alignment.Top) { index ->
            val page = pages.getOrNull(index) ?: return@HorizontalPager
            DeckGridPage(
                buttons = DeckLayout(ui.deck, pages).buttonsOn(page.id),
                ui = ui,
                remote = remote,
                onTap = { b ->
                    if (editMode) {
                        editing = b
                    } else {
                        if (settings.haptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                        remote.runDeck(b)
                    }
                },
                onLongPress = { editing = it },
            )
        }
    }

    editing?.let { b ->
        DeckEditor(
            button = b,
            isNew = ui.deck.none { it.id == b.id },
            pageId = currentPage.id,
            remote = remote,
            onDismiss = { editing = null },
        )
    }

    renamingPage?.let { p ->
        var name by remember(p.id) { mutableStateOf(p.name) }
        AlertDialog(
            onDismissRequest = { renamingPage = null },
            title = { Text("Deck page") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(name, { name = it.take(64) }, label = { Text("Name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    HintText("Page id for profiles: ${p.id}")
                    if (pages.size > 1) {
                        TextButton(onClick = { remote.deleteDeckPage(p.id); renamingPage = null }) {
                            Text("Delete page (its buttons move to the first page)", color = MaterialTheme.colorScheme.error)
                        }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    if (name.trim() != p.name) remote.renameDeckPage(p.id, name)
                    renamingPage = null
                }) { Text("Done") }
            },
            dismissButton = { TextButton(onClick = { renamingPage = null }) { Text("Cancel") } },
        )
    }
}

/** One deck page: buttons packed into a grid; 2×1 / 2×2 buttons span cells. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DeckGridPage(
    buttons: List<DeckButton>,
    ui: RemoteUi,
    remote: RemoteViewModel,
    onTap: (DeckButton) -> Unit,
    onLongPress: (DeckButton) -> Unit,
) {
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val gap = 10.dp
        val cols = (maxWidth / 104.dp).toInt().coerceIn(3, 8)
        val cell = (maxWidth - gap * (cols - 1)) / cols
        val cells = DeckGrid.pack(buttons.map { ButtonSize.of(it.size) }, cols)
        val rows = DeckGrid.rowCount(cells)
        Box(
            Modifier
                .fillMaxWidth()
                .verticalScroll(rememberScrollState()),
        ) {
            Box(Modifier.fillMaxWidth().height(if (rows == 0) 0.dp else cell * rows + gap * (rows - 1))) {
                buttons.forEachIndexed { i, b ->
                    val c = cells[i]
                    val on = ui.deckOn[b.id]
                    val widgetText = ui.deckText[b.id]
                    val icon = IconRef.forButton(b, ui.apps)
                    Box(
                        Modifier
                            .offset(x = (cell + gap) * c.col, y = (cell + gap) * c.row)
                            .size(width = cell * c.cols + gap * (c.cols - 1), height = cell * c.rows + gap * (c.rows - 1))
                            .clip(RoundedCornerShape(16.dp))
                            .background(parseColor(b.color).copy(alpha = if (on == false) 0.45f else 1f))
                            .border(if (on == true) 3.dp else 0.dp, Color.White, RoundedCornerShape(16.dp))
                            .combinedClickable(onClick = { onTap(b) }, onLongClick = { onLongPress(b) })
                            .padding(8.dp),
                        contentAlignment = Alignment.Center,
                    ) {
                        Column(horizontalAlignment = Alignment.CenterHorizontally) {
                            val big = c.rows > 1
                            if (icon != IconRef.None && widgetText == null) {
                                IconView(icon, b.label, remote.icons, remote.customIcons, if (big) 64.dp else 30.dp, tint = Color.White)
                                Spacer(Modifier.size(4.dp))
                            }
                            val label = b.label.ifBlank { if (icon == IconRef.None) kindLabel(kindOf(b.action)) else "" }
                            if (label.isNotEmpty()) {
                                Text(label, color = Color.White, textAlign = TextAlign.Center, style = if (big) MaterialTheme.typography.titleMedium else MaterialTheme.typography.titleSmall, maxLines = 2)
                            }
                            if (widgetText != null) {
                                Text(widgetText, color = Color.White, textAlign = TextAlign.Center, style = if (big) MaterialTheme.typography.headlineMedium else MaterialTheme.typography.headlineSmall, maxLines = 2)
                            }
                            if (on != null) {
                                Text(if (on) "ON" else "OFF", color = Color.White, style = MaterialTheme.typography.labelSmall)
                            }
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun DeckEditor(button: DeckButton, isNew: Boolean, pageId: String, remote: RemoteViewModel, onDismiss: () -> Unit) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    var size by remember { mutableStateOf(ButtonSize.of(button.size)) }
    var icon by remember { mutableStateOf(button.icon) }
    val startPage = if (isNew) pageId else (ui.deckLayout.pageOf(button.id)?.id ?: pageId)
    var targetPage by remember { mutableStateOf(startPage) }
    var label by remember { mutableStateOf(button.label) }
    var color by remember { mutableStateOf(button.color) }
    var kind by remember { mutableStateOf(kindOf(button.action)) }
    var value by remember { mutableStateOf(valueOf(button.action)) }
    val steps = remember {
        mutableStateListOf<StepDraft>().apply { (button.action as? DeckAction.Macro)?.steps?.forEach { add(StepDraft.of(it)) } }
    }
    var times by remember { mutableStateOf(((button.action as? DeckAction.Repeat)?.times ?: 2).toString()) }
    var inner by remember { mutableStateOf((button.action as? DeckAction.Repeat)?.action?.let { StepDraft.of(it) } ?: StepDraft("hotkey", "")) }
    var stateBind by remember { mutableStateOf(button.stateBind) }
    var widget by remember { mutableStateOf(button.widget) }
    var picking by remember { mutableStateOf(false) }

    val action: DeckAction? = when (kind) {
        "macro" -> steps.map { it.build() }.takeIf { l -> l.isNotEmpty() && l.size <= DeckAction.MAX_MACRO_STEPS && l.all { it != null } }
            ?.let { DeckAction.Macro(it.filterNotNull()) }
        "repeat" -> {
            val n = times.trim().toIntOrNull()?.takeIf { it in 1..1000 }
            val a = inner.build()
            if (n != null && a != null) DeckAction.Repeat(n, a) else null
        }
        else -> buildAction(kind, value, button.action)
    }
    val p3 = ui.phase3 || DeckCodec.needsPhase3(button.action) || button.stateBind != null || button.widget != null
    val p5 = ui.phase5 || DeckCodec.needsPhase5(button.action)
    val kinds = DeckAction.KINDS + (if (p3) DeckAction.P3_KINDS else emptyList()) + (if (p5) DeckAction.P5_KINDS else emptyList())

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
                if (ui.g5) {
                    Text("Size", style = MaterialTheme.typography.labelLarge)
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        ButtonSize.entries.forEach { s -> FilterChip(selected = size == s, onClick = { size = s }, label = { Text(s.label) }) }
                    }
                    action?.let { a ->
                        IconChooserRow(icon, IconRef.forButton(button.copy(icon = null, action = a), ui.apps), label, remote) { icon = it }
                    }
                    if (ui.deckPages.size > 1) {
                        Text("Page", style = MaterialTheme.typography.labelLarge)
                        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            ui.deckPages.forEachIndexed { i, pg ->
                                FilterChip(selected = targetPage == pg.id, onClick = { targetPage = pg.id }, label = { Text(pg.name.ifBlank { "Page ${i + 1}" }) })
                            }
                        }
                    }
                }
                Text("Action", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    kinds.forEach { k ->
                        FilterChip(selected = kind == k, onClick = {
                            if (kind != k) {
                                kind = k
                                value = defaultValue(k)
                                if (k == "macro" && steps.isEmpty()) steps += StepDraft("hotkey", "")
                            }
                        }, label = { Text(kindLabel(k)) })
                    }
                }
                when (kind) {
                    "launch" -> {
                        val name = ui.apps?.firstOrNull { it.appId == value }?.name
                        Text(if (value.isEmpty()) "No app chosen" else (name ?: value))
                        OutlinedButton(onClick = { picking = true }) { Text("Choose app") }
                    }
                    "macro" -> {
                        HintText("Runs the steps in order (max ${DeckAction.MAX_MACRO_STEPS}). Add delays between steps if an app needs time.")
                        steps.forEachIndexed { i, st ->
                            Text("Step ${i + 1}", style = MaterialTheme.typography.labelMedium)
                            StepEditor(st) { steps[i] = it }
                            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                                TextButton(enabled = i > 0, onClick = { steps.add(i - 1, steps.removeAt(i)) }) { Text("Up") }
                                TextButton(enabled = i < steps.lastIndex, onClick = { steps.add(i + 1, steps.removeAt(i)) }) { Text("Down") }
                                TextButton(onClick = { steps.removeAt(i) }) { Text("Remove", color = MaterialTheme.colorScheme.error) }
                            }
                        }
                        OutlinedButton(enabled = steps.size < DeckAction.MAX_MACRO_STEPS, onClick = { steps += StepDraft("delay_ms", "250") }) { Text("Add step") }
                    }
                    "repeat" -> {
                        OutlinedTextField(
                            times, { times = it }, label = { Text("Times (1-1000)") }, singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.fillMaxWidth(),
                        )
                        StepEditor(inner) { inner = it }
                    }
                    "other" -> HintText("This action was made elsewhere and is kept as-is: $value")
                    else -> SimpleValueEditor(kind, value) { value = it }
                }
                if (p3) {
                    Text("Show live state", style = MaterialTheme.typography.labelLarge)
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        FilterChip(selected = stateBind == null, onClick = { stateBind = null }, label = { Text("None") })
                        DeckAction.STATE_BINDS.forEach { s -> FilterChip(selected = stateBind == s, onClick = { stateBind = s }, label = { Text(bindLabel(s)) }) }
                    }
                    Text("Live widget", style = MaterialTheme.typography.labelLarge)
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        FilterChip(selected = widget == null, onClick = { widget = null }, label = { Text("None") })
                        DeckAction.WIDGETS.forEach { w -> FilterChip(selected = widget == w, onClick = { widget = w }, label = { Text(widgetLabel(w)) }) }
                    }
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
                remote.saveDeckButton(
                    button.copy(
                        label = label.trim(), color = color.ifBlank { DeckCodec.DEFAULT_COLOR }, action = action!!,
                        stateBind = stateBind, widget = widget, size = size.wire, icon = icon,
                    ),
                    targetPage,
                )
                if (!isNew && targetPage != startPage) remote.moveDeckButtonToPage(button.id, targetPage)
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

/** Kind chips + value for one macro step or the repeated action. */
@Composable
private fun StepEditor(step: StepDraft, onChange: (StepDraft) -> Unit) {
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        STEP_KINDS.forEach { k ->
            FilterChip(selected = step.kind == k, onClick = { if (step.kind != k) onChange(StepDraft(k, defaultValue(k))) }, label = { Text(kindLabel(k)) })
        }
    }
    SimpleValueEditor(step.kind, step.value) { onChange(step.copy(value = it)) }
}

@Composable
internal fun SimpleValueEditor(kind: String, value: String, onValue: (String) -> Unit) {
    when (kind) {
        "hotkey" -> OutlinedTextField(value, onValue, label = { Text("Keys, e.g. ctrl+shift+esc") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        "text" -> OutlinedTextField(value, onValue, label = { Text("Text to type") }, modifier = Modifier.fillMaxWidth())
        "launch" -> OutlinedTextField(value, onValue, label = { Text("App id") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        "open" -> OutlinedTextField(value, onValue, label = { Text("URL, folder or file on the PC") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        "media" -> Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            DeckAction.MEDIA_ACTIONS.forEach { m -> FilterChip(selected = value == m, onClick = { onValue(m) }, label = { Text(m.replace('_', ' ')) }) }
        }
        "volume_step" -> OutlinedTextField(
            value, onValue, label = { Text("Step (-1..1), e.g. 0.05 or -0.05") }, singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal), modifier = Modifier.fillMaxWidth(),
        )
        "delay_ms" -> OutlinedTextField(
            value, onValue, label = { Text("Delay in ms") }, singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number), modifier = Modifier.fillMaxWidth(),
        )
        "remote" -> RemotePicker(value, onValue)
        "ha" -> HaActionEditor(value, onValue)
        else -> Unit
    }
}

/** PowerPoint / browser / OBS remote commands; OBS scenes take a name. */
@Composable
private fun RemotePicker(value: String, onValue: (String) -> Unit) {
    val isScene = value.startsWith(DeckAction.OBS_SCENE_PREFIX)
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        DeckAction.REMOTE_COMMANDS.forEach { c -> FilterChip(selected = value == c, onClick = { onValue(c) }, label = { Text(c) }) }
        FilterChip(selected = isScene, onClick = { if (!isScene) onValue(DeckAction.OBS_SCENE_PREFIX) }, label = { Text("obs.scene") })
    }
    if (isScene) {
        OutlinedTextField(
            value.removePrefix(DeckAction.OBS_SCENE_PREFIX), { onValue(DeckAction.OBS_SCENE_PREFIX + it) },
            label = { Text("OBS scene name") }, singleLine = true, modifier = Modifier.fillMaxWidth(),
        )
    }
}
