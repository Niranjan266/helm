package com.niranjan.helm.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.DropMode
import com.niranjan.helm.core.NotifyAccess
import com.niranjan.helm.device.EverydayStore

/** Settings for the everyday (`g6`) features: quick file drop and PC notifications. */
@Composable
fun EverydaySettingsSection() {
    val context = LocalContext.current
    val store = remember { EverydayStore.get(context) }
    val s by store.settings.collectAsStateWithLifecycle()
    val access by store.notifyAccess.collectAsStateWithLifecycle()

    SectionCard("When a file arrives on the PC") {
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            DropMode.entries.forEach { m ->
                FilterChip(selected = s.dropMode == m, onClick = { store.update { it.copy(dropMode = m) } }, label = { Text(m.label) })
            }
        }
        HintText("For files shared to Helm from other apps. Programs and scripts are never opened, only shown in Explorer.")
    }

    SectionCard("PC notifications") {
        Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Show PC notifications", Modifier.weight(1f))
                Switch(checked = s.pcNotify.enabled, onCheckedChange = { v -> store.update { it.copy(pcNotify = it.pcNotify.copy(enabled = v)) } })
            }
            HintText("Windows notifications from the connected PC appear on this phone. Only the apps you allow below.")
            if (s.pcNotify.enabled) {
                access?.let { a ->
                    Text(a.explain, color = if (a == NotifyAccess.ALLOWED) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.error)
                }
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("All apps", Modifier.weight(1f))
                    Switch(checked = s.pcNotify.allApps, onCheckedChange = { v -> store.update { it.copy(pcNotify = it.pcNotify.copy(allApps = v)) } })
                }
                if (s.pcNotify.apps.isEmpty()) {
                    HintText("Apps show up here after their first notification on the PC.")
                } else {
                    s.pcNotify.apps.toSortedMap(String.CASE_INSENSITIVE_ORDER).forEach { (app, on) ->
                        Row(verticalAlignment = Alignment.CenterVertically) {
                            Text(app, Modifier.weight(1f))
                            Switch(
                                checked = on || s.pcNotify.allApps,
                                enabled = !s.pcNotify.allApps,
                                onCheckedChange = { v -> store.update { it.copy(pcNotify = it.pcNotify.withApp(app, v)) } },
                            )
                        }
                    }
                }
            }
        }
    }
}
