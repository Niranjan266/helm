package com.niranjan.helm.device

import android.Manifest
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.google.android.gms.wearable.MessageEvent
import com.google.android.gms.wearable.PutDataRequest
import com.google.android.gms.wearable.Wearable
import com.google.android.gms.wearable.WearableListenerService
import com.niranjan.helm.HelmApp
import com.niranjan.helm.HelmController
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.ui.MainActivity
import com.niranjan.helm.wearlink.WearAction
import com.niranjan.helm.wearlink.WearCommand
import com.niranjan.helm.wearlink.WearLink
import com.niranjan.helm.wearlink.WearReply
import com.niranjan.helm.wearlink.WearStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull

/**
 * Wear OS companion bridge (phone side). The watch never reaches the PC itself: it sends a
 * [WearCommand] through the Wearable Data Layer, and this phone app runs it over its existing
 * pinned-TLS connection, with the same access level and admin check as the Quick Settings tiles.
 * The Data Layer only delivers between apps with the same package name and signing key.
 */
object WearBridge {
    @Volatile
    private var lastStatus: ByteArray? = null

    /** Publishes the connection state for the watch (a DataItem; no-op when unchanged). */
    fun publish(context: Context, st: BackgroundState) {
        val status = WearStatus(
            connected = st.connected,
            pcName = st.pcName,
            muted = st.muted,
            nowPlaying = st.media?.let { m -> listOf(m.title, m.artist).filter { it.isNotBlank() }.joinToString(" - ") }?.ifBlank { null },
            deck = st.deck.map { it.label },
        ).encode()
        if (lastStatus?.contentEquals(status) == true) return
        lastStatus = status
        putStatus(context, status)
    }

    internal fun putStatus(context: Context, bytes: ByteArray) {
        try {
            val req = PutDataRequest.create(WearLink.PATH_STATUS).setData(bytes).setUrgent()
            Wearable.getDataClient(context.applicationContext).putDataItem(req).addOnFailureListener { }
        } catch (_: Exception) {
            // No Google Play services / Wear API on this phone: nothing to publish to.
        }
    }

    /** Runs one watch command against the current PC; returns the reply for the watch. */
    suspend fun run(context: Context, controller: HelmController, cmd: WearCommand): WearReply {
        if (cmd.action == WearAction.STATUS) {
            lastStatus = null
            publish(context, HelmBackground.state())
            return WearReply(cmd.id, true, "OK")
        }
        if (cmd.action == WearAction.LOCK) {
            val pc = controller.session.value?.hostId ?: controller.savedPcs.pcs.value.firstOrNull()?.hostId
            if (controller.prefsFor(pc).biometricAdmin) {
                askToConfirmLock(context)
                return WearReply(cmd.id, false, "Confirm on your phone")
            }
        }
        val conn = controller.ensureConnected() ?: return WearReply(cmd.id, false, "Could not reach the PC")
        val api = HelmApi(conn)
        return try {
            val msg = when (cmd.action) {
                WearAction.PLAY_PAUSE -> { api.mediaControl("toggle_play_pause"); "OK" }
                WearAction.NEXT -> { api.mediaControl("next"); "OK" }
                WearAction.PREVIOUS -> { api.mediaControl("previous"); "OK" }
                WearAction.VOLUME_UP, WearAction.VOLUME_DOWN -> {
                    val cur = api.audio() ?: return WearReply(cmd.id, false, "No audio device")
                    val step = if (cmd.action == WearAction.VOLUME_UP) VOLUME_STEP else -VOLUME_STEP
                    val level = (cur.level + step).coerceIn(0f, 1f)
                    api.setAudio(level = level)
                    "Volume ${(level * 100).toInt()}%"
                }
                WearAction.MUTE -> {
                    val muted = api.audio()?.muted ?: false
                    api.setAudio(muted = !muted)
                    if (muted) "Sound on" else "Muted"
                }
                WearAction.LOCK -> { api.powerAction("lock", null); "Locked" }
                WearAction.DECK -> {
                    val slot = cmd.slot ?: return WearReply(cmd.id, false, "No button")
                    val buttons = HelmBackground.state().deck.ifEmpty { api.deck().take(WearLink.DECK_SLOTS) }
                    val b = buttons.getOrNull(slot) ?: return WearReply(cmd.id, false, "No button ${slot + 1}")
                    api.runDeck(b.id)
                    b.label
                }
                WearAction.STATUS -> "OK"
            }
            WearReply(cmd.id, true, msg)
        } catch (e: HelmException) {
            WearReply(cmd.id, false, if (e.code == "forbidden") "Not allowed for this phone" else (e.message ?: e.code))
        }
    }

    /** Lock needs the phone's fingerprint / screen-lock check: post a tap-to-confirm notification. */
    private fun askToConfirmLock(context: Context) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) return
        HelmBackground.createChannels(context)
        val open = PendingIntent.getActivity(
            context, 8,
            Intent(context, MainActivity::class.java)
                .setAction(MainActivity.ACTION_LOCK_PC)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val n = NotificationCompat.Builder(context, HelmBackground.CHANNEL_ALERTS)
            .setSmallIcon(android.R.drawable.ic_lock_lock)
            .setContentTitle("Lock PC?")
            .setContentText("Your watch asked to lock the PC. Tap to confirm.")
            .setContentIntent(open)
            .setAutoCancel(true)
            .setTimeoutAfter(60_000)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        runCatching { NotificationManagerCompat.from(context).notify(NOTIFY_LOCK_ID, n) }
    }

    private const val VOLUME_STEP = 0.05f
    private const val NOTIFY_LOCK_ID = 0x4857
}

/** Receives watch commands (path [WearLink.PATH_COMMAND]) and answers on [WearLink.PATH_REPLY]. */
class HelmWearService : WearableListenerService() {

    override fun onMessageReceived(event: MessageEvent) {
        if (event.path != WearLink.PATH_COMMAND) return
        val cmd = WearCommand.decode(event.data) ?: return
        val controller = (application as HelmApp).controller
        val source = event.sourceNodeId
        val app = applicationContext
        // App-scoped: the listener service may be unbound as soon as this callback returns.
        scope.launch {
            val reply = withTimeoutOrNull(20_000) { WearBridge.run(app, controller, cmd) }
                ?: WearReply(cmd.id, false, "Timed out")
            runCatching { Wearable.getMessageClient(app).sendMessage(source, WearLink.PATH_REPLY, reply.encode()).await() }
        }
    }

    private companion object {
        val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    }
}
