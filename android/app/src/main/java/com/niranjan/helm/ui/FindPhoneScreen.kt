package com.niranjan.helm.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.niranjan.helm.core.PhoneEntry
import com.niranjan.helm.device.PhoneRinger

/** Everyday §3: ring another phone paired with this PC; plus this phone's ring status. */
@Composable
fun FindPhoneScreen(vm: EverydayViewModel = viewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val ringing by PhoneRinger.ringing.collectAsStateWithLifecycle()
    val context = LocalContext.current
    LaunchedEffect(ui.g6) { if (ui.g6) vm.loadPhones() }

    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
        if (!ui.g6) {
            HintText("This PC's Helm agent does not offer Find my phone yet. Update Helm on the PC.")
            return@Column
        }
        if (ringing != null) {
            SectionCard("This phone is ringing") {
                Button(onClick = { PhoneRinger.stop(context) }) { Text("Found it") }
            }
        }
        SectionCard("Ring another phone") {
            val list = ui.phones
            val ringable = list?.let { PhoneEntry.ringable(it) }
            when {
                list == null -> HintText("Loading…")
                ringable.isNullOrEmpty() -> HintText("No other phone paired with this PC is connected right now.")
                else -> ringable.forEach { p ->
                    Column(Modifier.padding(vertical = 4.dp)) {
                        Text(p.name, style = MaterialTheme.typography.titleSmall)
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalAlignment = Alignment.CenterVertically) {
                            Button(onClick = { vm.ring(p, stop = false) }) { Text("Ring") }
                            OutlinedButton(onClick = { vm.ring(p, stop = true) }) { Text("Stop") }
                        }
                    }
                }
            }
            val offline = list.orEmpty().filter { !it.online && !it.isThis }
            if (offline.isNotEmpty()) HintText("Not connected: " + offline.joinToString { it.name })
            OutlinedButton(onClick = vm::loadPhones) { Text("Refresh") }
        }
        SectionCard("When this phone is rung") {
            HintText(
                "It plays a loud alarm at full volume (even on silent), vibrates and flashes the screen and torch until you tap Found it. " +
                    "The PC tray menu has \"Ring my phone\" too.",
            )
            if (!PhoneRinger.canUseFullScreen(context) && Build.VERSION.SDK_INT >= 34) {
                Text("Allow full-screen alarms so the ringing screen can open over the lock screen.", color = MaterialTheme.colorScheme.error)
                OutlinedButton(onClick = {
                    runCatching {
                        context.startActivity(
                            Intent(android.provider.Settings.ACTION_MANAGE_APP_USE_FULL_SCREEN_INTENT, Uri.parse("package:" + context.packageName))
                                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                        )
                    }
                }) { Text("Allow full-screen alarms") }
            }
        }
    }
}
