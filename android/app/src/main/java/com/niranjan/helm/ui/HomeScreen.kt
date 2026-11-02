package com.niranjan.helm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.DiscoveredHost
import com.niranjan.helm.core.SavedPc

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun HomeScreen(
    saved: List<SavedPc>,
    discovered: Map<String, DiscoveredHost>,
    snackbar: SnackbarHostState,
    onConnect: (SavedPc) -> Unit,
    onForgetLocal: (SavedPc) -> Unit,
    onWake: (SavedPc) -> Unit,
    onScan: () -> Unit,
    onPaste: (String) -> String?,
    /** g4: pair with a nearby PC by the 6-digit code it shows. Returns an error or null. */
    onPairCode: (DiscoveredHost, String) -> String? = { _, _ -> "Code pairing is not available." },
) {
    var nearbyInfo by remember { mutableStateOf<DiscoveredHost?>(null) }
    var showPaste by remember { mutableStateOf(false) }
    var removeTarget by remember { mutableStateOf<SavedPc?>(null) }

    val savedIds = saved.map { it.hostId.lowercase() }.toSet()
    val nearby = discovered.values.filter { it.hostId !in savedIds }.sortedBy { it.name.lowercase() }

    Scaffold(
        topBar = { TopAppBar(title = { Text("Helm") }) },
        snackbarHost = { SnackbarHost(snackbar) },
        bottomBar = {
            Column(Modifier.fillMaxWidth().padding(16.dp)) {
                Button(onClick = onScan, modifier = Modifier.fillMaxWidth().height(56.dp)) {
                    Text("Pair with QR", style = MaterialTheme.typography.titleMedium)
                }
                TextButton(onClick = { showPaste = true }, modifier = Modifier.fillMaxWidth()) {
                    Text("Paste pairing link")
                }
            }
        },
    ) { padding ->
        LazyColumn(
            modifier = Modifier.fillMaxSize().padding(padding),
            contentPadding = PaddingValues(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            item { SectionTitle("Your PCs") }
            if (saved.isEmpty()) {
                item { Hint("No paired PCs yet. Run Helm on your PC and scan its QR code.") }
            }
            items(saved, key = { "s-" + it.hostId }) { pc ->
                val online = discovered.containsKey(pc.hostId.lowercase())
                PcCard(
                    title = pc.name,
                    subtitle = (discovered[pc.hostId.lowercase()]?.address ?: pc.lastAddress ?: pc.addresses.first()) +
                        if (online) " · online" else "",
                    online = online,
                    onClick = { onConnect(pc) },
                    onLongClick = { removeTarget = pc },
                    onWake = if (!online && pc.macs.isNotEmpty()) ({ onWake(pc) }) else null,
                )
            }
            item { Spacer(Modifier.height(8.dp)); SectionTitle("Nearby PCs") }
            if (nearby.isEmpty()) {
                item { Hint("Looking for PCs running Helm on this Wi-Fi…") }
            }
            items(nearby, key = { "n-" + it.hostId }) { host ->
                PcCard(
                    title = host.name,
                    subtitle = "${host.address} · not paired",
                    online = true,
                    onClick = { nearbyInfo = host },
                    onLongClick = null,
                    onWake = null,
                )
            }
            item {
                Hint("Tip: long-press a paired PC to remove it from this phone.")
            }
        }
    }

    nearbyInfo?.let { host ->
        var code by remember(host) { mutableStateOf("") }
        var codeError by remember(host) { mutableStateOf<String?>(null) }
        AlertDialog(
            onDismissRequest = { nearbyInfo = null },
            title = { Text("Pair with ${host.name}") },
            text = {
                Column {
                    Text("Scan the QR code shown on that PC, or — if the camera can't — choose Pair with code on the PC and type the 6-digit code. Check that the PC shows the name ${host.name}.")
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = code,
                        onValueChange = { v -> code = v.filter { it.isDigit() }.take(6); codeError = null },
                        label = { Text("6-digit code") },
                        singleLine = true,
                        isError = codeError != null,
                        supportingText = codeError?.let { { Text(it) } },
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                            keyboardType = androidx.compose.ui.text.input.KeyboardType.NumberPassword,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                if (code.length == 6) {
                    TextButton(onClick = {
                        val err = onPairCode(host, code)
                        if (err == null) nearbyInfo = null else codeError = err
                    }) { Text("Pair with code") }
                } else {
                    TextButton(onClick = { nearbyInfo = null; onScan() }) { Text("Scan QR") }
                }
            },
            dismissButton = { TextButton(onClick = { nearbyInfo = null }) { Text("Close") } },
        )
    }

    removeTarget?.let { pc ->
        AlertDialog(
            onDismissRequest = { removeTarget = null },
            title = { Text("Remove ${pc.name}?") },
            text = { Text("This removes the PC from this phone only. Connect and use \"Forget this PC\" to also remove the phone from the PC.") },
            confirmButton = {
                TextButton(onClick = { onForgetLocal(pc); removeTarget = null }) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { removeTarget = null }) { Text("Cancel") } },
        )
    }

    if (showPaste) {
        var text by remember { mutableStateOf("") }
        var error by remember { mutableStateOf<String?>(null) }
        AlertDialog(
            onDismissRequest = { showPaste = false },
            title = { Text("Paste pairing link") },
            text = {
                Column {
                    Text("Copy the helm://pair link shown on the PC and paste it here.")
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = text,
                        onValueChange = { text = it; error = null },
                        placeholder = { Text("helm://pair?…") },
                        isError = error != null,
                        supportingText = error?.let { { Text(it) } },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    val err = onPaste(text)
                    if (err == null) showPaste = false else error = err
                }) { Text("Pair") }
            },
            dismissButton = { TextButton(onClick = { showPaste = false }) { Text("Cancel") } },
        )
    }
}

@Composable
private fun SectionTitle(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.titleSmall,
        color = MaterialTheme.colorScheme.primary,
        modifier = Modifier.padding(top = 8.dp, bottom = 4.dp),
    )
}

@Composable
private fun Hint(text: String) {
    Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
}

@OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)
@Composable
private fun PcCard(
    title: String,
    subtitle: String,
    online: Boolean,
    onClick: () -> Unit,
    onLongClick: (() -> Unit)?,
    onWake: (() -> Unit)?,
) {
    Card(
        colors = CardDefaults.cardColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh),
        modifier = Modifier.fillMaxWidth(),
    ) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .let {
                    if (onLongClick != null) {
                        it.then(
                            Modifier.combinedClickable(onClick = onClick, onLongClick = onLongClick),
                        )
                    } else {
                        it.clickable(onClick = onClick)
                    }
                }
                .padding(16.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(
                Modifier
                    .size(10.dp)
                    .background(if (online) OnlineGreen else Color(0xFF5A616B), CircleShape),
            )
            Spacer(Modifier.width(14.dp))
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleMedium, fontWeight = FontWeight.SemiBold)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (onWake != null) TextButton(onClick = onWake) { Text("Wake") }
        }
    }
}
