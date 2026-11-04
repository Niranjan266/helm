package com.niranjan.helm.device

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.niranjan.helm.HelmController
import com.niranjan.helm.Session
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmConnection
import com.niranjan.helm.core.HelmEvent
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.PhoneControl
import com.niranjan.helm.core.PhoneIn
import com.niranjan.helm.core.PhoneRequest
import com.niranjan.helm.core.PhoneSessionLogic
import com.niranjan.helm.core.PhoneStage
import com.niranjan.helm.core.PhoneStateChange
import com.niranjan.helm.core.RequestDecision
import com.niranjan.helm.core.optLong
import com.niranjan.helm.core.phoneCaps
import com.niranjan.helm.core.phoneDecline
import com.niranjan.helm.core.phoneState
import com.niranjan.helm.core.phoneStop
import com.niranjan.helm.core.supportsPhoneControl
import com.niranjan.helm.ui.PhoneConsentActivity
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * App-scoped glue for "Let PC see this phone" (Docs/protocol/phone-control.md): follows the
 * current PC connection, keeps it told what this phone allows, turns `phone.request` into the
 * consent screen and hands live input to the accessibility service. All decisions are made by
 * [PhoneSessionLogic]. The only thing stored is the on / off switch, which is off by default.
 */
@OptIn(ExperimentalCoroutinesApi::class)
object PhoneBridge {
    private const val PREFS = "helm_phone_control"
    private const val KEY_ENABLED = "enabled"
    private const val CHANNEL_ASK = "phone_request"
    private const val NOTIFICATION_ASK = 4796
    private const val REASON_STOPPED_BY_PC = "stopped_by_pc"

    private val logic = PhoneSessionLogic()
    private val _stage = MutableStateFlow<PhoneStage>(PhoneStage.Idle)
    val stage: StateFlow<PhoneStage> = _stage.asStateFlow()

    private val _enabled = MutableStateFlow(false)

    /** "Let PC see this phone". */
    val enabled: StateFlow<Boolean> = _enabled.asStateFlow()

    private val _accessibility = MutableStateFlow(false)

    /** The Helm accessibility service is connected. */
    val accessibility: StateFlow<Boolean> = _accessibility.asStateFlow()

    /** Why the last session ended, for the in-app screen. */
    private val _lastEnd = MutableStateFlow<String?>(null)
    val lastEnd: StateFlow<String?> = _lastEnd.asStateFlow()

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private lateinit var appContext: Context
    private lateinit var controller: HelmController

    fun start(context: Context, controller: HelmController) {
        appContext = context.applicationContext
        this.controller = controller
        val prefs = appContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        _enabled.value = prefs.getBoolean(KEY_ENABLED, false)
        scope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.map { st -> if (st is ConnectionState.Ready) s else null } ?: flowOf(null) }
                .distinctUntilChanged()
                .collectLatest { s ->
                    // No connection (or another one): whatever was running does not survive it.
                    endRunning(PhoneScreenService.REASON_DISCONNECTED)
                    if (s == null || !s.connection.supportsPhoneControl) return@collectLatest
                    launch { followCaps(s) }
                    s.connection.events.collect { ev -> onEvent(s, ev) }
                }
        }
    }

    private suspend fun followCaps(s: Session) {
        val api = HelmApi(s.connection)
        _enabled
            .flatMapLatest { on -> _accessibility.map { a11y -> on to a11y } }
            .distinctUntilChanged()
            .collect { (on, a11y) ->
                try {
                    api.phoneCaps(on, a11y)
                } catch (_: HelmException) {
                    // Connection going away; the next one tells the PC again.
                }
            }
    }

    private fun onEvent(s: Session, ev: HelmEvent) {
        when (ev.name) {
            PhoneControl.EV_REQUEST -> PhoneRequest.from(ev.data)?.let { onRequest(s, it) }
            PhoneControl.EV_CANCEL -> ev.data.optLong("request_id")?.let { onCancelled(it) }
            PhoneControl.EV_IDR -> PhoneScreenService.current?.requestKeyframe()
        }
    }

    private fun onRequest(s: Session, req: PhoneRequest) {
        when (logic.onRequest(req, _enabled.value)) {
            RequestDecision.ASK -> {
                publish()
                ask(req)
            }
            RequestDecision.DECLINE_DISABLED -> decline(s.connection, req.requestId, PhoneControl.REASON_DISABLED)
            RequestDecision.DECLINE_BUSY -> decline(s.connection, req.requestId, PhoneControl.REASON_BUSY)
        }
    }

    private fun decline(conn: HelmConnection, id: Long, reason: String) {
        scope.launch {
            try {
                HelmApi(conn).phoneDecline(id, reason)
            } catch (_: HelmException) {
            }
        }
    }

    private fun onCancelled(id: Long) {
        val running = PhoneScreenService.current
        if (running != null) {
            running.finishSession(REASON_STOPPED_BY_PC, tellPc = false)
        } else if (logic.end(id) != null) {
            _lastEnd.value = REASON_STOPPED_BY_PC
            publish()
        }
        cancelAsk()
    }

    private fun endRunning(reason: String) {
        val running = PhoneScreenService.current
        if (running != null) {
            running.finishSession(reason, tellPc = false)
        } else if (logic.end() != null) {
            _lastEnd.value = reason
            publish()
        }
        cancelAsk()
    }

    private fun publish() {
        _stage.value = logic.stage
    }

    // ------------------------------------------------------------ consent

    /** Consent screen now if Android lets us, and a heads-up notification either way. */
    private fun ask(req: PhoneRequest) {
        val context = appContext
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val open = PendingIntent.getActivity(context, 0, PhoneConsentActivity.intent(context), flags)
        if (Build.VERSION.SDK_INT < 33 ||
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) == PackageManager.PERMISSION_GRANTED
        ) {
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL_ASK, "PC asks to see this phone", NotificationManager.IMPORTANCE_HIGH),
            )
            val n = NotificationCompat.Builder(context, CHANNEL_ASK)
                .setSmallIcon(android.R.drawable.ic_menu_view)
                .setContentTitle("${req.pcName} asks to see this phone")
                .setContentText("Tap to answer. Nothing is shared unless you accept.")
                .setPriority(NotificationCompat.PRIORITY_MAX)
                .setCategory(NotificationCompat.CATEGORY_CALL)
                .setFullScreenIntent(open, true)
                .setContentIntent(open)
                .setAutoCancel(true)
                .build()
            runCatching { NotificationManagerCompat.from(context).notify(NOTIFICATION_ASK, n) }
        }
        runCatching { context.startActivity(PhoneConsentActivity.intent(context)) }
    }

    private fun cancelAsk() {
        runCatching { NotificationManagerCompat.from(appContext).cancel(NOTIFICATION_ASK) }
    }

    /** The consent screen's Decline (or it was closed without an answer). */
    fun userDeclined(id: Long, reason: String = PhoneControl.REASON_DECLINED) {
        if (logic.end(id) == null) return
        publish()
        cancelAsk()
        connection()?.let { decline(it, id, reason) }
    }

    /** No in Android's own prompt. */
    fun onCaptureDenied(id: Long) {
        if (logic.end(id) == null) return
        _lastEnd.value = PhoneControl.REASON_CAPTURE_DENIED
        publish()
        connection()?.let { decline(it, id, PhoneControl.REASON_CAPTURE_DENIED) }
    }

    /** The consent screen's Continue; Android's own capture prompt comes next. */
    fun userAccepted(id: Long, wantControl: Boolean): Boolean {
        val ok = logic.onAccepted(id, wantControl && PhoneAccessService.running)
        publish()
        cancelAsk()
        return ok
    }

    // ------------------------------------------------------------ the running session

    fun connection(): HelmConnection? = controller.session.value?.connection

    fun streamSocket(): HelmConnection = controller.streamSocket { }

    fun onCaptureFailed(id: Long) {
        // phone.stop, not phone.decline: phone.accept may already have been sent.
        onSessionEnded(id, PhoneControl.REASON_CAPTURE_STOPPED, tellPc = true)
    }

    fun onLive(id: Long): Boolean {
        val ok = logic.onLive(id, PhoneAccessService.running)
        publish()
        return ok
    }

    /** One PC input event. Dropped here unless control is live (the second gate, after the PC). */
    fun perform(ev: PhoneIn): Boolean {
        if (!logic.mayInject()) return false
        PhoneAccessService.instance?.perform(ev) ?: return false
        return true
    }

    fun sendState(id: Long, change: PhoneStateChange) {
        val conn = connection() ?: return
        scope.launch {
            try {
                HelmApi(conn).phoneState(id, change)
            } catch (_: HelmException) {
            }
        }
    }

    /** A field refused typing, or stopped refusing. */
    fun reportSecure(secure: String?) {
        val live = logic.stage as? PhoneStage.Live ?: return
        sendState(live.request.requestId, PhoneStateChange(secureChanged = true, secure = secure))
    }

    fun onSessionEnded(id: Long, reason: String, tellPc: Boolean) {
        if (logic.end(id) == null) return
        _lastEnd.value = reason
        publish()
        val conn = connection()
        if (tellPc && conn != null) {
            scope.launch {
                try {
                    HelmApi(conn).phoneStop(id, reason)
                } catch (_: HelmException) {
                }
            }
        }
    }

    // ------------------------------------------------------------ settings and service state

    fun setEnabled(on: Boolean) {
        appContext.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putBoolean(KEY_ENABLED, on).apply()
        _enabled.value = on
        // Off while a session runs ends it.
        if (!on) PhoneScreenService.current?.finishSession(PhoneControl.REASON_STOPPED, tellPc = true)
    }

    fun onAccessibilityChanged(on: Boolean) {
        _accessibility.value = on
        val control = logic.onAccessibility(on) ?: return
        publish()
        val live = logic.stage as? PhoneStage.Live ?: return
        sendState(live.request.requestId, PhoneStateChange(control = control, accessibility = on))
        PhoneScreenService.current?.updateNotification()
    }

    /** Opens the consent screen again if it is still waiting (from the in-app screen). */
    fun reopenConsent(context: Context) {
        if (logic.stage is PhoneStage.Asking) {
            context.startActivity(PhoneConsentActivity.intent(context))
        }
    }

    fun stopFromApp() {
        val current = PhoneScreenService.current
        if (current != null) {
            current.finishSession(PhoneControl.REASON_STOPPED, tellPc = true)
        } else {
            (logic.stage as? PhoneStage.Asking)?.let { userDeclined(it.request.requestId) }
        }
    }
}
