package com.niranjan.helm.device

import android.Manifest
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraManager
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.MediaPlayer
import android.media.RingtoneManager
import android.media.ToneGenerator
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.VibrationEffect
import android.os.Vibrator
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import com.niranjan.helm.core.AlarmVolume
import com.niranjan.helm.core.AlarmVolumeGuard
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** The alarm stream of the phone. */
private class AndroidAlarmVolume(private val am: AudioManager) : AlarmVolume {
    override val current: Int get() = am.getStreamVolume(AudioManager.STREAM_ALARM)
    override val max: Int get() = am.getStreamMaxVolume(AudioManager.STREAM_ALARM)
    override fun set(level: Int) {
        try {
            am.setStreamVolume(AudioManager.STREAM_ALARM, level, 0)
        } catch (_: SecurityException) {
            // Do Not Disturb policy can refuse; the alarm still plays at the current level.
        }
    }
}

/**
 * "Find my phone" (everyday §3): loud looping sound on the alarm stream at full volume (also on
 * silent / vibrate), vibration, a blinking torch and a full-screen [RingActivity] until the user
 * taps **Found it** or the PC sends `stop`. Afterwards the alarm volume is put back.
 */
object PhoneRinger {
    const val CHANNEL = "find_phone"
    private const val NOTIFICATION_ID = 4790
    const val ACTION_STOP = "com.niranjan.helm.RING_STOP"

    private val _ringing = MutableStateFlow<String?>(null)

    /** Name of who is ringing us, or null when quiet. */
    val ringing: StateFlow<String?> = _ringing.asStateFlow()

    private val main = Handler(Looper.getMainLooper())
    private var guard: AlarmVolumeGuard? = null
    private var player: MediaPlayer? = null
    private var tone: ToneGenerator? = null
    private var vibrator: Vibrator? = null
    private var torchId: String? = null
    private var torchOn = false
    private var torchTask: Runnable? = null

    private val alarmAttrs: AudioAttributes = AudioAttributes.Builder()
        .setUsage(AudioAttributes.USAGE_ALARM)
        .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
        .build()

    fun start(context: Context, from: String) {
        val app = context.applicationContext
        main.post {
            val wasRinging = _ringing.value != null
            _ringing.value = from
            if (!wasRinging) {
                val am = app.getSystemService(AudioManager::class.java)
                guard = am?.let { AlarmVolumeGuard(AndroidAlarmVolume(it)) }?.also { it.raise() }
                startSound(app)
                startVibration(app)
                startTorch(app)
            }
            notifyRinging(app, from)
            // Works when the app is in front; otherwise the full-screen intent opens it.
            try {
                app.startActivity(RingActivity.intent(app))
            } catch (_: Exception) {
            }
        }
    }

    fun stop(context: Context) {
        val app = context.applicationContext
        main.post {
            if (_ringing.value == null) return@post
            _ringing.value = null
            runCatching { player?.stop() }
            runCatching { player?.release() }
            player = null
            runCatching { tone?.stopTone() }
            runCatching { tone?.release() }
            tone = null
            runCatching { vibrator?.cancel() }
            vibrator = null
            stopTorch(app)
            guard?.restore()
            guard = null
            runCatching { NotificationManagerCompat.from(app).cancel(NOTIFICATION_ID) }
        }
    }

    private fun startSound(context: Context) {
        val uri = RingtoneManager.getActualDefaultRingtoneUri(context, RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_ALARM)
            ?: RingtoneManager.getDefaultUri(RingtoneManager.TYPE_RINGTONE)
        player = try {
            MediaPlayer().apply {
                setAudioAttributes(alarmAttrs)
                setDataSource(context, uri ?: throw IllegalStateException("no alarm sound"))
                isLooping = true
                prepare()
                start()
            }
        } catch (_: Exception) {
            null
        }
        if (player == null) {
            tone = runCatching {
                ToneGenerator(AudioManager.STREAM_ALARM, ToneGenerator.MAX_VOLUME).also {
                    it.startTone(ToneGenerator.TONE_CDMA_EMERGENCY_RINGBACK)
                }
            }.getOrNull()
        }
    }

    @Suppress("DEPRECATION")
    private fun startVibration(context: Context) {
        val v = context.getSystemService(Vibrator::class.java) ?: return
        if (!v.hasVibrator()) return
        vibrator = v
        runCatching { v.vibrate(VibrationEffect.createWaveform(longArrayOf(0, 800, 400), 0), alarmAttrs) }
    }

    private fun startTorch(context: Context) {
        val cm = context.getSystemService(CameraManager::class.java) ?: return
        val id = runCatching {
            cm.cameraIdList.firstOrNull { cm.getCameraCharacteristics(it).get(CameraCharacteristics.FLASH_INFO_AVAILABLE) == true }
        }.getOrNull() ?: return
        torchId = id
        val task = object : Runnable {
            override fun run() {
                torchOn = !torchOn
                // setTorchMode needs no CAMERA permission; it fails if the camera is in use.
                if (runCatching { cm.setTorchMode(id, torchOn) }.isFailure) return
                main.postDelayed(this, 350)
            }
        }
        torchTask = task
        main.post(task)
    }

    private fun stopTorch(context: Context) {
        torchTask?.let { main.removeCallbacks(it) }
        torchTask = null
        val id = torchId ?: return
        torchId = null
        torchOn = false
        runCatching { context.getSystemService(CameraManager::class.java)?.setTorchMode(id, false) }
    }

    fun createChannel(context: Context) {
        if (Build.VERSION.SDK_INT < 26) return
        val ch = NotificationChannel(CHANNEL, "Find my phone", NotificationManager.IMPORTANCE_HIGH).apply {
            setSound(null, null) // the ringer plays its own alarm sound
            enableVibration(false)
            setBypassDnd(true)
        }
        context.getSystemService(NotificationManager::class.java)?.createNotificationChannel(ch)
    }

    /** Android 14+: whether full-screen alarms may open over the lock screen. */
    fun canUseFullScreen(context: Context): Boolean =
        Build.VERSION.SDK_INT < 34 || context.getSystemService(NotificationManager::class.java)?.canUseFullScreenIntent() != false

    private fun notifyRinging(context: Context, from: String) {
        if (Build.VERSION.SDK_INT >= 33 &&
            ContextCompat.checkSelfPermission(context, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) return
        createChannel(context)
        val flags = PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        val full = PendingIntent.getActivity(context, 0, RingActivity.intent(context), flags)
        val stop = PendingIntent.getBroadcast(context, 0, Intent(context, RingStopReceiver::class.java).setAction(ACTION_STOP), flags)
        val n = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
            .setContentTitle("Find my phone")
            .setContentText("$from is ringing this phone")
            .setPriority(NotificationCompat.PRIORITY_MAX)
            .setCategory(NotificationCompat.CATEGORY_ALARM)
            .setOngoing(true)
            .setFullScreenIntent(full, true)
            .setContentIntent(full)
            .addAction(0, "Found it", stop)
            .build()
        runCatching { NotificationManagerCompat.from(context).notify(NOTIFICATION_ID, n) }
    }
}

/** "Found it" on the ringing notification. */
class RingStopReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action == PhoneRinger.ACTION_STOP) PhoneRinger.stop(context)
    }
}
