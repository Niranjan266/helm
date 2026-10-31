package com.niranjan.helm.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
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
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.LinearProgressIndicator
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
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.FsEntry
import com.niranjan.helm.core.PcPath
import java.text.DateFormat
import java.util.Date

/** Browse the PC (`fs.*`, admin): open, download, delete, new folder; upload to Downloads\Helm. */
@Composable
fun PcFilesScreen(remote: RemoteViewModel, power: PowerViewModel) {
    val ui by power.ui.collectAsStateWithLifecycle()
    val rui by remote.ui.collectAsStateWithLifecycle()
    val gate = LocalAdminGate.current
    if (!ui.phase3) return Phase3Required("The file browser")
    var unlocked by rememberSaveable { mutableStateOf(false) }
    var selected by remember { mutableStateOf<FsEntry?>(null) }
    var confirmDelete by remember { mutableStateOf<FsEntry?>(null) }
    var newFolder by remember { mutableStateOf<String?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { remote.uploadUris(it) }

    if (!unlocked) {
        Column(Modifier.fillMaxSize().padding(24.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            HintText("Browsing the PC's files is an admin action.")
            Button(onClick = { gate("Browse the PC's files") { unlocked = true; power.browse(ui.fsPath) } }) { Text("Open file browser") }
        }
        return
    }

    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Text(ui.fsPath.ifEmpty { "This PC" }, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(enabled = ui.fsPath.isNotEmpty(), onClick = power::up) { Text("↑ Up") }
            OutlinedButton(onClick = power::refreshFolder) { Text("Refresh") }
            OutlinedButton(enabled = ui.fsPath.isNotEmpty(), onClick = { newFolder = "" }) { Text("New folder") }
            OutlinedButton(onClick = { pick.launch(arrayOf("*/*")) }) { Text("Upload") }
        }
        rui.transfer?.let { t ->
            Text(t.label, style = MaterialTheme.typography.bodySmall)
            if (t.progress != null) LinearProgressIndicator(progress = { t.progress }, modifier = Modifier.fillMaxWidth()) else LinearProgressIndicator(Modifier.fillMaxWidth())
        }
        if (ui.fsLoading) LinearProgressIndicator(Modifier.fillMaxWidth())
        val entries = ui.fsEntries
        if (entries != null && entries.isEmpty()) HintText("This folder is empty.")
        LazyColumn(Modifier.fillMaxSize()) {
            items(entries.orEmpty(), key = { it.name }) { e ->
                Row(
                    Modifier.fillMaxWidth().clickable { if (e.isDir) power.browse(power.pathOf(e)) else selected = e }.padding(vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(if (e.isDir) "📁" else "📄", Modifier.padding(end = 12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(e.name, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        val meta = listOfNotNull(
                            if (e.isDir) null else formatBytes(e.size),
                            e.modifiedMs?.let { DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(it)) },
                        ).joinToString(" · ")
                        if (meta.isNotEmpty()) HintText(meta)
                    }
                    if (e.isDir && ui.fsPath.isNotEmpty()) TextButton(onClick = { selected = e }) { Text("⋯") }
                }
                HorizontalDivider()
            }
        }
        HintText("Uploads go to the PC's Downloads\\Helm folder.")
    }

    selected?.let { e ->
        AlertDialog(
            onDismissRequest = { selected = null },
            title = { Text(e.name) },
            text = {
                Column {
                    TextButton(onClick = { selected = null; power.openOnPc(e) }) { Text("Open on the PC") }
                    if (!e.isDir) TextButton(onClick = { selected = null; remote.download(power.pathOf(e)) }) { Text("Download to this phone") }
                    if (ui.fsPath.isNotEmpty()) TextButton(onClick = { selected = null; confirmDelete = e }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
                }
            },
            confirmButton = {},
            dismissButton = { TextButton(onClick = { selected = null }) { Text("Close") } },
        )
    }
    confirmDelete?.let { e ->
        AlertDialog(
            onDismissRequest = { confirmDelete = null },
            title = { Text("Delete ${e.name}?") },
            text = { Text(if (e.isDir) "The folder and everything in it will be deleted on the PC." else "The file will be deleted on the PC.") },
            confirmButton = { TextButton(onClick = { confirmDelete = null; gate("Delete ${e.name}") { power.delete(e) } }) { Text("Delete", color = MaterialTheme.colorScheme.error) } },
            dismissButton = { TextButton(onClick = { confirmDelete = null }) { Text("Cancel") } },
        )
    }
    newFolder?.let { name ->
        AlertDialog(
            onDismissRequest = { newFolder = null },
            title = { Text("New folder") },
            text = { OutlinedTextField(name, { newFolder = it }, singleLine = true, label = { Text("Name") }) },
            confirmButton = { TextButton(enabled = PcPath.validName(name), onClick = { newFolder = null; power.mkdir(name) }) { Text("Create") } },
            dismissButton = { TextButton(onClick = { newFolder = null }) { Text("Cancel") } },
        )
    }
}
