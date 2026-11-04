package com.niranjan.helm.ui

import android.app.Activity
import android.bluetooth.BluetoothAdapter
import android.content.Context
import android.content.ContextWrapper
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.BtHidState
import com.niranjan.helm.core.BtHost
import com.niranjan.helm.core.BtInputBridge
import com.niranjan.helm.core.BtLink
import com.niranjan.helm.core.BtPermissions
import com.niranjan.helm.core.BtSupport
import com.niranjan.helm.core.ConsumerKey
import com.niranjan.helm.core.KeyPress
import com.niranjan.helm.device.AndroidBtHid
import com.niranjan.helm.device.BtHostStore
import com.niranjan.helm.device.GestureStore
import com.niranjan.helm.device.PadStore
import com.niranjan.helm.device.Settings
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

/**
 * Keeps the Bluetooth transport and input bridge alive across a screen rotation (the activity
 * is recreated, but the connection to the PC must not drop). [release] ends it for good.
 */
private class BtSession(context: Context) {
    val transport = AndroidBtHid(context)
    val bridge = BtInputBridge(transport)

    fun close() {
        bridge.releaseAll()
        transport.stop()
    }

    companion object {
        private var current: BtSession? = null

        @Synchronized
        fun acquire(context: Context): BtSession = current ?: BtSession(context.applicationContext).also { current = it }

        @Synchronized
        fun release() {
            current?.close()
            current = null
        }
    }
}

private tailrec fun Context.activity(): Activity? = when (this) {
    is Activity -> this
    is ContextWrapper -> baseContext.activity()
    else -> null
}

private fun hasBtPermissions(context: Context): Boolean =
    BtPermissions.missing(Build.VERSION.SDK_INT) {
        ContextCompat.checkSelfPermission(context, it) == PackageManager.PERMISSION_GRANTED
    }.isEmpty()

/**
 * Bluetooth mode: the phone is a plain Bluetooth mouse and keyboard. No Helm on the PC, no
 * Wi-Fi. Setup first (permissions, Bluetooth on, discoverable, pick or pair a PC), then the
 * touchpad once connected.
 */
@Composable
fun BluetoothScreen(settings: Settings, onExit: () -> Unit) {
    val context = LocalContext.current
    val session = remember { BtSession.acquire(context) }
    val transport = session.transport
    val bridge = session.bridge
    val state by transport.state.collectAsStateWithLifecycle()
    var permsOk by remember { mutableStateOf(hasBtPermissions(context)) }
    var showControl by remember { mutableStateOf(false) }

    val permissionLauncher = rememberLauncherForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        permsOk = hasBtPermissions(context)
        transport.refresh()
    }
    val enableLauncher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        transport.refresh()
    }
    val discoverableLauncher = rememberLauncherForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        transport.refresh()
    }

    // Register the HID app as soon as everything it needs is in place.
    LaunchedEffect(permsOk, state.adapterOn, state.support) {
        transport.refresh()
        if (permsOk && state.support == BtSupport.OK && state.adapterOn) transport.start()
    }

    // Never leave a key or button held when the app goes to the background or the link drops.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val observer = LifecycleEventObserver { _, event ->
            if (event == Lifecycle.Event.ON_STOP) bridge.releaseAll()
            if (event == Lifecycle.Event.ON_START) transport.refresh()
        }
        lifecycle.addObserver(observer)
        onDispose { lifecycle.removeObserver(observer) }
    }
    LaunchedEffect(state.link) {
        if (state.link == BtLink.CONNECTED) showControl = true else bridge.resetLocal()
    }
    // A rotation recreates the activity but keeps the session; leaving for good ends it.
    DisposableEffect(Unit) {
        onDispose {
            bridge.releaseAll()
            if (context.activity()?.isChangingConfigurations != true) BtSession.release()
        }
    }

    val leave = {
        bridge.releaseAll()
        BtSession.release()
        onExit()
    }
    BackHandler { if (showControl) showControl = false else leave() }

    val c = Bp.c
    Scaffold(containerColor = Color.Transparent, contentColor = c.ink, modifier = Modifier.gridPaper(c)) { padding ->
        Box(Modifier.fillMaxSize().padding(padding)) {
            if (showControl) {
                BluetoothControl(
                    state = state,
                    bridge = bridge,
                    settings = settings,
                    onSetup = { showControl = false },
                    onDisconnect = {
                        bridge.releaseAll()
                        transport.disconnect()
                    },
                )
            } else {
                BluetoothSetup(
                    state = state,
                    permsOk = permsOk,
                    onBack = leave,
                    onAllow = { permissionLauncher.launch(BtPermissions.required(Build.VERSION.SDK_INT).toTypedArray()) },
                    onEnable = { runCatching { enableLauncher.launch(Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE)) } },
                    onDiscoverable = {
                        val i = Intent(BluetoothAdapter.ACTION_REQUEST_DISCOVERABLE)
                            .putExtra(BluetoothAdapter.EXTRA_DISCOVERABLE_DURATION, 300)
                        runCatching { discoverableLauncher.launch(i) }
                    },
                    onConnect = transport::connect,
                    onDisconnect = {
                        bridge.releaseAll()
                        transport.disconnect()
                    },
                    onOpenPad = { showControl = true },
                )
            }
        }
    }
}

@Composable
private fun BluetoothSetup(
    state: BtHidState,
    permsOk: Boolean,
    onBack: () -> Unit,
    onAllow: () -> Unit,
    onEnable: () -> Unit,
    onDiscoverable: () -> Unit,
    onConnect: (BtHost) -> Unit,
    onDisconnect: () -> Unit,
    onOpenPad: () -> Unit,
) {
    val c = Bp.c
    Column(
        Modifier.fillMaxSize().padding(horizontal = 18.dp, vertical = 16.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            MonoChip("Back", onClick = onBack)
            Text("Bluetooth", style = BpType.title.copy(fontWeight = FontWeight.Black, fontSize = 20.sp), color = c.ink, modifier = Modifier.weight(1f))
        }
        SheetStrip("Bluetooth mouse & keyboard", right = state.statusText(), rightColor = if (state.connected) c.ok else c.ink2)

        Column(
            Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            val unsupported = state.support.message
            if (unsupported != null) {
                BpCard(Modifier.fillMaxWidth()) {
                    Text(unsupported, style = BpType.mono, color = c.ink, modifier = Modifier.padding(14.dp))
                }
                return@Column
            }

            HintText("Use this phone as a Bluetooth mouse and keyboard for any PC. Helm does not need to be installed on the PC, and Wi-Fi is not used.")

            if (!permsOk) {
                HintText("Helm needs permission to use Bluetooth: to connect to your PC and to be found by it.")
                PencilButton(onClick = onAllow, modifier = Modifier.fillMaxWidth().height(52.dp)) { Text("ALLOW BLUETOOTH ACCESS") }
            } else if (!state.adapterOn) {
                HintText("Bluetooth is off.")
                PencilButton(onClick = onEnable, modifier = Modifier.fillMaxWidth().height(52.dp)) { Text("TURN ON BLUETOOTH") }
            } else {
                StatusLine(
                    if (state.registered) "Ready: visible to PCs as a keyboard and mouse" else "Getting ready…",
                    color = if (state.registered) c.ok else c.pencilInk,
                )

                if (state.connected || state.link == BtLink.CONNECTING) {
                    BpCard(Modifier.fillMaxWidth()) {
                        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                            StatusLine(state.statusText(), color = if (state.connected) c.ok else c.pencilInk)
                            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                if (state.connected) {
                                    PencilButton(onClick = onOpenPad, modifier = Modifier.weight(1f).height(48.dp)) { Text("OPEN TOUCHPAD") }
                                }
                                OutlineButton(onClick = onDisconnect, modifier = Modifier.weight(1f).height(48.dp)) { Text("DISCONNECT") }
                            }
                        }
                    }
                } else {
                    MonoLabel("Not connected", color = c.ink)
                    val last = state.lastHost
                    if (last != null && state.registered) {
                        PencilButton(onClick = { onConnect(last) }, modifier = Modifier.fillMaxWidth().height(52.dp)) {
                            Text("RECONNECT TO ${last.name.uppercase()}")
                        }
                    }
                }

                MonoLabel("1 · Pair this phone with your PC", color = c.ink, strong = true)
                HintText("On Windows open Settings → Bluetooth & devices → Add device → Bluetooth, then pick this phone. Confirm the code on both.")
                OutlineButton(onClick = onDiscoverable, modifier = Modifier.fillMaxWidth().height(48.dp)) { Text("MAKE PHONE DISCOVERABLE") }

                MonoLabel("2 · Connect", color = c.ink, strong = true)
                if (state.bonded.isEmpty()) {
                    HintText("No paired devices yet. Pair from the PC first, then it appears here.")
                } else {
                    state.bonded.forEach { h ->
                        val isLast = state.lastHost?.address.equals(h.address, ignoreCase = true)
                        val here = state.host?.address.equals(h.address, ignoreCase = true) && state.link != BtLink.NOT_CONNECTED
                        BpCard(Modifier.fillMaxWidth()) {
                            Row(
                                Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
                                verticalAlignment = Alignment.CenterVertically,
                                horizontalArrangement = Arrangement.spacedBy(10.dp),
                            ) {
                                Column(Modifier.weight(1f)) {
                                    Text(h.name, style = BpType.monoStrong, color = c.ink, maxLines = 1)
                                    MonoLabel(if (isLast) "${h.address} · last used" else h.address, maxLines = 1)
                                }
                                OutlineButton(
                                    onClick = { onConnect(h) },
                                    enabled = state.registered && !here,
                                ) { Text(if (here) "…" else "CONNECT") }
                            }
                        }
                    }
                }
            }

            HintText("Characters are typed as on a US keyboard. What appears on the PC depends on the PC's keyboard language setting, so other layouts may type different symbols.")
        }
    }
}

private val MediaKeys = listOf(
    "Prev" to ConsumerKey.PREVIOUS,
    "Play/Pause" to ConsumerKey.PLAY_PAUSE,
    "Next" to ConsumerKey.NEXT,
    "Vol −" to ConsumerKey.VOLUME_DOWN,
    "Vol +" to ConsumerKey.VOLUME_UP,
    "Mute" to ConsumerKey.MUTE,
)

@Composable
private fun BluetoothControl(
    state: BtHidState,
    bridge: BtInputBridge,
    settings: Settings,
    onSetup: () -> Unit,
    onDisconnect: () -> Unit,
) {
    val context = LocalContext.current
    val c = Bp.c
    val hostStore = remember { BtHostStore(context) }
    val gestureStore = remember { GestureStore.get(context) }
    val gestures by gestureStore.settings.collectAsStateWithLifecycle()
    val padStore = remember { PadStore.get(context) }
    val kbd = rememberFloatingKeyboardState()
    var kbdOpen by remember { mutableStateOf(padStore.keyboardOpen()) }
    var typeOpen by remember { mutableStateOf(true) }
    var dragLock by remember { mutableStateOf(bridge.isDragLocked) }
    var speed by remember { mutableStateOf(hostStore.speed(settings.touchpadSpeed)) }
    var gestureDialog by remember { mutableStateOf(false) }
    var skipped by remember { mutableIntStateOf(0) }
    val mods = remember { mutableStateListOf<Int>() }
    val focus = remember { FocusRequester() }

    if (gestureDialog) GestureOptionsDialog(gestures, onChange = { g -> gestureStore.update { g } }, onDismiss = { gestureDialog = false })

    // Moves are flushed as each touch event arrives (rate-limited in the bridge); this sends
    // whatever was held back, and keeps the "skipped characters" count current.
    LaunchedEffect(bridge) {
        while (isActive) {
            delay(16)
            bridge.flushInput()
            skipped = bridge.skippedChars
            dragLock = bridge.isDragLocked
        }
    }

    fun press(vk: Int) {
        bridge.sendAll(KeyPress.frames(vk, mods.toList()))
        mods.clear()
    }

    Box(Modifier.fillMaxSize()) {
        Column(
            Modifier.fillMaxSize().padding(horizontal = 16.dp, vertical = 8.dp),
            verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                MonoChip("Setup", onClick = onSetup, height = 36.dp)
                StatusLine(
                    state.statusText(),
                    modifier = Modifier.weight(1f),
                    color = when (state.link) {
                        BtLink.CONNECTED -> c.ok
                        BtLink.CONNECTING -> c.pencilInk
                        BtLink.NOT_CONNECTED -> c.red
                    },
                )
                MonoChip("Disconnect", onClick = onDisconnect, enabled = state.link != BtLink.NOT_CONNECTED, height = 36.dp)
            }
            Row(
                Modifier.horizontalScroll(rememberScrollState()),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                MonoChip("Drag lock", onClick = { dragLock = bridge.setDragLock(!dragLock) }, selected = dragLock, height = 36.dp)
                MonoChip("Precision", onClick = { gestureStore.update { it.copy(precision = !it.precision) } }, selected = gestures.precision, height = 36.dp)
                MonoChip("Keyboard", onClick = { kbdOpen = !kbdOpen; padStore.setKeyboardOpen(kbdOpen) }, selected = kbdOpen, height = 36.dp)
                MonoChip("Type", onClick = { typeOpen = !typeOpen }, selected = typeOpen, height = 36.dp)
                MonoChip("Gestures", onClick = { gestureDialog = true }, dashed = true, height = 36.dp)
            }
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                MonoLabel("Speed")
                DiamondSlider(
                    value = speed,
                    onValueChange = { speed = it },
                    onValueChangeFinished = { hostStore.setSpeed(speed) },
                    valueRange = 0.4f..3f,
                    modifier = Modifier.weight(1f),
                )
                Text("%.1f×".format(speed), style = BpType.monoStrong, color = c.ink, textAlign = TextAlign.End, modifier = Modifier.width(40.dp))
            }

            TouchSurface(
                remote = bridge,
                speed = speed,
                haptics = settings.haptics,
                gestures = gestures,
                onLeftTap = { dragLock = bridge.isDragLocked },
                modifier = Modifier.weight(1f).fillMaxWidth(),
            )
            ClickButtons(bridge)

            Row(
                Modifier.horizontalScroll(rememberScrollState()),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
            ) {
                MediaKeys.forEach { (label, key) -> MonoChip(label, onClick = { bridge.pressConsumer(key) }, height = 36.dp) }
            }

            if (typeOpen) {
                KeyPanel(mods = mods, onKey = ::press)
                TypeField(remote = bridge, mods = mods, focus = focus, onKey = ::press)
            }

            if (skipped > 0) {
                MonoLabel(
                    "$skipped character${if (skipped == 1) "" else "s"} skipped: no key for them on a US layout",
                    maxLines = 2,
                )
            }
        }
        if (kbdOpen) {
            FloatingKeyboard(
                state = kbd,
                onFrames = bridge::sendAll,
                onClose = { kbdOpen = false; padStore.setKeyboardOpen(false) },
                haptics = settings.haptics,
            )
        }
    }
}
