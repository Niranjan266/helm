package com.niranjan.helm.device

import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import android.widget.Toast
import com.niranjan.helm.HelmApp
import com.niranjan.helm.HelmController
import com.niranjan.helm.QUICK_LOCK
import com.niranjan.helm.QUICK_MUTE
import com.niranjan.helm.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch

private val TileService.controller: HelmController get() = (application as HelmApp).controller

/** Runs one quick action, toasts the result, then refreshes the tile. */
private fun TileService.runQuick(kind: String, after: () -> Unit) {
    val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    scope.launch {
        Toast.makeText(this@runQuick, controller.quickAction(kind), Toast.LENGTH_SHORT).show()
        after()
        scope.cancel()
    }
}

/** Locks the PC. When the PC wants a biometric check for admin actions, opens the app to ask first. */
class LockPcTile : TileService() {
    override fun onStartListening() {
        qsTile?.apply {
            state = if (controller.savedPcs.pcs.value.isEmpty()) Tile.STATE_UNAVAILABLE else Tile.STATE_INACTIVE
            if (Build.VERSION.SDK_INT >= 29) subtitle = HelmBackground.state().pcName
            updateTile()
        }
    }

    override fun onClick() {
        val pc = controller.session.value?.hostId ?: controller.savedPcs.pcs.value.firstOrNull()?.hostId
        if (!controller.prefsFor(pc).biometricAdmin) return runQuick(QUICK_LOCK) {}
        val intent = Intent(this, MainActivity::class.java)
            .setAction(MainActivity.ACTION_LOCK_PC)
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(PendingIntent.getActivity(this, 7, intent, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT))
        } else {
            @Suppress("DEPRECATION")
            startActivityAndCollapse(intent)
        }
    }
}

/** Toggles the PC's master mute; shown active while muted. */
class MuteTile : TileService() {
    override fun onStartListening() {
        val st = HelmBackground.state()
        qsTile?.apply {
            state = when {
                controller.savedPcs.pcs.value.isEmpty() -> Tile.STATE_UNAVAILABLE
                st.muted == true -> Tile.STATE_ACTIVE
                else -> Tile.STATE_INACTIVE
            }
            if (Build.VERSION.SDK_INT >= 29) subtitle = st.pcName
            updateTile()
        }
    }

    override fun onClick() = runQuick(QUICK_MUTE) { onStartListening() }
}
