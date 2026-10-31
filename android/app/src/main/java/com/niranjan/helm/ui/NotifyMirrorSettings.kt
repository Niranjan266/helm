package com.niranjan.helm.ui

import android.content.Intent
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.material3.Checkbox
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.LifecycleEventObserver
import androidx.lifecycle.compose.LocalLifecycleOwner
import com.niranjan.helm.device.NotifyMirrorService
import com.niranjan.helm.device.Settings
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

private data class PhoneApp(val pkg: String, val label: String)

/** Settings section: "Mirror notifications" to the PC, per-app allow-list (§13). Off by default. */
@Composable
fun NotifyMirrorSettings(settings: Settings, onSettings: ((Settings) -> Settings) -> Unit) {
    val context = LocalContext.current
    var access by remember { mutableStateOf(NotifyMirrorService.hasAccess(context)) }
    var apps by remember { mutableStateOf<List<PhoneApp>?>(null) }
    var filter by remember { mutableStateOf("") }

    // Re-check access when coming back from the system settings screen.
    val lifecycle = LocalLifecycleOwner.current.lifecycle
    DisposableEffect(lifecycle) {
        val obs = LifecycleEventObserver { _, e -> if (e == Lifecycle.Event.ON_RESUME) access = NotifyMirrorService.hasAccess(context) }
        lifecycle.addObserver(obs)
        onDispose { lifecycle.removeObserver(obs) }
    }
    LaunchedEffect(settings.mirrorNotifications) {
        if (!settings.mirrorNotifications || apps != null) return@LaunchedEffect
        apps = withContext(Dispatchers.IO) {
            val pm = context.packageManager
            val launcher = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
            pm.queryIntentActivities(launcher, 0)
                .map { PhoneApp(it.activityInfo.packageName, it.loadLabel(pm).toString()) }
                .filter { it.pkg != context.packageName }
                .distinctBy { it.pkg }
                .sortedBy { it.label.lowercase() }
        }
    }

    SectionCard("Notification mirroring") {
        Column {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Mirror notifications to the PC", modifier = Modifier.weight(1f))
                Switch(checked = settings.mirrorNotifications, onCheckedChange = { v -> onSettings { it.copy(mirrorNotifications = v) } })
            }
            HintText("Only the apps you tick below are sent, and only to a connected PC. Replies typed on the PC go back to the app.")
            if (settings.mirrorNotifications) {
                if (!access) {
                    Text("Helm needs notification access.", color = MaterialTheme.colorScheme.error)
                    OutlineButton(onClick = { context.startActivity(NotifyMirrorService.accessSettingsIntent()) }) { Text("Open notification access") }
                }
                Text("${settings.mirrorApps.size} app(s) allowed", style = MaterialTheme.typography.labelLarge)
                OutlinedTextField(filter, { filter = it }, label = { Text("Find app") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                val list = apps
                if (list == null) {
                    HintText("Loading apps…")
                } else {
                    list.filter { filter.isBlank() || it.label.contains(filter, true) || it.pkg.contains(filter, true) }
                        .sortedByDescending { it.pkg in settings.mirrorApps }
                        .take(if (filter.isBlank()) 60 else 200)
                        .forEach { app ->
                            val on = app.pkg in settings.mirrorApps
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                Checkbox(checked = on, onCheckedChange = { v ->
                                    onSettings { s -> s.copy(mirrorApps = if (v) s.mirrorApps + app.pkg else s.mirrorApps - app.pkg) }
                                })
                                Column {
                                    Text(app.label)
                                    Text(app.pkg, style = MaterialTheme.typography.labelSmall)
                                }
                            }
                        }
                }
            }
        }
    }
}
