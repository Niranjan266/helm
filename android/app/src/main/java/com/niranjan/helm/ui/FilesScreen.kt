package com.niranjan.helm.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Image
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.device.SharedItem

@Composable
fun FilesScreen(remote: RemoteViewModel) {
    val ui by remote.ui.collectAsStateWithLifecycle()
    val pending by remote.pendingShare.collectAsStateWithLifecycle()
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris -> remote.uploadUris(uris) }
    var url by rememberSaveable { mutableStateOf("") }
    var path by rememberSaveable { mutableStateOf("") }
    var monitorIndex by rememberSaveable { mutableStateOf(-1) }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (pending.isNotEmpty()) {
            SectionCard("Shared to Helm") {
                pending.forEach { item ->
                    Text(
                        when (item) {
                            is SharedItem.File -> "File: " + (item.uri.lastPathSegment ?: item.uri.toString())
                            is SharedItem.Text -> (if (item.isUrl) "Link: " else "Text: ") + item.text
                        },
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                HintText("Files go to Downloads\\Helm on the PC, links open in its browser, text goes to its clipboard.")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PencilButton(onClick = remote::sendPendingShare) { Text("Send to PC") }
                    OutlineButton(onClick = remote::discardPendingShare) { Text("Discard") }
                }
            }
        }

        ui.transfer?.let { t ->
            SectionCard("Transfer") {
                Text(t.label, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.size(6.dp))
                val p = t.progress
                if (p == null) LinearProgressIndicator(Modifier.fillMaxWidth()) else LinearProgressIndicator(progress = { p }, modifier = Modifier.fillMaxWidth())
            }
        }

        SectionCard("Send files") {
            HintText("Saved to Downloads\\Helm on the PC.")
            Spacer(Modifier.size(6.dp))
            PencilButton(onClick = { pick.launch(arrayOf("*/*")) }, enabled = ui.transfer == null) { Text("Pick files…") }
        }

        SectionCard("Clipboard") {
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlineButton(onClick = remote::pushClipboard) { Text("Phone → PC") }
                OutlineButton(onClick = remote::pullClipboard) { Text("PC → phone") }
            }
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Copy PC clipboard to the phone automatically while Helm is open", modifier = Modifier.weight(1f))
                Switch(checked = ui.clipboardWatch, onCheckedChange = remote::setClipboardWatch)
            }
        }

        SectionCard("Open on PC") {
            OutlinedTextField(url, { url = it }, label = { Text("URL") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            Spacer(Modifier.size(6.dp))
            PencilButton(onClick = { remote.openOnPc(url) }, enabled = url.isNotBlank()) { Text("Open") }
        }

        SectionCard("Screenshot") {
            val monitors = ui.monitors
            if (monitors.size > 1) {
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    BpFilterChip(selected = monitorIndex < 0, onClick = { monitorIndex = -1 }, label = { Text("Primary") })
                    monitors.forEachIndexed { i, m ->
                        BpFilterChip(selected = monitorIndex == i, onClick = { monitorIndex = i }, label = { Text(monitorName(m, i)) })
                    }
                }
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                PencilButton(onClick = { remote.capture(monitors.getOrNull(monitorIndex)) }, enabled = !ui.capturing) { Text("Capture") }
                if (ui.capturing) CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
                if (ui.screenshot != null) OutlineButton(onClick = remote::saveScreenshot) { Text("Save to gallery") }
            }
            ui.screenshot?.let { bmp ->
                Spacer(Modifier.size(8.dp))
                Image(
                    bmp.asImageBitmap(),
                    contentDescription = "Screenshot",
                    contentScale = ContentScale.Fit,
                    modifier = Modifier.fillMaxWidth().heightIn(max = 480.dp),
                )
            }
        }

        SectionCard("Get a file from the PC") {
            HintText("Full path on the PC, e.g. C:\\Users\\you\\Documents\\report.pdf. Saved to Downloads/Helm on this phone.")
            OutlinedTextField(path, { path = it }, label = { Text("Path") }, singleLine = true, modifier = Modifier.fillMaxWidth())
            Spacer(Modifier.size(6.dp))
            PencilButton(onClick = { remote.download(path) }, enabled = path.isNotBlank() && ui.transfer == null) { Text("Download") }
        }
        Spacer(Modifier.width(1.dp))
    }
}

private fun monitorName(m: MonitorInfo, i: Int): String =
    "${i + 1}: ${m.width}×${m.height}" + if (m.isPrimary) " ★" else ""
