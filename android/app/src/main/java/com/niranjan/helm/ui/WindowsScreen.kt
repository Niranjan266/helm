package com.niranjan.helm.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.Profile

/** Window move/snap and virtual desktops (§11). */
@Composable
fun WindowsScreen(remote: RemoteViewModel, power: PowerViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val pui by power.ui.collectAsStateWithLifecycle()
    LaunchedEffect(Unit) { remote.refreshWindows() }
    if (!pui.phase3) return Phase3Required("Window snapping and virtual desktops")
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionCard("Virtual desktops") {
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedButton(onClick = { power.desktop("prev") }) { Text("◀ Previous") }
                OutlinedButton(onClick = { power.desktop("next") }) { Text("Next ▶") }
                OutlinedButton(onClick = { power.desktop("new") }) { Text("New") }
                OutlinedButton(onClick = { power.desktop("close") }) { Text("Close") }
            }
        }
        LazyColumn(Modifier.fillMaxSize()) {
            items(ui.windows, key = { it.hwnd.toString() }) { w ->
                Column(Modifier.padding(vertical = 8.dp)) {
                    Text(w.title.ifBlank { w.appId ?: "Window" }, maxLines = 1, overflow = TextOverflow.Ellipsis, fontWeight = if (w.isForeground) FontWeight.Bold else null)
                    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        listOf("left" to "◧ Left", "right" to "◨ Right", "maximize" to "Max", "restore" to "Restore").forEach { (pos, label) ->
                            OutlinedButton(onClick = { power.snap(w, pos) }) { Text(label) }
                        }
                        if (ui.monitors.size > 1) {
                            ui.monitors.forEachIndexed { i, m ->
                                OutlinedButton(onClick = { power.moveToMonitor(w, i) }) { Text("→ ${m.name.ifBlank { "Monitor ${i + 1}" }}") }
                            }
                        }
                    }
                }
                HorizontalDivider()
            }
        }
    }
}

/** Deck/dock profiles that switch by foreground app (§11). */
@Composable
fun ProfilesScreen(remote: RemoteViewModel, power: PowerViewModel) {
    val pui by power.ui.collectAsStateWithLifecycle()
    LaunchedEffect(pui.phase3) { if (pui.phase3) power.loadProfiles() }
    if (!pui.phase3) return Phase3Required("Profiles")
    var editing by remember { mutableStateOf<Profile?>(null) }
    val list = pui.profiles
    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        HintText("When the matching program is in front on the PC, the deck and dock switch to this profile.")
        if (list == null) HintText("Loading…")
        list?.forEach { p ->
            SectionCard(p.name.ifBlank { p.id } + if (pui.activeProfile == p.id) "  (active)" else "") {
                HintText(listOfNotNull(p.matchExe?.let { "When $it is in front" }, p.deckId?.let { "deck $it" }, p.dockPage?.let { "dock page ${it + 1}" }).joinToString(" · ").ifEmpty { "Manual" })
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    TextButton(onClick = { editing = p }) { Text("Edit") }
                    TextButton(onClick = { power.saveProfiles(list.filterNot { it.id == p.id }) }) { Text("Delete") }
                }
            }
        }
        OutlinedButton(enabled = list != null, onClick = { editing = Profile(remote.newId(), "") }) { Text("Add profile") }
    }
    editing?.let { p ->
        var name by remember(p) { mutableStateOf(p.name) }
        var exe by remember(p) { mutableStateOf(p.matchExe ?: "") }
        var deck by remember(p) { mutableStateOf(p.deckId ?: "") }
        var page by remember(p) { mutableStateOf(p.dockPage?.plus(1)?.toString() ?: "") }
        val pageNum = page.trim().takeIf { it.isNotEmpty() }?.toIntOrNull()
        val pageOk = page.isBlank() || (pageNum != null && pageNum >= 1)
        AlertDialog(
            onDismissRequest = { editing = null },
            title = { Text("Profile") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(name, { name = it }, label = { Text("Name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    OutlinedTextField(exe, { exe = it }, label = { Text("Program, e.g. chrome.exe") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    OutlinedTextField(deck, { deck = it }, label = { Text("Deck id (optional)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    OutlinedTextField(page, { page = it }, label = { Text("Dock page number (optional)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                }
            },
            confirmButton = {
                TextButton(enabled = name.isNotBlank() && pageOk, onClick = {
                    val updated = p.copy(name = name.trim(), matchExe = exe.trim().ifEmpty { null }, deckId = deck.trim().ifEmpty { null }, dockPage = pageNum?.minus(1))
                    val cur = list.orEmpty()
                    power.saveProfiles(if (cur.any { it.id == p.id }) cur.map { if (it.id == p.id) updated else it } else cur + updated)
                    editing = null
                }) { Text("Save") }
            },
            dismissButton = { TextButton(onClick = { editing = null }) { Text("Cancel") } },
        )
    }
}
