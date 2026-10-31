package com.niranjan.helm.ui

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
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.DeckAction
import com.niranjan.helm.core.Trigger
import com.niranjan.helm.core.TriggerWhen
import java.util.UUID

/** Action kinds a trigger can run (launch needs the app picker, so it is set from the deck). */
private val TRIGGER_KINDS = listOf("hotkey", "text", "open", "media", "volume_step", "remote", "ha")

private const val W_TIME = "time"
private const val W_APP = "app_start"
private const val W_PHONE = "phone_connected"
private const val W_WIFI = "wifi"

/** Phase 5 triggers editor (§13): when X happens on the PC, run a deck action. */
@Composable
fun TriggersScreen(vm: ExtrasViewModel) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<Trigger?>(null) }
    LaunchedEffect(Unit) { vm.loadTriggers() }
    val list = ui.triggers

    Column(Modifier.fillMaxSize().padding(12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            PencilButton(enabled = list != null, onClick = {
                editing = Trigger(UUID.randomUUID().toString().take(8), TriggerWhen.Time("08:00 weekdays"), DeckAction.Media("play"))
            }) { Text("Add trigger") }
            TextButton(onClick = vm::loadTriggers) { Text("Refresh") }
        }
        when {
            list == null -> HintText("Loading…")
            list.isEmpty() -> HintText("No triggers. Add one to run an action at a time, when an app starts, when this phone connects or on a Wi-Fi network.")
            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                items(list, key = { it.id }) { t ->
                    BpCard(Modifier.fillMaxWidth().clickable { editing = t }) {
                        Column(Modifier.padding(12.dp)) {
                            Text(describeWhen(t.`when`, vm.myDeviceId), style = MaterialTheme.typography.titleSmall)
                            Text("→ " + kindLabel(kindOf(t.action)) + valueOf(t.action).takeIf { it.isNotBlank() }?.let { ": $it" }.orEmpty(),
                                style = MaterialTheme.typography.bodySmall)
                        }
                    }
                }
            }
        }
    }

    editing?.let { t ->
        val isNew = list.orEmpty().none { it.id == t.id }
        TriggerEditor(
            trigger = t,
            isNew = isNew,
            myDeviceId = vm.myDeviceId,
            onSave = { saved ->
                val cur = list.orEmpty()
                vm.saveTriggers(if (isNew) cur + saved else cur.map { if (it.id == saved.id) saved else it })
                editing = null
            },
            onDelete = { vm.saveTriggers(list.orEmpty().filterNot { it.id == t.id }); editing = null },
            onDismiss = { editing = null },
        )
    }
}

private fun describeWhen(w: TriggerWhen, me: String): String =
    if (w is TriggerWhen.PhoneConnected && w.deviceId != null && w.deviceId != me) "When another phone connects" else TriggerWhen.describe(w)

private fun whenKind(w: TriggerWhen) = when (w) {
    is TriggerWhen.Time -> W_TIME
    is TriggerWhen.AppStart -> W_APP
    is TriggerWhen.PhoneConnected -> W_PHONE
    is TriggerWhen.Wifi -> W_WIFI
    is TriggerWhen.Unknown -> "other"
}

@Composable
private fun TriggerEditor(
    trigger: Trigger,
    isNew: Boolean,
    myDeviceId: String,
    onSave: (Trigger) -> Unit,
    onDelete: () -> Unit,
    onDismiss: () -> Unit,
) {
    val w0 = trigger.`when`
    var wKind by remember { mutableStateOf(whenKind(w0)) }
    var time by remember { mutableStateOf((w0 as? TriggerWhen.Time)?.spec ?: "08:00 weekdays") }
    var exe by remember { mutableStateOf((w0 as? TriggerWhen.AppStart)?.exe ?: "") }
    var ssid by remember { mutableStateOf((w0 as? TriggerWhen.Wifi)?.ssid ?: "") }
    var anyPhone by remember { mutableStateOf((w0 as? TriggerWhen.PhoneConnected)?.deviceId == null) }
    var kind by remember { mutableStateOf(kindOf(trigger.action)) }
    var value by remember { mutableStateOf(valueOf(trigger.action)) }

    val newWhen: TriggerWhen? = when (wKind) {
        W_TIME -> time.trim().takeIf { TriggerWhen.validTime(it) }?.let { TriggerWhen.Time(it) }
        W_APP -> exe.trim().takeIf { it.isNotEmpty() }?.let { TriggerWhen.AppStart(it) }
        W_PHONE -> TriggerWhen.PhoneConnected(if (anyPhone) null else ((w0 as? TriggerWhen.PhoneConnected)?.deviceId ?: myDeviceId))
        W_WIFI -> ssid.takeIf { it.isNotBlank() }?.let { TriggerWhen.Wifi(it) }
        else -> w0
    }
    // Kinds this editor does not edit (macro, launch, …) are kept unchanged.
    val action: DeckAction? = if (kind !in TRIGGER_KINDS && kind == kindOf(trigger.action)) trigger.action else buildAction(kind, value, trigger.action)
    val kinds = TRIGGER_KINDS + listOf(kindOf(trigger.action)).filterNot { it in TRIGGER_KINDS }

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (isNew) "New trigger" else "Edit trigger") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("When", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    listOf(W_TIME to "Time", W_APP to "App starts", W_PHONE to "Phone connects", W_WIFI to "Wi-Fi").forEach { (k, l) ->
                        BpFilterChip(selected = wKind == k, onClick = { wKind = k }, label = { Text(l) })
                    }
                }
                when (wKind) {
                    W_TIME -> {
                        OutlinedTextField(time, { time = it }, label = { Text("HH:MM and days") }, singleLine = true, modifier = Modifier.fillMaxWidth(),
                            isError = !TriggerWhen.validTime(time))
                        HintText("Days: daily, weekdays, weekends, mon-fri or sat,sun. Leave out for every day. Uses the PC's clock.")
                    }
                    W_APP -> OutlinedTextField(exe, { exe = it }, label = { Text("Program, e.g. obs64.exe") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    W_PHONE -> Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        BpFilterChip(selected = anyPhone, onClick = { anyPhone = true }, label = { Text("Any phone") })
                        BpFilterChip(selected = !anyPhone, onClick = { anyPhone = false }, label = { Text("This phone") })
                    }
                    W_WIFI -> OutlinedTextField(ssid, { ssid = it }, label = { Text("PC's Wi-Fi network name") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                    else -> HintText("Made elsewhere; kept as-is: ${TriggerWhen.describe(w0)}")
                }
                Text("Do", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    kinds.forEach { k ->
                        BpFilterChip(selected = kind == k, onClick = { if (kind != k) { kind = k; value = defaultValue(k) } }, label = { Text(kindLabel(k)) })
                    }
                }
                when (kind) {
                    in TRIGGER_KINDS -> SimpleValueEditor(kind, value) { value = it }
                    else -> HintText("Kept as-is: ${valueOf(trigger.action).ifBlank { kindLabel(kind) }}")
                }
                if (!isNew) TextButton(onClick = onDelete) { Text("Delete trigger", color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            TextButton(enabled = newWhen != null && action != null, onClick = {
                onSave(trigger.copy(`when` = newWhen!!, action = action!!))
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
