package com.niranjan.helm.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.AlertRule
import com.niranjan.helm.core.Stats
import java.util.UUID

internal fun formatBytes(b: Long): String {
    val units = listOf("B", "KB", "MB", "GB", "TB")
    var v = b.toDouble()
    var i = 0
    while (v >= 1024 && i < units.lastIndex) {
        v /= 1024
        i++
    }
    return if (i == 0) "$b B" else "%.1f %s".format(v, units[i])
}

internal fun formatRate(bps: Long) = formatBytes(bps) + "/s"

@Composable
fun Phase3Required(what: String) {
    HintText("$what needs a newer Helm agent on the PC (Phase 3). Update Helm on the PC.", Modifier.padding(16.dp))
}

/** A simple line chart; values are scaled to [max] (or the series max when null). */
@Composable
fun LineChart(values: List<Float>, max: Float?, color: Color, modifier: Modifier = Modifier) {
    val grid = MaterialTheme.colorScheme.outlineVariant
    Canvas(modifier.fillMaxWidth().height(72.dp)) {
        drawLine(grid, Offset(0f, size.height), Offset(size.width, size.height))
        if (values.size < 2) return@Canvas
        val top = (max ?: values.max()).coerceAtLeast(1e-3f)
        val step = size.width / (values.size - 1)
        val path = Path()
        values.forEachIndexed { i, v ->
            val p = Offset(i * step, size.height * (1f - (v / top).coerceIn(0f, 1f)))
            if (i == 0) path.moveTo(p.x, p.y) else path.lineTo(p.x, p.y)
        }
        drawPath(path, color, style = Stroke(width = 2.dp.toPx()))
    }
}

@Composable
private fun Bars(values: List<Float>, color: Color) {
    Row(Modifier.fillMaxWidth().height(40.dp), horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.Bottom) {
        values.forEach { v ->
            Box(Modifier.weight(1f).fillMaxHeight((v / 100f).coerceIn(0.02f, 1f)).background(color))
        }
    }
}

@Composable
fun MonitorScreen(power: PowerViewModel) {
    val ui by power.ui.collectAsStateWithLifecycle()
    val gate = LocalAdminGate.current
    DisposableEffect(ui.phase3) {
        if (ui.phase3) power.monitorVisible(true)
        onDispose { if (ui.phase3) power.monitorVisible(false) }
    }
    if (!ui.phase3) return Phase3Required("Live monitoring")
    var editing by remember { mutableStateOf<AlertRule?>(null) }
    val s: Stats? = ui.stats.lastOrNull()
    val primary = MaterialTheme.colorScheme.primary
    val tertiary = MaterialTheme.colorScheme.tertiary

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (s == null) HintText("Waiting for the first sample…")
        s?.let { st ->
            SectionCard("CPU  %.0f%%".format(st.cpuTotal)) {
                LineChart(ui.stats.map { it.cpuTotal }, 100f, primary)
                if (st.cpuPerCore.isNotEmpty()) Bars(st.cpuPerCore, primary.copy(alpha = 0.7f))
            }
            SectionCard("Memory  %.0f%%  (%s of %s)".format(st.ramPct, formatBytes(st.ramUsed), formatBytes(st.ramTotal))) {
                LineChart(ui.stats.map { it.ramPct }, 100f, tertiary)
            }
            st.gpu.forEach { g ->
                SectionCard("${g.name}  %.0f%%".format(g.loadPct)) {
                    LineChart(ui.stats.map { x -> x.gpu.firstOrNull { it.name == g.name }?.loadPct ?: 0f }, 100f, primary)
                    if (g.vramTotal > 0) HintText("VRAM ${formatBytes(g.vramUsed)} of ${formatBytes(g.vramTotal)}")
                }
            }
            SectionCard("Network  ↓ ${formatRate(st.netRxBps)}  ↑ ${formatRate(st.netTxBps)}") {
                LineChart(ui.stats.map { it.netRxBps.toFloat() }, null, primary)
                LineChart(ui.stats.map { it.netTxBps.toFloat() }, null, tertiary)
            }
            SectionCard("Disks") {
                st.disks.forEach { d ->
                    Text("${d.mount} ${d.label}".trim() + " — ${formatBytes(d.total - d.used)} free of ${formatBytes(d.total)}")
                    LinearProgressIndicator(progress = { if (d.total > 0) d.used.toFloat() / d.total else 0f }, modifier = Modifier.fillMaxWidth().padding(vertical = 4.dp))
                }
            }
        }
        SectionCard("Alerts") {
            HintText("The PC checks these and this phone shows a notification when one fires.")
            val rules = ui.alerts
            if (rules == null) HintText("Loading…")
            rules?.forEach { r ->
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text((if (r.id in ui.firing) "● " else "") + r.describe(), Modifier.weight(1f), color = if (r.id in ui.firing) MaterialTheme.colorScheme.error else Color.Unspecified)
                    TextButton(onClick = { editing = r }) { Text("Edit") }
                    TextButton(onClick = { gate("Delete alert") { power.saveAlerts(rules.filterNot { it.id == r.id }) } }) { Text("Delete") }
                }
            }
            OutlineButton(enabled = rules != null, onClick = { editing = AlertRule(UUID.randomUUID().toString().take(8), AlertRule.CPU_ABOVE, 90.0, 30) }) {
                Text("Add alert")
            }
        }
    }

    editing?.let { r ->
        AlertEditor(r, onDismiss = { editing = null }) { updated ->
            editing = null
            val list = ui.alerts.orEmpty()
            gate("Save alert") { power.saveAlerts(if (list.any { it.id == updated.id }) list.map { if (it.id == updated.id) updated else it } else list + updated) }
        }
    }
}

@Composable
private fun AlertEditor(rule: AlertRule, onDismiss: () -> Unit, onSave: (AlertRule) -> Unit) {
    var kind by remember { mutableStateOf(rule.kind) }
    var threshold by remember { mutableStateOf(rule.threshold.toString()) }
    var forS by remember { mutableStateOf((rule.forS ?: 0).toString()) }
    var mount by remember { mutableStateOf(rule.mount ?: "") }
    val t = threshold.trim().toDoubleOrNull()?.takeIf { it in 0.0..100.0 }
    val f = forS.trim().toIntOrNull()?.takeIf { it in 0..3600 }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Alert") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    listOf(AlertRule.CPU_ABOVE to "CPU", AlertRule.RAM_ABOVE to "RAM", AlertRule.DISK_FREE_BELOW to "Disk free").forEach { (k, l) ->
                        BpFilterChip(selected = kind == k, onClick = { kind = k }, label = { Text(l) })
                    }
                }
                OutlinedTextField(threshold, { threshold = it }, label = { Text(if (kind == AlertRule.DISK_FREE_BELOW) "Free space below (%)" else "Above (%)") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Decimal))
                if (kind == AlertRule.CPU_ABOVE) {
                    OutlinedTextField(forS, { forS = it }, label = { Text("For at least (seconds)") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Number))
                }
                if (kind == AlertRule.DISK_FREE_BELOW) {
                    OutlinedTextField(mount, { mount = it }, label = { Text("Drive, e.g. C:\\ (empty = any)") }, singleLine = true)
                }
            }
        },
        confirmButton = {
            TextButton(enabled = t != null && f != null, onClick = {
                onSave(
                    AlertRule(
                        rule.id, kind, t!!,
                        forS = if (kind == AlertRule.CPU_ABOVE) f else null,
                        mount = if (kind == AlertRule.DISK_FREE_BELOW) mount.trim().ifEmpty { null } else null,
                    ),
                )
            }) { Text("Save") }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}
