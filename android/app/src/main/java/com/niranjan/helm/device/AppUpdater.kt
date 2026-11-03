package com.niranjan.helm.device

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.util.Log
import com.niranjan.helm.BuildConfig
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.ApkUpdate
import com.niranjan.helm.core.ApkUpdater
import com.niranjan.helm.core.AppUpdate
import com.niranjan.helm.core.InstallOutcome
import com.niranjan.helm.core.SemVer
import com.niranjan.helm.core.UpdateException
import com.niranjan.helm.core.UpdateFailure
import com.niranjan.helm.core.UpdateFetcher
import com.niranjan.helm.core.VerifiedApk
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import okhttp3.OkHttpClient
import okhttp3.Request
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.util.concurrent.TimeUnit

/** What the Settings section and the home banner show. */
sealed interface UpdateState {
    data object Idle : UpdateState
    /** This build has no release key, so updates are refused. */
    data object Disabled : UpdateState
    data object Checking : UpdateState
    data object UpToDate : UpdateState
    data class Available(val update: ApkUpdate, val problem: String? = null) : UpdateState
    /** done/total in bytes; total 0 = unknown. */
    data class Downloading(val update: ApkUpdate, val done: Long, val total: Long) : UpdateState
    /** Verified, but Android must first allow Helm to install apps (Settings page opened). */
    data class NeedsInstallPermission(val apk: VerifiedApk) : UpdateState
    data class Installing(val update: ApkUpdate) : UpdateState
    /** Android's confirm screen could not be opened by itself; a tap opens it. */
    data class NeedsConfirm(val update: ApkUpdate, val confirm: Intent) : UpdateState
    data class CheckFailed(val message: String) : UpdateState
}

/**
 * In-app updates: at most one background check a day (when the app comes to the foreground), a
 * check on demand, and download + verify + PackageInstaller on request. Only a file that has just
 * passed the SHA-256 and Ed25519 checks is ever handed to the installer.
 */
class AppUpdater(context: Context) {
    private val app = context.applicationContext
    private val prefs = app.getSharedPreferences("helm_updates", Context.MODE_PRIVATE)
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private val dir = File(app.cacheDir, "updates")

    val currentVersion: String = BuildConfig.VERSION_NAME

    private val updater: ApkUpdater? = ApkUpdater.create(
        BuildConfig.UPDATE_PUBLIC_KEY,
        OkHttpFetcher(),
        SemVer.parse(currentVersion) ?: SemVer(0, 0, 0),
    )

    private val _state = MutableStateFlow<UpdateState>(if (updater == null) UpdateState.Disabled else UpdateState.Idle)
    val state: StateFlow<UpdateState> = _state.asStateFlow()

    @Volatile private var visible = false

    fun onAppVisible(isVisible: Boolean) {
        visible = isVisible
        if (!isVisible) return
        // Back from the "install unknown apps" page with the switch on: carry on by itself.
        val waiting = _state.value as? UpdateState.NeedsInstallPermission
        if (waiting != null && canInstall()) resume(waiting.apk) else checkIfDue()
    }

    /** The daily background check; quiet about failures. */
    fun checkIfDue() {
        if (updater == null || !isIdleForCheck(_state.value)) return
        if (!AppUpdate.checkDue(prefs.getLong(KEY_LAST_CHECK, 0), System.currentTimeMillis())) return
        if (!hasNetwork()) return
        check(quiet = true)
    }

    /** "Check for updates" in Settings. */
    fun checkNow() {
        if (updater == null || !isIdleForCheck(_state.value)) return
        if (!hasNetwork()) {
            _state.value = UpdateState.CheckFailed(NO_NETWORK)
            return
        }
        check(quiet = false)
    }

    private fun isIdleForCheck(s: UpdateState) = when (s) {
        is UpdateState.Idle, is UpdateState.UpToDate, is UpdateState.Available, is UpdateState.CheckFailed -> true
        else -> false
    }

    private fun check(quiet: Boolean) {
        val up = updater ?: return
        val before = _state.value
        _state.value = UpdateState.Checking
        scope.launch {
            try {
                val found = up.check()
                prefs.edit().putLong(KEY_LAST_CHECK, System.currentTimeMillis()).apply()
                _state.value = if (found != null) UpdateState.Available(found) else UpdateState.UpToDate
            } catch (e: Exception) {
                Log.i(TAG, "update check failed: $e")
                _state.value = when {
                    before is UpdateState.Available -> before
                    quiet -> UpdateState.Idle
                    else -> UpdateState.CheckFailed(describe(e))
                }
            }
        }
    }

    /** "Install": download and verify, then install. */
    fun install(update: ApkUpdate) {
        val up = updater ?: return
        if (_state.value is UpdateState.Downloading || _state.value is UpdateState.Installing) return
        if (!hasNetwork()) {
            _state.value = UpdateState.Available(update, NO_NETWORK)
            return
        }
        _state.value = UpdateState.Downloading(update, 0, update.apk.size)
        scope.launch {
            try {
                var lastPercent = -1L
                val apk = up.downloadAndVerify(update, dir) { done, total ->
                    val percent = if (total > 0) done * 100 / total else done / PROGRESS_STEP_BYTES
                    if (percent != lastPercent) {
                        lastPercent = percent
                        _state.value = UpdateState.Downloading(update, done, total)
                    }
                }
                installVerified(apk)
            } catch (e: Exception) {
                Log.w(TAG, "update refused: $e")
                _state.value = UpdateState.Available(update, describe(e))
            }
        }
    }

    /** After the user has been to the "install unknown apps" page. */
    fun resume(apk: VerifiedApk) {
        scope.launch {
            try {
                installVerified(apk)
            } catch (e: Exception) {
                _state.value = UpdateState.Available(apk.update, describe(e))
            }
        }
    }

    private fun canInstall() = app.packageManager.canRequestPackageInstalls()

    private fun installVerified(apk: VerifiedApk) {
        val up = updater ?: throw UpdateException(UpdateFailure.NO_KEY, "Updates are off in this build.")
        // Checked again here: nothing unverified reaches the installer, whatever happened since.
        up.reverify(apk)
        if (!canInstall()) {
            _state.value = UpdateState.NeedsInstallPermission(apk)
            openInstallPermissionPage()
            return
        }
        _state.value = UpdateState.Installing(apk.update)
        current = apk
        val installer = app.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL).apply {
            setAppPackageName(app.packageName)
            setSize(apk.file.length())
        }
        val sessionId = installer.createSession(params)
        try {
            installer.openSession(sessionId).use { session ->
                apk.file.inputStream().use { input ->
                    session.openWrite("Helm-android.apk", 0, apk.file.length()).use { out ->
                        input.copyTo(out, 64 * 1024)
                        session.fsync(out)
                    }
                }
                session.commit(statusSender(sessionId))
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(sessionId) }
            throw UpdateException(UpdateFailure.STORAGE, "Could not hand the update to Android: ${e.message}", e)
        }
    }

    private fun statusSender(sessionId: Int): android.content.IntentSender {
        val intent = Intent(app, UpdateInstallReceiver::class.java)
        // The system adds the status extras, so this one has to be mutable (explicit intent).
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= 31) PendingIntent.FLAG_MUTABLE else 0)
        return PendingIntent.getBroadcast(app, sessionId, intent, flags).intentSender
    }

    /** Opens Android's "install unknown apps" page for Helm. */
    fun openInstallPermissionPage() {
        val intent = Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:${app.packageName}"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        runCatching { app.startActivity(intent) }
    }

    fun confirm(intent: Intent) {
        runCatching { app.startActivity(intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
    }

    @Volatile private var current: VerifiedApk? = null

    internal fun onInstallStatus(status: Int, message: String?, confirm: Intent?) {
        val apk = current
        val update = apk?.update
        when (val outcome = InstallOutcome.fromStatus(status, message)) {
            InstallOutcome.NeedsUserAction -> {
                if (confirm == null) {
                    _state.value = if (update != null) UpdateState.Available(update, "Android did not ask to confirm.") else UpdateState.Idle
                    return
                }
                // Started by the system's broadcast: works while Helm is on screen. Otherwise the
                // banner offers it again.
                val opened = visible && runCatching {
                    app.startActivity(confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
                }.isSuccess
                if (!opened && update != null) _state.value = UpdateState.NeedsConfirm(update, confirm)
            }
            // Android restarts the app with the new version; nothing more to do.
            InstallOutcome.Success -> {
                apk?.file?.delete()
                _state.value = UpdateState.UpToDate
            }
            InstallOutcome.Cancelled -> {
                _state.value = if (update != null) UpdateState.Available(update) else UpdateState.Idle
            }
            is InstallOutcome.Failed -> {
                Log.w(TAG, "install failed: status=$status $message")
                apk?.file?.delete()
                _state.value = if (update != null) UpdateState.Available(update, outcome.message) else UpdateState.CheckFailed(outcome.message)
            }
        }
    }

    private fun hasNetwork(): Boolean {
        val cm = app.getSystemService(ConnectivityManager::class.java) ?: return true
        val caps = cm.getNetworkCapabilities(cm.activeNetwork) ?: return false
        return caps.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
    }

    private fun describe(e: Exception): String = when (e) {
        is UpdateException -> when (e.failure) {
            UpdateFailure.NETWORK -> e.message ?: NO_NETWORK
            UpdateFailure.CHECKSUM_MISMATCH, UpdateFailure.SIGNATURE_MISMATCH, UpdateFailure.VERIFY_FAILED,
            UpdateFailure.SIZE_MISMATCH, UpdateFailure.TOO_LARGE, UpdateFailure.INSECURE_URL ->
                "Update refused and deleted: ${e.message}"
            else -> e.message ?: "The update failed."
        }
        is IOException -> "Could not reach GitHub. Check your connection and try again."
        else -> "The update failed: ${e.message}"
    }

    private class OkHttpFetcher : UpdateFetcher {
        private val client = OkHttpClient.Builder()
            // Never follow a redirect from HTTPS down to HTTP.
            .followSslRedirects(false)
            .connectTimeout(15, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .build()

        override fun open(url: String): InputStream {
            val request = Request.Builder().url(url)
                .header("User-Agent", "helm-android/${BuildConfig.VERSION_NAME}")
                .header("Accept", "application/vnd.github+json, application/octet-stream")
                .build()
            val response = client.newCall(request).execute()
            if (!response.isSuccessful) {
                val code = response.code
                response.close()
                throw UpdateException(UpdateFailure.NETWORK, "GitHub answered HTTP $code. Try again later.")
            }
            return response.body?.byteStream()
                ?: throw UpdateException(UpdateFailure.NETWORK, "GitHub sent an empty answer.")
        }
    }

    private companion object {
        const val TAG = "HelmUpdate"
        const val KEY_LAST_CHECK = "last_check_ms"
        const val NO_NETWORK = "No network. Connect to the internet and try again."
        const val PROGRESS_STEP_BYTES = 256L * 1024
    }
}

/** Receives PackageInstaller's result for an update session. */
class UpdateInstallReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, PackageInstaller.STATUS_FAILURE)
        val message = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        @Suppress("DEPRECATION")
        val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
        (context.applicationContext as HelmApp).updates.onInstallStatus(status, message, confirm)
    }
}
