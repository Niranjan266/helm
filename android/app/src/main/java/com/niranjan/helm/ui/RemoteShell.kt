package com.niranjan.helm.ui

import android.os.SystemClock
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.selection.selectable
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowLeft
import androidx.compose.material.icons.filled.AccountBox
import androidx.compose.material.icons.filled.AddCircle
import androidx.compose.material.icons.filled.Build
import androidx.compose.material.icons.filled.Create
import androidx.compose.material.icons.filled.DateRange
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Email
import androidx.compose.material.icons.filled.Face
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Place
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.ThumbUp
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.RectangleShape
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.Session
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.ReconnectState
import com.niranjan.helm.device.Settings
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch

enum class Screen(val label: String, val icon: ImageVector, val primary: Boolean, val p3: Boolean = false, val p5: Boolean = false, val g6: Boolean = false, val g2: Boolean = false) {
    DOCK("Dock", Icons.Filled.Home, true),
    APPS("Apps", Icons.Filled.Search, true),
    TOUCHPAD("Touchpad", Icons.Filled.Create, true),
    SCREEN("Screen", Icons.Filled.AccountBox, true),
    MEDIA("Media & Controls", Icons.Filled.PlayArrow, true),
    DECK("Deck", Icons.Filled.Star, true),
    GAMEPAD("Gamepad", Icons.Filled.ThumbUp, false),
    MONITOR("Monitor & alerts", Icons.Filled.Info, false, p3 = true),
    TASKS("Task manager", Icons.Filled.Build, false, p3 = true),
    MIXER("Volume mixer", Icons.Filled.Notifications, false, p3 = true),
    WINDOWS("Windows & desktops", Icons.Filled.List, false, p3 = true),
    PROFILES("Profiles", Icons.Filled.Person, false, p3 = true),
    PC_FILES("PC files", Icons.Filled.Email, false, p3 = true),
    TERMINAL("Terminal", Icons.Filled.Edit, false, p5 = true),
    TRIGGERS("Triggers", Icons.Filled.DateRange, false, p5 = true),
    HOME_ASSISTANT("Home Assistant", Icons.Filled.Place, false, p5 = true),
    PLUGINS("Plugins", Icons.Filled.AddCircle, false, p5 = true),
    CLIPBOARD_HISTORY("Clipboard history", Icons.Filled.List, false, g6 = true),
    SCHEDULES("Schedules", Icons.Filled.DateRange, false, g6 = true),
    FIND_PHONE("Ring another phone", Icons.Filled.Notifications, false, g6 = true),
    PRESENT("Presentation", Icons.Filled.Face, false, g2 = true),
    FILES("Files & sharing", Icons.Filled.Share, false),
    SECURITY("Devices & security", Icons.Filled.Lock, false),
    SETTINGS("Settings", Icons.Filled.Settings, false),
}

private val Screen.short: String
    get() = when (this) {
        Screen.MEDIA -> "Media"
        Screen.FILES -> "Files"
        else -> label
    }


/** Connected UI: Dock home plus drawer / rail / bottom-bar navigation to every screen. */
@Composable
fun RemoteShell(
    session: Session,
    state: ConnectionState,
    reconnect: ReconnectState,
    sessionUi: SessionUi,
    settings: Settings,
    snackbar: SnackbarHostState,
    main: MainViewModel,
    remote: RemoteViewModel,
    power: PowerViewModel,
    otherPcs: List<com.niranjan.helm.core.SavedPc>,
    screen: Screen,
    onScreen: (Screen) -> Unit,
) {
    // While auto-reconnect works on it, keep the screens up (with a banner) instead of the
    // connection page.
    if (state !is ConnectionState.Ready && !reconnect.isReconnecting) {
        ConnectedScreen(
            session = session,
            state = state,
            ui = sessionUi,
            snackbar = snackbar,
            onBack = main::disconnect,
            onPing = main::ping,
            onRefreshInfo = main::refreshInfo,
            onRefreshDevices = main::refreshDevices,
            onForget = main::forgetCurrent,
            onRetry = main::retry,
        )
        return
    }

    val c = Bp.c
    val drawer = rememberDrawerState(DrawerValue.Closed)
    val extras: ExtrasViewModel = androidx.lifecycle.viewmodel.compose.viewModel()
    val remoteUi by remote.ui.collectAsStateWithLifecycle()
    val ready = session.connection.readyInfo
    val linkBadge = remember(ready) {
        when {
            ready == null -> null
            ready.viaRelay -> "Relay"
            com.niranjan.helm.core.UsbTether.isOverUsb(ready.endpoint.host, com.niranjan.helm.device.NetIfaces.local()) -> "USB"
            else -> "Wi-Fi"
        }
    }
    val scope = rememberCoroutineScope()
    val config = LocalConfiguration.current
    val landscape = config.screenWidthDp > config.screenHeightDp
    // Landscape touchpad: the touch surface owns the whole screen (no rail); it has its own back button.
    val immersive = landscape && screen == Screen.TOUCHPAD
    var confirmForget by remember { mutableStateOf(false) }
    // Landscape on the Screen tab is true full screen: no header, rail or bars.
    val streamVm: StreamViewModel = androidx.lifecycle.viewmodel.compose.viewModel()
    val streamExited by streamVm.fullscreenExited.collectAsStateWithLifecycle()
    val fullscreen = screen == Screen.SCREEN && com.niranjan.helm.core.StreamFullscreen.active(landscape, streamExited)

    // Latency for the status line: a light system.ping every 15 s while connected.
    val online = state is ConnectionState.Ready
    LaunchedEffect(session.connection, online) {
        while (online) {
            main.pingQuiet()
            delay(15_000)
        }
    }

    // Desk-dock mode: keep the screen on while the Dock is showing.
    val view = LocalView.current
    val keepOn = settings.keepScreenOnInDock && screen == Screen.DOCK
    DisposableEffect(keepOn) {
        view.keepScreenOn = keepOn
        onDispose { view.keepScreenOn = false }
    }

    val go: (Screen) -> Unit = { s ->
        onScreen(s)
        scope.launch { drawer.close() }
    }
    val openDrawer: () -> Unit = { scope.launch { drawer.open() } }
    val status = when {
        reconnect.isReconnecting -> "Reconnecting…"
        else -> listOfNotNull(session.name, sessionUi.pingMs?.let { "$it ms" }, linkBadge).joinToString(" · ")
    }
    val statusColor = if (reconnect.isReconnecting) c.pencilInk else c.ok

    ModalNavigationDrawer(
        drawerState = drawer,
        gesturesEnabled = drawer.isOpen || (screen != Screen.TOUCHPAD && screen != Screen.SCREEN && screen != Screen.TERMINAL && screen != Screen.PRESENT),
        drawerContent = {
            ShellDrawer(
                session = session,
                status = status,
                statusColor = statusColor,
                screens = Screen.entries.filter { (!it.p3 || remoteUi.phase3) && (!it.p5 || remoteUi.phase5) && (!it.g6 || com.niranjan.helm.core.Everyday.FEATURE in session.connection.hostFeatures) && (!it.g2 || com.niranjan.helm.core.PresentFeature.G2 in session.connection.hostFeatures) },
                current = screen,
                onGo = go,
                otherPcs = otherPcs,
                canWakeVia = remoteUi.phase4,
                onSwitch = { pc -> scope.launch { drawer.close() }; main.connect(pc) },
                onWakeVia = main::wakeVia,
                onDisconnect = { scope.launch { drawer.close() }; main.disconnect() },
            )
        },
    ) {
        Scaffold(
            containerColor = Color.Transparent,
            contentColor = c.ink,
            modifier = Modifier.gridPaper(c),
            topBar = {
                if (!landscape && !fullscreen) {
                    ShellTopBar(
                        title = if (screen == Screen.DOCK) session.name else screen.label,
                        status = status,
                        statusColor = statusColor,
                        isHome = screen == Screen.DOCK,
                        onBack = { onScreen(Screen.DOCK) },
                        onMenu = openDrawer,
                    )
                }
            },
            bottomBar = {
                if (!landscape && !fullscreen) {
                    ShellNavBar(Screen.entries.filter { it.primary }, screen, go)
                }
            },
            snackbarHost = { SnackbarHost(snackbar) },
        ) { padding ->
            Row(Modifier.fillMaxSize().padding(if (fullscreen || immersive) PaddingValues() else padding)) {
                if (landscape && !immersive && !fullscreen) ShellRail(Screen.entries.filter { it.primary }, screen, go, openDrawer)
                Box(Modifier.weight(1f).fillMaxSize()) {
                    Column(Modifier.fillMaxSize()) {
                        if (reconnect.isReconnecting) {
                            ReconnectBanner(session.name, reconnect, onRetry = main::reconnectNow, onDisconnect = main::disconnect)
                        }
                        if (screen == Screen.DOCK) UpdateBanner(Modifier.padding(12.dp))
                        if (online && !remoteUi.phase1) {
                            HintText(
                                "This PC's Helm agent is older and does not offer remote control yet. Update Helm on the PC.",
                                Modifier.fillMaxWidth().padding(12.dp),
                            )
                        }
                        Box(Modifier.weight(1f).fillMaxWidth()) {
                            ScreenContent(screen, session, sessionUi, settings, landscape, main, remote, power, extras, go, snackbar, onRequestForget = { confirmForget = true })
                        }
                    }
                }
            }
        }
    }

    if (confirmForget) {
        ForgetDialog(session.name, onConfirm = { confirmForget = false; main.forgetCurrent() }, onDismiss = { confirmForget = false })
    }
}

@Composable
private fun ScreenContent(
    screen: Screen,
    session: Session,
    sessionUi: SessionUi,
    settings: Settings,
    landscape: Boolean,
    main: MainViewModel,
    remote: RemoteViewModel,
    power: PowerViewModel,
    extras: ExtrasViewModel,
    go: (Screen) -> Unit,
    snackbar: SnackbarHostState,
    onRequestForget: () -> Unit,
) {
    when (screen) {
        Screen.MONITOR -> MonitorScreen(power)
        Screen.TASKS -> TasksScreen(power)
        Screen.MIXER -> MixerScreen(power)
        Screen.WINDOWS -> WindowsScreen(remote, power)
        Screen.PROFILES -> ProfilesScreen(remote, power)
        Screen.PC_FILES -> PcFilesScreen(remote, power)
        Screen.TERMINAL -> TerminalScreen(extras)
        Screen.TRIGGERS -> TriggersScreen(extras)
        Screen.HOME_ASSISTANT -> HomeAssistantScreen(extras, remote)
        Screen.PLUGINS -> PluginsScreen(extras)
        Screen.PRESENT -> PresentationScreen(snackbar)
        Screen.CLIPBOARD_HISTORY -> ClipboardHistoryScreen()
        Screen.SCHEDULES -> SchedulesScreen()
        Screen.FIND_PHONE -> FindPhoneScreen()
        Screen.SECURITY -> SecurityScreen(main, power)
        Screen.DOCK -> DockScreen(remote, settings, landscape, onOpenApps = { go(Screen.APPS) })
        Screen.APPS -> AppsScreen(remote)
        Screen.TOUCHPAD -> TouchpadScreen(remote, settings, hostId = session.hostId, onBack = { go(Screen.DOCK) })
        Screen.GAMEPAD -> GamepadScreen(remote, settings)
        Screen.SCREEN -> StreamScreen(remote, settings)
        Screen.MEDIA -> MediaScreen(remote, landscape)
        Screen.DECK -> DeckScreen(remote, settings)
        Screen.FILES -> FilesScreen(remote)
        Screen.SETTINGS -> SettingsScreen(
            settings = settings,
            onSettings = main::updateSettings,
            pcPrefs = main.prefsFor(session.hostId),
            onPcPrefs = { change -> main.updatePcPrefs(session.hostId, change) },
            pcDetails = {
                PcDetails(
                    session, sessionUi, main::ping, main::refreshInfo, main::refreshDevices,
                    onRequestForget = onRequestForget,
                    onDisconnect = main::disconnect,
                )
            },
        )
    }
}

/** Square back (or Helm mark on the Dock), uppercase title with a mono status line, menu. */
@Composable
private fun ShellTopBar(
    title: String,
    status: String,
    statusColor: Color,
    isHome: Boolean,
    onBack: () -> Unit,
    onMenu: () -> Unit,
) {
    Row(
        Modifier
            .fillMaxWidth()
            .statusBarsPadding()
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (isHome) {
            HelmMark(size = 28.dp)
        } else {
            SquareIconButton(onClick = onBack, contentDescription = "Back to the Dock") {
                Icon(Icons.AutoMirrored.Filled.KeyboardArrowLeft, contentDescription = null)
            }
        }
        Column(Modifier.weight(1f)) {
            DisplayText(title, maxLines = 1)
            StatusLine(status, color = statusColor)
        }
        SquareIconButton(onClick = onMenu, contentDescription = "All screens") {
            Icon(Icons.Filled.Menu, contentDescription = null)
        }
    }
}

/** Bottom navigation: ink rule on top, the current tab is a pencil block. */
@Composable
private fun ShellNavBar(items: List<Screen>, current: Screen, onGo: (Screen) -> Unit) {
    val c = Bp.c
    Column(Modifier.fillMaxWidth().background(c.paper.copy(alpha = 0.94f)).navigationBarsPadding()) {
        Box(Modifier.fillMaxWidth().height(1.5.dp).background(c.ink))
        Row(Modifier.fillMaxWidth().height(62.dp).padding(horizontal = 4.dp, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            items.forEach { s ->
                val selected = s == current
                Column(
                    Modifier
                        .weight(1f)
                        .fillMaxHeight()
                        .then(if (selected) Modifier.background(c.pencil).border(2.dp, c.deep) else Modifier)
                        .selectable(selected = selected, role = Role.Tab, onClick = { onGo(s) }),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    Icon(s.icon, contentDescription = null, tint = if (selected) c.deep else c.ink2, modifier = Modifier.size(20.dp))
                    Text(
                        s.short.uppercase(),
                        style = BpType.mono.copy(fontSize = 9.sp, letterSpacing = 0.04.em),
                        color = if (selected) c.deep else c.ink2,
                        maxLines = 1,
                        overflow = TextOverflow.Clip,
                    )
                }
            }
        }
    }
}

/** Landscape: the same tabs as a column on the left. */
@Composable
private fun ShellRail(items: List<Screen>, current: Screen, onGo: (Screen) -> Unit, onMenu: () -> Unit) {
    val c = Bp.c
    Row(Modifier.fillMaxHeight()) {
        Column(
            Modifier.fillMaxHeight().width(72.dp).padding(vertical = 8.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            SquareIconButton(onClick = onMenu, contentDescription = "All screens") {
                Icon(Icons.Filled.Menu, contentDescription = null)
            }
            Spacer(Modifier.height(4.dp))
            items.forEach { s ->
                val selected = s == current
                Box(
                    Modifier
                        .size(52.dp, 44.dp)
                        .then(if (selected) Modifier.background(c.pencil).border(2.dp, c.deep) else Modifier)
                        .selectable(selected = selected, role = Role.Tab, onClick = { onGo(s) })
                        .semantics { contentDescription = s.label },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(s.icon, contentDescription = null, tint = if (selected) c.deep else c.ink2)
                }
            }
        }
        Box(Modifier.fillMaxHeight().width(1.5.dp).background(c.ink))
    }
}

/** Drawer: a sheet of the drawing set — every screen, other PCs, disconnect. */
@Composable
private fun ShellDrawer(
    session: Session,
    status: String,
    statusColor: Color,
    screens: List<Screen>,
    current: Screen,
    onGo: (Screen) -> Unit,
    otherPcs: List<com.niranjan.helm.core.SavedPc>,
    canWakeVia: Boolean,
    onSwitch: (com.niranjan.helm.core.SavedPc) -> Unit,
    onWakeVia: (com.niranjan.helm.core.SavedPc) -> Unit,
    onDisconnect: () -> Unit,
) {
    val c = Bp.c
    ModalDrawerSheet(
        drawerShape = RectangleShape,
        drawerContainerColor = Color.Transparent,
        drawerContentColor = c.ink,
        modifier = Modifier.width(300.dp).gridPaper(c),
    ) {
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(16.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                HelmMark(size = 28.dp)
                Text("Helm", style = BpType.title.copy(fontWeight = FontWeight.Black, fontSize = 20.sp), color = c.ink)
            }
            SheetStrip("Sheet 02 · All screens", right = null)
            Column {
                DisplayText(session.name, maxLines = 1)
                StatusLine(status, color = statusColor)
            }
            PcHealthCard(Modifier.fillMaxWidth(), compact = true)
            Column(Modifier.border(1.5.dp, c.ink).background(c.panel)) {
                screens.forEachIndexed { i, s ->
                    if (i > 0) Box(Modifier.fillMaxWidth().height(1.dp).background(c.line))
                    DrawerRow(index = i + 1, screen = s, selected = s == current, onClick = { onGo(s) })
                }
            }
            if (otherPcs.isNotEmpty()) {
                MonoLabel("Switch PC", color = c.ink)
                otherPcs.forEach { pc ->
                    Row(
                        Modifier.fillMaxWidth().dashedBorder(1.5.dp, c.ink.copy(alpha = 0.6f)).clickable { onSwitch(pc) }.padding(start = 12.dp, end = 6.dp, top = 6.dp, bottom = 6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(pc.name, style = MaterialTheme.typography.titleMedium, color = c.ink, modifier = Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        if (canWakeVia && pc.macs.isNotEmpty()) MonoChip("Wake", onClick = { onWakeVia(pc) })
                    }
                }
            }
            OutlineButton(onClick = onDisconnect, modifier = Modifier.fillMaxWidth().height(48.dp)) { Text("Disconnect") }
        }
    }
}

@Composable
private fun DrawerRow(index: Int, screen: Screen, selected: Boolean, onClick: () -> Unit) {
    val c = Bp.c
    val ink = if (selected) c.deep else c.ink
    Row(
        Modifier
            .fillMaxWidth()
            .height(48.dp)
            .then(if (selected) Modifier.background(c.pencil) else Modifier)
            .selectable(selected = selected, role = Role.Tab, onClick = onClick)
            .padding(horizontal = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        Text("%02d".format(index), style = BpType.mono, color = if (selected) c.deep else c.ink2)
        Icon(screen.icon, contentDescription = null, tint = ink, modifier = Modifier.size(18.dp))
        Text(screen.label.uppercase(), style = BpType.button.copy(fontSize = 13.sp), color = ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** "Lost the PC" banner: a speech bubble plus the retry status in mono. */
@Composable
private fun ReconnectBanner(name: String, state: ReconnectState, onRetry: () -> Unit, onDisconnect: () -> Unit) {
    val c = Bp.c
    val detail by produceState("", state) {
        while (true) {
            value = when (state) {
                is ReconnectState.Waiting -> {
                    val left = ((state.retryAtMs - SystemClock.elapsedRealtime()) / 1000.0).coerceAtLeast(0.0)
                    "Try ${state.attempt} · next in ${"%.0f".format(kotlin.math.ceil(left))} s"
                }
                is ReconnectState.Trying -> "Try ${state.attempt} · " + (state.via?.let { "at ${it.host}" } ?: "connecting")
                else -> ""
            }
            delay(250)
        }
    }
    Column(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp).semantics(mergeDescendants = true) { liveRegion = LiveRegionMode.Polite },
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        Speech("Lost $name. Reconnecting… I'll find it again by itself.")
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            MonoLabel(detail, color = c.pencilInk, modifier = Modifier.weight(1f), maxLines = 1)
            MonoChip("Retry now", onClick = onRetry, height = 36.dp)
            MonoChip("Stop", onClick = onDisconnect, height = 36.dp)
        }
    }
}
