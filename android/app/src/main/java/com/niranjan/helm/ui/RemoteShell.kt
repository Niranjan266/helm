package com.niranjan.helm.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.TextButton
import androidx.compose.ui.Alignment
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.AccountBox
import androidx.compose.material.icons.filled.AddCircle
import androidx.compose.material.icons.filled.DateRange
import androidx.compose.material.icons.filled.Edit
import androidx.compose.material.icons.filled.Place
import androidx.compose.material.icons.filled.Create
import androidx.compose.material.icons.filled.Build
import androidx.compose.material.icons.filled.Email
import androidx.compose.material.icons.filled.Home
import androidx.compose.material.icons.filled.Info
import androidx.compose.material.icons.filled.List
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.Notifications
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Search
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.filled.Share
import androidx.compose.material.icons.filled.Star
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationDrawerItem
import androidx.compose.material3.NavigationRail
import androidx.compose.material3.NavigationRailItem
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.Session
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.device.Settings
import kotlinx.coroutines.launch

enum class Screen(val label: String, val icon: ImageVector, val primary: Boolean, val p3: Boolean = false, val p5: Boolean = false, val g6: Boolean = false) {
    DOCK("Dock", Icons.Filled.Home, true),
    APPS("Apps", Icons.Filled.Search, true),
    TOUCHPAD("Touchpad", Icons.Filled.Create, true),
    SCREEN("Screen", Icons.Filled.AccountBox, true),
    MEDIA("Media & Controls", Icons.Filled.PlayArrow, true),
    DECK("Deck", Icons.Filled.Star, true),
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
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun RemoteShell(
    session: Session,
    state: ConnectionState,
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
    if (state !is ConnectionState.Ready) {
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

    val drawer = rememberDrawerState(DrawerValue.Closed)
    val extras: ExtrasViewModel = androidx.lifecycle.viewmodel.compose.viewModel()
    val remoteUi by remote.ui.collectAsStateWithLifecycle()
    val ready = session.connection.readyInfo
    val linkBadge = remember(ready) {
        when {
            ready == null -> null
            ready.viaRelay -> "Relay"
            com.niranjan.helm.core.UsbTether.isOverUsb(ready.endpoint.host, com.niranjan.helm.device.NetIfaces.local()) -> "USB"
            else -> null
        }
    }
    val scope = rememberCoroutineScope()
    val config = LocalConfiguration.current
    val landscape = config.screenWidthDp > config.screenHeightDp
    var confirmForget by remember { mutableStateOf(false) }

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

    ModalNavigationDrawer(
        drawerState = drawer,
        gesturesEnabled = drawer.isOpen || (screen != Screen.TOUCHPAD && screen != Screen.SCREEN && screen != Screen.TERMINAL),
        drawerContent = {
            ModalDrawerSheet(Modifier.verticalScroll(rememberScrollState())) {
                Text(session.name, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(24.dp))
                val g6 = com.niranjan.helm.core.Everyday.FEATURE in session.connection.hostFeatures
                PcHealthCard(Modifier.padding(start = 24.dp, end = 24.dp, bottom = 8.dp), compact = true)
                Screen.entries.filter { (!it.p3 || remoteUi.phase3) && (!it.p5 || remoteUi.phase5) && (!it.g6 || g6) }.forEach { s ->
                    NavigationDrawerItem(
                        label = { Text(s.label) },
                        icon = { Icon(s.icon, contentDescription = null) },
                        selected = s == screen,
                        onClick = { go(s) },
                        modifier = Modifier.padding(horizontal = 12.dp),
                    )
                }
                if (otherPcs.isNotEmpty()) {
                    Text("Switch PC", style = MaterialTheme.typography.titleSmall, modifier = Modifier.padding(start = 24.dp, top = 16.dp, bottom = 4.dp))
                    otherPcs.forEach { pc ->
                        NavigationDrawerItem(
                            label = { Text(pc.name) },
                            icon = null,
                            selected = false,
                            onClick = { scope.launch { drawer.close() }; main.connect(pc) },
                            badge = if (remoteUi.phase4 && pc.macs.isNotEmpty()) {
                                { TextButton(onClick = { main.wakeVia(pc) }) { Text("Wake") } }
                            } else null,
                            modifier = Modifier.padding(horizontal = 12.dp),
                        )
                    }
                }
                Spacer(Modifier.height(12.dp))
                NavigationDrawerItem(
                    label = { Text("Disconnect") },
                    icon = null,
                    selected = false,
                    onClick = { scope.launch { drawer.close() }; main.disconnect() },
                    modifier = Modifier.padding(horizontal = 12.dp),
                )
            }
        },
    ) {
        Scaffold(
            topBar = {
                if (!landscape) {
                    TopAppBar(
                        title = {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Text(if (screen == Screen.DOCK) session.name else screen.label)
                                linkBadge?.let { LinkBadge(it) }
                            }
                        },
                        navigationIcon = {
                            IconButton(onClick = { scope.launch { drawer.open() } }) {
                                Icon(Icons.Filled.Menu, contentDescription = "All screens")
                            }
                        },
                    )
                }
            },
            bottomBar = {
                if (!landscape) {
                    NavigationBar {
                        Screen.entries.filter { it.primary }.forEach { s ->
                            NavigationBarItem(
                                selected = s == screen,
                                onClick = { go(s) },
                                icon = { Icon(s.icon, contentDescription = null) },
                                label = { Text(s.short) },
                            )
                        }
                    }
                }
            },
            snackbarHost = { SnackbarHost(snackbar) },
        ) { padding ->
            Row(Modifier.fillMaxSize().padding(padding)) {
                if (landscape) {
                    NavigationRail(
                        header = {
                            IconButton(onClick = { scope.launch { drawer.open() } }) {
                                Icon(Icons.Filled.Menu, contentDescription = "All screens")
                            }
                        },
                    ) {
                        Screen.entries.filter { it.primary }.forEach { s ->
                            NavigationRailItem(
                                selected = s == screen,
                                onClick = { go(s) },
                                icon = { Icon(s.icon, contentDescription = s.label) },
                                label = null,
                            )
                        }
                    }
                }
                Box(Modifier.weight(1f).fillMaxSize()) {
                    Column(Modifier.fillMaxSize()) {
                        if (!remoteUi.phase1) {
                            HintText(
                                "This PC's Helm agent is older and does not offer remote control yet. Update Helm on the PC.",
                                Modifier.fillMaxWidth().padding(12.dp),
                            )
                        }
                        Box(Modifier.weight(1f).fillMaxWidth()) {
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
                                Screen.CLIPBOARD_HISTORY -> ClipboardHistoryScreen()
                                Screen.SCHEDULES -> SchedulesScreen()
                                Screen.FIND_PHONE -> FindPhoneScreen()
                                Screen.SECURITY -> SecurityScreen(main, power)
                                Screen.DOCK -> DockScreen(remote, settings, landscape, onOpenApps = { go(Screen.APPS) })
                                Screen.APPS -> AppsScreen(remote)
                                Screen.TOUCHPAD -> TouchpadScreen(remote, settings)
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
                                            onRequestForget = { confirmForget = true },
                                            onDisconnect = main::disconnect,
                                        )
                                    },
                                )
                            }
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
private fun LinkBadge(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onTertiaryContainer,
        modifier = Modifier
            .padding(start = 8.dp)
            .background(MaterialTheme.colorScheme.tertiaryContainer, androidx.compose.foundation.shape.RoundedCornerShape(6.dp))
            .padding(horizontal = 6.dp, vertical = 2.dp),
    )
}
