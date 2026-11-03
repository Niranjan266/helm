package com.niranjan.helm.core

import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import org.bouncycastle.crypto.params.Ed25519PublicKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import java.io.File
import java.io.InputStream
import java.security.MessageDigest

/**
 * In-app update logic, no android.* imports. Same rules as the PC's `helm-update` crate:
 * newest stable release newer than this app, with `Helm-android.apk` plus `.sha256` and `.sig`
 * sidecars; a file only counts as verified if its size, SHA-256 and Ed25519 signature all pass.
 * Anything else is deleted and never reaches the installer.
 *
 * Signed message: `"helm-update-v1\n" || file name || "\n" || SHA-256(file)` (32 raw bytes).
 */
object AppUpdate {
    const val APK_ASSET = "Helm-android.apk"
    const val DEFAULT_REPO = "Niranjan266/helm"
    const val RELEASES_URL = "https://api.github.com/repos/$DEFAULT_REPO/releases?per_page=30"
    const val CHECK_INTERVAL_MS = 24L * 60 * 60 * 1000
    const val MAX_JSON_BYTES = 4L * 1024 * 1024
    const val MAX_SIDECAR_BYTES = 4L * 1024
    const val MAX_APK_BYTES = 512L * 1024 * 1024

    private val json = Json { ignoreUnknownKeys = true }

    fun parseReleases(text: String): List<GithubRelease> =
        try {
            json.decodeFromString(text)
        } catch (e: IllegalArgumentException) { // includes SerializationException
            throw UpdateException(UpdateFailure.BAD_RESPONSE, "GitHub sent an unexpected answer.", e)
        }

    /** Newest stable release newer than [current] that carries the APK and both sidecars. */
    fun selectUpdate(releases: List<GithubRelease>, current: SemVer): ApkUpdate? =
        releases.asSequence()
            .filter { !it.draft && !it.prerelease }
            .mapNotNull { r ->
                val version = SemVer.parse(r.tagName) ?: return@mapNotNull null
                if (!version.isStable || version <= current) return@mapNotNull null
                fun asset(name: String) = r.assets.firstOrNull { it.name == name }
                val apk = asset(APK_ASSET) ?: return@mapNotNull null
                val sha = asset("$APK_ASSET.sha256") ?: return@mapNotNull null
                val sig = asset("$APK_ASSET.sig") ?: return@mapNotNull null
                ApkUpdate(version, r.htmlUrl, apk, sha, sig)
            }
            .maxByOrNull { it.version }

    /** At most daily; a clock moved backwards counts as due. */
    fun checkDue(lastCheckMs: Long, nowMs: Long): Boolean =
        lastCheckMs <= 0 || nowMs < lastCheckMs || nowMs - lastCheckMs >= CHECK_INTERVAL_MS

    fun sha256(file: File): ByteArray = file.inputStream().use { input ->
        val md = MessageDigest.getInstance("SHA-256")
        val buf = ByteArray(64 * 1024)
        while (true) {
            val n = input.read(buf)
            if (n < 0) break
            md.update(buf, 0, n)
        }
        md.digest()
    }

    /** `.sha256` sidecar: 64 hex chars, optionally followed by whitespace and a file name. */
    fun parseChecksum(text: String): ByteArray {
        val first = text.trim().split(Regex("\\s+")).firstOrNull().orEmpty()
        return decodeHex(first, 32)
            ?: throw UpdateException(UpdateFailure.VERIFY_FAILED, "The release's checksum file is malformed.")
    }

    /** `.sig` sidecar: 64 raw bytes or 128 hex chars. */
    fun parseSignature(data: ByteArray): ByteArray {
        if (data.size == 64) return data
        return decodeHex(String(data, Charsets.ISO_8859_1).trim(), 64)
            ?: throw UpdateException(UpdateFailure.VERIFY_FAILED, "The release's signature file is malformed.")
    }

    /** A valid 32-byte key in hex, or null: no key means updates are refused. */
    fun parsePublicKey(hex: String?): Ed25519PublicKeyParameters? {
        val bytes = decodeHex(hex?.trim().orEmpty(), 32) ?: return null
        if (bytes.all { it == 0.toByte() }) return null
        return runCatching { Ed25519PublicKeyParameters(bytes, 0) }.getOrNull()
    }

    fun signedMessage(fileName: String, digest: ByteArray): ByteArray =
        "helm-update-v1\n".toByteArray() + fileName.toByteArray() + "\n".toByteArray() + digest

    fun verifySignature(key: Ed25519PublicKeyParameters, fileName: String, digest: ByteArray, signature: ByteArray): Boolean {
        if (signature.size != 64) return false
        return runCatching {
            Ed25519Signer().run {
                init(false, key)
                val message = signedMessage(fileName, digest)
                update(message, 0, message.size)
                verifySignature(signature)
            }
        }.getOrDefault(false)
    }

    /** Throws [UpdateException] unless [file] has this SHA-256 and a good signature for [fileName]. */
    fun verifyFile(file: File, fileName: String, sidecars: UpdateSidecars, key: Ed25519PublicKeyParameters) {
        val digest = sha256(file)
        if (!MessageDigest.isEqual(digest, sidecars.sha256)) {
            throw UpdateException(UpdateFailure.CHECKSUM_MISMATCH, "The download does not match the published SHA-256.")
        }
        if (!verifySignature(key, fileName, digest, sidecars.signature)) {
            throw UpdateException(UpdateFailure.SIGNATURE_MISMATCH, "The download is not signed by the Helm release key.")
        }
    }

    private fun decodeHex(text: String, bytes: Int): ByteArray? {
        if (text.length != bytes * 2) return null
        val out = ByteArray(bytes)
        for (i in 0 until bytes) {
            val hi = Character.digit(text[2 * i], 16)
            val lo = Character.digit(text[2 * i + 1], 16)
            if (hi < 0 || lo < 0) return null
            out[i] = ((hi shl 4) or lo).toByte()
        }
        return out
    }
}

@Serializable
data class GithubRelease(
    @SerialName("tag_name") val tagName: String,
    val draft: Boolean = false,
    val prerelease: Boolean = false,
    @SerialName("html_url") val htmlUrl: String? = null,
    val assets: List<GithubAsset> = emptyList(),
)

@Serializable
data class GithubAsset(
    val name: String,
    @SerialName("browser_download_url") val url: String,
    val size: Long = 0,
)

/** A newer release with a verifiable APK. */
data class ApkUpdate(
    val version: SemVer,
    val pageUrl: String?,
    val apk: GithubAsset,
    val sha256: GithubAsset,
    val signature: GithubAsset,
)

class UpdateSidecars(val sha256: ByteArray, val signature: ByteArray)

/** A downloaded APK and what it was verified against, so it can be checked again before install. */
class VerifiedApk(val file: File, val update: ApkUpdate, val sidecars: UpdateSidecars)

enum class UpdateFailure { NO_KEY, NETWORK, BAD_RESPONSE, INSECURE_URL, TOO_LARGE, SIZE_MISMATCH, CHECKSUM_MISMATCH, SIGNATURE_MISMATCH, VERIFY_FAILED, STORAGE }

class UpdateException(val failure: UpdateFailure, message: String, cause: Throwable? = null) : Exception(message, cause)

/** Semantic version (semver.org 2.0.0 section 11); build metadata is ignored. */
data class SemVer(val major: Long, val minor: Long, val patch: Long, val pre: List<String> = emptyList()) : Comparable<SemVer> {
    val isStable: Boolean get() = pre.isEmpty()

    override fun compareTo(other: SemVer): Int {
        compareValuesBy(this, other, SemVer::major, SemVer::minor, SemVer::patch).let { if (it != 0) return it }
        if (pre.isEmpty() || other.pre.isEmpty()) {
            return when {
                pre.isEmpty() && other.pre.isEmpty() -> 0
                pre.isEmpty() -> 1
                else -> -1
            }
        }
        for (i in 0 until minOf(pre.size, other.pre.size)) {
            val c = comparePre(pre[i], other.pre[i])
            if (c != 0) return c
        }
        return pre.size.compareTo(other.pre.size)
    }

    override fun toString(): String =
        "$major.$minor.$patch" + if (pre.isEmpty()) "" else pre.joinToString(".", prefix = "-")

    companion object {
        /** Parses `1.2.3`, `v1.2.3`, `1.2.3-beta.1`, `1.2.3+build.5`; null when invalid. */
        fun parse(text: String): SemVer? {
            val s = text.trim().removePrefix("v").removePrefix("V").let { t ->
                val plus = t.indexOf('+')
                when {
                    plus < 0 -> t
                    plus == t.length - 1 -> return null
                    else -> t.substring(0, plus)
                }
            }
            val dash = s.indexOf('-')
            val core = if (dash < 0) s else s.substring(0, dash)
            val parts = core.split('.')
            if (parts.size != 3) return null
            val nums = parts.map { number(it) ?: return null }
            val pre = if (dash < 0) {
                emptyList()
            } else {
                s.substring(dash + 1).split('.').onEach { id ->
                    if (id.isEmpty() || !id.all { it.isAsciiAlnum() || it == '-' }) return null
                    if (id.all { it.isDigit() } && number(id) == null) return null
                }
            }
            return SemVer(nums[0], nums[1], nums[2], pre)
        }

        private fun Char.isAsciiAlnum() = this in '0'..'9' || this in 'a'..'z' || this in 'A'..'Z'

        private fun number(s: String): Long? {
            if (s.isEmpty() || !s.all { it in '0'..'9' } || (s.length > 1 && s[0] == '0')) return null
            return s.toLongOrNull()
        }

        private fun comparePre(a: String, b: String): Int {
            val na = number(a)
            val nb = number(b)
            return when {
                na != null && nb != null -> na.compareTo(nb)
                na != null -> -1
                nb != null -> 1
                else -> a.compareTo(b)
            }
        }
    }
}

/** Opens a URL for reading; tests serve fixtures from memory. Must follow only HTTPS redirects. */
interface UpdateFetcher {
    fun open(url: String): InputStream
}

/**
 * Check + download + verify. Refuses to exist without a key, so nothing unverifiable can be
 * fetched. [create] returns null for "no release key in this build".
 */
class ApkUpdater private constructor(
    private val key: Ed25519PublicKeyParameters,
    private val fetcher: UpdateFetcher,
    private val current: SemVer,
    private val releasesUrl: String,
) {
    /** The update on offer, or null when up to date. */
    fun check(): ApkUpdate? {
        val body = fetcher.open(secure(releasesUrl)).use { readLimited(it, AppUpdate.MAX_JSON_BYTES) }
        return AppUpdate.selectUpdate(AppUpdate.parseReleases(String(body)), current)
    }

    /**
     * Downloads to `[dir]/Helm-android.apk`, returning it only if everything verifies. A partial
     * or bad file is deleted, and so is any earlier download.
     */
    fun downloadAndVerify(update: ApkUpdate, dir: File, onProgress: (done: Long, total: Long) -> Unit = { _, _ -> }): VerifiedApk {
        listOf(update.apk, update.sha256, update.signature).forEach { secure(it.url) }
        val sidecars = UpdateSidecars(
            AppUpdate.parseChecksum(String(sidecar(update.sha256))),
            AppUpdate.parseSignature(sidecar(update.signature)),
        )
        if (!dir.isDirectory && !dir.mkdirs()) {
            throw UpdateException(UpdateFailure.STORAGE, "Could not create the download folder.")
        }
        val final = File(dir, AppUpdate.APK_ASSET)
        val part = File(dir, AppUpdate.APK_ASSET + ".part")
        final.delete()
        try {
            download(update.apk, part, onProgress)
            verify(part, sidecars)
            if (!part.renameTo(final)) throw UpdateException(UpdateFailure.STORAGE, "Could not save the download.")
        } catch (e: Exception) {
            part.delete()
            throw e
        }
        return VerifiedApk(final, update, sidecars)
    }

    /** Checks a downloaded file again (right before installing it); a bad file is deleted. */
    fun reverify(apk: VerifiedApk) {
        try {
            verify(apk.file, apk.sidecars)
        } catch (e: Exception) {
            apk.file.delete()
            throw e
        }
    }

    private fun verify(file: File, sidecars: UpdateSidecars) =
        AppUpdate.verifyFile(file, AppUpdate.APK_ASSET, sidecars, key)

    private fun sidecar(asset: GithubAsset): ByteArray =
        fetcher.open(secure(asset.url)).use { readLimited(it, AppUpdate.MAX_SIDECAR_BYTES) }

    private fun download(asset: GithubAsset, to: File, onProgress: (Long, Long) -> Unit) {
        val limit = if (asset.size > 0) asset.size else AppUpdate.MAX_APK_BYTES
        var done = 0L
        fetcher.open(secure(asset.url)).use { input ->
            to.outputStream().use { out ->
                val buf = ByteArray(64 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    done += n
                    if (done > limit) throw UpdateException(UpdateFailure.TOO_LARGE, "The download is larger than the release says.")
                    out.write(buf, 0, n)
                    onProgress(done, asset.size)
                }
            }
        }
        if (asset.size > 0 && done != asset.size) {
            throw UpdateException(UpdateFailure.SIZE_MISMATCH, "The download is not the size the release says.")
        }
    }

    private fun secure(url: String): String {
        if (!url.startsWith("https://", ignoreCase = true)) {
            throw UpdateException(UpdateFailure.INSECURE_URL, "Refusing a download that is not HTTPS.")
        }
        return url
    }

    private fun readLimited(input: InputStream, max: Long): ByteArray {
        val bytes = input.readNBytesCompat(max + 1)
        if (bytes.size > max) throw UpdateException(UpdateFailure.TOO_LARGE, "The response is too large.")
        return bytes
    }

    private fun InputStream.readNBytesCompat(limit: Long): ByteArray {
        val out = java.io.ByteArrayOutputStream()
        val buf = ByteArray(16 * 1024)
        while (out.size() < limit) {
            val n = read(buf, 0, minOf(buf.size.toLong(), limit - out.size()).toInt())
            if (n < 0) break
            out.write(buf, 0, n)
        }
        return out.toByteArray()
    }

    companion object {
        fun create(
            publicKeyHex: String?,
            fetcher: UpdateFetcher,
            current: SemVer,
            releasesUrl: String = AppUpdate.RELEASES_URL,
        ): ApkUpdater? = AppUpdate.parsePublicKey(publicKeyHex)?.let { ApkUpdater(it, fetcher, current, releasesUrl) }
    }
}

/** What the system's PackageInstaller reported, in plain terms. */
sealed interface InstallOutcome {
    data object NeedsUserAction : InstallOutcome
    data object Success : InstallOutcome
    data object Cancelled : InstallOutcome
    data class Failed(val message: String) : InstallOutcome

    companion object {
        // android.content.pm.PackageInstaller.STATUS_* (kept here so this is testable on the JVM).
        const val STATUS_PENDING_USER_ACTION = -1
        const val STATUS_SUCCESS = 0
        const val STATUS_FAILURE = 1
        const val STATUS_FAILURE_BLOCKED = 2
        const val STATUS_FAILURE_ABORTED = 3
        const val STATUS_FAILURE_INVALID = 4
        const val STATUS_FAILURE_CONFLICT = 5
        const val STATUS_FAILURE_STORAGE = 6
        const val STATUS_FAILURE_INCOMPATIBLE = 7

        const val SIGNATURE_MESSAGE =
            "This phone's Helm was signed with a different key (for example a development build), so " +
                "Android will not update it. Uninstall Helm, install Helm-android.apk from the releases " +
                "page once, and pair your PCs again. Updates work by themselves after that."

        fun fromStatus(status: Int, systemMessage: String?): InstallOutcome {
            val text = systemMessage.orEmpty()
            val signatureProblem = text.contains("signature", ignoreCase = true) ||
                text.contains("UPDATE_INCOMPATIBLE", ignoreCase = true)
            return when {
                status == STATUS_PENDING_USER_ACTION -> NeedsUserAction
                status == STATUS_SUCCESS -> Success
                status == STATUS_FAILURE_ABORTED -> Cancelled
                signatureProblem -> Failed(SIGNATURE_MESSAGE)
                status == STATUS_FAILURE_CONFLICT -> Failed(
                    if (text.contains("DOWNGRADE", ignoreCase = true)) {
                        "A newer Helm is already installed."
                    } else {
                        SIGNATURE_MESSAGE
                    },
                )
                status == STATUS_FAILURE_STORAGE -> Failed("Not enough free storage to install the update.")
                status == STATUS_FAILURE_INCOMPATIBLE -> Failed("This update does not support this phone.")
                status == STATUS_FAILURE_BLOCKED -> Failed("This phone's policy blocked the install.")
                status == STATUS_FAILURE_INVALID -> Failed("Android says the update file is invalid.")
                else -> Failed(if (text.isBlank()) "The install failed." else "The install failed: $text")
            }
        }
    }
}
