package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.niranjan.helm.core.SystemHealth
import kotlinx.coroutines.delay

/**
 * Compact PC health (battery, CPU temperature, free space on the system drive, uptime),
 * refreshed when shown and every 60 s while it stays on screen. Draws nothing without `g6`.
 */
@Composable
fun PcHealthCard(modifier: Modifier = Modifier, compact: Boolean = false, vm: EverydayViewModel = viewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    LaunchedEffect(ui.g6) {
        if (!ui.g6) return@LaunchedEffect
        while (true) {
            vm.refreshHealth()
            delay(60_000)
        }
    }
    if (!ui.g6) return
    val h = ui.health
    if (compact) {
        Text(
            h?.let { SystemHealth.lines(it).joinToString(" · ") } ?: "Checking PC health…",
            style = MaterialTheme.typography.labelMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = modifier,
        )
        return
    }
    SectionCard("PC health", modifier) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            if (h == null) HintText("Loading…") else SystemHealth.lines(h).forEach { Text(it) }
        }
    }
}
