package com.niranjan.helm.ui

import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.niranjan.helm.core.ClipItem
import com.niranjan.helm.core.SystemHealth
import java.text.DateFormat
import java.util.Date

/** Everyday §1: the PC's last 20 clipboard items (opt-in on the PC side). */
@Composable
fun ClipboardHistoryScreen(vm: EverydayViewModel = viewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val context = LocalContext.current
    var confirmClear by remember { mutableStateOf(false) }
    LaunchedEffect(ui.g6) { if (ui.g6) vm.loadClips() }
    val h = ui.clips

    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        if (!ui.g6) {
            HintText("This PC's Helm agent does not keep a clipboard history yet. Update Helm on the PC.")
            return@Column
        }
        SectionCard("Clipboard history") {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Keep the last 20 items", Modifier.weight(1f))
                Switch(checked = h?.enabled == true, enabled = h != null, onCheckedChange = { vm.setClipHistory(enabled = it) })
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Keep after the PC restarts (encrypted)", Modifier.weight(1f))
                Switch(checked = h?.persist == true, enabled = h?.enabled == true, onCheckedChange = { vm.setClipHistory(persist = it) })
            }
            HintText("Items copied from password managers are never recorded.")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlineButton(onClick = vm::loadClips) { Text("Refresh") }
                OutlineButton(enabled = !h?.items.isNullOrEmpty(), onClick = { confirmClear = true }) { Text("Clear") }
            }
        }
        when {
            h == null -> HintText("Loading…")
            !h.enabled -> HintText("History is off. Turn it on to see what you copy on the PC.")
            h.items.isEmpty() -> HintText("Nothing copied yet.")
            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(h.items, key = { it.id }) { item ->
                    ClipCard(
                        item = item,
                        thumb = ui.thumbs[item.id],
                        onToPhone = { vm.copyToPhone(item) },
                        onShare = { vm.share(item) { context.startActivity(it) } },
                        onToPc = { vm.copyOnPc(item) },
                        onDelete = { vm.deleteClip(item) },
                    )
                }
            }
        }
    }

    if (confirmClear) {
        AlertDialog(
            onDismissRequest = { confirmClear = false },
            title = { Text("Clear clipboard history?") },
            text = { Text("Every item is removed from the PC's history.") },
            confirmButton = { TextButton(onClick = { confirmClear = false; vm.clearClips() }) { Text("Clear") } },
            dismissButton = { TextButton(onClick = { confirmClear = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun ClipCard(
    item: ClipItem,
    thumb: android.graphics.Bitmap?,
    onToPhone: () -> Unit,
    onShare: () -> Unit,
    onToPc: () -> Unit,
    onDelete: () -> Unit,
) {
    BpCard(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            if (item.isImage) {
                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (thumb != null) {
                        Image(thumb.asImageBitmap(), contentDescription = "Copied image", contentScale = ContentScale.Fit, modifier = Modifier.size(96.dp))
                    }
                    Text("Image" + (if (item.width != null && item.height != null) " ${item.width} × ${item.height}" else ""))
                }
            } else {
                Text(
                    (item.text ?: "").ifBlank { "(blank text)" } + if (item.truncated) "…" else "",
                    maxLines = 6,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.heightIn(max = 160.dp),
                )
            }
            Text(
                DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(item.timeMs)) + " · " + SystemHealth.formatBytes(item.size),
                style = MaterialTheme.typography.labelSmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                TextButton(onClick = onToPhone) { Text("Copy to phone") }
                TextButton(onClick = onShare) { Text("Share") }
                TextButton(onClick = onToPc) { Text("Copy on PC") }
            }
            TextButton(onClick = onDelete) { Text("Delete", color = MaterialTheme.colorScheme.error) }
        }
    }
}
