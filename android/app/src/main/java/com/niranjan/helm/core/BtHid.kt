package com.niranjan.helm.core

import kotlinx.coroutines.flow.StateFlow

/*
 * Bluetooth mode: the phone is a standard Bluetooth HID mouse + keyboard (no Helm needed on
 * the PC). Everything that needs `android.bluetooth` sits behind [BtHidTransport]; the rules
 * around it are plain Kotlin here so they can be unit tested.
 */

/** Whether this phone can act as a Bluetooth HID device at all. */
enum class BtSupport(val message: String?) {
    OK(null),
    API_TOO_OLD("This phone's Android version is too old to act as a Bluetooth mouse and keyboard (Android 9 or newer is needed)."),
    NO_ADAPTER("This phone has no Bluetooth, so it can't act as a Bluetooth mouse and keyboard."),
    NO_HID_PROFILE("This phone's Bluetooth doesn't offer the HID Device profile, so it can't act as a Bluetooth mouse and keyboard."),
    ;

    companion object {
        const val MIN_SDK = 28

        /**
         * [profileAvailable] is null while it is not known yet (the profile is only found
         * once the proxy is requested).
         */
        fun evaluate(sdk: Int, hasAdapter: Boolean, profileAvailable: Boolean?): BtSupport = when {
            sdk < MIN_SDK -> API_TOO_OLD
            !hasAdapter -> NO_ADAPTER
            profileAvailable == false -> NO_HID_PROFILE
            else -> OK
        }
    }
}

/** Runtime permissions needed on this Android version (older ones are granted at install). */
object BtPermissions {
    const val CONNECT = "android.permission.BLUETOOTH_CONNECT"
    const val ADVERTISE = "android.permission.BLUETOOTH_ADVERTISE"

    fun required(sdk: Int): List<String> = if (sdk >= 31) listOf(CONNECT, ADVERTISE) else emptyList()

    fun missing(sdk: Int, granted: (String) -> Boolean): List<String> = required(sdk).filterNot(granted)
}

enum class BtLink { NOT_CONNECTED, CONNECTING, CONNECTED }

data class BtHost(val address: String, val name: String)

/** Bonded devices with the last host first, then by name. */
object BtHosts {
    fun order(bonded: List<BtHost>, lastAddress: String?): List<BtHost> =
        bonded.distinctBy { it.address.uppercase() }
            .sortedWith(
                compareByDescending<BtHost> { lastAddress != null && it.address.equals(lastAddress, ignoreCase = true) }
                    .thenBy { it.name.lowercase() },
            )
}

data class BtHidState(
    val support: BtSupport = BtSupport.OK,
    val adapterOn: Boolean = false,
    /** The HID app is registered with the system, so a PC can pair with and connect to the phone. */
    val registered: Boolean = false,
    val link: BtLink = BtLink.NOT_CONNECTED,
    /** The host we are connected or connecting to. */
    val host: BtHost? = null,
    val bonded: List<BtHost> = emptyList(),
    val lastHost: BtHost? = null,
) {
    val connected: Boolean get() = link == BtLink.CONNECTED

    fun statusText(): String = when (link) {
        BtLink.CONNECTED -> "Connected to ${host?.name ?: "PC"}"
        BtLink.CONNECTING -> "Connecting to ${host?.name ?: "PC"}…"
        BtLink.NOT_CONNECTED -> "Not connected"
    }
}

/** The Android side. Reports are queued and sent off the calling thread. */
interface BtHidTransport : HidReportSink {
    val state: StateFlow<BtHidState>

    /** Looks at what this phone supports and re-reads the bonded list; cheap, safe to repeat. */
    fun refresh()

    /** Registers the HID app (needs Bluetooth on and the runtime permissions). */
    fun start()

    fun connect(host: BtHost)

    fun disconnect()

    /** Unregisters the HID app and releases the profile. After this the object is unusable. */
    fun stop()
}
