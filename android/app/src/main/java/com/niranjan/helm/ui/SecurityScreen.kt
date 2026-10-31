package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.Tiers

/** Paired devices + access levels, the PC's audit log, and the relay setting. */
@Composable
fun SecurityScreen(main: MainViewModel, power: PowerViewModel) {
    val sui by main.ui.collectAsStateWithLifecycle()
    val ui by power.ui.collectAsStateWithLifecycle()
    val gate = LocalAdminGate.current
    LaunchedEffect(ui.phase4) { if (ui.phase4) power.loadRelay() }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionCard("Paired devices") {
            val devices = sui.devices
            if (devices == null) HintText("Loading…")
            devices?.forEach { d ->
                Text(d.name + if (d.isThis) "  (this phone)" else "", style = MaterialTheme.typography.bodyLarge)
                if (ui.phase3) {
                    Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        Tiers.ALL.forEach { t ->
                            BpFilterChip(
                                selected = d.tier == t,
                                onClick = {
                                    if (d.tier != t) gate("Change ${d.name} to $t") { power.setTier(d.deviceId, t) { main.refreshDevices() } }
                                },
                                label = { Text(t) },
                            )
                        }
                    }
                } else {
                    HintText("Access: ${d.tier}")
                }
            }
            OutlineButton(onClick = main::refreshDevices) { Text("Refresh") }
        }
        if (ui.phase3) {
            SectionCard("Audit log") {
                val lines = ui.audit
                if (lines == null) {
                    OutlineButton(onClick = { gate("Read the audit log") { power.loadAudit() } }) { Text("Show audit log") }
                } else {
                    if (lines.isEmpty()) HintText("No entries.")
                    lines.take(200).forEach { Text(it, style = MaterialTheme.typography.bodySmall, fontFamily = FontFamily.Monospace) }
                    OutlineButton(onClick = power::loadAudit) { Text("Refresh") }
                }
            }
        } else {
            Phase3Required("Access levels and the audit log")
        }
        if (ui.phase4) {
            val relay = ui.relay
            var enabled by remember(relay) { mutableStateOf(relay?.enabled == true) }
            var url by remember(relay) { mutableStateOf(relay?.url ?: "") }
            SectionCard("Relay (away from home)") {
                HintText("When on, the PC keeps a connection to the relay so this phone can reach it from mobile data. The relay only sees encrypted data.")
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Use relay", Modifier.weight(1f))
                    Switch(checked = enabled, onCheckedChange = { enabled = it })
                }
                OutlinedTextField(url, { url = it }, label = { Text("Relay URL, e.g. wss://relay.example.com") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlineButton(enabled = !enabled || url.isNotBlank(), onClick = { gate("Change the relay setting") { power.saveRelay(enabled, url) } }) { Text("Save") }
            }
        }
    }
}
