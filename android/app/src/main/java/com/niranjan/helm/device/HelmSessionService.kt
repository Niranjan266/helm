package com.niranjan.helm.device

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.graphics.drawable.Icon
import android.os.Build
import android.os.IBinder
import android.widget.Toast
import com.niranjan.helm.HelmApp
import com.niranjan.helm.QUICK_MEDIA
import com.niranjan.helm.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Keeps the PC connection alive while the app is in the background and shows the persistent
 * media notification (now playing + previous / play-pause / next, disconnect).
 */
class HelmSessionService : Service() {
    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        val n = build(this, HelmBackground.state())
        try {
            if (Build.VERSION.SDK_INT >= 29) startForeground(ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_CONNECTED_DEVICE)
            else startForeground(ID, n)
        } catch (_: Exception) {
            stopSelf()
            return START_NOT_STICKY
        }
        running = true
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        super.onDestroy()
    }

    companion object {
        private const val ID = 4782

        @Volatile
        private var running = false

        fun show(context: Context) {
            if (running) return
            try {
                context.startForegroundService(Intent(context, HelmSessionService::class.java))
            } catch (_: Exception) {
                // Background start not allowed (Android 12+): the next foreground connect shows it.
            }
        }

        fun hide(context: Context) {
            if (!running) return
            running = false
            context.stopService(Intent(context, HelmSessionService::class.java))
        }

        fun refresh(context: Context, st: BackgroundState) {
            if (!running || !st.connected) return
            runCatching { context.getSystemService(NotificationManager::class.java).notify(ID, build(context, st)) }
        }

        private fun build(context: Context, st: BackgroundState): Notification {
            val open = PendingIntent.getActivity(
                context, 0, Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val m = st.media
            val b = Notification.Builder(context, HelmBackground.CHANNEL_SESSION)
                .setSmallIcon(android.R.drawable.ic_menu_manage)
                .setContentTitle(m?.title?.ifBlank { null } ?: "Connected to ${st.pcName ?: "PC"}")
                .setContentText(if (m != null) listOf(m.artist, st.pcName).filter { !it.isNullOrBlank() }.joinToString(" · ") else "Helm")
                .setContentIntent(open)
                .setOngoing(true)
                .setShowWhen(false)
            if (m != null) {
                b.addAction(action(context, android.R.drawable.ic_media_previous, "Previous", QUICK_MEDIA, "previous", 1))
                b.addAction(
                    action(
                        context, if (m.isPlaying) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play,
                        if (m.isPlaying) "Pause" else "Play", QUICK_MEDIA, "toggle_play_pause", 2,
                    ),
                )
                b.addAction(action(context, android.R.drawable.ic_media_next, "Next", QUICK_MEDIA, "next", 3))
                b.setStyle(Notification.MediaStyle().setShowActionsInCompactView(0, 1, 2))
            }
            b.addAction(action(context, android.R.drawable.ic_menu_close_clear_cancel, "Disconnect", QuickActionReceiver.DISCONNECT, null, 4))
            return b.build()
        }

        private fun action(context: Context, icon: Int, title: String, kind: String, arg: String?, req: Int): Notification.Action =
            Notification.Action.Builder(Icon.createWithResource(context, icon), title, QuickActionReceiver.pending(context, kind, arg, req)).build()
    }
}

/** Runs widget / notification buttons: media keys, deck buttons, mute, lock, disconnect. */
class QuickActionReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val kind = intent.getStringExtra(EXTRA_KIND) ?: return
        val arg = intent.getStringExtra(EXTRA_ARG)
        val controller = (context.applicationContext as HelmApp).controller
        if (kind == DISCONNECT) {
            controller.disconnect()
            return
        }
        val pending = goAsync()
        CoroutineScope(SupervisorJob() + Dispatchers.Main).launch {
            try {
                val r = controller.quickAction(kind, arg)
                if (r != "OK") Toast.makeText(context, r, Toast.LENGTH_SHORT).show()
            } finally {
                pending.finish()
            }
        }
    }

    companion object {
        const val ACTION = "com.niranjan.helm.QUICK_ACTION"
        const val EXTRA_KIND = "kind"
        const val EXTRA_ARG = "arg"
        const val DISCONNECT = "disconnect"

        fun pending(context: Context, kind: String, arg: String?, req: Int): PendingIntent =
            PendingIntent.getBroadcast(
                context, req,
                Intent(context, QuickActionReceiver::class.java).setAction(ACTION).putExtra(EXTRA_KIND, kind).putExtra(EXTRA_ARG, arg),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
    }
}
