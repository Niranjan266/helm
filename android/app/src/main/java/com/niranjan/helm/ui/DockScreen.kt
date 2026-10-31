package com.niranjan.helm.ui

import androidx.compose.ui.unit.sp
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.DockTile
import com.niranjan.helm.device.Settings

/** Rows × columns for a page size, wide in landscape. */
private fun gridFor(pageSize: Int, landscape: Boolean): Pair<Int, Int> = when (pageSize) {
    4 -> if (landscape) 1 to 4 else 2 to 2
    8 -> if (landscape) 2 to 4 else 4 to 2
    else -> if (landscape) 2 to 3 else 3 to 2
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun DockScreen(remote: RemoteViewModel, settings: Settings, landscape: Boolean, onOpenApps: () -> Unit) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val haptic = LocalHapticFeedback.current
    var adding by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf<DockTile?>(null) }
    val pages = ui.dock
    val (rows, cols) = gridFor(settings.dockPageSize, landscape)

    Box(Modifier.fillMaxSize()) {
        if (pages.isEmpty()) {
            Column(
                Modifier.fillMaxSize().padding(24.dp),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text("Your dock is empty", style = MaterialTheme.typography.titleLarge)
                Spacer(Modifier.height(8.dp))
                HintText("Add apps, folders or links. Tap a tile to launch or switch to it; long-press to move or remove it.")
                Spacer(Modifier.height(16.dp))
                PencilButton(onClick = { adding = true }) { Text("Add to dock") }
            }
        } else {
            val pager = rememberPagerState { pages.size }
            Column(Modifier.fillMaxSize()) {
                HorizontalPager(state = pager, modifier = Modifier.weight(1f).fillMaxWidth()) { index ->
                    val page = pages.getOrElse(index) { emptyList() }
                    // Pages may hold more tiles than this phone's page size (set on another phone).
                    val r = maxOf(rows, (page.size + cols - 1) / cols)
                    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        for (row in 0 until r) {
                            Row(Modifier.weight(1f).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                for (col in 0 until cols) {
                                    val tile = page.getOrNull(row * cols + col)
                                    Box(Modifier.weight(1f).fillMaxSize()) {
                                        if (tile != null) {
                                            DockTileView(
                                                tile = tile,
                                                running = remote.isRunning(tile.appId) && ui.windows.isNotEmpty(),
                                                icons = remote.icons,
                                                onTap = {
                                                    if (settings.haptics) haptic.performHapticFeedback(HapticFeedbackType.TextHandleMove)
                                                    remote.tapTile(tile)
                                                },
                                                onLongPress = {
                                                    if (settings.haptics) haptic.performHapticFeedback(HapticFeedbackType.LongPress)
                                                    editing = tile
                                                },
                                            )
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                if (pages.size > 1) {
                    Row(Modifier.fillMaxWidth().padding(bottom = 8.dp), horizontalArrangement = Arrangement.Center) {
                        repeat(pages.size) { i ->
                            Box(
                                Modifier
                                    .padding(4.dp)
                                    .size(8.dp)
                                    .background(
                                        if (i == pager.currentPage) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.outlineVariant,
                                        RectangleShape,
                                    ),
                            )
                        }
                    }
                }
            }
            FloatingActionButton(
                onClick = { adding = true },
                modifier = Modifier.align(Alignment.BottomEnd).padding(16.dp),
                containerColor = MaterialTheme.colorScheme.primaryContainer,
            ) { Icon(Icons.Filled.Add, contentDescription = "Add to dock") }
        }
    }

    if (adding) {
        AppPickerDialog(
            title = "Add to dock",
            apps = ui.apps,
            loading = ui.appsLoading,
            icons = remote.icons,
            onLoad = { remote.loadApps() },
            onPick = { remote.addAppToDock(it); adding = false },
            onDismiss = { adding = false },
            allowLink = true,
            onLink = { t, l -> remote.addLinkToDock(t, l); adding = false },
        )
    }

    editing?.let { tile ->
        val pageIndex = pages.indexOfFirst { p -> p.any { it.id == tile.id } }
        var label by remember(tile.id) { mutableStateOf(tile.label ?: "") }
        AlertDialog(
            onDismissRequest = { editing = null },
            title = { Text(tile.label ?: tile.appId ?: tile.target ?: "Tile") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlinedTextField(label, { label = it }, label = { Text("Label") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlineButton(onClick = { remote.moveTile(tile.id, -1) }, modifier = Modifier.weight(1f)) { Text("◀ Earlier") }
                        OutlineButton(onClick = { remote.moveTile(tile.id, +1) }, modifier = Modifier.weight(1f)) { Text("Later ▶") }
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlineButton(
                            enabled = pageIndex > 0,
                            onClick = { remote.moveTileToPage(tile.id, pageIndex - 1); editing = null },
                            modifier = Modifier.weight(1f),
                        ) { Text("Prev page") }
                        OutlineButton(
                            onClick = { remote.moveTileToPage(tile.id, pageIndex + 1); editing = null },
                            modifier = Modifier.weight(1f),
                        ) { Text(if (pageIndex >= pages.lastIndex) "New page" else "Next page") }
                    }
                    TextButton(onClick = { remote.removeTile(tile.id); editing = null }) {
                        Text("Remove from dock", color = MaterialTheme.colorScheme.error)
                    }
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    if (label != (tile.label ?: "")) remote.renameTile(tile.id, label)
                    editing = null
                }) { Text("Done") }
            },
        )
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun DockTileView(tile: DockTile, running: Boolean, icons: IconLoader, onTap: () -> Unit, onLongPress: () -> Unit) {
    val name = tile.label ?: tile.appId ?: tile.target ?: "?"
    BpCard(modifier = Modifier.fillMaxSize()) {
        Box(Modifier.fillMaxSize().combinedClickable(onClick = onTap, onLongClick = onLongPress)) {
            Column(
                Modifier.fillMaxSize().padding(8.dp),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                if (tile.kind == DockTile.KIND_APP && tile.appId != null) {
                    AppIcon(tile.appId, name, icons, 48.dp)
                } else {
                    LetterIcon(name.removePrefix("https://").removePrefix("http://"), 48.dp)
                }
                Spacer(Modifier.height(8.dp))
                Text(
                    name,
                    style = BpType.mono.copy(fontSize = 12.sp),
                    maxLines = 2,
                    overflow = TextOverflow.Ellipsis,
                    textAlign = TextAlign.Center,
                )
            }
            if (running) {
                Box(
                    Modifier
                        .align(Alignment.TopEnd)
                        .padding(10.dp)
                        .size(10.dp)
                        .background(OnlineGreen, CircleShape),
                )
            }
        }
    }
}
