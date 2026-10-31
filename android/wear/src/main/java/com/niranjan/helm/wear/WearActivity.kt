package com.niranjan.helm.wear

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.wear.compose.foundation.lazy.ScalingLazyColumn
import androidx.wear.compose.foundation.lazy.rememberScalingLazyListState
import androidx.wear.compose.material.Button
import androidx.wear.compose.material.ButtonDefaults
import androidx.wear.compose.material.Chip
import androidx.wear.compose.material.ChipDefaults
import androidx.wear.compose.material.MaterialTheme
import androidx.wear.compose.material.PositionIndicator
import androidx.wear.compose.material.Scaffold
import androidx.wear.compose.material.Text
import androidx.wear.compose.material.TimeText
import com.niranjan.helm.wearlink.WearAction
import com.niranjan.helm.wearlink.WearLink
import kotlinx.coroutines.launch

/** Watch remote: connection state, media keys, volume, lock and the first four deck buttons. */
class WearActivity : ComponentActivity() {
    private val link by lazy { PhoneLink.get(this) }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { MaterialTheme { RemoteScreen(link) } }
    }

    override fun onStart() {
        super.onStart()
        link.start()
    }
}

@Composable
private fun RemoteScreen(link: PhoneLink) {
    val state by link.state.collectAsStateWithLifecycle()
    val scope = rememberCoroutineScope()
    var note by remember { mutableStateOf<String?>(null) }
    val listState = rememberScalingLazyListState()

    LaunchedEffect(Unit) { link.refresh() }

    fun run(action: WearAction, slot: Int? = null) {
        scope.launch {
            note = "…"
            val r = link.send(action, slot)
            note = if (r.ok && r.message == "OK") null else r.message
        }
    }

    val status = (state as? LinkState.Phone)?.status
    val connected = status?.connected == true

    Scaffold(
        timeText = { TimeText() },
        positionIndicator = { PositionIndicator(scalingLazyListState = listState) },
    ) {
        ScalingLazyColumn(state = listState, modifier = Modifier.fillMaxWidth()) {
            item {
                Chip(
                    onClick = { scope.launch { link.refresh() } },
                    label = { Text(statusTitle(state), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    secondaryLabel = statusDetail(state, note)?.let { d -> { Text(d, maxLines = 2, overflow = TextOverflow.Ellipsis) } },
                    colors = if (connected) ChipDefaults.primaryChipColors() else ChipDefaults.secondaryChipColors(),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            item {
                KeyRow(
                    "⏮" to { run(WearAction.PREVIOUS) },
                    "⏯" to { run(WearAction.PLAY_PAUSE) },
                    "⏭" to { run(WearAction.NEXT) },
                    enabled = connected,
                )
            }
            item {
                KeyRow(
                    "−" to { run(WearAction.VOLUME_DOWN) },
                    (if (status?.muted == true) "🔇" else "🔈") to { run(WearAction.MUTE) },
                    "+" to { run(WearAction.VOLUME_UP) },
                    enabled = connected,
                )
            }
            item {
                Chip(
                    onClick = { run(WearAction.LOCK) },
                    label = { Text("Lock PC") },
                    enabled = connected,
                    colors = ChipDefaults.secondaryChipColors(),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            items(WearLink.DECK_SLOTS) { i ->
                val label = status?.deck?.getOrNull(i)?.ifBlank { null }
                Chip(
                    onClick = { run(WearAction.DECK, i) },
                    label = { Text(label ?: "Deck ${i + 1}", maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    enabled = connected && label != null,
                    colors = ChipDefaults.secondaryChipColors(),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
        }
    }
}

@Composable
private fun KeyRow(vararg keys: Pair<String, () -> Unit>, enabled: Boolean) {
    Row(
        horizontalArrangement = Arrangement.spacedBy(6.dp, Alignment.CenterHorizontally),
        modifier = Modifier.fillMaxWidth().padding(vertical = 2.dp),
    ) {
        for ((glyph, onClick) in keys) {
            Button(onClick = onClick, enabled = enabled, modifier = Modifier.padding(0.dp), colors = ButtonDefaults.secondaryButtonColors()) {
                Text(glyph, textAlign = TextAlign.Center)
            }
        }
    }
}

private fun statusTitle(state: LinkState): String = when (state) {
    LinkState.Searching -> "Finding phone…"
    LinkState.NoPhone -> "Phone not found"
    is LinkState.Phone -> state.status.pcName ?: "No PC"
}

private fun statusDetail(state: LinkState, note: String?): String? = note ?: when (state) {
    LinkState.Searching -> null
    LinkState.NoPhone -> "Open Helm on your phone"
    is LinkState.Phone -> if (state.status.connected) state.status.nowPlaying ?: "Connected" else "Phone not connected to a PC"
}
