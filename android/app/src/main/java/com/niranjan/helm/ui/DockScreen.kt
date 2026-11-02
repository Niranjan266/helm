package com.niranjan.helm.ui

import androidx.compose.ui.unit.sp
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.awaitLongPressOrCancellation
import androidx.compose.foundation.gestures.drag
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.zIndex
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.AppEntry
import com.niranjan.helm.core.DockFolders
import com.niranjan.helm.core.DockTile
import com.niranjan.helm.core.IconRef
import com.niranjan.helm.core.SmartOrder
import com.niranjan.helm.device.Settings

/** Rows × columns for a page size, wide in landscape. */
private fun gridFor(pageSize: Int, landscape: Boolean): Pair<Int, Int> = when (pageSize) {
    4 -> if (landscape) 1 to 4 else 2 to 2
    8 -> if (landscape) 2 to 4 else 4 to 2
    else -> if (landscape) 2 to 3 else 3 to 2
}

private fun tileName(t: DockTile): String = t.label ?: t.appId ?: t.target ?: if (t.kind == DockTile.KIND_FOLDER) "Folder" else "?"

/**
 * Tap, long-press (no movement) and long-press-then-drag on one tile. Before the long press,
 * moving the finger is left to the pager (page swipes still work).
 */
private fun Modifier.tileGestures(
    key: Any,
    onTap: () -> Unit,
    onLongPress: () -> Unit,
    onDragStart: (Offset) -> Unit,
    onDrag: (Offset) -> Unit,
    onDrop: () -> Unit,
    onCancel: () -> Unit,
): Modifier = pointerInput(key) {
    awaitEachGesture {
        val down = awaitFirstDown(requireUnconsumed = false)
        val held = awaitLongPressOrCancellation(down.id)
        if (held == null) {
            val up = currentEvent.changes.firstOrNull { it.id == down.id }
            if (up != null && !up.pressed && !up.isConsumed) {
                up.consume()
                onTap()
            }
            return@awaitEachGesture
        }
        onDragStart(held.position)
        var total = Offset.Zero
        var dragging = false
        val released = drag(held.id) { change ->
            total += change.positionChange()
            change.consume()
            if (!dragging && total.getDistance() > viewConfiguration.touchSlop) dragging = true
            if (dragging) onDrag(total)
        }
        when {
            !dragging -> {
                onCancel()
                onLongPress()
            }
            released -> onDrop()
            else -> onCancel()
        }
    }
}

@OptIn(ExperimentalFoundationApi::class, ExperimentalMaterial3Api::class)
@Composable
fun DockScreen(remote: RemoteViewModel, settings: Settings, landscape: Boolean, onOpenApps: () -> Unit) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val haptic = LocalHapticFeedback.current
    var adding by remember { mutableStateOf(false) }
    var editingId by remember { mutableStateOf<String?>(null) }
    var openFolderId by remember { mutableStateOf<String?>(null) }
    val pages = ui.dock
    val (rows, cols) = gridFor(settings.dockPageSize, landscape)

    // Drag-to-folder state: tile bounds in root coordinates, the dragged tile and its offset.
    val bounds = remember { mutableMapOf<String, Rect>() }
    var dragId by remember { mutableStateOf<String?>(null) }
    var dragStart by remember { mutableStateOf(Offset.Zero) }
    var dragDelta by remember { mutableStateOf(Offset.Zero) }
    var hoverId by remember { mutableStateOf<String?>(null) }

    fun buzz(type: HapticFeedbackType) {
        if (settings.haptics) haptic.performHapticFeedback(type)
    }

    Box(Modifier.fillMaxSize()) {
        if (pages.isEmpty()) {
            Column(
                Modifier.fillMaxSize().padding(24.dp),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text("Your dock is empty", style = MaterialTheme.typography.titleLarge)
                Spacer(Modifier.height(8.dp))
                HintText("Add apps, folders or links. Tap a tile to launch or switch to it; long-press to edit it, or hold and drag it onto another tile to make a folder.")
                Spacer(Modifier.height(16.dp))
                PencilButton(onClick = { adding = true }) { Text("Add to dock") }
            }
        } else {
            val pager = rememberPagerState { pages.size }
            Column(Modifier.fillMaxSize()) {
                HorizontalPager(state = pager, modifier = Modifier.weight(1f).fillMaxWidth(), userScrollEnabled = dragId == null) { index ->
                    val saved = pages.getOrElse(index) { emptyList() }
                    val page = if (settings.smartOrder) SmartOrder.sortTiles(saved, ui.usage) else saved
                    // Pages may hold more tiles than this phone's page size (set on another phone).
                    val r = maxOf(rows, (page.size + cols - 1) / cols)
                    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                        for (row in 0 until r) {
                            Row(Modifier.weight(1f).fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                for (col in 0 until cols) {
                                    val tile = page.getOrNull(row * cols + col)
                                    val isDragged = tile != null && tile.id == dragId
                                    Box(
                                        Modifier
                                            .weight(1f)
                                            .fillMaxSize()
                                            .zIndex(if (isDragged) 1f else 0f),
                                    ) {
                                        if (tile != null) {
                                            DisposableEffect(tile.id) { onDispose { bounds.remove(tile.id) } }
                                            Box(
                                                Modifier
                                                    .fillMaxSize()
                                                    .onGloballyPositioned { bounds[tile.id] = it.boundsInRoot() }
                                                    .graphicsLayer {
                                                        if (isDragged) {
                                                            translationX = dragDelta.x
                                                            translationY = dragDelta.y
                                                            scaleX = 1.06f
                                                            scaleY = 1.06f
                                                            alpha = 0.85f
                                                        }
                                                    }
                                                    .tileGestures(
                                                        key = tile.id,
                                                        onTap = {
                                                            buzz(HapticFeedbackType.TextHandleMove)
                                                            if (tile.kind == DockTile.KIND_FOLDER) openFolderId = tile.id else remote.tapTile(tile)
                                                        },
                                                        onLongPress = {
                                                            buzz(HapticFeedbackType.LongPress)
                                                            editingId = tile.id
                                                        },
                                                        onDragStart = { local ->
                                                            buzz(HapticFeedbackType.LongPress)
                                                            dragId = tile.id
                                                            dragStart = (bounds[tile.id]?.topLeft ?: Offset.Zero) + local
                                                            dragDelta = Offset.Zero
                                                        },
                                                        onDrag = { d ->
                                                            dragDelta = d
                                                            val at = dragStart + d
                                                            val over = bounds.entries.firstOrNull { (id, b) -> id != tile.id && b.contains(at) }?.key
                                                            if (over != hoverId && over != null) buzz(HapticFeedbackType.TextHandleMove)
                                                            hoverId = over
                                                        },
                                                        onDrop = {
                                                            val target = hoverId
                                                            dragId = null
                                                            hoverId = null
                                                            dragDelta = Offset.Zero
                                                            if (target != null) remote.dropTile(tile.id, target)
                                                        },
                                                        onCancel = {
                                                            dragId = null
                                                            hoverId = null
                                                            dragDelta = Offset.Zero
                                                        },
                                                    ),
                                            ) {
                                                DockTileView(
                                                    tile = tile,
                                                    running = remote.isRunning(tile.appId) && ui.windows.isNotEmpty(),
                                                    highlighted = tile.id == hoverId,
                                                    apps = ui.apps,
                                                    remote = remote,
                                                )
                                            }
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
            apps = ui.apps?.let { if (settings.smartOrder) SmartOrder.sortApps(it, ui.usage) else it },
            loading = ui.appsLoading,
            icons = remote.icons,
            onLoad = { remote.loadApps() },
            onPick = { remote.addAppToDock(it); adding = false },
            onDismiss = { adding = false },
            allowLink = true,
            onLink = { t, l -> remote.addLinkToDock(t, l); adding = false },
        )
    }

    openFolderId?.let { fid ->
        val folder = pages.flatten().firstOrNull { it.id == fid && it.kind == DockTile.KIND_FOLDER }
        if (folder == null) {
            openFolderId = null
        } else {
            val children = if (settings.smartOrder) SmartOrder.sortTiles(folder.children, ui.usage) else folder.children
            ModalBottomSheet(onDismissRequest = { openFolderId = null }) {
                Column(Modifier.fillMaxWidth().padding(horizontal = 16.dp).padding(bottom = 24.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(tileName(folder), style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                        TextButton(onClick = { editingId = folder.id }) { Text("Edit folder") }
                    }
                    HintText("Tap to launch. Long-press a tile to rename it or take it out of the folder.")
                    Spacer(Modifier.height(8.dp))
                    LazyVerticalGrid(GridCells.Adaptive(96.dp), Modifier.heightIn(max = 480.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        items(children, key = { it.id }) { child ->
                            Box(
                                Modifier
                                    .aspectRatio(1f)
                                    .combinedClickable(
                                        onClick = {
                                            buzz(HapticFeedbackType.TextHandleMove)
                                            remote.tapTile(child)
                                        },
                                        onLongClick = {
                                            buzz(HapticFeedbackType.LongPress)
                                            editingId = child.id
                                        },
                                    ),
                            ) {
                                DockTileView(child, remote.isRunning(child.appId) && ui.windows.isNotEmpty(), false, ui.apps, remote)
                            }
                        }
                    }
                }
            }
        }
    }

    editingId?.let { id ->
        val tile = DockFolders.find(pages, id)
        if (tile == null) {
            editingId = null
        } else {
            TileEditor(
                tile = tile,
                pages = pages,
                remote = remote,
                g5 = ui.g5,
                onDismiss = { editingId = null },
                onClosedFolder = { openFolderId = null },
            )
        }
    }
}

@Composable
private fun TileEditor(
    tile: DockTile,
    pages: List<List<DockTile>>,
    remote: RemoteViewModel,
    g5: Boolean,
    onDismiss: () -> Unit,
    onClosedFolder: () -> Unit,
) {
    val pageIndex = pages.indexOfFirst { p -> p.any { it.id == tile.id } }
    val inFolder = pageIndex < 0
    val isFolder = tile.kind == DockTile.KIND_FOLDER
    var label by remember(tile.id) { mutableStateOf(tile.label ?: "") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (isFolder) "Folder" else tileName(tile)) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(label, { label = it }, label = { Text(if (isFolder) "Folder name" else "Label") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                if (g5) {
                    IconChooserRow(tile.icon, IconRef.forTile(tile.copy(icon = null), remote.ui.value.apps), tileName(tile), remote) { remote.setTileIcon(tile.id, it) }
                }
                if (!inFolder) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlineButton(onClick = { remote.moveTile(tile.id, -1) }, modifier = Modifier.weight(1f)) { Text("◀ Earlier") }
                        OutlineButton(onClick = { remote.moveTile(tile.id, +1) }, modifier = Modifier.weight(1f)) { Text("Later ▶") }
                    }
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlineButton(
                            enabled = pageIndex > 0,
                            onClick = { remote.moveTileToPage(tile.id, pageIndex - 1); onDismiss() },
                            modifier = Modifier.weight(1f),
                        ) { Text("Prev page") }
                        OutlineButton(
                            onClick = { remote.moveTileToPage(tile.id, pageIndex + 1); onDismiss() },
                            modifier = Modifier.weight(1f),
                        ) { Text(if (pageIndex >= pages.lastIndex) "New page" else "Next page") }
                    }
                } else {
                    OutlineButton(onClick = { remote.takeOutOfFolder(tile.id); onDismiss() }) { Text("Take out of folder") }
                }
                if (isFolder) {
                    OutlineButton(onClick = { remote.ungroupFolder(tile.id); onClosedFolder(); onDismiss() }) { Text("Ungroup (keep the tiles)") }
                }
                TextButton(onClick = {
                    remote.removeTile(tile.id)
                    if (isFolder) onClosedFolder()
                    onDismiss()
                }) {
                    Text(if (isFolder) "Remove folder and its tiles" else "Remove from dock", color = MaterialTheme.colorScheme.error)
                }
            }
        },
        confirmButton = {
            TextButton(onClick = {
                if (label != (tile.label ?: "")) remote.renameTile(tile.id, label)
                onDismiss()
            }) { Text("Done") }
        },
    )
}

@Composable
private fun DockTileView(tile: DockTile, running: Boolean, highlighted: Boolean, apps: List<AppEntry>?, remote: RemoteViewModel) {
    val name = tileName(tile)
    BpCard(
        modifier = Modifier
            .fillMaxSize()
            .then(if (highlighted) Modifier.border(3.dp, Bp.c.pencil) else Modifier),
    ) {
        Box(Modifier.fillMaxSize()) {
            Column(
                Modifier.fillMaxSize().padding(8.dp),
                verticalArrangement = Arrangement.Center,
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                if (tile.kind == DockTile.KIND_FOLDER && tile.icon == null) {
                    FolderPreview(tile.children, remote, apps)
                } else {
                    IconView(IconRef.forTile(tile, apps), name.removePrefix("https://").removePrefix("http://"), remote.icons, remote.customIcons, 48.dp)
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

/** Up to four child icons in a 2×2 grid. */
@Composable
private fun FolderPreview(children: List<DockTile>, remote: RemoteViewModel, apps: List<AppEntry>?) {
    Box(
        Modifier
            .size(52.dp)
            .background(MaterialTheme.colorScheme.surfaceVariant, RoundedCornerShape(12.dp))
            .padding(4.dp),
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            for (r in 0 until 2) {
                Row(horizontalArrangement = Arrangement.spacedBy(2.dp)) {
                    for (c in 0 until 2) {
                        val child = children.getOrNull(r * 2 + c)
                        Box(Modifier.size(21.dp), contentAlignment = Alignment.Center) {
                            if (child != null) IconView(IconRef.forTile(child, apps), tileName(child), remote.icons, remote.customIcons, 20.dp)
                        }
                    }
                }
            }
        }
    }
}
