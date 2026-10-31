package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import com.niranjan.helm.device.Settings
import com.niranjan.helm.device.ThemeMode

@Composable
fun SettingsScreen(settings: Settings, onSettings: ((Settings) -> Settings) -> Unit, pcDetails: @Composable () -> Unit) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionCard("This phone") {
            ToggleRow("Keep screen on while the Dock is open", settings.keepScreenOnInDock) { v -> onSettings { it.copy(keepScreenOnInDock = v) } }
            ToggleRow("Haptic feedback", settings.haptics) { v -> onSettings { it.copy(haptics = v) } }
            Text("Theme")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                FilterChip(selected = settings.theme == ThemeMode.DARK, onClick = { onSettings { it.copy(theme = ThemeMode.DARK) } }, label = { Text("Dark") })
                FilterChip(selected = settings.theme == ThemeMode.AMOLED, onClick = { onSettings { it.copy(theme = ThemeMode.AMOLED) } }, label = { Text("AMOLED black") })
            }
            Text("Dock tiles per page")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf(4, 6, 8).forEach { n ->
                    FilterChip(selected = settings.dockPageSize == n, onClick = { onSettings { it.copy(dockPageSize = n) } }, label = { Text("$n") })
                }
            }
        }
        pcDetails()
    }
}

@Composable
private fun ToggleRow(label: String, checked: Boolean, onChange: (Boolean) -> Unit) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        Text(label, modifier = Modifier.weight(1f))
        Switch(checked = checked, onCheckedChange = onChange)
    }
}
