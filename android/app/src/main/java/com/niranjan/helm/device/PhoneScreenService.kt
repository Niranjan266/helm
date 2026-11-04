package com.niranjan.helm.device

import android.app.KeyguardManager
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.graphics.Point
import android.graphics.drawable.Icon
import android.hardware.display.DisplayManager
import android.hardware.display.VirtualDisplay
import android.media.projection.MediaProjection
import android.media.projection.MediaProjectionManager
import android.os.Build
import android.os.Handler
import android.os.IBinder
import android.os.Looper
import android.os.SystemClock
import android.util.DisplayMetrics
import android.view.Display
import android.view.Surface
import com.niranjan.helm.core.ConnectionState
import com.niranjan.helm.core.HelmException
import com.niranjan.helm.core.IdleWatchdog
import com.niranjan.helm.core.KeyframeLimiter
import com.niranjan.helm.core.PhoneControl
import com.niranjan.helm.core.PhoneLink
import com.niranjan.helm.core.PhoneRequest
import com.niranjan.helm.core.PhoneScale
import com.niranjan.helm.core.PhoneSendQueue
import com.niranjan.helm.core.PhoneSize
import com.niranjan.helm.core.PhoneStage
import com.niranjan.helm.core.PhoneStateChange
import kotlin.concurrent.thread
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * The foreground service (type `mediaProjection`) that owns one session: the projection, the
 * encoder, the socket and the Stop notification. Started once per accepted request with that
 * request's Android consent result, which is never kept. Ends itself on Stop, screen off or lock,
 * disconnect, the idle timeout, and whenever Android ends the projection.
 */
class PhoneScreenService : Service() {
    private val main = Handler(Looper.getMainLooper())
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var projection: MediaProjection? = null
    private var display: VirtualDisplay? = null
    private var encoder: PhoneEncoder? = null
    private var link: PhoneLink? = null
    private var queue: PhoneSendQueue? = null
    private var request: PhoneRequest? = null
    private val keyframes = KeyframeLimiter()
    private var idle: IdleWatchdog? = null

    @Volatile
    private var ended = false

    private val screenOff = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            if (intent.action == Intent.ACTION_SCREEN_OFF) finishSession(PhoneControl.REASON_LOCKED, tellPc = true)
        }
    }

    private val displayListener = object : DisplayManager.DisplayListener {
        override fun onDisplayAdded(displayId: Int) = Unit
        override fun onDisplayRemoved(displayId: Int) = Unit
        override fun onDisplayChanged(displayId: Int) {
            if (displayId == Display.DEFAULT_DISPLAY) followRotation()
        }
    }

    private val projectionEnded = object : MediaProjection.Callback() {
        override fun onStop() = finishSession(PhoneControl.REASON_CAPTURE_STOPPED, tellPc = true)
    }

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_START -> begin(intent)
            ACTION_STOP -> finishSession(PhoneControl.REASON_STOPPED, tellPc = true)
            else -> stopSelf()
        }
        return START_NOT_STICKY
    }

    private fun begin(intent: Intent) {
        val stage = PhoneBridge.stage.value as? PhoneStage.Capturing
        if (stage == null || encoder != null) {
            // The request went away meanwhile. Android still insists on startForeground after
            // startForegroundService, even to leave at once.
            if (encoder == null) {
                runCatching { goForeground(buildNotification(this, PhoneRequest(0, "PC", false, 0, 0, 0, 0), control = false, starting = true)) }
                runCatching { stopForeground(STOP_FOREGROUND_REMOVE) }
            }
            stopSelf()
            return
        }
        val req = stage.request
        request = req
        current = this
        // Android requires the foreground service before it hands out the projection.
        try {
            goForeground(buildNotification(this, req, control = false, starting = true))
        } catch (_: Exception) {
            failToStart(req)
            return
        }
        val code = intent.getIntExtra(EXTRA_CODE, 0)
        @Suppress("DEPRECATION")
        val data = intent.getParcelableExtra<Intent>(EXTRA_DATA)
        val mp = data?.let {
            runCatching { getSystemService(MediaProjectionManager::class.java).getMediaProjection(code, it) }.getOrNull()
        }
        if (mp == null) {
            failToStart(req)
            return
        }
        projection = mp
        mp.registerCallback(projectionEnded, main)
        val q = PhoneSendQueue()
        queue = q
        val (w, h, dpi) = realMetrics()
        val enc = createEncoder(PhoneScale.fit(w, h, req.maxLong, req.maxShort), req, q)
        if (enc == null) {
            failToStart(req)
            return
        }
        encoder = enc
        display = try {
            mp.createVirtualDisplay(
                "helm-phone",
                enc.size.width,
                enc.size.height,
                dpi,
                DisplayManager.VIRTUAL_DISPLAY_FLAG_AUTO_MIRROR,
                enc.surface,
                null,
                main,
            )
        } catch (_: RuntimeException) {
            null
        }
        if (display == null) {
            failToStart(req)
            return
        }
        registerReceiver(screenOff, IntentFilter(Intent.ACTION_SCREEN_OFF))
        getSystemService(DisplayManager::class.java).registerDisplayListener(displayListener, main)
        goLive(req, stage.wantControl, enc.size, q)
    }

    private fun goForeground(n: Notification) {
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(NOTIFICATION_ID, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PROJECTION)
        } else {
            startForeground(NOTIFICATION_ID, n)
        }
    }

    private fun failToStart(req: PhoneRequest) {
        PhoneBridge.onCaptureFailed(req.requestId)
        release()
    }

    private fun createEncoder(wanted: PhoneSize, req: PhoneRequest, q: PhoneSendQueue): PhoneEncoder? {
        val make = { s: PhoneSize ->
            PhoneEncoder.create(
                s,
                req.fps,
                req.bitrateKbps,
                onUnit = { au -> q.push(au) },
                onFailed = { main.post { finishSession(PhoneControl.REASON_ENCODER, tellPc = true) } },
            )
        }
        return make(wanted) ?: make(PhoneScale.align16(wanted))
    }

    private fun goLive(req: PhoneRequest, wantControl: Boolean, size: PhoneSize, q: PhoneSendQueue) {
        val conn = PhoneBridge.connection()
        if (conn == null) {
            failToStart(req)
            return
        }
        val l = PhoneLink(conn) { PhoneBridge.streamSocket() }
        link = l
        scope.launch {
            val control = wantControl && PhoneAccessService.running
            val sc = try {
                l.open(req.requestId, size, rotationDegrees(), control)
            } catch (_: HelmException) {
                failToStart(req)
                return@launch
            }
            if (!PhoneBridge.onLive(req.requestId)) {
                // Cancelled while coming up.
                release()
                return@launch
            }
            updateNotification()
            encoder?.requestKeyframe()
            startSender(l, q)
            val watchdog = IdleWatchdog(PhoneControl.IDLE_TIMEOUT_MS, SystemClock.elapsedRealtime())
            idle = watchdog
            launch {
                l.input(sc).collect { ev ->
                    if (PhoneBridge.perform(ev)) watchdog.touch(SystemClock.elapsedRealtime())
                }
            }
            launch {
                sc.state.first { it is ConnectionState.Failed || it is ConnectionState.Closed }
                finishSession(REASON_DISCONNECTED, tellPc = false)
            }
            launch {
                while (isActive) {
                    delay(2_000)
                    if (keyguardLocked()) {
                        finishSession(PhoneControl.REASON_LOCKED, tellPc = true)
                    } else if (watchdog.expired(SystemClock.elapsedRealtime())) {
                        finishSession(PhoneControl.REASON_IDLE, tellPc = true)
                    }
                }
            }
        }
    }

    /** Socket writer: if the network is slower than the encoder, the queue drops deltas. */
    private fun startSender(l: PhoneLink, q: PhoneSendQueue) {
        thread(name = "helm-phone-sender", isDaemon = true) {
            while (true) {
                val popped = q.take() ?: break
                if (popped.requestKeyframe) requestKeyframe()
                if (!l.send(popped.au)) break
                while (l.queuedBytes > MAX_SOCKET_BACKLOG && !ended) Thread.sleep(2)
            }
        }
    }

    fun requestKeyframe() {
        if (keyframes.allow(SystemClock.elapsedRealtime())) encoder?.requestKeyframe()
    }

    private fun keyguardLocked(): Boolean = getSystemService(KeyguardManager::class.java)?.isKeyguardLocked == true

    private fun realMetrics(): Triple<Int, Int, Int> {
        val d = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        val size = Point()
        val metrics = DisplayMetrics()
        @Suppress("DEPRECATION")
        d.getRealSize(size)
        @Suppress("DEPRECATION")
        d.getRealMetrics(metrics)
        return Triple(size.x, size.y, metrics.densityDpi)
    }

    private fun rotationDegrees(): Int {
        val d = getSystemService(DisplayManager::class.java).getDisplay(Display.DEFAULT_DISPLAY)
        return when (d.rotation) {
            Surface.ROTATION_90 -> 90
            Surface.ROTATION_180 -> 180
            Surface.ROTATION_270 -> 270
            else -> 0
        }
    }

    /**
     * Android lets one projection have one virtual display, so a rotation keeps it and swaps the
     * size and the surface. The new encoder's first frame is a keyframe with a new SPS, which
     * tells the PC the picture's new shape.
     */
    private fun followRotation() {
        val req = request ?: return
        val old = encoder ?: return
        val vd = display ?: return
        val q = queue ?: return
        val (w, h, dpi) = realMetrics()
        val wanted = PhoneScale.fit(w, h, req.maxLong, req.maxShort)
        val rotation = rotationDegrees()
        val sameShape = (wanted.width > wanted.height) == (old.size.width > old.size.height)
        if (sameShape && (wanted == old.size || PhoneScale.align16(wanted) == old.size)) {
            PhoneBridge.sendState(req.requestId, PhoneStateChange(rotation = rotation))
            return
        }
        val next = createEncoder(wanted, req, q)
        if (next == null) {
            finishSession(PhoneControl.REASON_ENCODER, tellPc = true)
            return
        }
        vd.surface = null
        vd.resize(next.size.width, next.size.height, dpi)
        vd.surface = next.surface
        encoder = next
        old.release()
        next.requestKeyframe()
        PhoneBridge.sendState(
            req.requestId,
            PhoneStateChange(width = next.size.width, height = next.size.height, rotation = rotation),
        )
    }

    fun updateNotification() {
        val req = request ?: return
        val control = (PhoneBridge.stage.value as? PhoneStage.Live)?.control == true
        val n = buildNotification(this, req, control, starting = false)
        runCatching { getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, n) }
        PhoneAccessService.instance?.showIndicator(req.pcName, control)
    }

    /** The one way out. Safe to call more than once. */
    fun finishSession(reason: String, tellPc: Boolean) {
        if (ended) return
        ended = true
        request?.let { PhoneBridge.onSessionEnded(it.requestId, reason, tellPc) }
        release()
    }

    private fun release() {
        ended = true
        if (current === this) current = null
        PhoneAccessService.instance?.hideIndicator()
        runCatching { unregisterReceiver(screenOff) }
        runCatching { getSystemService(DisplayManager::class.java).unregisterDisplayListener(displayListener) }
        queue?.close()
        queue = null
        link?.close()
        link = null
        runCatching { display?.release() }
        display = null
        encoder?.release()
        encoder = null
        runCatching { projection?.unregisterCallback(projectionEnded) }
        runCatching { projection?.stop() }
        projection = null
        scope.cancel()
        runCatching { stopForeground(STOP_FOREGROUND_REMOVE) }
        stopSelf()
    }

    override fun onDestroy() {
        finishSession(PhoneControl.REASON_STOPPED, tellPc = true)
        super.onDestroy()
    }

    companion object {
        const val CHANNEL = "phone_session"
        const val REASON_DISCONNECTED = "disconnected"
        private const val NOTIFICATION_ID = 4795
        private const val ACTION_START = "com.niranjan.helm.PHONE_START"
        private const val ACTION_STOP = "com.niranjan.helm.PHONE_STOP"
        private const val EXTRA_CODE = "code"
        private const val EXTRA_DATA = "data"
        private const val MAX_SOCKET_BACKLOG = 1024L * 1024L

        @Volatile
        var current: PhoneScreenService? = null
            private set

        /** Hands this request's Android consent to the service. Called once per accepted prompt. */
        fun start(context: Context, resultCode: Int, data: Intent) {
            val i = Intent(context, PhoneScreenService::class.java)
                .setAction(ACTION_START)
                .putExtra(EXTRA_CODE, resultCode)
                .putExtra(EXTRA_DATA, data)
            context.startForegroundService(i)
        }

        private fun buildNotification(context: Context, req: PhoneRequest, control: Boolean, starting: Boolean): Notification {
            val nm = context.getSystemService(NotificationManager::class.java)
            nm.createNotificationChannel(NotificationChannel(CHANNEL, "PC viewing this phone", NotificationManager.IMPORTANCE_DEFAULT))
            val stop = PendingIntent.getService(
                context,
                1,
                Intent(context, PhoneScreenService::class.java).setAction(ACTION_STOP),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
            val title = when {
                starting -> "Starting…"
                control -> "${req.pcName} is controlling this phone"
                else -> "${req.pcName} can see this phone"
            }
            val b = Notification.Builder(context, CHANNEL)
                .setSmallIcon(android.R.drawable.ic_menu_view)
                .setContentTitle(title)
                .setContentText(
                    if (control) {
                        "PC is controlling. It can see the screen, tap and type. Tap Stop to end it."
                    } else {
                        "View only. It can see everything on the screen. Tap Stop to end it."
                    },
                )
                .setOngoing(true)
                .setShowWhen(true)
                .setUsesChronometer(true)
                .addAction(
                    Notification.Action.Builder(
                        Icon.createWithResource(context, android.R.drawable.ic_menu_close_clear_cancel),
                        "Stop",
                        stop,
                    ).build(),
                )
            if (Build.VERSION.SDK_INT >= 31) b.setForegroundServiceBehavior(Notification.FOREGROUND_SERVICE_IMMEDIATE)
            return b.build()
        }
    }
}
