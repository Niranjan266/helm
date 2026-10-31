package com.niranjan.helm.device

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.util.Log
import android.view.Surface
import com.niranjan.helm.core.DecoderConfigTracker
import com.niranjan.helm.core.StreamFrame
import com.niranjan.helm.core.StreamStatsCounter
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.TimeUnit

/**
 * H.264 decoder that renders straight to a [Surface] (PROTOCOL.md §10, ROADMAP Phase 2):
 * - configures on the first keyframe and again whenever a keyframe carries a new SPS/PPS;
 * - asks for low-latency decoding where the codec supports it (API 30+);
 * - renders every output buffer immediately, and when several are ready only the newest one
 *   (older ones are dropped as late);
 * - if frames back up on the phone, flushes the backlog and waits for a keyframe (IDR request).
 *
 * [submit] is called from the socket reader and never blocks.
 */
class VideoDecoder(
    private val stats: StreamStatsCounter,
    private val requestKeyframe: () -> Unit,
    private val onVideoSize: (width: Int, height: Int) -> Unit,
) {
    private val queue = LinkedBlockingDeque<StreamFrame.Video>()
    private val tracker = DecoderConfigTracker()

    @Volatile
    private var running = true

    @Volatile
    private var surface: Surface? = null

    @Volatile
    private var surfaceChanged = false

    @Volatile
    private var waitForKey = true
    private var lastIdrRequestMs = 0L
    private var codec: MediaCodec? = null
    private var codecSurface: Surface? = null
    private val info = MediaCodec.BufferInfo()

    /** True when the codec reported low-latency support (for the stats overlay). */
    @Volatile
    var lowLatency = false
        private set

    @Volatile
    var codecName: String? = null
        private set

    private val thread = Thread(::loop, "helm-video").apply {
        priority = Thread.MAX_PRIORITY
        isDaemon = true
        start()
    }

    fun submit(frame: StreamFrame.Video) {
        if (!running) return
        if (queue.size >= MAX_BACKLOG) {
            // The decoder is falling behind: drop everything queued and restart from a keyframe.
            val n = queue.size
            queue.clear()
            stats.onDropped(n)
            waitForKey = true
            askForKeyframe(force = true)
        }
        queue.offer(frame)
    }

    /** Called from the UI thread when the SurfaceView's surface is created / destroyed. */
    fun setSurface(s: Surface?) {
        surface = s
        surfaceChanged = true
    }

    fun release() {
        running = false
        thread.interrupt()
    }

    private fun loop() {
        try {
            while (running) {
                if (surfaceChanged) applySurface()
                val f = try {
                    queue.poll(10, TimeUnit.MILLISECONDS)
                } catch (_: InterruptedException) {
                    null
                }
                drainOutput()
                if (f != null) handle(f)
                drainOutput()
            }
        } finally {
            releaseCodec()
        }
    }

    private fun handle(f: StreamFrame.Video) {
        if (surface == null) {
            stats.onDropped()
            return
        }
        when (tracker.onAccessUnit(f.data, f.key)) {
            DecoderConfigTracker.Action.CONFIGURE -> {
                if (!configure()) return
                waitForKey = false
                queueInput(f)
            }
            DecoderConfigTracker.Action.DECODE -> {
                if (codec == null || (waitForKey && !f.key)) {
                    stats.onDropped()
                    askForKeyframe(force = false)
                    return
                }
                waitForKey = false
                queueInput(f)
            }
            DecoderConfigTracker.Action.DROP_WAIT_KEY -> {
                stats.onDropped()
                askForKeyframe(force = false)
            }
        }
    }

    private fun askForKeyframe(force: Boolean) {
        val now = System.currentTimeMillis()
        if (force || now - lastIdrRequestMs > 1_000) {
            lastIdrRequestMs = now
            requestKeyframe()
        }
    }

    private fun applySurface() {
        surfaceChanged = false
        val s = surface
        val c = codec
        if (s == null) {
            releaseCodec()
            tracker.reset()
            waitForKey = true
            return
        }
        if (c != null && s !== codecSurface) {
            try {
                c.setOutputSurface(s) // API 23+, minSdk is 26
                codecSurface = s
                return
            } catch (e: Exception) {
                Log.w(TAG, "setOutputSurface failed; reconfiguring", e)
            }
            releaseCodec()
        }
        if (codec == null) {
            tracker.reset()
            waitForKey = true
            askForKeyframe(force = true)
        }
    }

    private fun configure(): Boolean {
        releaseCodec()
        val s = surface ?: return false
        val sps = tracker.info
        val w = sps?.width ?: 1920
        val h = sps?.height ?: 1080
        return try {
            val c = MediaCodec.createDecoderByType(MIME)
            val format = MediaFormat.createVideoFormat(MIME, w, h).apply {
                tracker.csd0()?.let { setByteBuffer("csd-0", java.nio.ByteBuffer.wrap(it)) }
                tracker.csd1()?.let { setByteBuffer("csd-1", java.nio.ByteBuffer.wrap(it)) }
                setInteger(MediaFormat.KEY_PRIORITY, 0) // realtime
                setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, maxOf(w * h, 512 * 1024))
            }
            var ll = false
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                val caps = c.codecInfo.getCapabilitiesForType(MIME)
                if (caps.isFeatureSupported(MediaCodecInfo.CodecCapabilities.FEATURE_LowLatency)) {
                    format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1)
                    ll = true
                }
            }
            c.configure(format, s, null, 0)
            c.start()
            codec = c
            codecSurface = s
            lowLatency = ll
            codecName = c.name
            onVideoSize(w, h)
            true
        } catch (e: Exception) {
            Log.w(TAG, "decoder configure failed", e)
            releaseCodec()
            tracker.reset()
            waitForKey = true
            false
        }
    }

    private fun queueInput(f: StreamFrame.Video) {
        val c = codec ?: return
        try {
            val idx = c.dequeueInputBuffer(INPUT_TIMEOUT_US)
            if (idx < 0) {
                // No room: this frame is late. Dropping a reference frame breaks the chain,
                // so resync from the next keyframe.
                stats.onDropped()
                waitForKey = true
                askForKeyframe(force = true)
                return
            }
            val buf = c.getInputBuffer(idx) ?: return
            buf.clear()
            if (f.data.size > buf.capacity()) {
                c.queueInputBuffer(idx, 0, 0, f.ptsUs, 0)
                stats.onDropped()
                waitForKey = true
                askForKeyframe(force = true)
                return
            }
            buf.put(f.data)
            c.queueInputBuffer(idx, 0, f.data.size, f.ptsUs, 0)
        } catch (e: Exception) {
            onCodecError(e)
        }
    }

    private fun drainOutput() {
        val c = codec ?: return
        try {
            var pending = -1
            while (true) {
                val idx = c.dequeueOutputBuffer(info, 0)
                when {
                    idx >= 0 -> {
                        if (pending >= 0) {
                            c.releaseOutputBuffer(pending, false)
                            stats.onDropped()
                        }
                        pending = idx
                    }
                    idx == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                        val f = c.outputFormat
                        val w = cropped(f, true)
                        val h = cropped(f, false)
                        if (w > 0 && h > 0) onVideoSize(w, h)
                    }
                    idx == MediaCodec.INFO_TRY_AGAIN_LATER -> break
                    else -> Unit
                }
            }
            if (pending >= 0) {
                c.releaseOutputBuffer(pending, true) // render now, no scheduling
                stats.onDecoded()
            }
        } catch (e: Exception) {
            onCodecError(e)
        }
    }

    private fun cropped(f: MediaFormat, width: Boolean): Int {
        val keyLo = if (width) "crop-left" else "crop-top"
        val keyHi = if (width) "crop-right" else "crop-bottom"
        return if (f.containsKey(keyLo) && f.containsKey(keyHi)) {
            f.getInteger(keyHi) - f.getInteger(keyLo) + 1
        } else {
            f.getInteger(if (width) MediaFormat.KEY_WIDTH else MediaFormat.KEY_HEIGHT)
        }
    }

    private fun onCodecError(e: Exception) {
        Log.w(TAG, "decoder error; waiting for a keyframe", e)
        releaseCodec()
        tracker.reset()
        waitForKey = true
        askForKeyframe(force = true)
    }

    private fun releaseCodec() {
        val c = codec ?: return
        codec = null
        codecSurface = null
        try {
            c.stop()
        } catch (_: Exception) {
        }
        try {
            c.release()
        } catch (_: Exception) {
        }
    }

    companion object {
        private const val TAG = "HelmVideo"
        private const val MIME = MediaFormat.MIMETYPE_VIDEO_AVC
        private const val MAX_BACKLOG = 6
        private const val INPUT_TIMEOUT_US = 8_000L
    }
}
