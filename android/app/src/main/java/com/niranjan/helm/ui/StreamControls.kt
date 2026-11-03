package com.niranjan.helm.ui

import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import com.niranjan.helm.core.KeyPress
import com.niranjan.helm.core.MonitorInfo
import com.niranjan.helm.core.StreamPreset
import com.niranjan.helm.core.TextDiff
import com.niranjan.helm.core.Vk
import com.niranjan.helm.device.Settings

/** Mode, keyboard, monitor, quality, audio and stats controls above the live screen. */
@Composable
fun StreamControls(
    vm: StreamViewModel,
    ui: StreamUi,
    settings: Settings,
    monitors: List<MonitorInfo>,
    keyboardOn: Boolean,
    onKeyboard: (Boolean) -> Unit,
    onResetView: () -> Unit,
    onHide: () -> Unit,
) {
    val relayChoice = ui.relayChoice
    val preset = relayChoice?.preset ?: StreamPreset.byName(settings.streamPreset)
    val kbps = relayChoice?.kbps ?: if (settings.streamBitrateKbps > 0) settings.streamBitrateKbps else preset.defaultKbps
    Row(
        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 8.dp, vertical = 4.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        BpFilterChip(selected = !settings.streamDirectTouch, onClick = { vm.setDirectTouch(false) }, label = { Text("Trackpad") })
        BpFilterChip(selected = settings.streamDirectTouch, onClick = { vm.setDirectTouch(true) }, label = { Text("Touch") })
        BpFilterChip(selected = keyboardOn, onClick = { onKeyboard(!keyboardOn) }, label = { Text("Keyboard") })

        Picker(
            label = ui.monitor?.name ?: "Primary screen",
            items = listOf<MonitorInfo?>(null) + monitors,
            itemLabel = { m -> m?.let { "${it.name} ${it.width}×${it.height}${if (it.isPrimary) " (primary)" else ""}" } ?: "Primary screen" },
            onPick = vm::selectMonitor,
        )
        Picker(
            label = preset.label,
            items = StreamPreset.entries.toList(),
            itemLabel = { "${it.label} · ${it.defaultKbps / 1000} Mbps" },
            onPick = vm::setPreset,
        )
        Picker(
            label = "${"%.1f".format(kbps / 1000f)} Mbps",
            items = if (ui.viaRelay) com.niranjan.helm.core.RelayStreamPolicy.BITRATES else preset.bitrateChoices,
            itemLabel = { "${"%.1f".format(it / 1000f)} Mbps" },
            onPick = vm::setBitrate,
        )
        BpFilterChip(selected = settings.streamAudio, onClick = { vm.setAudio(!settings.streamAudio) }, label = { Text("Sound") })
        BpFilterChip(selected = settings.streamShowStats, onClick = { vm.setShowStats(!settings.streamShowStats) }, label = { Text("Stats") })
        TextButton(onClick = onResetView) { Text("Fit") }
        TextButton(onClick = vm::requestKeyframe) { Text("Refresh") }
        TextButton(onClick = onHide) { Text("Hide") }
    }
}

@Composable
private fun <T> Picker(label: String, items: List<T>, itemLabel: (T) -> String, onPick: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box {
        OutlineButton(onClick = { open = true }) { Text(label) }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }) {
            items.forEach { item ->
                DropdownMenuItem(text = { Text(itemLabel(item)) }, onClick = {
                    open = false
                    onPick(item)
                })
            }
        }
    }
}

/** Special keys (sticky modifiers) plus a text box that types on the PC. */
@Composable
fun StreamKeyboard(remote: RemoteViewModel) {
    val mods = remember { mutableStateListOf<Int>() }
    var text by remember { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }

    fun press(vk: Int) {
        remote.sendAll(KeyPress.frames(vk, mods.toList()))
        mods.clear()
    }

    Column(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp)) {
        HorizontalDivider()
        Row(Modifier.horizontalScroll(rememberScrollState()).padding(vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
            listOf("Ctrl" to Vk.CONTROL, "Alt" to Vk.ALT, "Shift" to Vk.SHIFT, "Win" to Vk.LWIN).forEach { (label, vk) ->
                BpFilterChip(selected = vk in mods, onClick = { if (vk in mods) mods.remove(vk) else mods.add(vk) }, label = { Text(label) })
            }
            val keys = listOf(
                "Esc" to Vk.ESCAPE, "Tab" to Vk.TAB, "←" to Vk.LEFT, "↑" to Vk.UP, "↓" to Vk.DOWN, "→" to Vk.RIGHT,
                "Enter" to Vk.RETURN, "Bksp" to Vk.BACK, "Del" to Vk.DELETE, "Home" to Vk.HOME, "End" to Vk.END,
                "PgUp" to Vk.PAGE_UP, "PgDn" to Vk.PAGE_DOWN,
            ) + (1..12).map { "F$it" to Vk.f(it) }
            keys.forEach { (label, vk) ->
                OutlineButton(onClick = { press(vk) }, modifier = Modifier.width(if (label.length > 3) 72.dp else 56.dp)) { Text(label) }
            }
        }
        OutlinedTextField(
            value = text,
            onValueChange = { new ->
                if (mods.isNotEmpty() && new.length == text.length + 1 && new.startsWith(text)) {
                    val vk = Vk.forChar(new.last())
                    if (vk != null) {
                        press(vk)
                        return@OutlinedTextField
                    }
                }
                remote.sendAll(TextDiff.frames(text, new))
                text = if (new.length > 80) "" else new
            },
            placeholder = { Text("Type on the PC") },
            singleLine = true,
            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Text, imeAction = ImeAction.Send, autoCorrectEnabled = false),
            keyboardActions = KeyboardActions(onSend = {
                press(Vk.RETURN)
                text = ""
            }),
            modifier = Modifier.fillMaxWidth().focusRequester(focus),
        )
    }
}
