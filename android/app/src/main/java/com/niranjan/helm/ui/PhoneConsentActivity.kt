package com.niranjan.helm.ui

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.media.projection.MediaProjectionConfig
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.PhoneRequest
import com.niranjan.helm.core.PhoneStage
import com.niranjan.helm.device.PhoneBridge
import com.niranjan.helm.device.PhoneScreenService

/**
 * The consent screen: one per request, never remembered. Decline ends it; Continue leads to
 * Android's own capture prompt, whose answer is good for this one session only. Touches that come
 * through another app's overlay are ignored, so nothing can trick the person into accepting.
 */
class PhoneConsentActivity : ComponentActivity() {
    private val capture = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) { result ->
        val stage = PhoneBridge.stage.value as? PhoneStage.Capturing
        val data = result.data
        if (stage != null && result.resultCode == Activity.RESULT_OK && data != null) {
            PhoneScreenService.start(this, result.resultCode, data)
        } else if (stage != null) {
            // Refused in Android's prompt: same as declining, with its own reason for the PC.
            PhoneBridge.onCaptureDenied(stage.request.requestId)
        }
        finish()
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.decorView.filterTouchesWhenObscured = true
        if (Build.VERSION.SDK_INT >= 31) window.setHideOverlayWindows(true)
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(false)
            setTurnScreenOn(true)
        } else {
            @Suppress("DEPRECATION")
            window.addFlags(WindowManager.LayoutParams.FLAG_TURN_SCREEN_ON)
        }
        setContent {
            val settings by (application as HelmApp).controller.settings.settings.collectAsStateWithLifecycle()
            HelmTheme(settings.theme) {
                val stage by PhoneBridge.stage.collectAsStateWithLifecycle()
                val a11y by PhoneBridge.accessibility.collectAsStateWithLifecycle()
                when (val s = stage) {
                    is PhoneStage.Asking -> ConsentBody(
                        request = s.request,
                        accessibilityOn = a11y,
                        onDecline = {
                            PhoneBridge.userDeclined(s.request.requestId)
                            finish()
                        },
                        onContinue = { wantControl ->
                            if (PhoneBridge.userAccepted(s.request.requestId, wantControl)) launchCapture() else finish()
                        },
                        onOpenSettings = {
                            runCatching {
                                startActivity(Intent(Settings.ACTION_ACCESSIBILITY_SETTINGS))
                            }
                        },
                    )
                    is PhoneStage.Capturing -> Unit // Android's prompt is on top
                    else -> LaunchedEffect(Unit) { finish() }
                }
            }
        }
    }

    private fun launchCapture() {
        val mgr = getSystemService(MediaProjectionManager::class.java)
        // The whole screen: control would land in the wrong place on a single-app capture.
        val prompt = if (Build.VERSION.SDK_INT >= 34) {
            mgr.createScreenCaptureIntent(MediaProjectionConfig.createConfigForDefaultDisplay())
        } else {
            mgr.createScreenCaptureIntent()
        }
        capture.launch(prompt)
    }

    companion object {
        fun intent(context: Context): Intent = Intent(context, PhoneConsentActivity::class.java)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS)
    }
}

@Composable
private fun ConsentBody(
    request: PhoneRequest,
    accessibilityOn: Boolean,
    onDecline: () -> Unit,
    onContinue: (Boolean) -> Unit,
    onOpenSettings: () -> Unit,
) {
    var wantControl by rememberSaveable { mutableStateOf(false) }
    Column(
        Modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .systemBarsPadding()
            .verticalScroll(rememberScrollState())
            .padding(20.dp),
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        DisplayText("${request.pcName} asks to see this phone")
        SectionCard("What this means") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("The PC will see everything on this screen: messages, photos, notifications. Protected apps and the lock screen stay hidden.")
                HintText("It ends when you tap Stop, lock the screen or disconnect, or after 10 minutes without input from the PC. You are asked again every time.")
                if (request.relayed) {
                    Text("This goes through the relay, so it will be slow.", color = MaterialTheme.colorScheme.error)
                }
            }
        }
        SectionCard("Control") {
            Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Also let the PC tap and type", Modifier.weight(1f))
                    Switch(
                        checked = wantControl && accessibilityOn,
                        enabled = accessibilityOn,
                        onCheckedChange = { wantControl = it },
                    )
                }
                if (accessibilityOn) {
                    HintText("Off means view only. Password fields are never typed into.")
                } else {
                    HintText("The Helm accessibility service is off, so this session can only be view only.")
                    OutlineButton(onClick = onOpenSettings) { Text("Open accessibility settings") }
                }
            }
        }
        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            OutlineButton(onClick = onDecline, modifier = Modifier.weight(1f)) { Text("Decline") }
            PencilButton(onClick = { onContinue(wantControl && accessibilityOn) }, modifier = Modifier.weight(1f)) { Text("Continue") }
        }
        HintText("Android asks once more after this.")
    }
}
