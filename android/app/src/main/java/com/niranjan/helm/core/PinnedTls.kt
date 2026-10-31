package com.niranjan.helm.core

import java.net.Socket
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.HostnameVerifier
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.SSLSession
import javax.net.ssl.X509ExtendedTrustManager
import okhttp3.OkHttpClient

object Fingerprints {
    /** Lowercase hex SHA-256 over the DER bytes. */
    fun sha256Hex(der: ByteArray): String = toHex(MessageDigest.getInstance("SHA-256").digest(der))

    fun toHex(bytes: ByteArray): String {
        val sb = StringBuilder(bytes.size * 2)
        for (b in bytes) {
            val v = b.toInt() and 0xff
            sb.append(HEX[v ushr 4]).append(HEX[v and 0x0f])
        }
        return sb.toString()
    }

    fun fromHex(hex: String): ByteArray {
        require(hex.length % 2 == 0) { "odd hex length" }
        return ByteArray(hex.length / 2) { i ->
            val hi = Character.digit(hex[i * 2], 16)
            val lo = Character.digit(hex[i * 2 + 1], 16)
            require(hi >= 0 && lo >= 0) { "bad hex" }
            ((hi shl 4) or lo).toByte()
        }
    }

    private const val HEX = "0123456789abcdef"
}

/** Thrown when the PC's certificate does not match the pinned fingerprint. */
class PinMismatchException(message: String) : CertificateException(message)

/**
 * Accepts a server only if SHA-256(leaf certificate DER) equals the pinned fingerprint
 * (PROTOCOL.md §1). No CA validation: pinning replaces it. This extends the "extended" trust
 * manager so the platform does not wrap it and add its own endpoint checks.
 */
class PinnedTrustManager(expectedFingerprintHex: String) : X509ExtendedTrustManager() {
    private val expected: ByteArray = Fingerprints.fromHex(expectedFingerprintHex.lowercase())

    init {
        require(expected.size == 32) { "fingerprint must be 32 bytes (64 hex chars)" }
    }

    fun check(chain: Array<out X509Certificate>?) {
        if (chain.isNullOrEmpty()) throw CertificateException("server sent no certificate")
        val actual = MessageDigest.getInstance("SHA-256").digest(chain[0].encoded)
        // MessageDigest.isEqual is constant-time for equal-length inputs.
        if (!MessageDigest.isEqual(actual, expected)) {
            throw PinMismatchException("certificate fingerprint does not match the paired PC")
        }
    }

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) =
        check(chain)

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) =
        check(chain)

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) =
        check(chain)

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) =
        throw CertificateException("client certificates are not accepted")

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) =
        throw CertificateException("client certificates are not accepted")

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) =
        throw CertificateException("client certificates are not accepted")

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

/** Pinning replaces hostname checks (we connect to raw IPs with a self-signed cert). */
object PinnedHostnameVerifier : HostnameVerifier {
    override fun verify(hostname: String?, session: SSLSession?): Boolean = true
}

/** How [HelmConnection] reaches the PC. Injectable so tests can use plain `ws://`. */
interface HelmTransport {
    /** `wss` in production. */
    val scheme: String

    /** True for a relay tunnel (§12): `pair` is never sent over it. */
    val relayed: Boolean get() = false

    fun client(fingerprintHex: String): OkHttpClient
}

/**
 * Plain sockets with Nagle's algorithm off: touchpad frames are tiny and must leave at once,
 * not wait to be bundled with the next one (which made the cursor stutter).
 */
object NoDelaySocketFactory : javax.net.SocketFactory() {
    private val base = javax.net.SocketFactory.getDefault()
    private fun tune(s: java.net.Socket) = s.apply { tcpNoDelay = true }

    override fun createSocket(): java.net.Socket = tune(base.createSocket())
    override fun createSocket(host: String?, port: Int): java.net.Socket = tune(base.createSocket(host, port))
    override fun createSocket(host: String?, port: Int, localHost: java.net.InetAddress?, localPort: Int): java.net.Socket =
        tune(base.createSocket(host, port, localHost, localPort))
    override fun createSocket(host: java.net.InetAddress?, port: Int): java.net.Socket = tune(base.createSocket(host, port))
    override fun createSocket(address: java.net.InetAddress?, port: Int, localAddress: java.net.InetAddress?, localPort: Int): java.net.Socket =
        tune(base.createSocket(address, port, localAddress, localPort))
}

/** Production transport: TLS pinned to the PC's certificate fingerprint. */
class PinnedTlsTransport(
    private val base: OkHttpClient = OkHttpClient(),
    private val connectTimeoutMs: Long = 4_000,
    private val pingIntervalMs: Long = 15_000,
) : HelmTransport {
    override val scheme: String = "wss"

    override fun client(fingerprintHex: String): OkHttpClient {
        val tm = PinnedTrustManager(fingerprintHex)
        val ctx = SSLContext.getInstance("TLS")
        ctx.init(null, arrayOf(tm), null)
        return base.newBuilder()
            .socketFactory(NoDelaySocketFactory)
            .sslSocketFactory(ctx.socketFactory, tm)
            .hostnameVerifier(PinnedHostnameVerifier)
            .connectTimeout(connectTimeoutMs, TimeUnit.MILLISECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .pingInterval(pingIntervalMs, TimeUnit.MILLISECONDS)
            .build()
    }
}
