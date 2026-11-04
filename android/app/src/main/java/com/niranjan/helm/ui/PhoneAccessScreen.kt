package com.niranjan.helm.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.core.PhoneControl
import com.niranjan.helm.core.PhoneStage
import com.niranjan.helm.device.PhoneBridge

/** "Let PC see this phone": the off-by-default switch, the live state, the accessibility steps. */
@Composable
fun PhoneAccessScreen() {
    val context = LocalContext.current
    val enabled by PhoneBridge.enabled.collectAsStateWithLifecycle()
    val a11y by PhoneBridge.accessibility.collectAsStateWithLifecycle()
    val stage by PhoneBridge.stage.collectAsStateWithLifecycle()
    val lastEnd by PhoneBridge.lastEnd.collectAsStateWithLifecycle()

    Column(
        Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(12.dp),
        verticalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        SectionCard("Let PC see this phone") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("A paired PC may ask to see this phone", Modifier.weight(1f))
                    Switch(checked = enabled, onCheckedChange = { PhoneBridge.setEnabled(it) })
                }
                HintText(
                    "Off by default. When on, the PC can only ask: you accept each session here and then in Android's " +
                        "prompt, every time. Nothing is remembered. Sound is not sent.",
                )
            }
        }

        when (val s = stage) {
            is PhoneStage.Live -> SectionCard(if (s.control) "PC is controlling" else "PC can see this phone") {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    Text(
                        if (s.control) "${s.request.pcName} sees the screen and can tap and type."
                        else "${s.request.pcName} sees the screen. View only.",
                    )
                    OutlineButton(onClick = { PhoneBridge.stopFromApp() }, danger = true) { Text("Stop") }
                }
            }
            is PhoneStage.Asking -> SectionCard("${s.request.pcName} is asking") {
                OutlineButton(onClick = { PhoneBridge.reopenConsent(context) }) { Text("Answer") }
            }
            is PhoneStage.Capturing -> SectionCard("Starting") { HintText("Waiting for Android's prompt…") }
            PhoneStage.Idle -> lastEnd?.let { HintText("Last session: ${endText(it)}") }
        }

        SectionCard("Let the PC tap and type (optional)") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(if (a11y) "The Helm accessibility service is on." else "The Helm accessibility service is off: sessions are view only.")
                Step(1, "Open Android's accessibility settings with the button below.")
                Step(2, "Find Helm under Downloaded apps or Installed services and open it.")
                Step(3, "Turn on \"Use Helm\" and confirm.")
                if (Build.VERSION.SDK_INT >= 33) {
                    Step(4, "If it is greyed out (Android blocks this for apps not from a store): open App info, the ⋮ menu, Allow restricted settings, then try again.")
                }
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    OutlineButton(onClick = { runCatching { context.startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS)) } }) {
                        Text("Accessibility settings")
                    }
                    if (Build.VERSION.SDK_INT >= 33) {
                        OutlineButton(
                            onClick = {
                                runCatching {
                                    context.startActivity(
                                        Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:${context.packageName}")),
                                    )
                                }
                            },
                        ) { Text("App info") }
                    }
                }
                HintText(
                    "Helm uses this service only to perform what the PC sends during a session you accepted. It does not read, " +
                        "store or send screen text or accessibility events. View only keeps working with it off.",
                )
            }
        }

        SectionCard("What stays private") {
            HintText(
                "Android hides protected apps (banking, DRM video) and the lock screen from capture; the PC sees black there and is " +
                    "told why. Helm does not try to get around it. Password fields refuse typing from the PC. Locking the screen ends the session.",
            )
        }
    }
}

@Composable
private fun Step(n: Int, text: String) {
    Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
        MonoLabel("$n", color = MaterialTheme.colorScheme.primary)
        Text(text, Modifier.weight(1f))
    }
}

private fun endText(reason: String): String = when (reason) {
    PhoneControl.REASON_STOPPED -> "you stopped it"
    PhoneControl.REASON_LOCKED -> "the screen was locked"
    PhoneControl.REASON_IDLE -> "no input from the PC for 10 minutes"
    PhoneControl.REASON_CAPTURE_STOPPED -> "Android ended the capture"
    PhoneControl.REASON_ENCODER -> "the video encoder failed"
    PhoneControl.REASON_CAPTURE_DENIED -> "Android's prompt was refused"
    "stopped_by_pc" -> "the PC stopped it"
    "disconnected" -> "the connection to the PC ended"
    else -> reason.replace('_', ' ')
}
