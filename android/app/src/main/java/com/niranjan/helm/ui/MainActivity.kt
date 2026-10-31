package com.niranjan.helm.ui

import android.content.Intent
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.BackHandler
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.material3.SnackbarHostState
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.google.mlkit.vision.barcode.common.Barcode
import com.google.mlkit.vision.codescanner.GmsBarcodeScannerOptions
import com.google.mlkit.vision.codescanner.GmsBarcodeScanning
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.PairingUri

class MainActivity : ComponentActivity() {
    private val vm: MainViewModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        enableEdgeToEdge()
        if (savedInstanceState == null) handleIntent(intent)

        setContent {
            HelmTheme {
                val saved by vm.savedPcs.collectAsStateWithLifecycle()
                val discovered by vm.discovered.collectAsStateWithLifecycle()
                val session by vm.session.collectAsStateWithLifecycle()
                val state by vm.connectionState.collectAsStateWithLifecycle()
                val ui by vm.ui.collectAsStateWithLifecycle()
                val snackbar = remember { SnackbarHostState() }

                LaunchedEffect(Unit) {
                    vm.messages.collect { snackbar.showSnackbar(it) }
                }

                val s = session
                if (s == null) {
                    HomeScreen(
                        saved = saved,
                        discovered = discovered,
                        snackbar = snackbar,
                        onConnect = vm::connect,
                        onForgetLocal = { vm.forgetLocally(it.hostId) },
                        onScan = ::scanQr,
                        onPaste = vm::pairFromText,
                    )
                } else {
                    BackHandler { vm.disconnect() }
                    ConnectedScreen(
                        session = s,
                        state = state,
                        ui = ui,
                        snackbar = snackbar,
                        onBack = vm::disconnect,
                        onPing = vm::ping,
                        onRefreshInfo = vm::refreshInfo,
                        onRefreshDevices = vm::refreshDevices,
                        onForget = vm::forgetCurrent,
                        onRetry = vm::retry,
                    )
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

    /** `helm://pair?...` deep link (QR scanner apps, adb). */
    private fun handleIntent(intent: Intent?) {
        val data = intent?.data ?: return
        if (!data.scheme.equals(PairingUri.SCHEME, true) || !data.host.equals(PairingUri.HOST, true)) return
        val err = vm.pairFromText(data.toString())
        if (err != null) vm.message(err)
        // Don't re-pair with the same one-time link on configuration changes.
        intent.data = null
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
