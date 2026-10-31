package com.niranjan.helm.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import kotlinx.coroutines.launch

/** Searchable app list (launch / add to dock) and open windows (focus, minimize, …). */
@Composable
fun AppsScreen(remote: RemoteViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    var windowsTab by rememberSaveable { mutableStateOf(false) }
    var query by rememberSaveable { mutableStateOf("") }
    val scope = rememberCoroutineScope()
    LaunchedEffect(Unit) { remote.loadApps() }

    Column(Modifier.fillMaxSize().padding(horizontal = 12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(selected = !windowsTab, onClick = { windowsTab = false }, label = { Text("Apps") })
            FilterChip(selected = windowsTab, onClick = { windowsTab = true }, label = { Text("Open windows (${ui.windows.size})") })
            Spacer(Modifier.weight(1f))
            IconButton(onClick = { if (windowsTab) scope.launch { remote.refreshWindows() } else remote.loadApps(force = true) }) {
                Icon(Icons.Filled.Refresh, contentDescription = "Refresh")
            }
        }
        if (!windowsTab) {
            OutlinedTextField(query, { query = it }, placeholder = { Text("Search apps") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            val apps = ui.apps
            when {
                apps == null && ui.appsLoading -> Row(Modifier.padding(16.dp), verticalAlignment = Alignment.CenterVertically) {
                    CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                    Spacer(Modifier.width(12.dp))
                    Text("Loading apps…")
                }
                apps == null -> HintText("Could not load the app list.", Modifier.padding(16.dp))
                else -> {
                    val q = query.trim().lowercase()
                    val shown = if (q.isEmpty()) apps else apps.filter { q in it.name.lowercase() }
                    LazyColumn(Modifier.fillMaxSize()) {
                        items(shown, key = { it.appId }) { app ->
                            Row(
                                Modifier.fillMaxWidth().clickable { remote.launchApp(app) }.padding(vertical = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                AppIcon(app.appId, app.name, remote.icons, 36.dp)
                                Spacer(Modifier.width(12.dp))
                                Column(Modifier.weight(1f)) {
                                    Text(app.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    if (remote.isRunning(app.appId)) {
                                        Text("Running", style = MaterialTheme.typography.bodySmall, color = OnlineGreen)
                                    }
                                }
                                IconButton(onClick = { remote.addAppToDock(app) }) {
                                    Icon(Icons.Filled.Add, contentDescription = "Add ${app.name} to dock")
                                }
                            }
                        }
                    }
                }
            }
        } else {
            if (ui.windows.isEmpty()) HintText("No open windows.", Modifier.padding(16.dp))
            LazyColumn(Modifier.fillMaxSize()) {
                items(ui.windows, key = { it.hwnd.content }) { w ->
                    Column(Modifier.fillMaxWidth().clickable { remote.windowOp(w, "focus") }.padding(vertical = 8.dp)) {
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            if (w.appId != null) AppIcon(w.appId, w.title, remote.icons, 28.dp) else LetterIcon(w.title, 28.dp)
                            Spacer(Modifier.width(12.dp))
                            Text(
                                w.title.ifBlank { "(untitled)" } + if (w.isForeground) "  · front" else if (w.isMinimized) "  · minimized" else "",
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                            )
                        }
                        Row {
                            TextButton(onClick = { remote.windowOp(w, "minimize") }) { Text("Minimize") }
                            TextButton(onClick = { remote.windowOp(w, "maximize") }) { Text("Maximize") }
                            TextButton(onClick = { remote.windowOp(w, "restore") }) { Text("Restore") }
                            TextButton(onClick = { remote.windowOp(w, "close") }) { Text("Close", color = MaterialTheme.colorScheme.error) }
                        }
                    }
                }
            }
        }
    }
}
