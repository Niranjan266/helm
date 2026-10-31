package com.niranjan.helm.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.ProcessEntry
import com.niranjan.helm.core.ProcessSort
import com.niranjan.helm.core.ProcessSorter

@Composable
fun TasksScreen(power: PowerViewModel) {
    val ui by power.ui.collectAsStateWithLifecycle()
    val gate = LocalAdminGate.current
    DisposableEffect(ui.phase3) {
        power.tasksVisible(ui.phase3)
        onDispose { power.tasksVisible(false) }
    }
    if (!ui.phase3) return Phase3Required("The task manager")
    var selected by remember { mutableStateOf<ProcessEntry?>(null) }
    var confirmKill by remember { mutableStateOf<ProcessEntry?>(null) }

    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        OutlinedTextField(ui.processFilter, power::setProcessFilter, placeholder = { Text("Filter by name or pid") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            ProcessSort.entries.forEach { s -> FilterChip(selected = ui.processSort == s, onClick = { power.setProcessSort(s) }, label = { Text(s.name.lowercase().replaceFirstChar { it.uppercase() }) }) }
        }
        val list = ui.processes
        if (list == null) HintText("Loading processes…")
        LazyColumn(Modifier.fillMaxSize()) {
            items(ProcessSorter.sort(list.orEmpty(), ui.processSort, ui.processFilter), key = { it.pid }) { p ->
                Row(Modifier.fillMaxWidth().clickable { selected = p }.padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(p.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        HintText("pid ${p.pid}")
                    }
                    Text("%.1f%%".format(p.cpuPct), Modifier.width(64.dp), style = MaterialTheme.typography.bodyMedium)
                    Text(formatBytes(p.ramBytes), Modifier.width(80.dp), style = MaterialTheme.typography.bodyMedium)
                }
                HorizontalDivider()
            }
        }
    }

    selected?.let { p ->
        AlertDialog(
            onDismissRequest = { selected = null },
            title = { Text(p.name) },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    p.exe?.let { HintText(it) }
                    Text("Priority", style = MaterialTheme.typography.labelLarge)
                    ProcessEntry.PRIORITIES.forEach { level ->
                        TextButton(onClick = {
                            selected = null
                            gate("Change priority of ${p.name}") { power.priority(p, level) }
                        }) { Text(level.replace('_', ' ')) }
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = { selected = null; confirmKill = p }) { Text("End task", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { selected = null }) { Text("Close") } },
        )
    }
    confirmKill?.let { p ->
        AlertDialog(
            onDismissRequest = { confirmKill = null },
            title = { Text("End ${p.name}?") },
            text = { Text("Unsaved work in this program will be lost.") },
            confirmButton = {
                TextButton(onClick = { confirmKill = null; gate("End ${p.name}") { power.kill(p) } }) { Text("End task", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { TextButton(onClick = { confirmKill = null }) { Text("Cancel") } },
        )
    }
}

@Composable
fun MixerScreen(power: PowerViewModel) {
    val ui by power.ui.collectAsStateWithLifecycle()
    LaunchedEffect(ui.phase3) { if (ui.phase3) power.loadSessions() }
    if (!ui.phase3) return Phase3Required("The volume mixer")
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            HintText("Per-app volume", Modifier.weight(1f))
            TextButton(onClick = power::loadSessions) { Text("Refresh") }
        }
        val sessions = ui.sessions
        if (sessions == null) HintText("Loading…") else if (sessions.isEmpty()) HintText("No app is playing sound.")
        LazyColumn {
            items(sessions.orEmpty(), key = { it.id.toString() }) { s ->
                Column(Modifier.padding(vertical = 6.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(s.name, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        Text("${(s.level * 100).toInt()}%")
                        IconButton(onClick = { power.toggleSessionMute(s) }) { Text(if (s.muted) "🔇" else "🔊") }
                    }
                    Slider(value = s.level, onValueChange = { power.setSessionLevel(s, it) }, enabled = !s.muted)
                }
            }
        }
    }
}
