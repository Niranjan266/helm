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
import com.niranjan.helm.HelmController
import com.niranjan.helm.core.AlertEvent
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.DeckButton
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.MediaState
import com.niranjan.helm.core.alerts
import com.niranjan.helm.ui.MainActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/** What the widget, tiles and the media notification show. */
data class BackgroundState(
    val pcName: String? = null,
    val connected: Boolean = false,
    val media: MediaState? = null,
    val muted: Boolean? = null,
    /** First four deck buttons, for the home-screen widget. */
    val deck: List<DeckButton> = emptyList(),
)

/**
 * App-scoped bridge between the current session and Android surfaces outside the app UI:
 * alert notifications (§11 `alert`), the persistent media notification (foreground service),
 * the home-screen widget and the Quick Settings tiles.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class HelmBackground(private val context: Context, private val controller: HelmController) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val _state = MutableStateFlow(BackgroundState())

    fun start() {
        instance = this
        createChannels(context)
        scope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.map { st -> s to st } ?: flowOf(null) }
                .collectLatest { pair ->
                    if (pair == null) {
                        _state.value = BackgroundState()
                        publish()
                        return@collectLatest
                    }
                    val (s, st) = pair
                    if (st !is ConnectionState.Ready) {
                        _state.value = _state.value.copy(pcName = s.name, connected = false)
                        publish()
                        return@collectLatest
                    }
                    val conn = s.connection
                    val api = HelmApi(conn)
                    _state.value = BackgroundState(pcName = s.name, connected = true)
                    publish()
                    if (conn.supportsPhase1) {
                        launch { runCatching { api.mediaState() }.getOrNull()?.let { m -> update { it.copy(media = m) } } }
                        launch { runCatching { api.audio() }.getOrNull()?.let { a -> update { it.copy(muted = a.muted) } } }
                        launch { runCatching { api.deck() }.getOrNull()?.let { d -> update { it.copy(deck = d.take(4)) } } }
                    }
                    val ruleNames = HashMap<String, String>()
                    if (conn.supportsPhase3) launch {
                        try {
                            api.alerts().forEach { r -> ruleNames[r.id] = r.describe() }
                        } catch (_: HelmException) {
                        }
                    }
                    conn.events.collect { ev ->
                        when (ev.name) {
                            "media.changed" -> update { it.copy(media = MediaState.fromHolder(ev.data)) }
                            "audio.changed" -> update { it.copy(muted = (ev.data["muted"]?.toString() == "true")) }
                            "deck.changed" -> launch { runCatching { api.deck() }.getOrNull()?.let { d -> update { it.copy(deck = d.take(4)) } } }
                            "alert" -> AlertEvent.from(ev.data)?.let { a ->
                                if (a.firing && controller.prefsFor(s.hostId).notifyAlerts) notifyAlert(s.name, a, ruleNames[a.id])
                            }
                        }
                    }
                }
        }
    }

    private fun update(change: (BackgroundState) -> BackgroundState) {
        _state.value = change(_state.value)
        publish()
    }

    private fun publish() {
        val st = _state.value
        HelmWidget.updateAll(context, st)
        if (st.connected) HelmSessionService.show(context) else HelmSessionService.hide(context)
        HelmSessionService.refresh(context, st)
        WearBridge.publish(context, st)
    }

    private fun notifyAlert(pc: String, a: AlertEvent, rule: String?) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) return
        val open = PendingIntent.getActivity(
            context, 0, Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = a.message ?: (rule ?: a.id) + " (now %.0f)".format(a.value)
        val n = NotificationCompat.Builder(context, CHANNEL_ALERTS)
            .setSmallIcon(android.R.drawable.stat_sys_warning)
            .setContentTitle("$pc alert")
            .setContentText(text)
            .setContentIntent(open)
            .setAutoCancel(true)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        runCatching { NotificationManagerCompat.from(context).notify(a.id.hashCode(), n) }
    }

    companion object {
        const val CHANNEL_ALERTS = "alerts"
        const val CHANNEL_SESSION = "session"

        @Volatile
        var instance: HelmBackground? = null
            private set

        val current: StateFlow<BackgroundState>? get() = instance?._state?.asStateFlow()

        fun state(): BackgroundState = instance?._state?.value ?: BackgroundState()

        fun createChannels(context: Context) {
            if (Build.VERSION.SDK_INT < 26) return
            val nm = context.getSystemService(NotificationManager::class.java)
            nm.createNotificationChannel(NotificationChannel(CHANNEL_ALERTS, "PC alerts", NotificationManager.IMPORTANCE_HIGH))
            nm.createNotificationChannel(NotificationChannel(CHANNEL_SESSION, "Connected PC", NotificationManager.IMPORTANCE_LOW))
        }
    }
}
