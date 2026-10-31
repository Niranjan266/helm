package com.niranjan.helm.wearlink

/**
 * Watch <-> phone messages over the Wearable Data Layer.
 *
 * The watch never talks to the PC. It sends a [WearCommand] to the phone (MessageClient,
 * path [WearLink.PATH_COMMAND]); the phone runs it over its own pinned-TLS connection and
 * answers with a [WearReply] (path [WearLink.PATH_REPLY]). The phone also publishes a
 * [WearStatus] as a DataItem at [WearLink.PATH_STATUS] whenever its connection state changes.
 *
 * Payloads are small UTF-8 text: a magic/version line, then one field per line. Field values
 * are escaped (`\\`, `\n`, `\r`) so they can never break the framing. Anything malformed
 * decodes to null and is dropped; there is no partial parse.
 */
object WearLink {
    const val PATH_COMMAND = "/helm/wear/cmd"
    const val PATH_REPLY = "/helm/wear/reply"
    const val PATH_STATUS = "/helm/wear/status"

    /** Capability the phone app advertises (res/values/wear.xml) so the watch can find it. */
    const val PHONE_CAPABILITY = "helm_phone"

    const val MAGIC = "helm-wear/1"

    /** Hard cap on any payload (the Data Layer allows ~100 KB; we need a few hundred bytes). */
    const val MAX_PAYLOAD = 4096
    const val MAX_FIELD = 200
    const val DECK_SLOTS = 4

    internal fun escape(s: String): String {
        val b = StringBuilder(s.length)
        for (c in s) when (c) {
            '\\' -> b.append("\\\\")
            '\n' -> b.append("\\n")
            '\r' -> b.append("\\r")
            else -> b.append(c)
        }
        return b.toString()
    }

    internal fun unescape(s: String): String? {
        val b = StringBuilder(s.length)
        var i = 0
        while (i < s.length) {
            val c = s[i]
            if (c != '\\') {
                b.append(c)
                i++
                continue
            }
            if (i + 1 >= s.length) return null
            when (s[i + 1]) {
                '\\' -> b.append('\\')
                'n' -> b.append('\n')
                'r' -> b.append('\r')
                else -> return null
            }
            i += 2
        }
        return b.toString()
    }

    internal fun frame(fields: List<String>): ByteArray =
        (listOf(MAGIC) + fields.map { escape(it.take(MAX_FIELD)) }).joinToString("\n").toByteArray(Charsets.UTF_8)

    /** Splits a payload into its unescaped fields (without the magic line), or null. */
    internal fun unframe(bytes: ByteArray?, fieldCount: Int): List<String>? {
        if (bytes == null || bytes.isEmpty() || bytes.size > MAX_PAYLOAD) return null
        val text = try {
            Charsets.UTF_8.newDecoder().decode(java.nio.ByteBuffer.wrap(bytes)).toString()
        } catch (_: java.nio.charset.CharacterCodingException) {
            return null
        }
        val lines = text.split('\n')
        if (lines.size != fieldCount + 1 || lines[0] != MAGIC) return null
        val out = ArrayList<String>(fieldCount)
        for (l in lines.drop(1)) {
            val v = unescape(l) ?: return null
            if (v.length > MAX_FIELD) return null
            out += v
        }
        return out
    }
}

/** What the watch can ask for. A closed set: the phone never runs anything else. */
enum class WearAction(val wire: String) {
    PLAY_PAUSE("play_pause"),
    NEXT("next"),
    PREVIOUS("previous"),
    VOLUME_UP("volume_up"),
    VOLUME_DOWN("volume_down"),
    MUTE("mute"),
    LOCK("lock"),

    /** Run deck slot 0..3 (the first four deck buttons on the PC). */
    DECK("deck"),

    /** Ask the phone to re-publish [WearStatus]. */
    STATUS("status");

    companion object {
        fun fromWire(s: String): WearAction? = entries.firstOrNull { it.wire == s }
    }
}

/** Watch -> phone. [id] pairs the reply with the request; [slot] is only used by [WearAction.DECK]. */
data class WearCommand(val id: String, val action: WearAction, val slot: Int? = null) {
    init {
        require(id.isNotEmpty() && id.length <= 32 && id.all { it in 'a'..'z' || it in 'A'..'Z' || it in '0'..'9' || it == '-' }) { "bad id" }
        if (action == WearAction.DECK) {
            require(slot != null && slot in 0 until WearLink.DECK_SLOTS) { "bad slot" }
        } else {
            require(slot == null) { "slot only for deck" }
        }
    }

    fun encode(): ByteArray = WearLink.frame(listOf(id, action.wire, slot?.toString() ?: ""))

    companion object {
        fun decode(bytes: ByteArray?): WearCommand? {
            val f = WearLink.unframe(bytes, 3) ?: return null
            val action = WearAction.fromWire(f[1]) ?: return null
            val slot = if (f[2].isEmpty()) null else (f[2].toIntOrNull() ?: return null)
            return try {
                WearCommand(f[0], action, slot)
            } catch (_: IllegalArgumentException) {
                null
            }
        }
    }
}

/** Phone -> watch answer to one [WearCommand]. [message] is short text to show on the watch. */
data class WearReply(val id: String, val ok: Boolean, val message: String) {
    fun encode(): ByteArray = WearLink.frame(listOf(id, if (ok) "ok" else "err", message))

    companion object {
        fun decode(bytes: ByteArray?): WearReply? {
            val f = WearLink.unframe(bytes, 3) ?: return null
            val ok = when (f[1]) {
                "ok" -> true
                "err" -> false
                else -> return null
            }
            if (f[0].isEmpty()) return null
            return WearReply(f[0], ok, f[2])
        }
    }
}

/** Phone -> watch connection state. Deck labels are blank for empty slots. */
data class WearStatus(
    val connected: Boolean,
    val pcName: String?,
    val muted: Boolean?,
    val nowPlaying: String?,
    val deck: List<String> = emptyList(),
) {
    fun encode(): ByteArray {
        val labels = (0 until WearLink.DECK_SLOTS).map { deck.getOrNull(it) ?: "" }
        return WearLink.frame(
            listOf(
                if (connected) "1" else "0",
                pcName ?: "",
                when (muted) {
                    true -> "1"
                    false -> "0"
                    null -> ""
                },
                nowPlaying ?: "",
            ) + labels,
        )
    }

    companion object {
        val DISCONNECTED = WearStatus(connected = false, pcName = null, muted = null, nowPlaying = null)

        fun decode(bytes: ByteArray?): WearStatus? {
            val f = WearLink.unframe(bytes, 4 + WearLink.DECK_SLOTS) ?: return null
            val connected = when (f[0]) {
                "1" -> true
                "0" -> false
                else -> return null
            }
            val muted = when (f[2]) {
                "1" -> true
                "0" -> false
                "" -> null
                else -> return null
            }
            return WearStatus(
                connected = connected,
                pcName = f[1].ifEmpty { null },
                muted = muted,
                nowPlaying = f[3].ifEmpty { null },
                deck = f.subList(4, 4 + WearLink.DECK_SLOTS).toList(),
            )
        }
    }
}
