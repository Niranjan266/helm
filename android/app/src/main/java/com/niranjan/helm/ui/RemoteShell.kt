package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Create
import androidx.compose.material.icons.filled.Home
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

enum class Screen(val label: String, val icon: ImageVector, val primary: Boolean) {
    DOCK("Dock", Icons.Filled.Home, true),
    APPS("Apps", Icons.Filled.Search, true),
    TOUCHPAD("Touchpad", Icons.Filled.Create, true),
    MEDIA("Media & Controls", Icons.Filled.PlayArrow, true),
    DECK("Deck", Icons.Filled.Star, true),
    FILES("Files & sharing", Icons.Filled.Share, false),
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
        gesturesEnabled = drawer.isOpen || screen != Screen.TOUCHPAD,
        drawerContent = {
            ModalDrawerSheet {
                Text(session.name, style = MaterialTheme.typography.titleLarge, modifier = Modifier.padding(24.dp))
                Screen.entries.forEach { s ->
                    NavigationDrawerItem(
                        label = { Text(s.label) },
                        icon = { Icon(s.icon, contentDescription = null) },
                        selected = s == screen,
                        onClick = { go(s) },
                        modifier = Modifier.padding(horizontal = 12.dp),
                    )
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
                        title = { Text(if (screen == Screen.DOCK) session.name else screen.label) },
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
                        val ui by remote.ui.collectAsStateWithLifecycle()
                        if (!ui.phase1) {
                            HintText(
                                "This PC's Helm agent is older and does not offer remote control yet. Update Helm on the PC.",
                                Modifier.fillMaxWidth().padding(12.dp),
                            )
                        }
                        Box(Modifier.weight(1f).fillMaxWidth()) {
                            when (screen) {
                                Screen.DOCK -> DockScreen(remote, settings, landscape, onOpenApps = { go(Screen.APPS) })
                                Screen.APPS -> AppsScreen(remote)
                                Screen.TOUCHPAD -> TouchpadScreen(remote, settings)
                                Screen.MEDIA -> MediaScreen(remote, landscape)
                                Screen.DECK -> DeckScreen(remote, settings)
                                Screen.FILES -> FilesScreen(remote)
                                Screen.SETTINGS -> SettingsScreen(
                                    settings = settings,
                                    onSettings = main::updateSettings,
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
