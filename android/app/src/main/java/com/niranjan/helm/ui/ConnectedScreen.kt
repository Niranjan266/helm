package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Refresh
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Card
import androidx.compose.material3.CardDefaults
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.niranjan.helm.Session
import com.niranjan.helm.core.ConnectionState
import java.text.DateFormat
import java.util.Date

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConnectedScreen(
    session: Session,
    state: ConnectionState,
    ui: SessionUi,
    snackbar: SnackbarHostState,
    onBack: () -> Unit,
    onPing: () -> Unit,
    onRefreshInfo: () -> Unit,
    onRefreshDevices: () -> Unit,
    onForget: () -> Unit,
    onRetry: () -> Unit,
) {
    var confirmForget by remember { mutableStateOf(false) }
    val ready = state as? ConnectionState.Ready

    val c = Bp.c
    Scaffold(
        containerColor = androidx.compose.ui.graphics.Color.Transparent,
        contentColor = c.ink,
        modifier = Modifier.gridPaper(c),
        topBar = {
            Row(
                Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 16.dp, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(10.dp),
            ) {
                SquareIconButton(onClick = onBack, contentDescription = "Back to your PCs") {
                    Icon(Icons.AutoMirrored.Filled.ArrowBack, contentDescription = null)
                }
                Column(Modifier.weight(1f)) {
                    DisplayText(session.name, maxLines = 1)
                    when (state) {
                        is ConnectionState.Ready -> StatusLine("Connected")
                        is ConnectionState.Failed -> StatusLine("Not connected", color = c.redInk)
                        ConnectionState.Closed -> StatusLine("Disconnected", color = c.ink2)
                        else -> StatusLine(if (session.isPairing) "Pairing…" else "Connecting…", color = c.pencilInk)
                    }
                }
            }
        },
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            SheetStrip(if (session.isPairing) "Sheet 00 · Pairing" else "Sheet 00 · Connection")
            val approval by session.connection.approvalPending.collectAsStateWithLifecycle()
            StatusCard(state, onRetry, session.isPairing, onBack, approvalPending = approval)

            if (ready != null) {
                PcDetails(session, ui, onPing, onRefreshInfo, onRefreshDevices, onRequestForget = { confirmForget = true }, onDisconnect = onBack)
            }
        }
    }

    if (confirmForget) {
        AlertDialog(
            onDismissRequest = { confirmForget = false },
            title = { Text("Forget ${session.name}?") },
            text = { Text("The PC will no longer trust this phone, and the phone will remove the PC. You will need to scan a new QR code to pair again.") },
            confirmButton = {
                TextButton(onClick = { confirmForget = false; onForget() }) { Text("Forget") }
            },
            dismissButton = { TextButton(onClick = { confirmForget = false }) { Text("Cancel") } },
        )
    }
}

/** Ping, system info, paired devices, forget and disconnect for the connected PC. */
@Composable
fun PcDetails(
    session: Session,
    ui: SessionUi,
    onPing: () -> Unit,
    onRefreshInfo: () -> Unit,
    onRefreshDevices: () -> Unit,
    onRequestForget: () -> Unit,
    onDisconnect: () -> Unit,
) {
    Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
            InfoCard("Ping") {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    PencilButton(onClick = onPing, enabled = !ui.pingBusy) { Text("Ping") }
                    Spacer(Modifier.width(16.dp))
                    Text(
                        when {
                            ui.pingBusy -> "…"
                            ui.pingMs != null -> "${ui.pingMs} ms round trip"
                            else -> "Tap to measure"
                        },
                        style = MaterialTheme.typography.bodyLarge,
                    )
                }
            }

            InfoCard("System info", onRefresh = onRefreshInfo) {
                val rows = ui.info
                if (rows == null) {
                    Text("Loading…")
                } else {
                    rows.forEach { (k, v) -> KeyValue(k, v) }
                }
            }

            InfoCard("Devices", onRefresh = onRefreshDevices) {
                val devices = ui.devices
                if (devices == null) {
                    Text("Loading…")
                } else if (devices.isEmpty()) {
                    Text("No devices.")
                } else {
                    devices.forEachIndexed { i, d ->
                        if (i > 0) HorizontalDivider(Modifier.padding(vertical = 8.dp))
                        Text(
                            d.name + if (d.isThis) " (this phone)" else "",
                            style = MaterialTheme.typography.titleSmall,
                            fontWeight = FontWeight.SemiBold,
                        )
                        Text(
                            "Tier: ${d.tier}" + (d.lastSeenMs?.let { " · last seen ${formatTime(it)}" } ?: ""),
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                        d.pairedAtMs?.let {
                            Text(
                                "Paired ${formatTime(it)}",
                                style = MaterialTheme.typography.bodySmall,
                                color = MaterialTheme.colorScheme.onSurfaceVariant,
                            )
                        }
                    }
                }
            }

            Spacer(Modifier.height(4.dp))
            OutlineButton(
                onClick = onRequestForget,
                enabled = !ui.forgetting,
                modifier = Modifier.fillMaxWidth(),
            ) { Text("Forget this PC") }
            PencilButton(onClick = onDisconnect, modifier = Modifier.fillMaxWidth()) { Text("Disconnect") }
    }
}

/** Confirmation for "Forget this PC". */
@Composable
fun ForgetDialog(name: String, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text("Forget $name?") },
        text = { Text("The PC will no longer trust this phone, and the phone will remove the PC. You will need to scan a new QR code to pair again.") },
        confirmButton = { TextButton(onClick = onConfirm) { Text("Forget") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
internal fun StatusCard(
    state: ConnectionState,
    onRetry: () -> Unit,
    isPairing: Boolean,
    onBack: () -> Unit,
    approvalPending: Boolean = false,
) {
    InfoCard("Connection") {
        when (state) {
            ConnectionState.Idle -> Busy("Starting…")
            is ConnectionState.Connecting -> Busy("Connecting to ${state.endpoint}…")
            ConnectionState.Pairing -> Busy(if (approvalPending) "Waiting for approval — choose Allow on the PC…" else "Pairing…")
            ConnectionState.Authenticating -> Busy("Signing in…")
            is ConnectionState.Ready -> {
                KeyValue("State", "Connected")
                KeyValue("Address", state.endpoint.toString())
                KeyValue("Tier", state.tier)
            }
            is ConnectionState.Failed -> {
                Text(failureTitle(state.code), style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.error)
                Spacer(Modifier.height(4.dp))
                Text(state.message, style = MaterialTheme.typography.bodyMedium)
                Spacer(Modifier.height(12.dp))
                Row {
                    if (!isPairing && state.code != "unknown_device") {
                        PencilButton(onClick = onRetry) { Text("Retry") }
                        Spacer(Modifier.width(8.dp))
                    }
                    OutlineButton(onClick = onBack) { Text("Back") }
                }
            }
            ConnectionState.Closed -> {
                Text("Disconnected.")
                Spacer(Modifier.height(12.dp))
                Row {
                    if (!isPairing) {
                        PencilButton(onClick = onRetry) { Text("Reconnect") }
                        Spacer(Modifier.width(8.dp))
                    }
                    OutlineButton(onClick = onBack) { Text("Back") }
                }
            }
        }
    }
}

private fun failureTitle(code: String): String = when (code) {
    "bad_token" -> "Pairing code expired or already used. Show a new QR code on the PC."
    "pairing_locked" -> "Too many wrong codes. Pairing is blocked for a while."
    "bad_code" -> "The pairing code did not match. Show a new code on the PC."
    "denied" -> "The PC did not allow this phone."
    "unknown_device" -> "This PC no longer knows this phone. Remove it and pair again."
    "bad_signature" -> "The PC rejected this phone's key."
    "timeout" -> "The PC timed out waiting for this phone."
    "pin_mismatch" -> "Security check failed: certificate changed."
    "host_mismatch" -> "That address belongs to a different PC."
    "unreachable" -> "Can't reach the PC. Is it on and on the same Wi-Fi?"
    else -> "Connection failed ($code)."
}

@Composable
private fun Busy(text: String) {
    Row(verticalAlignment = Alignment.CenterVertically) {
        CircularProgressIndicator(Modifier.size(20.dp), strokeWidth = 2.dp)
        Spacer(Modifier.width(12.dp))
        Text(text)
    }
}

@Composable
internal fun InfoCard(title: String, onRefresh: (() -> Unit)? = null, content: @Composable () -> Unit) {
    FigBox(
        title = title,
        trailing = if (onRefresh != null) "Refresh" else null,
        trailingColor = Bp.c.pencilInk,
        onTrailingClick = onRefresh,
        modifier = Modifier.fillMaxWidth(),
    ) { content() }
}

@Composable
internal fun KeyValue(key: String, value: String) {
    Row(Modifier.fillMaxWidth().padding(vertical = 2.dp)) {
        Text(key.uppercase(), style = BpType.mono, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.width(96.dp).padding(top = 2.dp))
        Text(value)
    }
}

private fun formatTime(ms: Long): String =
    DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(ms))
