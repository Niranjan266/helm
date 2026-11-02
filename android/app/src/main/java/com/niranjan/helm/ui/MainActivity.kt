package com.niranjan.helm.ui

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Parcelable
import androidx.fragment.app.FragmentActivity
import androidx.compose.runtime.CompositionLocalProvider
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.PairingUri
import com.niranjan.helm.device.SharedItem
import com.niranjan.helm.device.ThemeMode
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.launch
import androidx.lifecycle.lifecycleScope

class MainActivity : FragmentActivity() {
    private val vm: MainViewModel by viewModels()
    private val remote: RemoteViewModel by viewModels()
    private val power: PowerViewModel by viewModels()
    private val extras: ExtrasViewModel by viewModels()

    /** G2: volume keys change slides while the Presentation screen is open. */
    override fun dispatchKeyEvent(event: android.view.KeyEvent): Boolean =
        VolumeKeys.dispatch(event.keyCode, event.action == android.view.KeyEvent.ACTION_DOWN, event.repeatCount) ||
            super.dispatchKeyEvent(event)

    /** Fingerprint / screen-lock check before admin actions (per-PC setting). */
    private val adminGate by lazy {
        AdminGate(
            this,
            required = {
                val c = (application as HelmApp).controller
                c.prefsFor(c.session.value?.hostId).biometricAdmin
            },
            onError = { vm.message(it) },
        )
    }

    /** Screen to switch to (from a share intent). */
    private val navTo = MutableStateFlow<Screen?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) handleIntent(intent)
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(android.Manifest.permission.POST_NOTIFICATIONS) != android.content.pm.PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(android.Manifest.permission.POST_NOTIFICATIONS), 1)
        }

        setContent {
            val settings by vm.settings.collectAsStateWithLifecycle()
            HelmTheme(amoled = settings.theme == ThemeMode.AMOLED) {
                val saved by vm.savedPcs.collectAsStateWithLifecycle()
                val discovered by vm.discovered.collectAsStateWithLifecycle()
                val session by vm.session.collectAsStateWithLifecycle()
                val state by vm.connectionState.collectAsStateWithLifecycle()
                val ui by vm.ui.collectAsStateWithLifecycle()
                val snackbar = remember { SnackbarHostState() }
                var screen by rememberSaveable { mutableStateOf(Screen.DOCK) }

                LaunchedEffect(Unit) {
                    launch { vm.messages.collect { snackbar.showSnackbar(it) } }
                    launch { remote.messages.collect { snackbar.showSnackbar(it) } }
                    launch { power.messages.collect { snackbar.showSnackbar(it) } }
                    launch { extras.messages.collect { snackbar.showSnackbar(it) } }
                    navTo.collect { s ->
                        if (s != null) {
                            screen = s
                            navTo.value = null
                        }
                    }
                }

                val s = session
                if (s == null) {
                    HomeScreen(
                        saved = saved,
                        discovered = discovered,
                        snackbar = snackbar,
                        onConnect = vm::connect,
                        onForgetLocal = { vm.forgetLocally(it.hostId) },
                        onWake = vm::wake,
                        onScan = ::scanQr,
                        onPaste = vm::pairFromText,
                    )
                } else {
                    BackHandler { if (screen != Screen.DOCK) screen = Screen.DOCK else vm.disconnect() }
                    val pcPrefs = saved.firstOrNull { it.hostId.equals(s.hostId, true) }?.prefs
                    val effective = pcPrefs?.touchpadSpeed?.let { settings.copy(touchpadSpeed = it) } ?: settings
                    CompositionLocalProvider(LocalAdminGate provides { reason, action -> adminGate.run(reason, action) }) {
                        RemoteShell(
                            session = s,
                            state = state,
                            sessionUi = ui,
                            settings = effective,
                            snackbar = snackbar,
                            main = vm,
                            remote = remote,
                            power = power,
                            otherPcs = saved.filterNot { it.hostId.equals(s.hostId, true) },
                            screen = screen,
                            onScreen = { screen = it },
                        )
                    }
                }
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handleIntent(intent)
    }

    override fun onStart() {
        super.onStart()
        (application as HelmApp).controller.discovery.start()
    }

    override fun onStop() {
        (application as HelmApp).controller.discovery.stop()
        super.onStop()
    }

    private fun handleIntent(intent: Intent?) {
        intent ?: return
        when (intent.action) {
            ACTION_LOCK_PC -> {
                intent.action = null
                adminGate.run("Lock the PC") {
                    lifecycleScope.launch {
                        vm.message((application as HelmApp).controller.quickAction(com.niranjan.helm.QUICK_LOCK))
                    }
                }
            }
            Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE -> handleShare(intent)
            else -> handlePairLink(intent)
        }
    }

    /** "Send to PC" from another app: files, text or a URL. */
    private fun handleShare(intent: Intent) {
        val items = mutableListOf<SharedItem>()
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            parcelableList(intent)?.forEach { items += SharedItem.File(it) }
        } else {
            parcelable(intent)?.let { items += SharedItem.File(it) }
        }
        if (items.isEmpty()) intent.getStringExtra(Intent.EXTRA_TEXT)?.takeIf { it.isNotBlank() }?.let { items += SharedItem.Text(it) }
        if (items.isEmpty()) return
        val controller = (application as HelmApp).controller
        controller.pendingShare.value = controller.pendingShare.value + items
        navTo.value = Screen.FILES
        // Not connected: reconnect to the most recently used PC.
        if (controller.session.value == null) {
            controller.savedPcs.pcs.value.firstOrNull()?.let { controller.connectSaved(it) }
                ?: vm.message("Pair with a PC first, then share again.")
        }
        intent.action = null
    }

    @Suppress("DEPRECATION")
    private fun parcelable(intent: Intent): Uri? =
        if (Build.VERSION.SDK_INT >= 33) intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        else intent.getParcelableExtra<Parcelable>(Intent.EXTRA_STREAM) as? Uri

    @Suppress("DEPRECATION")
    private fun parcelableList(intent: Intent): List<Uri>? =
        if (Build.VERSION.SDK_INT >= 33) intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
        else intent.getParcelableArrayListExtra<Parcelable>(Intent.EXTRA_STREAM)?.filterIsInstance<Uri>()

    /** `helm://pair?...` deep link (QR scanner apps, adb). */
    private fun handlePairLink(intent: Intent) {
        val data = intent.data ?: return
        if (!data.scheme.equals(PairingUri.SCHEME, true) || !data.host.equals(PairingUri.HOST, true)) return
        val err = vm.pairFromText(data.toString())
        if (err != null) vm.message(err)
        // Don't re-pair with the same one-time link on configuration changes.
        intent.data = null
    }

    companion object {
        const val ACTION_LOCK_PC = "com.niranjan.helm.LOCK_PC"
    }

    private fun scanQr() {
        val options = GmsBarcodeScannerOptions.Builder()
            .setBarcodeFormats(Barcode.FORMAT_QR_CODE)
            .build()
        GmsBarcodeScanning.getClient(this, options)
            .startScan()
            .addOnSuccessListener { code ->
                val err = vm.pairFromText(code.rawValue)
                if (err != null) vm.message(err)
            }
            .addOnFailureListener { e ->
                vm.message("Scanner unavailable: ${e.message ?: "unknown error"}. Use \"Paste pairing link\" instead.")
            }
    }
}
