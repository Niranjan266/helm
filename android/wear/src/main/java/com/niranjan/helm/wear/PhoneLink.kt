package com.niranjan.helm.wear

import android.content.Context
import android.net.Uri
import com.google.android.gms.wearable.CapabilityClient
import com.google.android.gms.wearable.DataClient
import com.google.android.gms.wearable.DataEvent
import com.google.android.gms.wearable.MessageClient
import com.google.android.gms.wearable.PutDataRequest
import com.google.android.gms.wearable.Wearable
import com.niranjan.helm.wearlink.WearAction
import com.niranjan.helm.wearlink.WearCommand
import com.niranjan.helm.wearlink.WearLink
import com.niranjan.helm.wearlink.WearReply
import com.niranjan.helm.wearlink.WearStatus
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.tasks.await
import kotlinx.coroutines.withTimeoutOrNull
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong

/** What the watch shows about the phone and its PC. */
sealed interface LinkState {
    data object Searching : LinkState
    data object NoPhone : LinkState
    data class Phone(val status: WearStatus) : LinkState
}

/**
 * Watch side of the Data Layer link: finds the phone running Helm (capability
 * [WearLink.PHONE_CAPABILITY]), sends [WearCommand]s and waits for the matching [WearReply],
 * and follows the [WearStatus] DataItem the phone publishes.
 */
class PhoneLink(context: Context) {
    private val app = context.applicationContext
    private val messages: MessageClient = Wearable.getMessageClient(app)
    private val data: DataClient = Wearable.getDataClient(app)
    private val capabilities: CapabilityClient = Wearable.getCapabilityClient(app)

    private val _state = MutableStateFlow<LinkState>(LinkState.Searching)
    val state: StateFlow<LinkState> = _state.asStateFlow()

    private val pending = ConcurrentHashMap<String, CompletableDeferred<WearReply>>()
    private val nextId = AtomicLong(System.currentTimeMillis() % 1_000_000)

    private val onMessage = MessageClient.OnMessageReceivedListener { ev ->
        if (ev.path != WearLink.PATH_REPLY) return@OnMessageReceivedListener
        val r = WearReply.decode(ev.data) ?: return@OnMessageReceivedListener
        pending.remove(r.id)?.complete(r)
    }

    private val onData = DataClient.OnDataChangedListener { events ->
        for (ev in events) {
            if (ev.type == DataEvent.TYPE_CHANGED && ev.dataItem.uri.path == WearLink.PATH_STATUS) {
                WearStatus.decode(ev.dataItem.data)?.let { _state.value = LinkState.Phone(it) }
            }
        }
        events.release()
    }

    private val listening = AtomicBoolean(false)

    /** Registers the reply and status listeners once for the process (the link is app-scoped). */
    fun start() {
        if (!listening.compareAndSet(false, true)) return
        messages.addListener(onMessage)
        data.addListener(onData)
    }

    /** The phone's last published status without asking it for a new one (for the tile). */
    suspend fun peek(): WearStatus? {
        val node = phoneNode() ?: return null
        return readStatus(node)?.also { _state.value = LinkState.Phone(it) }
    }

    /** Re-reads the phone's last status and asks it to publish a fresh one. */
    suspend fun refresh() {
        val node = phoneNode()
        if (node == null) {
            _state.value = LinkState.NoPhone
            return
        }
        readStatus(node)?.let { _state.value = LinkState.Phone(it) }
        send(WearAction.STATUS)
        readStatus(node)?.let { _state.value = LinkState.Phone(it) }
    }

    /** Sends one command to the phone; returns its reply (or a local failure reply). */
    suspend fun send(action: WearAction, slot: Int? = null): WearReply {
        val id = nextId.incrementAndGet().toString(36)
        val node = phoneNode() ?: run {
            _state.value = LinkState.NoPhone
            return WearReply(id, false, "Phone not reachable")
        }
        val wait = CompletableDeferred<WearReply>()
        pending[id] = wait
        return try {
            messages.sendMessage(node, WearLink.PATH_COMMAND, WearCommand(id, action, slot).encode()).await()
            withTimeoutOrNull(REPLY_TIMEOUT_MS) { wait.await() } ?: WearReply(id, false, "No answer from phone")
        } catch (e: Exception) {
            WearReply(id, false, "Phone not reachable")
        } finally {
            pending.remove(id)
        }
    }

    private suspend fun phoneNode(): String? = try {
        val info = capabilities.getCapability(WearLink.PHONE_CAPABILITY, CapabilityClient.FILTER_REACHABLE).await()
        info.nodes.firstOrNull { it.isNearby }?.id ?: info.nodes.firstOrNull()?.id
    } catch (_: Exception) {
        null
    }

    private suspend fun readStatus(node: String): WearStatus? = try {
        val uri = Uri.Builder().scheme(PutDataRequest.WEAR_URI_SCHEME).authority(node).path(WearLink.PATH_STATUS).build()
        val buf = data.getDataItems(uri).await()
        try {
            buf.firstOrNull()?.let { WearStatus.decode(it.data) }
        } finally {
            buf.release()
        }
    } catch (_: Exception) {
        null
    }

    companion object {
        private const val REPLY_TIMEOUT_MS = 20_000L

        @Volatile
        private var instance: PhoneLink? = null

        fun get(context: Context): PhoneLink =
            instance ?: synchronized(this) { instance ?: PhoneLink(context).also { instance = it } }
    }
}
