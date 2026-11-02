package com.niranjan.helm.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.BUILTIN_ICONS
import com.niranjan.helm.core.IconRef
import kotlinx.coroutines.launch

/** Small preview of an icon value plus a button that opens [IconPickerDialog]. */
@Composable
fun IconChooserRow(icon: String?, auto: IconRef, label: String, remote: RemoteViewModel, onIcon: (String?) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        val ref = IconRef.parse(icon).takeIf { it != IconRef.None } ?: auto
        IconView(ref, label.ifBlank { "?" }, remote.icons, remote.customIcons, 40.dp)
        OutlineButton(onClick = { open = true }) { Text(if (icon == null) "Icon: automatic" else "Change icon") }
    }
    if (open) {
        IconPickerDialog(current = icon, remote = remote, onPick = { onIcon(it); open = false }, onDismiss = { open = false })
    }
}

/** Automatic (real app icon / fitting built-in), one of the built-in stroke icons, or a picture from the phone. */
@Composable
fun IconPickerDialog(current: String?, remote: RemoteViewModel, onPick: (String?) -> Unit, onDismiss: () -> Unit) {
    val scope = rememberCoroutineScope()
    var uploading by remember { mutableStateOf(false) }
    val g5 = remote.ui.value.g5
    val pickImage = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri != null) {
            uploading = true
            scope.launch {
                val id = remote.uploadIcon(uri)
                uploading = false
                if (id != null) onPick(id)
            }
        }
    }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Icon") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                    OutlineButton(onClick = { onPick(null) }) { Text("Automatic") }
                    OutlineButton(enabled = g5 && !uploading, onClick = { pickImage.launch("image/*") }) { Text("Upload picture") }
                    if (uploading) CircularProgressIndicator(Modifier.padding(4.dp), strokeWidth = 2.dp)
                }
                if (!g5) HintText("Pictures need a newer Helm agent on the PC.")
                HintText("Automatic uses the PC app's own icon for apps and files.")
                LazyVerticalGrid(GridCells.Adaptive(52.dp), Modifier.heightIn(max = 320.dp), horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(BUILTIN_ICONS) { name ->
                        val value = IconRef.builtin(name)
                        Box(
                            Modifier
                                .aspectRatio(1f)
                                .clip(RoundedCornerShape(10.dp))
                                .border(if (value == current) 2.dp else 1.dp, if (value == current) MaterialTheme.colorScheme.primary else Color.Gray.copy(alpha = 0.4f), RoundedCornerShape(10.dp))
                                .clickable { onPick(value) }
                                .padding(10.dp),
                            contentAlignment = Alignment.Center,
                        ) { BuiltinIcon(name, MaterialTheme.colorScheme.onSurface, 28.dp, Modifier.fillMaxWidth()) }
                    }
                }
            }
        },
        confirmButton = { TextButton(onClick = onDismiss) { Text("Close") } },
    )
}
