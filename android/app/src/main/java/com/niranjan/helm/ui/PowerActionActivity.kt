package com.niranjan.helm.ui

import android.app.AlertDialog
import android.content.Intent
import android.os.Bundle
import android.widget.Toast
import androidx.fragment.app.FragmentActivity
import com.niranjan.helm.HelmApp
import com.niranjan.helm.QUICK_POWER
import com.niranjan.helm.R
import com.niranjan.helm.core.WidgetPower
import com.niranjan.helm.device.QuickActionReceiver

/**
 * See-through screen behind the power widget's buttons. Asks before shutting down, runs the
 * per-PC fingerprint / screen-lock check when it is on, then hands the command to
 * [QuickActionReceiver] (which connects with the saved PC's credentials and toasts the result)
 * and closes straight away.
 */
class PowerActionActivity : FragmentActivity() {
    private var dialog: AlertDialog? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val action = intent.getStringExtra(EXTRA_ACTION)
        if (action == null || !WidgetPower.isSupported(action)) return finish()
        val controller = (application as HelmApp).controller
        val pc = controller.session.value?.let { s -> controller.savedPcs.get(s.hostId) }
            ?: controller.savedPcs.pcs.value.firstOrNull()
        if (pc == null) {
            Toast.makeText(this, "Pair with a PC in the Helm app first.", Toast.LENGTH_SHORT).show()
            return finish()
        }
        val gate = AdminGate(
            this,
            required = { controller.prefsFor(pc.hostId).biometricAdmin },
            onError = { msg ->
                Toast.makeText(this, msg, Toast.LENGTH_SHORT).show()
                finish()
            },
            onCancel = ::finish,
        )
        val send = {
            gate.run(WidgetPower.gateReason(action)) {
                if (controller.readyConnection() == null) {
                    Toast.makeText(this, "Connecting to ${pc.name}…", Toast.LENGTH_SHORT).show()
                }
                sendBroadcast(
                    Intent(this, QuickActionReceiver::class.java)
                        .setAction(QuickActionReceiver.ACTION)
                        .putExtra(QuickActionReceiver.EXTRA_KIND, QUICK_POWER)
                        .putExtra(QuickActionReceiver.EXTRA_ARG, action),
                )
                finish()
            }
        }
        if (!WidgetPower.needsConfirm(action)) return send()
        dialog = AlertDialog.Builder(this, R.style.Theme_Helm_WidgetDialog)
            .setTitle(WidgetPower.confirmTitle(controller.session.value?.name ?: pc.name))
            .setMessage(WidgetPower.CONFIRM_TEXT)
            .setPositiveButton(WidgetPower.label(action)) { _, _ -> send() }
            .setNegativeButton("Cancel") { _, _ -> finish() }
            .setOnCancelListener { finish() }
            .show()
    }

    override fun onDestroy() {
        dialog?.setOnCancelListener(null)
        dialog?.dismiss()
        dialog = null
        super.onDestroy()
    }

    override fun finish() {
        super.finish()
        @Suppress("DEPRECATION")
        overridePendingTransition(0, 0)
    }

    companion object {
        const val EXTRA_ACTION = "com.niranjan.helm.extra.POWER_ACTION"
    }
}
