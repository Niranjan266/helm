package com.niranjan.helm.core

/**
 * Pure rules behind the power widget: which buttons exist, which ones ask first, and the short
 * texts shown on the widget and in its toasts.
 */
object WidgetPower {
    const val LOCK = "lock"
    const val SLEEP = "sleep"
    const val SHUTDOWN = "shutdown"

    /** The `power.action` kinds the widget offers, in button order. */
    val ACTIONS = listOf(LOCK, SLEEP, SHUTDOWN)

    fun isSupported(action: String?): Boolean = action in ACTIONS

    /** Shut down ends the user's session on the PC, so the widget asks before sending it. */
    fun needsConfirm(action: String): Boolean = action == SHUTDOWN

    fun label(action: String): String = when (action) {
        LOCK -> "Lock PC"
        SLEEP -> "Sleep"
        SHUTDOWN -> "Shut down"
        else -> action.replace('_', ' ')
    }

    /** Subtitle of the fingerprint / screen-lock prompt (per-PC "admin check" setting). */
    fun gateReason(action: String): String = when (action) {
        LOCK -> "Lock the PC"
        SLEEP -> "Put the PC to sleep"
        SHUTDOWN -> "Shut down the PC"
        else -> label(action)
    }

    fun confirmTitle(pcName: String?): String = "Shut down ${pcName?.takeIf { it.isNotBlank() } ?: "the PC"}?"

    const val CONFIRM_TEXT = "Open apps close and unsaved work may be lost."

    /**
     * Toast after the PC accepted the action. [scheduledInMs] is set when the PC answered with a
     * delayed schedule instead of acting at once.
     */
    fun doneText(action: String, scheduledInMs: Long? = null): String {
        if (scheduledInMs != null && scheduledInMs > 0) {
            val s = (scheduledInMs + 999) / 1000
            val wait = if (s >= 60) "${(s + 59) / 60} min" else "$s s"
            return "${label(action)} in $wait."
        }
        return when (action) {
            LOCK -> "Locked the PC."
            SLEEP -> "Putting the PC to sleep."
            SHUTDOWN -> "Shutting down the PC."
            else -> "Sent ${label(action)} to the PC."
        }
    }

    /** Title and status line of the widget header. */
    data class Header(val title: String, val status: String, val online: Boolean)

    /**
     * [livePcName] is the PC of the current session (null without one), [savedPcName] the most
     * recently used saved PC. "Online" is only claimed while a session is actually ready, which
     * is free to know; otherwise the widget does not guess and says it is not connected.
     */
    fun header(livePcName: String?, savedPcName: String?, connected: Boolean): Header {
        val name = livePcName?.takeIf { it.isNotBlank() } ?: savedPcName?.takeIf { it.isNotBlank() }
        return when {
            name == null -> Header("Helm", "Pair with a PC in the app", false)
            connected -> Header(name, "Online", true)
            else -> Header(name, "Not connected", false)
        }
    }
}
