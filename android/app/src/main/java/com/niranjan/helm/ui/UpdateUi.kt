package com.niranjan.helm.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.HelmApp
import com.niranjan.helm.device.AppUpdater
import com.niranjan.helm.device.UpdateState

@Composable
private fun appUpdater(): AppUpdater = (LocalContext.current.applicationContext as HelmApp).updates

/** On the PC list and the Dock while a newer version exists or an install is under way. */
@Composable
fun UpdateBanner(modifier: Modifier = Modifier) {
    val updater = appUpdater()
    val state by updater.state.collectAsStateWithLifecycle()
    when (state) {
        is UpdateState.Available, is UpdateState.Downloading, is UpdateState.Installing,
        is UpdateState.NeedsInstallPermission, is UpdateState.NeedsConfirm,
        -> FigBox(title = "Update", modifier = modifier.fillMaxWidth()) {
            UpdateProgressRows(updater, state)
        }
        else -> Unit
    }
}

/** Settings > Updates (while connected to a PC). */
@Composable
fun UpdateSettingsSection() {
    SectionCard("Updates") { UpdateSettingsRows() }
}

/** Current version, "Check for updates" and the install flow. */
@Composable
fun UpdateSettingsRows() {
    val updater = appUpdater()
    val state by updater.state.collectAsStateWithLifecycle()
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        MonoLabel("Helm ${updater.currentVersion}")
        when (val s = state) {
            UpdateState.Disabled -> HintText("Updates are off in this build: it has no release signing key.")
            UpdateState.Checking -> HintText("Checking…")
            UpdateState.UpToDate -> HintText("You have the newest version.")
            is UpdateState.CheckFailed -> HintText(s.message)
            UpdateState.Idle -> Unit
            else -> UpdateProgressRows(updater, s)
        }
        if (state != UpdateState.Disabled) {
            val busy = state is UpdateState.Checking || state is UpdateState.Downloading || state is UpdateState.Installing
            OutlineButton(onClick = updater::checkNow, enabled = !busy) { Text("Check for updates") }
        }
        HintText("Updates come from the project's GitHub releases. Each one is checked (SHA-256 and signature) before it installs.")
    }
}

@Composable
private fun UpdateProgressRows(updater: AppUpdater, state: UpdateState) {
    Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
        when (state) {
            is UpdateState.Available -> {
                Text("Helm ${state.update.version} is available (you have ${updater.currentVersion}).")
                state.problem?.let { HintText(it) }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PencilButton(onClick = { updater.install(state.update) }) {
                        Text(if (state.problem == null) "Install" else "Try again")
                    }
                }
            }
            is UpdateState.Downloading -> {
                val mb = state.done / (1024 * 1024)
                if (state.total > 0) {
                    val fraction = (state.done.toFloat() / state.total).coerceIn(0f, 1f)
                    Text("Downloading Helm ${state.update.version}: ${(fraction * 100).toInt()}%")
                    LinearProgressIndicator(progress = { fraction }, modifier = Modifier.fillMaxWidth())
                } else {
                    Text("Downloading Helm ${state.update.version}: $mb MB")
                    LinearProgressIndicator(modifier = Modifier.fillMaxWidth())
                }
            }
            is UpdateState.NeedsInstallPermission -> {
                Text("Verified. One step left: allow Helm to install apps.")
                HintText("On the page that opens, turn on \"Allow from this source\", then come back and tap Continue.")
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    PencilButton(onClick = { updater.resume(state.apk) }) { Text("Continue") }
                    OutlineButton(onClick = updater::openInstallPermissionPage) { Text("Open setting") }
                }
            }
            is UpdateState.Installing -> Text("Verified. Installing Helm ${state.update.version}…")
            is UpdateState.NeedsConfirm -> {
                Text("Verified. Android needs you to confirm the install.")
                PencilButton(onClick = { updater.confirm(state.confirm) }) { Text("Confirm") }
            }
            else -> Unit
        }
    }
}
