package com.niranjan.helm.core

import java.net.Socket
import java.security.MessageDigest
import java.security.SecureRandom
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.crypto.Mac
import javax.crypto.spec.SecretKeySpec
import javax.net.ssl.SSLContext
import javax.net.ssl.SSLEngine
import javax.net.ssl.X509ExtendedTrustManager
import okhttp3.OkHttpClient

/**
 * Short-code pairing (hello feature `g4`, Docs/protocol/pairing-security.md), pure parts.
 * Must produce exactly the bytes of the agent's `g4/code.rs` (shared test vector).
 *
 * ```
 * h      = SHA-256("helm-g4-code-v1|" host_id "|" nonce_a "|" nonce_p "|" fp "|" device_id "|" public_key)
 * C(R,i) = HMAC-SHA256(key = r(R,i), "helm-g4-commit|" R "|" i "|" digit_i "|" hex(h))
 * ```
 */
object CodePairingCrypto {
    const val DIGITS = 6
    const val R_LEN = 32
    const val ROLE_AGENT = "A"
    const val ROLE_PHONE = "P"

    fun hmacSha256(key: ByteArray, msg: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        // An empty key is not allowed by SecretKeySpec; keys here are always 32 bytes.
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(msg)
    }

    fun transcriptHash(
        hostId: String,
        nonceAgent: String,
        noncePhone: String,
        fingerprint: String,
        deviceId: String,
        publicKey: String,
    ): ByteArray {
        val text = "helm-g4-code-v1|$hostId|$nonceAgent|$noncePhone|${fingerprint.lowercase()}|$deviceId|$publicKey"
        return MessageDigest.getInstance("SHA-256").digest(text.toByteArray(Charsets.UTF_8))
    }

    /** Commitment to [digit] (0-9) in [round] (1-based) by [role] ("A" or "P"). */
    fun commitment(role: String, round: Int, digit: Int, h: ByteArray, r: ByteArray): ByteArray {
        val msg = "helm-g4-commit|$role|$round|$digit|${Fingerprints.toHex(h)}"
        return hmacSha256(r, msg.toByteArray(Charsets.UTF_8))
    }

    fun openingMatches(commit: ByteArray, role: String, round: Int, digit: Int, h: ByteArray, r: ByteArray): Boolean {
        if (r.size != R_LEN || commit.size != 32) return false
        return MessageDigest.isEqual(commitment(role, round, digit, h, r), commit)
    }

    /** "123 456" / "123-456" / "123456" -> digits, or null unless exactly 6 digits. */
    fun parseCode(text: String): IntArray? {
        val cleaned = text.filter { it != ' ' && it != '-' }
        if (cleaned.length != DIGITS || !cleaned.all { it in '0'..'9' }) return null
        return IntArray(DIGITS) { cleaned[it] - '0' }
    }
}

/**
 * The phone's side of one code-pairing attempt. Usage: [commitments] go into `pair_code`; for
 * each `code_open` from the PC call [checkAgentOpening] and, only if it returns true, send
 * [opening]. A false result means a wrong code or a man in the middle: stop and never reuse
 * the code.
 */
class CodePairingPhone(
    code: IntArray,
    private val h: ByteArray,
    random: SecureRandom = SecureRandom(),
    /** Test hook: fixed openings. */
    openings: List<ByteArray>? = null,
) {
    private val digits = code.copyOf()
    private val r: List<ByteArray> = openings ?: List(CodePairingCrypto.DIGITS) {
        ByteArray(CodePairingCrypto.R_LEN).also(random::nextBytes)
    }
    private var agentCommits: List<ByteArray>? = null
    private var verified = 0

    init {
        require(digits.size == CodePairingCrypto.DIGITS && digits.all { it in 0..9 }) { "code must be 6 digits" }
        require(r.size == CodePairingCrypto.DIGITS && r.all { it.size == CodePairingCrypto.R_LEN })
    }

    val commitments: List<ByteArray> = List(CodePairingCrypto.DIGITS) { i ->
        CodePairingCrypto.commitment(CodePairingCrypto.ROLE_PHONE, i + 1, digits[i], h, r[i])
    }

    /** True once all six PC openings matched: the fingerprint seen is the real PC's. */
    val complete: Boolean get() = verified == CodePairingCrypto.DIGITS

    fun setAgentCommitments(commits: List<ByteArray>): Boolean {
        if (agentCommits != null || commits.size != CodePairingCrypto.DIGITS || commits.any { it.size != 32 }) return false
        agentCommits = commits
        return true
    }

    /** Checks the PC's opening for [round] (rounds must come in order 1..6). */
    fun checkAgentOpening(round: Int, opening: ByteArray): Boolean {
        val commits = agentCommits ?: return false
        if (round != verified + 1) return false
        val ok = CodePairingCrypto.openingMatches(commits[round - 1], CodePairingCrypto.ROLE_AGENT, round, digits[round - 1], h, opening)
        if (ok) verified = round
        return ok
    }

    /** Our opening for [round]; only after the PC's opening for that round was checked. */
    fun opening(round: Int): ByteArray {
        check(round in 1..verified) { "PC opening for round $round not verified yet" }
        return r[round - 1].copyOf()
    }
}

/**
 * Codes this phone already tried, per PC (in memory, newest 64). A code gets one attempt: a fake
 * PC must not learn more about a code from the user retyping it.
 */
class UsedCodes {
    private val used = LinkedHashSet<String>()

    /** Records the attempt. False if this code was already tried with this PC. */
    @Synchronized
    fun tryUse(hostId: String, code: String): Boolean {
        val digits = code.filter { it != ' ' && it != '-' }
        if (!used.add(hostId.lowercase() + "|" + digits)) return false
        if (used.size > 64) used.remove(used.first())
        return true
    }
}

/** A transport that tells which certificate fingerprint the last TLS handshake saw. */
interface FingerprintObserver {
    /** Lowercase hex SHA-256 of the leaf certificate of the most recent handshake, or null. */
    fun observedFingerprint(): String?

    fun resetObserved()
}

/**
 * Accepts any server certificate and records its fingerprint. Only for code pairing (g4):
 * nothing is trusted until the code exchange proves that this fingerprint is the PC's.
 */
class RecordingTrustManager : X509ExtendedTrustManager() {
    @Volatile
    var fingerprint: String? = null

    private fun record(chain: Array<out X509Certificate>?) {
        if (chain.isNullOrEmpty()) throw CertificateException("server sent no certificate")
        fingerprint = Fingerprints.sha256Hex(chain[0].encoded)
    }

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) = record(chain)
    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) = record(chain)
    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) = record(chain)
    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?) =
        throw CertificateException("client certificates are not accepted")
    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, socket: Socket?) =
        throw CertificateException("client certificates are not accepted")
    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?, engine: SSLEngine?) =
        throw CertificateException("client certificates are not accepted")
    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

/** TLS for code pairing: records the PC's certificate instead of pinning it (see above). */
class CodePairingTransport(
    private val base: OkHttpClient = OkHttpClient(),
    private val connectTimeoutMs: Long = 4_000,
) : HelmTransport, FingerprintObserver {
    override val scheme: String = "wss"
    private val tm = RecordingTrustManager()

    override fun client(fingerprintHex: String): OkHttpClient {
        val ctx = SSLContext.getInstance("TLS")
        ctx.init(null, arrayOf(tm), null)
        return base.newBuilder()
            .socketFactory(NoDelaySocketFactory)
            .sslSocketFactory(ctx.socketFactory, tm)
            .hostnameVerifier(PinnedHostnameVerifier)
            .connectTimeout(connectTimeoutMs, TimeUnit.MILLISECONDS)
            .readTimeout(0, TimeUnit.MILLISECONDS)
            .pingInterval(15_000, TimeUnit.MILLISECONDS)
            .build()
    }

    override fun observedFingerprint(): String? = tm.fingerprint

    override fun resetObserved() {
        tm.fingerprint = null
    }
}

/** Test transport (plain `ws://`) that reports a fixed fingerprint, as if TLS had seen it. */
class FixedFingerprintTransport(
    private val fingerprint: String,
    private val base: OkHttpClient = OkHttpClient(),
) : HelmTransport, FingerprintObserver {
    override val scheme: String = "ws"
    override fun client(fingerprintHex: String): OkHttpClient = base
    override fun observedFingerprint(): String = fingerprint
    override fun resetObserved() = Unit
}

/**
 * Lock Helm after it was in the background for a while (setting, default off). Pure logic;
 * the activity reports background/foreground and shows the unlock prompt when [shouldLock].
 */
class AppLockTimer(
    /** 0 = never lock. */
    @Volatile var timeoutMs: Long = 0,
) {
    private var backgroundSince: Long? = null

    @Volatile
    var locked: Boolean = false
        private set

    @Synchronized
    fun onBackground(nowMs: Long) {
        if (backgroundSince == null) backgroundSince = nowMs
    }

    /** Call when the app comes to the front. Returns true when it must be unlocked first. */
    @Synchronized
    fun onForeground(nowMs: Long): Boolean {
        val since = backgroundSince
        backgroundSince = null
        if (timeoutMs > 0 && since != null && nowMs - since >= timeoutMs) locked = true
        return locked
    }

    @Synchronized
    fun unlocked() {
        locked = false
        backgroundSince = null
    }

    companion object {
        /** Choices offered in Settings (minutes; 0 = off). */
        val CHOICES_MIN = listOf(0, 1, 5, 15, 30)
    }
}
