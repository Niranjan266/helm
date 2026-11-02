package com.niranjan.helm.ui

import android.os.SystemClock
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_WEAK
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import com.niranjan.helm.core.AppLockTimer
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/**
 * "Lock Helm after N minutes in the background" (g4, default off): when the app comes back
 * after the timeout it shows [LockScreen] until fingerprint / face / screen lock succeeds.
 * Admin actions keep their own check ([AdminGate]). A phone without any screen lock cannot be
 * locked (there is nothing to check against).
 */
class AppLockController(
    private val activity: FragmentActivity,
    private val timeoutMinutes: () -> Int,
) {
    private val timer = AppLockTimer()
    private val _locked = MutableStateFlow(false)
    val locked: StateFlow<Boolean> = _locked.asStateFlow()

    private val authenticators = BIOMETRIC_WEAK or DEVICE_CREDENTIAL

    private fun canAuthenticate(): Boolean =
        BiometricManager.from(activity).canAuthenticate(authenticators) == BiometricManager.BIOMETRIC_SUCCESS

    fun onStop() {
        timer.timeoutMs = timeoutMinutes().coerceAtLeast(0) * 60_000L
        timer.onBackground(SystemClock.elapsedRealtime())
    }

    fun onStart() {
        timer.timeoutMs = timeoutMinutes().coerceAtLeast(0) * 60_000L
        if (timer.onForeground(SystemClock.elapsedRealtime())) {
            if (canAuthenticate()) _locked.value = true else timer.unlocked()
        }
    }

    fun unlock() {
        if (!canAuthenticate()) {
            timer.unlocked()
            _locked.value = false
            return
        }
        val prompt = BiometricPrompt(
            activity,
            ContextCompat.getMainExecutor(activity),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    timer.unlocked()
                    _locked.value = false
                }
            },
        )
        prompt.authenticate(
            BiometricPrompt.PromptInfo.Builder()
                .setTitle("Unlock Helm")
                .setSubtitle("Helm was locked while in the background")
                .setAllowedAuthenticators(authenticators)
                .build(),
        )
    }
}

/** Covers the whole app while locked. Asks once on show; the button asks again. */
@Composable
fun LockScreen(onUnlock: () -> Unit) {
    LaunchedEffect(Unit) { onUnlock() }
    Column(
        Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).padding(32.dp),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Helm is locked", style = MaterialTheme.typography.headlineSmall)
        Spacer(Modifier.height(8.dp))
        Text("Unlock with your fingerprint, face or screen lock.", style = MaterialTheme.typography.bodyMedium)
        Spacer(Modifier.height(24.dp))
        PencilButton(onClick = onUnlock) { Text("Unlock") }
    }
}
