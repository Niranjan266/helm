package com.niranjan.helm.device

import android.app.Notification
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.Canvas
import android.os.Bundle
import android.provider.Settings as AndroidSettings
import android.service.notification.NotificationListenerService
import android.service.notification.StatusBarNotification
import com.niranjan.helm.HelmApp
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.NotifyActionButton
import com.niranjan.helm.core.NotifyActionEvent
import com.niranjan.helm.core.NotifyAllowList
import com.niranjan.helm.core.NotifyPush
import com.niranjan.helm.core.notifyPush
import java.io.ByteArrayOutputStream
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.launch

/**
 * Phase 5 notification mirroring (§13): notifications from apps on the user's allow-list go to
 * the connected PC as `notify.push`; `notify.action` clicks come back and run the matching
 * notification action, with the typed text filled into its RemoteInput for replies.
 * Nothing is sent unless "Mirror notifications" is on and the app is allowed.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class NotifyMirrorService : NotificationListenerService() {
    private var scope: CoroutineScope? = null

    /** Recently mirrored notifications by key, so toast clicks can find them. */
    private val recent = object : LinkedHashMap<String, StatusBarNotification>(32, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<String, StatusBarNotification>?) = size > MAX_RECENT
    }

    private val controller get() = (application as HelmApp).controller

    override fun onListenerConnected() {
        val s = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
        scope = s
        s.launch {
            controller.session
                .flatMapLatest { it?.connection?.events ?: emptyFlow() }
                .filter { it.name == "notify.action" }
                .collect { ev -> NotifyActionEvent.from(ev.data)?.let { onAction(it) } }
        }
    }

    override fun onListenerDisconnected() {
        scope?.cancel()
        scope = null
    }

    override fun onDestroy() {
        scope?.cancel()
        scope = null
        super.onDestroy()
    }

    override fun onNotificationPosted(sbn: StatusBarNotification) {
        val settings = controller.settings.settings.value
        if (!NotifyAllowList.shouldMirror(settings.mirrorNotifications, settings.mirrorApps, sbn.packageName, packageName)) return
        val n = sbn.notification
        if (sbn.isOngoing || (n.flags and Notification.FLAG_GROUP_SUMMARY) != 0 || (n.flags and Notification.FLAG_FOREGROUND_SERVICE) != 0) return
        val conn = controller.readyConnection()?.takeIf { it.supportsPhase5 } ?: return
        val push = toPush(sbn) ?: return
        synchronized(recent) { recent[sbn.key] = sbn }
        scope?.launch(Dispatchers.IO) {
            try {
                HelmApi(conn).notifyPush(push)
            } catch (_: HelmException) {
                // Not allowed / agent offline: mirroring is best effort.
            }
        }
    }

    override fun onNotificationRemoved(sbn: StatusBarNotification) {
        synchronized(recent) { recent.remove(sbn.key) }
    }

    private fun toPush(sbn: StatusBarNotification): NotifyPush? {
        val n = sbn.notification
        val extras: Bundle = n.extras
        val title = extras.getCharSequence(Notification.EXTRA_TITLE)?.toString().orEmpty()
        val body = (extras.getCharSequence(Notification.EXTRA_BIG_TEXT) ?: extras.getCharSequence(Notification.EXTRA_TEXT))?.toString().orEmpty()
        if (title.isBlank() && body.isBlank()) return null
        val actions = n.actions.orEmpty()
        val buttons = actions.withIndex()
            .filter { (_, a) -> a.remoteInputs.isNullOrEmpty() && !a.title.isNullOrBlank() }
            .map { (i, a) -> NotifyActionButton(i.toString(), a.title.toString()) }
        val canReply = actions.any { a -> a.remoteInputs?.any { it.allowFreeFormInput } == true }
        return NotifyPush(
            app = appLabel(sbn.packageName),
            title = title,
            body = body,
            actions = buttons,
            reply = canReply,
            key = sbn.key,
            iconPng = appIconPng(sbn.packageName),
        )
    }

    private fun appLabel(pkg: String): String = try {
        packageManager.getApplicationLabel(packageManager.getApplicationInfo(pkg, 0)).toString()
    } catch (_: Exception) {
        pkg
    }

    private fun appIconPng(pkg: String): ByteArray? = try {
        val d = packageManager.getApplicationIcon(pkg)
        val bmp = Bitmap.createBitmap(ICON_PX, ICON_PX, Bitmap.Config.ARGB_8888)
        d.setBounds(0, 0, ICON_PX, ICON_PX)
        d.draw(Canvas(bmp))
        ByteArrayOutputStream().also { bmp.compress(Bitmap.CompressFormat.PNG, 100, it) }.toByteArray()
    } catch (_: Exception) {
        null
    }

    /** A toast click on the PC: reply, run an action button, or open the notification. */
    private fun onAction(ev: NotifyActionEvent) {
        val sbn = synchronized(recent) { recent[ev.key] } ?: return
        val n = sbn.notification
        val actions = n.actions.orEmpty()
        try {
            val text = ev.replyText
            if (text != null) {
                val action = actions.firstOrNull { a -> a.remoteInputs?.any { it.allowFreeFormInput } == true } ?: return
                val intent = Intent()
                val results = Bundle()
                action.remoteInputs.filter { it.allowFreeFormInput }.forEach { results.putCharSequence(it.resultKey, text) }
                RemoteInput.addResultsToIntent(action.remoteInputs, intent, results)
                action.actionIntent.send(this, 0, intent)
                return
            }
            val idx = ev.actionId?.toIntOrNull()
            if (idx != null && idx in actions.indices) {
                actions[idx].actionIntent?.send()
            } else if (ev.actionId == null) {
                n.contentIntent?.send()
            }
        } catch (_: PendingIntent.CanceledException) {
            // The app withdrew the action.
        }
    }

    companion object {
        private const val MAX_RECENT = 50
        private const val ICON_PX = 64

        /** Whether the user granted Helm notification access in system settings. */
        fun hasAccess(context: Context): Boolean {
            val flat = AndroidSettings.Secure.getString(context.contentResolver, "enabled_notification_listeners") ?: return false
            val me = ComponentName(context, NotifyMirrorService::class.java)
            return flat.split(':').any { ComponentName.unflattenFromString(it) == me }
        }

        fun accessSettingsIntent(): Intent =
            Intent(AndroidSettings.ACTION_NOTIFICATION_LISTENER_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
}
