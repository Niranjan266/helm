package com.niranjan.helm.ui

import android.app.DatePickerDialog
import android.app.TimePickerDialog
import android.text.format.DateFormat as AndroidDateFormat
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
import androidx.compose.material3.Button
import androidx.compose.material3.Card
import androidx.compose.material3.FilterChip
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
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
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.niranjan.helm.core.DeckAction
import com.niranjan.helm.core.Schedule
import com.niranjan.helm.core.ScheduleAction
import java.text.DateFormat
import java.util.Calendar
import java.util.Date

/** Deck action kinds the schedule editor edits (launch needs an app id, typed as text). */
private val SCHEDULE_DECK_KINDS = listOf("hotkey", "text", "launch", "open", "media", "volume_step", "remote", "ha")
private const val K_POWER = "power"

private fun actionKind(a: ScheduleAction) = when (a) {
    is ScheduleAction.Power -> K_POWER
    is ScheduleAction.Deck -> kindOf(a.action)
}

private fun describeAction(a: ScheduleAction): String = when (a) {
    is ScheduleAction.Power -> ScheduleAction.powerLabel(a.kind) + " the PC"
    is ScheduleAction.Deck -> kindLabel(kindOf(a.action)) + valueOf(a.action).takeIf { it.isNotBlank() }?.let { ": $it" }.orEmpty()
}

/** Everyday §5: run deck or power actions at a time, once or on weekdays (PC's local time). */
@Composable
fun SchedulesScreen(vm: EverydayViewModel = viewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<Schedule?>(null) }
    val gate = LocalAdminGate.current
    LaunchedEffect(ui.g6) { if (ui.g6) vm.loadSchedules() }
    val list = ui.schedules

    // Power schedules need the admin tier: confirm like other admin actions.
    val save: (List<Schedule>) -> Unit = { next ->
        if (Schedule.needsAdmin(next) && next != list) gate("Save power schedules") { vm.saveSchedules(next) } else vm.saveSchedules(next)
    }

    Column(Modifier.fillMaxSize().padding(12.dp)) {
        if (!ui.g6) {
            HintText("This PC's Helm agent does not offer schedules yet. Update Helm on the PC.")
            return@Column
        }
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            Button(enabled = list != null && list.size < Schedule.MAX, onClick = {
                editing = Schedule(
                    id = Schedule.newId(list.orEmpty().map { it.id }),
                    time = "08:00",
                    days = listOf(1, 2, 3, 4, 5),
                    action = ScheduleAction.Deck(DeckAction.Media("toggle_play_pause")),
                )
            }) { Text("Add schedule") }
            TextButton(onClick = vm::loadSchedules) { Text("Refresh") }
        }
        when {
            list == null -> HintText("Loading…")
            list.isEmpty() -> HintText("No schedules. Add one to lock, sleep or shut down the PC, or run any deck action, at a set time.")
            else -> LazyColumn(verticalArrangement = Arrangement.spacedBy(8.dp), modifier = Modifier.padding(top = 8.dp)) {
                items(list, key = { it.id }) { s ->
                    Card(Modifier.fillMaxWidth().clickable { editing = s }) {
                        Row(Modifier.padding(12.dp), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(
                                    s.time + " · " + Schedule.describeDays(s.days, s.date) + (s.name?.let { " · $it" } ?: ""),
                                    style = MaterialTheme.typography.titleSmall,
                                )
                                Text("→ " + describeAction(s.action), style = MaterialTheme.typography.bodySmall)
                                val next = s.nextMs?.let { "Next: " + fmt(it) } ?: if (s.enabled) "Not scheduled" else "Off"
                                val last = s.lastRunMs?.let { " · Last run: " + fmt(it) }.orEmpty()
                                Text(next + last, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                            }
                            Switch(checked = s.enabled, onCheckedChange = { on -> save(list.map { if (it.id == s.id) it.copy(enabled = on) else it }) })
                        }
                    }
                }
            }
        }
    }

    editing?.let { s ->
        val isNew = list.orEmpty().none { it.id == s.id }
        ScheduleEditor(
            schedule = s,
            isNew = isNew,
            onSave = { saved ->
                val cur = list.orEmpty()
                save(if (isNew) cur + saved else cur.map { if (it.id == saved.id) saved else it })
                editing = null
            },
            onDelete = { save(list.orEmpty().filterNot { it.id == s.id }); editing = null },
            onDismiss = { editing = null },
        )
    }
}

private fun fmt(ms: Long): String = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT).format(Date(ms))

@Composable
private fun ScheduleEditor(
    schedule: Schedule,
    isNew: Boolean,
    onSave: (Schedule) -> Unit,
    onDelete: () -> Unit,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    var name by remember { mutableStateOf(schedule.name ?: "") }
    var time by remember { mutableStateOf(schedule.time) }
    var days by remember { mutableStateOf(schedule.days.toSet()) }
    var date by remember { mutableStateOf(schedule.date) }
    val origKind = actionKind(schedule.action)
    var kind by remember { mutableStateOf(origKind) }
    var power by remember { mutableStateOf((schedule.action as? ScheduleAction.Power)?.kind ?: "lock") }
    var value by remember { mutableStateOf((schedule.action as? ScheduleAction.Deck)?.action?.let { valueOf(it) } ?: "") }

    val origDeck = (schedule.action as? ScheduleAction.Deck)?.action
    val action: ScheduleAction? = when {
        kind == K_POWER -> ScheduleAction.Power(power)
        kind == origKind && kind !in SCHEDULE_DECK_KINDS -> schedule.action // macro, repeat, unknown: kept as-is
        else -> buildAction(kind, value, origDeck)?.let { ScheduleAction.Deck(it) }
    }
    val kinds = listOf(K_POWER) + SCHEDULE_DECK_KINDS + listOf(origKind).filterNot { it == K_POWER || it in SCHEDULE_DECK_KINDS }
    val valid = Schedule.validTime(time) && action != null && (date == null || Schedule.validDate(date!!))

    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(if (isNew) "New schedule" else "Edit schedule") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState()), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                OutlinedTextField(name, { name = it.take(Schedule.MAX_NAME) }, label = { Text("Name (optional)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                Text("Time (PC's clock)", style = MaterialTheme.typography.labelLarge)
                OutlinedButton(onClick = {
                    val h = time.substringBefore(':').toIntOrNull() ?: 8
                    val m = time.substringAfter(':').toIntOrNull() ?: 0
                    TimePickerDialog(context, { _, hh, mm -> time = Schedule.time(hh, mm) }, h, m, AndroidDateFormat.is24HourFormat(context)).show()
                }) { Text(time) }
                Text("Repeat on", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    Schedule.DAY_LABELS.forEachIndexed { i, l ->
                        val d = i + 1
                        FilterChip(selected = d in days, onClick = { days = if (d in days) days - d else days + d }, label = { Text(l) })
                    }
                }
                if (days.isEmpty()) {
                    HintText("No days: runs once" + (date?.let { " on $it" } ?: " the next time it is $time") + ".")
                    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                        OutlinedButton(onClick = {
                            val c = Calendar.getInstance()
                            date?.split('-')?.mapNotNull { it.toIntOrNull() }?.takeIf { it.size == 3 }?.let { c.set(it[0], it[1] - 1, it[2]) }
                            DatePickerDialog(context, { _, y, mo, d -> date = "%04d-%02d-%02d".format(y, mo + 1, d) },
                                c.get(Calendar.YEAR), c.get(Calendar.MONTH), c.get(Calendar.DAY_OF_MONTH)).show()
                        }) { Text(date ?: "Pick a date") }
                        if (date != null) TextButton(onClick = { date = null }) { Text("Next time") }
                    }
                }
                Text("Do", style = MaterialTheme.typography.labelLarge)
                Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    kinds.forEach { k ->
                        FilterChip(
                            selected = kind == k,
                            onClick = { if (kind != k) { kind = k; value = defaultValue(k) } },
                            label = { Text(if (k == K_POWER) "Power" else kindLabel(k)) },
                        )
                    }
                }
                when {
                    kind == K_POWER -> {
                        Row(Modifier.horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                            ScheduleAction.POWER_KINDS.forEach { p ->
                                FilterChip(selected = power == p, onClick = { power = p }, label = { Text(ScheduleAction.powerLabel(p)) })
                            }
                        }
                        HintText("Power actions need the admin access level on the PC.")
                    }
                    kind in SCHEDULE_DECK_KINDS -> SimpleValueEditor(kind, value) { value = it }
                    else -> HintText("Kept as-is: ${describeAction(schedule.action)}")
                }
                if (!isNew) TextButton(onClick = onDelete) { Text("Delete schedule", color = MaterialTheme.colorScheme.error) }
            }
        },
        confirmButton = {
            TextButton(enabled = valid, onClick = {
                onSave(
                    schedule.copy(
                        name = name.trim().ifBlank { null },
                        time = time,
                        days = Schedule.normDays(days),
                        date = if (days.isEmpty()) date else null,
                        action = action!!,
                    ),
                )
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
