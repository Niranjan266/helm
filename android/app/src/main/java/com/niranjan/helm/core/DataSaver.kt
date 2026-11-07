package com.niranjan.helm.core

/** Why the stream is in data-saver mode. */
enum class SaverReason { RELAY, MOBILE_DATA, MANUAL }

/** The user's per-session choice: follow the network (AUTO), or force saver on / off. */
enum class SaverOverride { AUTO, ON, OFF }

/**
 * Data saver for the screen stream: on mobile data (when the setting is on) or over the relay the
 * stream uses the low relay preset and bitrate ([RelayStreamPolicy]); on Wi-Fi / USB it uses the
 * normal settings again. A per-session override wins over both.
 */
object DataSaver {
    /** The reason saver is active, or null when the stream should use the normal settings. */
    fun reason(override: SaverOverride, onMobileDataSetting: Boolean, onMobileData: Boolean, viaRelay: Boolean): SaverReason? =
        when (override) {
            SaverOverride.OFF -> null
            SaverOverride.ON -> when {
                viaRelay -> SaverReason.RELAY
                onMobileData -> SaverReason.MOBILE_DATA
                else -> SaverReason.MANUAL
            }
            SaverOverride.AUTO -> when {
                viaRelay -> SaverReason.RELAY
                onMobileDataSetting && onMobileData -> SaverReason.MOBILE_DATA
                else -> null
            }
        }

    /** The override that flips saver from its current state, for this session. */
    fun toggled(active: Boolean): SaverOverride = if (active) SaverOverride.OFF else SaverOverride.ON

    /** Preset + bitrate for the stream; saver picks ([pick], [pickKbps]) only count while saver is on. */
    fun choose(
        reason: SaverReason?,
        perPc: String?,
        global: String?,
        globalKbps: Int,
        pick: StreamPreset? = null,
        pickKbps: Int = 0,
    ): StreamChoice = RelayStreamPolicy.choose(reason != null, perPc, global, globalKbps, pick, pickKbps)

    fun notice(reason: SaverReason): String = when (reason) {
        SaverReason.RELAY -> RelayStreamPolicy.NOTICE
        SaverReason.MOBILE_DATA -> "Data saver (mobile data): 720p30 at 2 Mbps. Change it in the controls."
        SaverReason.MANUAL -> "Data saver: 720p30 at 2 Mbps. Change it in the controls."
    }
}

/** Connection quality from round-trip time and, while streaming, dropped / decoded frames. */
enum class LinkQuality(val label: String) {
    GOOD("Good"),
    OK("OK"),
    WEAK("Weak"),
    ;

    private fun worse(): LinkQuality = entries[minOf(ordinal + 1, entries.size - 1)]

    companion object {
        const val GOOD_MS = 80L
        const val OK_MS = 200L

        /**
         * [rttMs] null or <= 0 = unknown. [fps] = decoded frames per second, [dropped] = dropped per
         * second (both 0 when not streaming). Null when there is nothing to rate.
         */
        fun of(rttMs: Long?, dropped: Int = 0, fps: Int = 0): LinkQuality? {
            val rtt = rttMs?.takeIf { it > 0 }
            if (rtt == null && fps <= 0 && dropped <= 0) return null
            var q = when {
                rtt == null -> GOOD
                rtt <= GOOD_MS -> GOOD
                rtt <= OK_MS -> OK
                else -> WEAK
            }
            val total = fps + dropped
            if (dropped > 0 && total > 0) {
                val share = dropped.toFloat() / total
                q = when {
                    share >= 0.3f -> WEAK
                    share >= 0.1f -> q.worse()
                    else -> q
                }
            }
            return q
        }

        /** "Good 42 ms", "Weak", or null when nothing is known. */
        fun statusText(rttMs: Long?): String? {
            val q = of(rttMs) ?: return null
            return "${q.label} ${rttMs} ms"
        }

        /** Stream line for the Screen tab: "30 fps · 8.2 Mbps". */
        fun streamText(fps: Int, rxKbps: Int): String = "$fps fps · ${"%.1f".format(java.util.Locale.ROOT, rxKbps / 1000f)} Mbps"
    }
}
