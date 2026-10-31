package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.Card
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.PluginInfo

/** Phase 5 plugins (§13): list, enable, grant capabilities. Changes are admin actions. */
@Composable
fun PluginsScreen(vm: ExtrasViewModel) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val gate = LocalAdminGate.current
    LaunchedEffect(Unit) { vm.loadPlugins() }
    val plugins = ui.plugins

    Column(Modifier.fillMaxSize().padding(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            HintText("Plugins run sandboxed on the PC and only get the capabilities you grant.", Modifier.weight(1f))
            OutlinedButton(onClick = vm::loadPlugins) { Text("Refresh") }
        }
        Spacer(Modifier.padding(4.dp))
        when {
            plugins == null -> HintText("Loading…")
            plugins.isEmpty() -> HintText("No plugins installed. Put plugin folders in the PC's Helm\\plugins folder.")
            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                items(plugins, key = { it.id }) { p ->
                    PluginCard(
                        p,
                        onEnabled = { on -> gate("${if (on) "Enable" else "Disable"} plugin ${p.name}") { vm.setPlugin(p, on, p.granted) } },
                        onGrant = { cap, grant ->
                            val granted = if (grant) p.granted + cap else p.granted - cap
                            gate("${if (grant) "Grant" else "Revoke"} \"${PluginInfo.describe(cap)}\" for ${p.name}") {
                                vm.setPlugin(p, p.enabled, granted)
                            }
                        },
                    )
                }
            }
        }
    }
}

@Composable
private fun PluginCard(p: PluginInfo, onEnabled: (Boolean) -> Unit, onGrant: (String, Boolean) -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(p.name, style = MaterialTheme.typography.titleMedium)
                    Text(listOf(p.id, p.version).filter { it.isNotBlank() }.joinToString(" · "), style = MaterialTheme.typography.bodySmall)
                }
                Switch(checked = p.enabled, onCheckedChange = onEnabled)
            }
            p.error?.let { Text("Error: $it", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
            if (p.capabilities.isEmpty()) {
                HintText("Needs no capabilities.")
            } else {
                Text("Capabilities", style = MaterialTheme.typography.labelLarge)
                p.capabilities.forEach { cap ->
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = cap in p.granted, onCheckedChange = { onGrant(cap, it) })
                        Column {
                            Text(PluginInfo.describe(cap))
                            Text(cap, style = MaterialTheme.typography.labelSmall)
                        }
                    }
                }
                if (p.enabled && p.missing.isNotEmpty()) HintText("Some features stay off until their capability is granted.")
            }
        }
    }
}
