package com.niranjan.helm.ui

import android.graphics.Bitmap
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.AppEntry

/** An app icon from the PC (cached), or the first letter while loading / if none. */
@Composable
fun AppIcon(appId: String, name: String, icons: IconLoader, size: Dp) {
    val bmp by produceState<Bitmap?>(icons.peek(appId), appId) {
        if (value == null) value = icons.load(appId)
    }
    val b = bmp
    if (b != null) {
        Image(b.asImageBitmap(), contentDescription = null, modifier = Modifier.size(size))
    } else {
        LetterIcon(name, size)
    }
}

@Composable
fun LetterIcon(name: String, size: Dp) {
    Box(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(size / 4))
            .background(MaterialTheme.colorScheme.primaryContainer),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            name.trim().firstOrNull()?.uppercase() ?: "?",
            color = MaterialTheme.colorScheme.onPrimaryContainer,
            style = if (size >= 40.dp) MaterialTheme.typography.titleLarge else MaterialTheme.typography.titleSmall,
        )
    }
}

@Composable
fun SectionCard(title: String, modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh),
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(Modifier.padding(16.dp)) {
            Text(title, style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.primary)
            Spacer(Modifier.height(8.dp))
            content()
        }
    }
}

@Composable
fun HintText(text: String, modifier: Modifier = Modifier) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = modifier)
}

/**
 * Searchable list of the PC's apps (`apps.list`). With [allowLink], a second tab adds a
 * URL / path tile instead.
 */
@Composable
fun AppPickerDialog(
    title: String,
    apps: List<AppEntry>?,
    loading: Boolean,
    icons: IconLoader,
    onLoad: () -> Unit,
    onPick: (AppEntry) -> Unit,
    onDismiss: () -> Unit,
    allowLink: Boolean = false,
    onLink: (target: String, label: String) -> Unit = { _, _ -> },
) {
    LaunchedEffect(Unit) { onLoad() }
    var query by remember { mutableStateOf("") }
    var linkMode by remember { mutableStateOf(false) }
    var target by remember { mutableStateOf("") }
    var label by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                if (allowLink) {
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        FilterChip(selected = !linkMode, onClick = { linkMode = false }, label = { Text("App") })
                        FilterChip(selected = linkMode, onClick = { linkMode = true }, label = { Text("Link or path") })
                    }
                    Spacer(Modifier.height(8.dp))
                }
                if (linkMode) {
                    OutlinedTextField(target, { target = it }, label = { Text("URL, folder or file on the PC") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Spacer(Modifier.height(8.dp))
                    OutlinedTextField(label, { label = it }, label = { Text("Label (optional)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                } else {
                    OutlinedTextField(query, { query = it }, placeholder = { Text("Search apps") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    Spacer(Modifier.height(8.dp))
                    when {
                        apps == null && loading -> Row(verticalAlignment = Alignment.CenterVertically) {
                            CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                            Spacer(Modifier.width(12.dp))
                            Text("Loading apps…")
                        }
                        apps == null -> HintText("Could not load the app list.")
                        else -> {
                            val q = query.trim().lowercase()
                            val shown = if (q.isEmpty()) apps else apps.filter { q in it.name.lowercase() }
                            LazyColumn(Modifier.heightIn(max = 360.dp)) {
                                items(shown, key = { it.appId }) { app ->
                                    Row(
                                        Modifier.fillMaxWidth().clickable { onPick(app) }.padding(vertical = 8.dp),
                                        verticalAlignment = Alignment.CenterVertically,
                                    ) {
                                        AppIcon(app.appId, app.name, icons, 32.dp)
                                        Spacer(Modifier.width(12.dp))
                                        Text(app.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                                    }
                                }
                            }
                        }
                    }
                }
            }
        },
        confirmButton = {
            if (linkMode) {
                TextButton(enabled = target.isNotBlank(), onClick = { onLink(target, label) }) { Text("Add") }
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}
