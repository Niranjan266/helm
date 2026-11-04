package com.niranjan.helm.device

import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Bundle
import android.view.Surface
import com.niranjan.helm.core.PhoneFrames
import com.niranjan.helm.core.PhoneSize
import com.niranjan.helm.core.QueuedAu
import java.io.IOException
import kotlin.concurrent.thread

/**
 * H.264 from a Surface (the MediaProjection virtual display draws into it): low latency, no
 * B-frames, keyframes on demand. Access units leave in Annex-B form; keyframes carry SPS / PPS.
 */
class PhoneEncoder private constructor(
    private val codec: MediaCodec,
    val surface: Surface,
    val size: PhoneSize,
    private val onUnit: (QueuedAu) -> Unit,
    private val onFailed: () -> Unit,
) {
    @Volatile
    private var running = true
    private val worker = thread(name = "helm-phone-encoder", isDaemon = true) { drain() }

    fun requestKeyframe() {
        if (!running) return
        runCatching {
            codec.setParameters(Bundle().apply { putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0) })
        }
    }

    private fun drain() {
        val info = MediaCodec.BufferInfo()
        var config: ByteArray? = null
        try {
            while (running) {
                val i = codec.dequeueOutputBuffer(info, 100_000)
                if (i < 0) continue
                val buffer = codec.getOutputBuffer(i)
                if (buffer != null && info.size > 0) {
                    buffer.position(info.offset)
                    buffer.limit(info.offset + info.size)
                    val bytes = ByteArray(info.size).also { buffer.get(it) }
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) {
                        config = bytes
                    } else {
                        val key = info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0
                        val unit = if (key) PhoneFrames.keyUnit(config, bytes) else bytes
                        onUnit(QueuedAu(info.presentationTimeUs, unit, key))
                    }
                }
                codec.releaseOutputBuffer(i, false)
            }
        } catch (_: IllegalStateException) {
            // The codec failed, or was stopped under us by release().
            if (running) onFailed()
        }
    }

    /** Stops the encoder and frees the Surface. */
    fun release() {
        running = false
        runCatching { worker.join(500) }
        runCatching { codec.stop() }
        runCatching { codec.release() }
        runCatching { surface.release() }
    }

    companion object {
        /** Null when no encoder takes this size (the caller retries with an aligned one). */
        fun create(
            size: PhoneSize,
            fps: Int,
            bitrateKbps: Int,
            onUnit: (QueuedAu) -> Unit,
            onFailed: () -> Unit,
        ): PhoneEncoder? {
            val format = MediaFormat.createVideoFormat(MediaFormat.MIMETYPE_VIDEO_AVC, size.width, size.height).apply {
                setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
                setInteger(MediaFormat.KEY_BIT_RATE, bitrateKbps * 1000)
                setInteger(MediaFormat.KEY_FRAME_RATE, fps)
                setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, 10)
                // A still screen produces no frames otherwise, and a viewer who joins sees nothing.
                setLong(MediaFormat.KEY_REPEAT_PREVIOUS_FRAME_AFTER, 200_000)
                // Baseline has no B-frames at all; the explicit keys say the same to newer codecs.
                setInteger(MediaFormat.KEY_PROFILE, MediaCodecInfo.CodecProfileLevel.AVCProfileBaseline)
                setInteger(MediaFormat.KEY_PRIORITY, 0)
                setInteger(MediaFormat.KEY_LATENCY, 1)
                if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
            }
            var codec: MediaCodec? = null
            return try {
                codec = MediaCodec.createEncoderByType(MediaFormat.MIMETYPE_VIDEO_AVC)
                codec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
                val surface = codec.createInputSurface()
                codec.start()
                PhoneEncoder(codec, surface, size, onUnit, onFailed)
            } catch (_: IOException) {
                runCatching { codec?.release() }
                null
            } catch (_: IllegalArgumentException) {
                runCatching { codec?.release() }
                null
            } catch (_: IllegalStateException) {
                runCatching { codec?.release() }
                null
            }
        }
    }
}
