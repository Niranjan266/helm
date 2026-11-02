package com.niranjan.helm.device

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.niranjan.helm.core.PcNotification
import com.niranjan.helm.ui.MainActivity

/** Android notifications for the everyday features: PC notifications and quick file drop. */
object PcNotifications {
    const val CHANNEL_PC = "pc_notifications"
    const val CHANNEL_DROP = "file_drop"
    private const val DROP_ID_BASE = 4800

    fun createChannels(context: Context) {
        if (Build.VERSION.SDK_INT < 26) return
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        nm.createNotificationChannel(NotificationChannel(CHANNEL_PC, "PC notifications", NotificationManager.IMPORTANCE_DEFAULT))
        nm.createNotificationChannel(NotificationChannel(CHANNEL_DROP, "Files sent to the PC", NotificationManager.IMPORTANCE_LOW))
        PhoneRinger.createChannel(context)
    }

    private fun allowed(context: Context): Boolean = Build.VERSION.SDK_INT < 33 ||
        ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED

    private fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
        context, 0, Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    /** One Windows toast shown on the phone. */
    fun showPc(context: Context, pcName: String, n: PcNotification) {
        if (!allowed(context)) return
        createChannels(context)
        val title = n.title.ifBlank { n.app }
        val b = NotificationCompat.Builder(context, CHANNEL_PC)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle(title)
            .setContentText(n.body)
            .setStyle(NotificationCompat.BigTextStyle().bigText(n.body))
            .setSubText("${n.app} · $pcName")
            .setContentIntent(openApp(context))
            .setAutoCancel(true)
        if (n.timeMs > 0) b.setWhen(n.timeMs).setShowWhen(true)
        runCatching { NotificationManagerCompat.from(context).notify("pc:" + n.id, 0, b.build()) }
    }

    /** Upload progress ([percent] null = preparing). */
    fun dropProgress(context: Context, slot: Int, name: String, percent: Int?) {
        if (!allowed(context)) return
        createChannels(context)
        val b = NotificationCompat.Builder(context, CHANNEL_DROP)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle("Sending $name to the PC")
            .setOnlyAlertOnce(true)
            .setOngoing(true)
            .setProgress(100, percent ?: 0, percent == null)
        if (percent != null) b.setContentText("$percent %")
        runCatching { NotificationManagerCompat.from(context).notify(DROP_ID_BASE + slot, b.build()) }
    }

    fun dropDone(context: Context, slot: Int, name: String, text: String, failed: Boolean = false) {
        if (!allowed(context)) return
        createChannels(context)
        val b = NotificationCompat.Builder(context, CHANNEL_DROP)
            .setSmallIcon(if (failed) android.R.drawable.stat_notify_error else android.R.drawable.stat_sys_upload_done)
            .setContentTitle(name)
            .setContentText(text)
            .setContentIntent(openApp(context))
            .setAutoCancel(true)
        runCatching { NotificationManagerCompat.from(context).notify(DROP_ID_BASE + slot, b.build()) }
    }
}
