package com.niranjan.helm.device

import android.media.AudioAttributes
import android.media.AudioFormat
import android.media.AudioTrack
import android.media.MediaCodec
import android.media.MediaFormat
import android.util.Log
import com.niranjan.helm.core.OpusConfig
import com.niranjan.helm.core.StreamFrame
import java.nio.ByteBuffer
import java.util.concurrent.LinkedBlockingDeque
import java.util.concurrent.TimeUnit

/**
 * Opus (48 kHz stereo, 20 ms packets) -> MediaCodec `audio/opus` -> low-latency [AudioTrack].
 * Keeps at most ~[MAX_LATENCY_MS] of audio buffered; older packets are dropped so the sound
 * stays in step with the picture. [submit] never blocks.
 */
class AudioPlayer {
    private val queue = LinkedBlockingDeque<StreamFrame.Audio>()

    @Volatile
    private var running = true

    @Volatile
    var enabled = true
        set(value) {
            field = value
            if (!value) queue.clear()
        }

    private var codec: MediaCodec? = null
    private var track: AudioTrack? = null
    private val info = MediaCodec.BufferInfo()
    private var framesWritten = 0L
    private var failed = false
    private var paused = false

    private val thread = Thread(::loop, "helm-audio").apply {
        priority = Thread.MAX_PRIORITY - 1
        isDaemon = true
        start()
    }

    fun submit(frame: StreamFrame.Audio) {
        if (!running || !enabled || failed) return
        while (queue.size >= MAX_QUEUE) queue.pollFirst()
        queue.offer(frame)
    }

    fun release() {
        running = false
        thread.interrupt()
    }

    private fun loop() {
        try {
            while (running) {
                val f = try {
                    queue.poll(20, TimeUnit.MILLISECONDS)
                } catch (_: InterruptedException) {
                    null
                }
                if (!enabled) {
                    if (!paused) {
                        track?.pause()
                        track?.flush()
                        paused = true
                    }
                    framesWritten = 0
                    continue
                }
                if (f == null) {
                    drain()
                    continue
                }
                if (codec == null && !setUp()) return
                if (paused) {
                    track?.play()
                    paused = false
                }
                feed(f)
                drain()
            }
        } finally {
            tearDown()
        }
    }

    private fun setUp(): Boolean = try {
        val format = MediaFormat.createAudioFormat(MediaFormat.MIMETYPE_AUDIO_OPUS, OpusConfig.SAMPLE_RATE, OpusConfig.CHANNELS).apply {
            setByteBuffer("csd-0", ByteBuffer.wrap(OpusConfig.opusHead()))
            setByteBuffer("csd-1", ByteBuffer.wrap(OpusConfig.nanosLe(OpusConfig.preSkipNs())))
            setByteBuffer("csd-2", ByteBuffer.wrap(OpusConfig.nanosLe(OpusConfig.SEEK_PREROLL_NS)))
        }
        val c = MediaCodec.createDecoderByType(MediaFormat.MIMETYPE_AUDIO_OPUS)
        c.configure(format, null, null, 0)
        c.start()
        codec = c
        val min = AudioTrack.getMinBufferSize(OpusConfig.SAMPLE_RATE, AudioFormat.CHANNEL_OUT_STEREO, AudioFormat.ENCODING_PCM_16BIT)
        track = AudioTrack.Builder()
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE)
                    .build(),
            )
            .setAudioFormat(
                AudioFormat.Builder()
                    .setSampleRate(OpusConfig.SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO)
                    .setEncoding(AudioFormat.ENCODING_PCM_16BIT)
                    .build(),
            )
            .setBufferSizeInBytes(maxOf(min, BYTES_PER_MS * 40))
            .setTransferMode(AudioTrack.MODE_STREAM)
            .setPerformanceMode(AudioTrack.PERFORMANCE_MODE_LOW_LATENCY)
            .build()
            .also { it.play() }
        framesWritten = 0
        paused = false
        true
    } catch (e: Exception) {
        Log.w(TAG, "Opus decoder unavailable; audio off", e)
        failed = true
        tearDown()
        false
    }

    private fun feed(f: StreamFrame.Audio) {
        val c = codec ?: return
        try {
            val idx = c.dequeueInputBuffer(5_000)
            if (idx < 0) return
            val buf = c.getInputBuffer(idx) ?: return
            buf.clear()
            if (f.data.size > buf.capacity()) {
                c.queueInputBuffer(idx, 0, 0, f.ptsUs, 0)
                return
            }
            buf.put(f.data)
            c.queueInputBuffer(idx, 0, f.data.size, f.ptsUs, 0)
        } catch (e: Exception) {
            restart(e)
        }
    }

    private fun drain() {
        val c = codec ?: return
        val t = track ?: return
        try {
            while (true) {
                val idx = c.dequeueOutputBuffer(info, 0)
                if (idx < 0) break
                val out = c.getOutputBuffer(idx)
                if (out != null && info.size > 0) {
                    // Skip this chunk if the track already holds too much (keeps latency bounded).
                    val buffered = framesWritten - t.playbackHeadPosition.toLong()
                    if (buffered * 1000 / OpusConfig.SAMPLE_RATE <= MAX_LATENCY_MS) {
                        out.position(info.offset)
                        out.limit(info.offset + info.size)
                        val n = t.write(out, info.size, AudioTrack.WRITE_NON_BLOCKING)
                        if (n > 0) framesWritten += n / (2 * OpusConfig.CHANNELS)
                    }
                }
                c.releaseOutputBuffer(idx, false)
            }
        } catch (e: Exception) {
            restart(e)
        }
    }

    private fun restart(e: Exception) {
        Log.w(TAG, "audio error; restarting", e)
        tearDown()
    }

    private fun tearDown() {
        codec?.let {
            runCatching { it.stop() }
            runCatching { it.release() }
        }
        codec = null
        track?.let {
            runCatching { it.pause() }
            runCatching { it.flush() }
            runCatching { it.release() }
        }
        track = null
    }

    companion object {
        private const val TAG = "HelmAudio"
        private const val MAX_QUEUE = 10 // 200 ms of packets
        private const val MAX_LATENCY_MS = 120
        private const val BYTES_PER_MS = OpusConfig.SAMPLE_RATE / 1000 * 2 * OpusConfig.CHANNELS
    }
}
