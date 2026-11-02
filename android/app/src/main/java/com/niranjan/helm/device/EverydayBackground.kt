package com.niranjan.helm.device

import android.content.Context
import com.niranjan.helm.HelmController
import com.niranjan.helm.Session
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.HelmApi
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.NotifyAccess
import com.niranjan.helm.core.PcNotification
import com.niranjan.helm.core.PhoneRingEvent
import com.niranjan.helm.core.RingCommand
import com.niranjan.helm.core.RingLogic
import com.niranjan.helm.core.pcNotifyWatch
import com.niranjan.helm.core.supportsEveryday
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch

/**
 * App-scoped handling of the everyday (`g6`) events while connected, in the foreground or the
 * background (the session service keeps the process alive): `phone.ring` rings this phone,
 * `pc.notification` becomes an Android notification (filtered), and `pc.notify.watch` follows
 * the "Show PC notifications" setting.
 */
@OptIn(ExperimentalCoroutinesApi::class)
class EverydayBackground(private val context: Context, private val controller: HelmController) {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val store = EverydayStore.get(context)

    fun start() {
        PcNotifications.createChannels(context)
        scope.launch {
            controller.session
                .flatMapLatest { s -> s?.connection?.state?.map { st -> if (st is ConnectionState.Ready) s else null } ?: flowOf(null) }
                .distinctUntilChanged()
                .collectLatest { s ->
                    store.notifyAccess.value = null
                    if (s == null || !s.connection.supportsEveryday) return@collectLatest
                    launch { followWatchSetting(s) }
                    s.connection.events.collect { ev ->
                        when (ev.name) {
                            PhoneRingEvent.NAME -> onRing(PhoneRingEvent.from(ev.data))
                            PcNotification.NAME -> PcNotification.from(ev.data)?.let { onPcNotification(s, it) }
                        }
                    }
                }
        }
    }

    private fun onRing(ev: PhoneRingEvent) {
        when (RingLogic.decide(ev, PhoneRinger.ringing.value != null)) {
            RingCommand.START -> PhoneRinger.start(context, ev.fromName)
            RingCommand.STOP -> PhoneRinger.stop(context)
            RingCommand.IGNORE -> Unit
        }
    }

    private fun onPcNotification(s: Session, n: PcNotification) {
        store.update { it.copy(pcNotify = it.pcNotify.seen(n.app)) }
        if (store.settings.value.pcNotify.allows(n.app)) PcNotifications.showPc(context, s.name, n)
    }

    /** `pc.notify.watch{on}` whenever the setting changes while connected. */
    private suspend fun followWatchSetting(s: Session) {
        val api = HelmApi(s.connection)
        var watching = false
        store.settings.map { it.pcNotify.enabled }.distinctUntilChanged().collect { on ->
            if (!on && !watching) {
                store.notifyAccess.value = null
                return@collect
            }
            try {
                val access = api.pcNotifyWatch(on)
                watching = on
                store.notifyAccess.value = if (on) access else null
            } catch (e: HelmException) {
                store.notifyAccess.value = if (on) NotifyAccess.UNSUPPORTED else null
            }
        }
    }

    companion object {
        @Volatile
        private var started = false

        fun start(context: Context, controller: HelmController) {
            if (started) return
            started = true
            EverydayBackground(context.applicationContext, controller).start()
        }
    }
}
