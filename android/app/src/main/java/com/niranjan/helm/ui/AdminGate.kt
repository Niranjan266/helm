package com.niranjan.helm.ui

import android.os.SystemClock
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_WEAK
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity

/**
 * Asks for fingerprint / face / screen lock before an admin action (ROADMAP Phase 3, A11).
 * One success unlocks admin actions for [graceMs]. When the phone has no screen lock at all
 * there is nothing to check against, so the action runs.
 */
class AdminGate(
    private val activity: FragmentActivity,
    /** Whether the current PC wants the check (per-PC setting). */
    private val required: () -> Boolean,
    private val onError: (String) -> Unit,
    private val graceMs: Long = 60_000,
) {
    private var unlockedUntil = 0L

    fun run(reason: String, action: () -> Unit) {
        if (!required() || SystemClock.elapsedRealtime() < unlockedUntil) return action()
        val authenticators = BIOMETRIC_WEAK or DEVICE_CREDENTIAL
        if (BiometricManager.from(activity).canAuthenticate(authenticators) != BiometricManager.BIOMETRIC_SUCCESS) return action()
        val prompt = BiometricPrompt(
            activity,
            ContextCompat.getMainExecutor(activity),
            object : BiometricPrompt.AuthenticationCallback() {
                override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                    unlockedUntil = SystemClock.elapsedRealtime() + graceMs
                    action()
                }

                override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                    if (errorCode != BiometricPrompt.ERROR_USER_CANCELED && errorCode != BiometricPrompt.ERROR_NEGATIVE_BUTTON &&
                        errorCode != BiometricPrompt.ERROR_CANCELED
                    ) {
                        onError("Not confirmed: $errString")
                    }
                }
            },
        )
        prompt.authenticate(
            BiometricPrompt.PromptInfo.Builder()
                .setTitle("Confirm admin action")
                .setSubtitle(reason)
                .setAllowedAuthenticators(authenticators)
                .build(),
        )
    }
}

/** `gate(reason) { action }` — runs the action after the admin check. Default: no check. */
val LocalAdminGate = staticCompositionLocalOf<(String, () -> Unit) -> Unit> { { _, action -> action() } }
