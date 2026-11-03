package com.niranjan.helm.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.niranjan.helm.core.DiscoveredHost
import com.niranjan.helm.core.PcLink
import com.niranjan.helm.core.PcReach
import com.niranjan.helm.core.TunnelProtocol
import com.niranjan.helm.core.SavedPc
import com.niranjan.helm.device.Settings
import com.niranjan.helm.device.ThemeMode

/** Sheet 01: your PCs (the most recent one as a big FIG. panel), nearby PCs, pairing. */
@Composable
fun HomeScreen(
    saved: List<SavedPc>,
    discovered: Map<String, DiscoveredHost>,
    connectingTo: String?,
    snackbar: SnackbarHostState,
    settings: Settings,
    onSettings: ((Settings) -> Settings) -> Unit,
    onConnect: (SavedPc, Screen) -> Unit,
    onForgetLocal: (SavedPc) -> Unit,
    onWake: (SavedPc) -> Unit,
    onScan: () -> Unit,
    onPaste: (String) -> String?,
    /** g4: pair with a nearby PC by the 6-digit code it shows. Returns an error or null. */
    onPairCode: (DiscoveredHost, String) -> String? = { _, _ -> "Code pairing is not available." },
    /** "Connect from anywhere" for one PC, with a typed address when asked. Returns an error or null. */
    onAnywhere: (SavedPc, Boolean, String?) -> String? = { _, _, _ -> null },
) {
    val c = Bp.c
    var nearbyInfo by remember { mutableStateOf<DiscoveredHost?>(null) }
    var showPaste by remember { mutableStateOf(false) }
    var showSettings by remember { mutableStateOf(false) }
    var removeTarget by remember { mutableStateOf<SavedPc?>(null) }
    var relayTarget by remember { mutableStateOf<SavedPc?>(null) }
    val anywhereSwitch: (SavedPc, Boolean) -> Unit = { pc, on ->
        // Turning on with no saved address asks for one (pre-filled); nothing is saved until then.
        if (on && pc.relayUrl.isNullOrBlank()) relayTarget = pc else onAnywhere(pc, on, null)
    }

    val savedIds = saved.map { it.hostId.lowercase() }.toSet()
    val nearby = discovered.values.filter { it.hostId !in savedIds }.sortedBy { it.name.lowercase() }

    Scaffold(
        containerColor = Color.Transparent,
        contentColor = c.ink,
        modifier = Modifier.gridPaper(c),
        snackbarHost = { SnackbarHost(snackbar) },
    ) { padding ->
        Column(
            Modifier
                .fillMaxSize()
                .padding(padding)
                .padding(horizontal = 18.dp, vertical = 20.dp),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                HelmMark(size = 28.dp)
                Text("Helm", style = BpType.title.copy(fontWeight = FontWeight.Black, fontSize = 20.sp), color = c.ink, modifier = Modifier.weight(1f))
                OutlineButton(onClick = { showSettings = true }, textStyle = BpType.monoStrong) { Text("SETTINGS") }
            }
            SheetStrip("Sheet 01 · Your PCs", right = "Auto-reconnect on")

            Column(
                Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()),
                verticalArrangement = Arrangement.spacedBy(16.dp),
            ) {
                Text(
                    buildAnnotatedString {
                        append("YOUR ")
                        withStyle(SpanStyle(color = c.pencilInk)) { append("PCS") }
                    },
                    style = BpType.display,
                    color = c.ink,
                )
                if (saved.isEmpty()) {
                    HintText("No paired PCs yet. Run Helm on your PC and scan its QR code.")
                }
                saved.firstOrNull()?.let { pc ->
                    MainPcPanel(
                        pc = pc,
                        found = discovered[pc.hostId.lowercase()],
                        connecting = connectingTo.equals(pc.hostId, ignoreCase = true),
                        onOpen = { target -> onConnect(pc, target) },
                        onLongPress = { removeTarget = pc },
                        onWake = { onWake(pc) },
                    )
                    AnywhereRow(pc, onSwitch = { on -> anywhereSwitch(pc, on) }, onEdit = { relayTarget = pc })
                }
                saved.drop(1).forEach { pc ->
                    val found = discovered[pc.hostId.lowercase()]
                    val connecting = connectingTo.equals(pc.hostId, ignoreCase = true)
                    val reach = PcReach.of(pc, found, connecting)
                    PcRow(
                        title = pc.name,
                        status = reach.text,
                        statusColor = if (reach.link == PcLink.WIFI || reach.link == PcLink.RELAY) c.ok else c.ink2,
                        action = if (found == null && pc.macs.isNotEmpty()) "Wake" else "Connect",
                        onAction = { if (found == null && pc.macs.isNotEmpty()) onWake(pc) else onConnect(pc, Screen.DOCK) },
                        onClick = { onConnect(pc, Screen.DOCK) },
                        onLongClick = { removeTarget = pc },
                    )
                    AnywhereRow(pc, onSwitch = { on -> anywhereSwitch(pc, on) }, onEdit = { relayTarget = pc })
                }
                if (nearby.isNotEmpty()) {
                    MonoLabel("Nearby · not paired", color = c.ink)
                    nearby.forEach { host ->
                        PcRow(
                            title = host.name,
                            status = "${host.address} · not paired",
                            statusColor = c.ink2,
                            action = "Pair",
                            onAction = { nearbyInfo = host },
                            onClick = { nearbyInfo = host },
                            onLongClick = null,
                        )
                    }
                } else if (saved.isEmpty()) {
                    MonoLabel("Looking for PCs running Helm on this Wi-Fi…")
                }
                Speech(
                    if (saved.isEmpty()) "Open Helm on your PC and choose Pair a phone. I'll do the rest."
                    else "Lost Wi-Fi? I'll find your PC again by itself.",
                )
                if (saved.isNotEmpty()) MonoLabel("Tip: long-press a PC to remove it from this phone.")
            }

            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                PencilButton(
                    onClick = onScan,
                    shadow = 5.dp,
                    textStyle = BpType.button.copy(fontWeight = FontWeight.Black, fontSize = 17.sp),
                    modifier = Modifier.fillMaxWidth().height(63.dp),
                ) { Text("PAIR WITH QR") }
                TextButton(onClick = { showPaste = true }) {
                    MonoLabel("Paste pairing link", color = c.pencilInk, strong = true)
                }
            }
        }
    }

    nearbyInfo?.let { host ->
        var code by remember(host) { mutableStateOf("") }
        var codeError by remember(host) { mutableStateOf<String?>(null) }
        AlertDialog(
            onDismissRequest = { nearbyInfo = null },
            title = { Text("Pair with ${host.name}") },
            text = {
                Column {
                    Text("Scan the QR code shown on that PC, or — if the camera can't — choose Pair with code on the PC and type the 6-digit code. Check that the PC shows the name ${host.name}.")
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = code,
                        onValueChange = { v -> code = v.filter { it.isDigit() }.take(6); codeError = null },
                        label = { Text("6-digit code") },
                        singleLine = true,
                        isError = codeError != null,
                        supportingText = codeError?.let { { Text(it) } },
                        keyboardOptions = androidx.compose.foundation.text.KeyboardOptions(
                            keyboardType = androidx.compose.ui.text.input.KeyboardType.NumberPassword,
                        ),
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                if (code.length == 6) {
                    TextButton(onClick = {
                        val err = onPairCode(host, code)
                        if (err == null) nearbyInfo = null else codeError = err
                    }) { Text("Pair with code") }
                } else {
                    TextButton(onClick = { nearbyInfo = null; onScan() }) { Text("Scan QR") }
                }
            },
            dismissButton = { TextButton(onClick = { nearbyInfo = null }) { Text("Close") } },
        )
    }

    relayTarget?.let { pc ->
        var address by remember(pc.hostId) { mutableStateOf(pc.relayUrl ?: TunnelProtocol.DEFAULT_RELAY_URL) }
        var addressError by remember(pc.hostId) { mutableStateOf<String?>(null) }
        AlertDialog(
            onDismissRequest = { relayTarget = null },
            title = { Text("Connect from anywhere") },
            text = {
                Column {
                    Text("Away from Wi-Fi, ${pc.name} is reached through this relay. Connect from anywhere must be on for the PC too. The relay only forwards encrypted traffic.")
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = address,
                        onValueChange = { address = it; addressError = null },
                        label = { Text("Relay address") },
                        singleLine = true,
                        isError = addressError != null,
                        supportingText = addressError?.let { { Text(it) } },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    val err = onAnywhere(pc, true, address)
                    if (err == null) relayTarget = null else addressError = err
                }) { Text("Save") }
            },
            dismissButton = { TextButton(onClick = { relayTarget = null }) { Text("Cancel") } },
        )
    }

    removeTarget?.let { pc ->
        AlertDialog(
            onDismissRequest = { removeTarget = null },
            title = { Text("Remove ${pc.name}?") },
            text = { Text("This removes the PC from this phone only. Connect and use \"Forget this PC\" to also remove the phone from the PC.") },
            confirmButton = {
                TextButton(onClick = { onForgetLocal(pc); removeTarget = null }) { Text("Remove") }
            },
            dismissButton = { TextButton(onClick = { removeTarget = null }) { Text("Cancel") } },
        )
    }

    if (showSettings) {
        AlertDialog(
            onDismissRequest = { showSettings = false },
            title = { Text("This phone") },
            text = {
                Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    MonoLabel("Theme")
                    ThemeChips(settings.theme) { t -> onSettings { it.copy(theme = t) } }
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text("Haptic feedback", modifier = Modifier.weight(1f))
                        Switch(checked = settings.haptics, onCheckedChange = { v -> onSettings { it.copy(haptics = v) } })
                    }
                    MonoLabel("More settings appear once a PC is connected.")
                }
            },
            confirmButton = { TextButton(onClick = { showSettings = false }) { Text("Done") } },
        )
    }

    if (showPaste) {
        var text by remember { mutableStateOf("") }
        var error by remember { mutableStateOf<String?>(null) }
        AlertDialog(
            onDismissRequest = { showPaste = false },
            title = { Text("Paste pairing link") },
            text = {
                Column {
                    Text("Copy the helm://pair link shown on the PC and paste it here.")
                    Spacer(Modifier.height(12.dp))
                    OutlinedTextField(
                        value = text,
                        onValueChange = { text = it; error = null },
                        placeholder = { Text("helm://pair?…") },
                        isError = error != null,
                        supportingText = error?.let { { Text(it) } },
                        modifier = Modifier.fillMaxWidth(),
                    )
                }
            },
            confirmButton = {
                TextButton(onClick = {
                    val err = onPaste(text)
                    if (err == null) showPaste = false else error = err
                }) { Text("Pair") }
            },
            dismissButton = { TextButton(onClick = { showPaste = false }) { Text("Cancel") } },
        )
    }
}

/** Blueprint / Whiteprint / Deep (AMOLED) theme picker. */
@Composable
fun ThemeChips(current: ThemeMode, onPick: (ThemeMode) -> Unit) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        listOf(ThemeMode.DARK to "Blueprint", ThemeMode.LIGHT to "Whiteprint", ThemeMode.AMOLED to "Deep").forEach { (mode, label) ->
            MonoChip(label, onClick = { onPick(mode) }, selected = current == mode)
        }
    }
}

/** FIG. 1: the PC used last, with big shortcuts straight into its main screens. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun MainPcPanel(
    pc: SavedPc,
    found: DiscoveredHost?,
    connecting: Boolean,
    onOpen: (Screen) -> Unit,
    onLongPress: () -> Unit,
    onWake: () -> Unit,
) {
    val c = Bp.c
    val reach = PcReach.of(pc, found, connecting)
    val (status, color) = when (reach.link) {
        PcLink.CONNECTING -> "● ${reach.text}" to c.pencilInk
        PcLink.WIFI, PcLink.RELAY -> "● ${reach.text}" to c.ok
        PcLink.SEARCHING -> "○ ${reach.text}" to c.ink2
    }
    FigBox(fig = "1", trailing = status, trailingColor = color, modifier = Modifier.fillMaxWidth(), contentPadding = androidx.compose.foundation.layout.PaddingValues(start = 12.dp, end = 12.dp, top = 14.dp, bottom = 12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                pc.name.uppercase(),
                style = BpType.title.copy(fontSize = 22.sp, lineHeight = 26.sp),
                color = c.ink,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).combinedClickable(onClick = { onOpen(Screen.DOCK) }, onLongClick = onLongPress),
            )
            if (found == null && !connecting && pc.macs.isNotEmpty()) MonoChip("Wake", onClick = onWake)
        }
        val tiles = listOf(Screen.TOUCHPAD to "Touchpad", Screen.SCREEN to "Screen", Screen.MEDIA to "Media", Screen.DECK to "Deck")
        tiles.chunked(2).forEach { row ->
            Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                row.forEach { (target, label) ->
                    val m = Modifier.weight(1f).height(56.dp)
                    if (target == Screen.TOUCHPAD) {
                        PencilButton(onClick = { onOpen(target) }, modifier = m) { Text(label.uppercase()) }
                    } else {
                        Box(m.padding(end = 3.dp, bottom = 3.dp)) {
                            OutlineButton(onClick = { onOpen(target) }, modifier = Modifier.fillMaxSize()) { Text(label.uppercase()) }
                        }
                    }
                }
            }
        }
    }
}

/** The per-PC "Connect from anywhere" switch, with the relay address one tap away. */
@Composable
private fun AnywhereRow(pc: SavedPc, onSwitch: (Boolean) -> Unit, onEdit: () -> Unit) {
    val c = Bp.c
    val on = PcReach.anywhereOn(pc)
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Column(Modifier.weight(1f)) {
            Text("Connect from anywhere", style = MaterialTheme.typography.bodyMedium, color = c.ink)
            MonoLabel(if (on) "Relay when Wi-Fi can't reach it" else "Wi-Fi only", color = c.ink2, maxLines = 1)
        }
        if (on) TextButton(onClick = onEdit) { MonoLabel("Address", color = c.pencilInk, strong = true) }
        Switch(checked = on, onCheckedChange = onSwitch)
    }
}

/** A dashed row for another PC: name, mono status, one action. */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun PcRow(
    title: String,
    status: String,
    statusColor: Color,
    action: String,
    onAction: () -> Unit,
    onClick: () -> Unit,
    onLongClick: (() -> Unit)?,
) {
    val c = Bp.c
    Row(
        Modifier
            .fillMaxWidth()
            .dashedBorder(1.5.dp, c.ink.copy(alpha = 0.6f))
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(title, style = MaterialTheme.typography.titleMedium, color = c.ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
            MonoLabel(status, color = statusColor, maxLines = 1)
        }
        OutlineButton(onClick = onAction, textStyle = BpType.monoStrong) { Text(action.uppercase()) }
    }
}
