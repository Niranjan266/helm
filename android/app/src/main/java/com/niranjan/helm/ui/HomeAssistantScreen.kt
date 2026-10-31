package com.niranjan.helm.ui

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
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.niranjan.helm.core.DeckAction
import com.niranjan.helm.core.DeckButton
import com.niranjan.helm.core.HaState

/** Phase 5 Home Assistant (§13): connection (admin), entity states, one-tap deck buttons. */
@Composable
fun HomeAssistantScreen(vm: ExtrasViewModel, remote: RemoteViewModel) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val remoteUi by remote.ui.collectAsStateWithLifecycle()
    val gate = LocalAdminGate.current
    var url by rememberSaveable { mutableStateOf("") }
    var token by remember { mutableStateOf("") }
    var filter by rememberSaveable { mutableStateOf("") }
    var showConfig by rememberSaveable { mutableStateOf(false) }
    LaunchedEffect(Unit) { if (ui.haStates == null) vm.loadHaStates() }
    val states = ui.haStates

    Column(Modifier.fillMaxSize().padding(12.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            OutlinedButton(onClick = { showConfig = !showConfig }) { Text(if (showConfig) "Hide connection" else "Connection") }
            OutlinedButton(enabled = !ui.haLoading, onClick = vm::loadHaStates) { Text(if (ui.haLoading) "Loading…" else "Refresh") }
        }
        if (showConfig || (states == null && !ui.haLoading)) {
            SectionCard("Home Assistant connection") {
                Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    HintText("The PC talks to Home Assistant; the token is stored encrypted on the PC, never on this phone.")
                    OutlinedTextField(url, { url = it }, label = { Text("URL, e.g. http://homeassistant.local:8123") }, singleLine = true,
                        keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri), modifier = Modifier.fillMaxWidth())
                    OutlinedTextField(token, { token = it }, label = { Text("Long-lived access token") }, singleLine = true,
                        visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth())
                    Button(enabled = url.startsWith("http") && token.isNotBlank(), onClick = {
                        gate("Save the Home Assistant connection on the PC") {
                            vm.saveHaConfig(url, token)
                            token = ""
                            showConfig = false
                        }
                    }) { Text("Save on PC") }
                }
            }
        }
        OutlinedTextField(filter, { filter = it }, label = { Text("Filter entities") }, singleLine = true, modifier = Modifier.fillMaxWidth())
        when {
            states == null -> HintText(if (ui.haLoading) "Loading…" else "Not connected to Home Assistant yet.")
            else -> {
                val shown = states.filter { filter.isBlank() || it.entityId.contains(filter, true) || it.label.contains(filter, true) }
                LazyColumn {
                    items(shown, key = { it.entityId }) { s ->
                        Row(Modifier.fillMaxWidth().padding(vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(s.label, style = MaterialTheme.typography.bodyLarge)
                                Text(s.entityId, style = MaterialTheme.typography.labelSmall)
                            }
                            Text(s.state, style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(horizontal = 8.dp))
                            TextButton(onClick = {
                                remote.saveDeckButton(
                                    DeckButton(
                                        remote.newId(), s.label.take(24), DeckColors[remoteUi.deck.size % DeckColors.size],
                                        action = DeckAction.Ha(s.toggleService, s.entityId),
                                    ),
                                )
                            }) { Text("Add to deck") }
                        }
                        HorizontalDivider()
                    }
                }
            }
        }
    }
}

/** Deck/trigger value editor for `{"ha":{service,entity_id}}`; value is "service entity_id". */
@Composable
internal fun HaActionEditor(value: String, onValue: (String) -> Unit) {
    val vm: ExtrasViewModel = viewModel()
    val ui by vm.ui.collectAsStateWithLifecycle()
    val service = value.substringBefore(' ')
    val entity = value.substringAfter(' ', "")
    LaunchedEffect(Unit) { if (ui.haStates == null && ui.phase5) vm.loadHaStates() }

    OutlinedTextField(entity, { onValue("$service ${it.trim()}") }, label = { Text("Entity, e.g. light.desk") }, singleLine = true,
        isError = entity.isNotEmpty() && !HaState.validEntityId(entity), modifier = Modifier.fillMaxWidth())
    ui.haStates?.takeIf { it.isNotEmpty() }?.let { states ->
        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            states.take(40).forEach { s ->
                AssistChip(onClick = { onValue("${s.toggleService} ${s.entityId}") }, label = { Text(s.label) })
            }
        }
    }
    Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        DeckAction.HA_SERVICES.forEach { svc -> FilterChip(selected = service == svc, onClick = { onValue("$svc $entity") }, label = { Text(svc) }) }
    }
    OutlinedTextField(service, { onValue("${it.trim()} $entity") }, label = { Text("Service, e.g. light.toggle") }, singleLine = true,
        isError = !HaState.validService(service), modifier = Modifier.fillMaxWidth())
}
