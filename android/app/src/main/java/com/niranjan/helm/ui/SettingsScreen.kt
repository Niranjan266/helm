package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.horizontalScroll
import androidx.compose.material3.Slider
import com.niranjan.helm.core.PcPrefs
import com.niranjan.helm.core.StreamPreset
import com.niranjan.helm.device.Settings
import com.niranjan.helm.device.ThemeMode

@Composable
fun SettingsScreen(
    settings: Settings,
    onSettings: ((Settings) -> Settings) -> Unit,
    pcDetails: @Composable () -> Unit,
    pcPrefs: PcPrefs? = null,
    onPcPrefs: ((PcPrefs) -> PcPrefs) -> Unit = {},
) {
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        SectionCard("This phone") {
            ToggleRow("Keep screen on while the Dock is open", settings.keepScreenOnInDock) { v -> onSettings { it.copy(keepScreenOnInDock = v) } }
            ToggleRow("Haptic feedback", settings.haptics) { v -> onSettings { it.copy(haptics = v) } }
            Text("Theme")
            ThemeChips(settings.theme) { t -> onSettings { it.copy(theme = t) } }
            Text("Dock tiles per page")
            Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                listOf(4, 6, 8).forEach { n ->
                    BpFilterChip(selected = settings.dockPageSize == n, onClick = { onSettings { it.copy(dockPageSize = n) } }, label = { Text("$n") })
                }
            }
            ToggleRow("Smart order: most used apps first (dock and app list)", settings.smartOrder) { v -> onSettings { it.copy(smartOrder = v) } }
        }
        SectionCard("App lock") {
            Text("Lock Helm after it is in the background for")
            Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                com.niranjan.helm.core.AppLockTimer.CHOICES_MIN.forEach { m ->
                    BpFilterChip(
                        selected = settings.lockAfterMin == m,
                        onClick = { onSettings { it.copy(lockAfterMin = m) } },
                        label = { Text(if (m == 0) "Never" else "$m min") },
                    )
                }
            }
            Text("Unlock with fingerprint, face or screen lock. Admin actions still ask separately.", style = androidx.compose.material3.MaterialTheme.typography.bodySmall)
        }
        NotifyMirrorSettings(settings, onSettings)
        EverydaySettingsSection()
        if (pcPrefs != null) {
            SectionCard("This PC on this phone") {
                ToggleRow("Connect from anywhere (relay when not on the same network)", pcPrefs.useRelay) { v -> onPcPrefs { it.copy(useRelay = v) } }
                ToggleRow("Notify me about this PC's alerts", pcPrefs.notifyAlerts) { v -> onPcPrefs { it.copy(notifyAlerts = v) } }
                ToggleRow("Confirm admin actions with fingerprint or screen lock", pcPrefs.biometricAdmin) { v -> onPcPrefs { it.copy(biometricAdmin = v) } }
                ToggleRow("Own touchpad speed for this PC", pcPrefs.touchpadSpeed != null) { v ->
                    onPcPrefs { it.copy(touchpadSpeed = if (v) settings.touchpadSpeed else null) }
                }
                pcPrefs.touchpadSpeed?.let { sp ->
                    DiamondSlider(value = sp, onValueChange = { v -> onPcPrefs { it.copy(touchpadSpeed = v) } }, valueRange = 0.4f..3f)
                }
                Text("Screen quality for this PC")
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    BpFilterChip(selected = pcPrefs.streamPreset == null, onClick = { onPcPrefs { it.copy(streamPreset = null) } }, label = { Text("Default") })
                    StreamPreset.entries.forEach { p ->
                        BpFilterChip(selected = pcPrefs.streamPreset == p.name, onClick = { onPcPrefs { it.copy(streamPreset = p.name) } }, label = { Text(p.label) })
                    }
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
