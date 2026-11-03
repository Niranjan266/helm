package com.niranjan.helm.core

import org.bouncycastle.crypto.params.Ed25519PrivateKeyParameters
import org.bouncycastle.crypto.signers.Ed25519Signer
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.ByteArrayInputStream
import java.io.File
import java.io.IOException
import java.io.InputStream
import java.security.MessageDigest

class AppUpdateTest {
    @get:Rule val tmp = TemporaryFolder()

    // ------------------------------------------------------------------ versions

    private fun v(s: String) = SemVer.parse(s)!!

    @Test fun parsesVersions() {
        assertEquals(SemVer(1, 2, 3), v("1.2.3"))
        assertEquals(SemVer(1, 2, 3), v("v1.2.3"))
        assertEquals(SemVer(1, 2, 3), v("1.2.3+build.9"))
        assertFalse(v("1.2.3-beta.1").isStable)
        assertEquals("1.2.3-beta.1", v("1.2.3-beta.1").toString())
        for (bad in listOf("", "1", "1.2", "1.2.3.4", "01.2.3", "1.2.x", "1.2.3-", "1.2.3-a..b", "1.2.3-01", "nightly", "1.2.3+")) {
            assertNull("$bad should be rejected", SemVer.parse(bad))
        }
    }

    @Test fun semverPrecedence() {
        val chain = listOf(
            "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2",
            "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.1.0", "2.0.0", "10.0.0",
        )
        chain.zipWithNext().forEach { (a, b) -> assertTrue("$a < $b", v(a) < v(b)) }
        assertTrue(v("0.2.10") > v("0.2.9"))
        assertEquals(0, v("1.0.0+a").compareTo(v("1.0.0+b")))
    }

    // ------------------------------------------------------------------ release JSON

    private fun url(tag: String, name: String) = "https://github.com/example/helm/releases/download/$tag/$name"

    private fun assetJson(tag: String, name: String, size: Long = 4096) =
        """{"name":"$name","size":$size,"browser_download_url":"${url(tag, name)}","unknown_field":1}"""

    private fun releaseJson(tag: String, names: List<String>, draft: Boolean = false, pre: Boolean = false, size: Long = 4096) =
        """{"tag_name":"$tag","draft":$draft,"prerelease":$pre,"html_url":"https://example.test/$tag","body":"notes",
           "assets":[${names.joinToString(",") { assetJson(tag, it, size) }}]}"""

    private val full = listOf("Helm-android.apk", "Helm-android.apk.sig", "Helm-android.apk.sha256", "Helm-windows-x64-setup.exe", "SHA256SUMS")

    private val fixture = "[" + listOf(
        releaseJson("v0.5.0-beta.1", full, pre = true),
        releaseJson("v0.4.5-rc.1", full),
        releaseJson("v0.4.2", full, draft = true),
        releaseJson("v0.4.1", listOf("Helm-windows-x64-setup.exe", "Helm-windows-x64-setup.exe.sig", "Helm-windows-x64-setup.exe.sha256")),
        releaseJson("v0.4.0", listOf("Helm-android.apk", "Helm-android.apk.sha256")),
        releaseJson("v0.3.0", full),
        releaseJson("not-a-version", full),
        releaseJson("v0.2.0", full),
    ).joinToString(",") + "]"

    @Test fun picksNewestStableVerifiableRelease() {
        val update = AppUpdate.selectUpdate(AppUpdate.parseReleases(fixture), v("0.2.0"))!!
        assertEquals(v("0.3.0"), update.version)
        assertEquals("Helm-android.apk", update.apk.name)
        assertEquals("Helm-android.apk.sig", update.signature.name)
        assertEquals("Helm-android.apk.sha256", update.sha256.name)
        assertEquals(4096, update.apk.size)
        assertEquals("https://example.test/v0.3.0", update.pageUrl)
    }

    @Test fun upToDateOrNewerGetsNothing() {
        val releases = AppUpdate.parseReleases(fixture)
        assertNull(AppUpdate.selectUpdate(releases, v("0.3.0")))
        assertNull(AppUpdate.selectUpdate(releases, v("9.0.0")))
        assertNull(AppUpdate.selectUpdate(emptyList(), v("0.1.0")))
    }

    @Test fun badJsonIsAnUpdateError() {
        for (bad in listOf("not json", """{"message":"API rate limit exceeded"}""")) {
            val e = assertThrows(UpdateException::class.java) { AppUpdate.parseReleases(bad) }
            assertEquals(UpdateFailure.BAD_RESPONSE, e.failure)
        }
    }

    @Test fun checksAtMostDaily() {
        val now = 10_000_000_000L
        assertTrue(AppUpdate.checkDue(0, now))
        assertFalse(AppUpdate.checkDue(now - 3_600_000, now))
        assertTrue(AppUpdate.checkDue(now - AppUpdate.CHECK_INTERVAL_MS, now))
        assertTrue(AppUpdate.checkDue(now + 60_000, now))
    }

    // ------------------------------------------------------------------ signatures

    private val testKey = Ed25519PrivateKeyParameters(ByteArray(32) { 7 }, 0)
    private val attacker = Ed25519PrivateKeyParameters(ByteArray(32) { 9 }, 0)
    private val testPub = hex(testKey.generatePublicKey().encoded)

    private fun hex(b: ByteArray) = b.joinToString("") { "%02x".format(it) }

    private fun sha(b: ByteArray) = MessageDigest.getInstance("SHA-256").digest(b)

    private fun sign(key: Ed25519PrivateKeyParameters, name: String, digest: ByteArray): ByteArray =
        Ed25519Signer().run {
            init(true, key)
            val m = AppUpdate.signedMessage(name, digest)
            update(m, 0, m.size)
            generateSignature()
        }

    /** Made with the PC's `helm-sign` and the same test key: both languages agree on the format. */
    @Test fun verifiesASignatureMadeByHelmSign() {
        val apk = tmp.newFile("Helm-android.apk").apply { writeBytes("Helm interop test apk bytes\n".toByteArray()) }
        val sidecars = UpdateSidecars(
            AppUpdate.parseChecksum("48a34d79df7e73c1f8a7747cf06907f673959c3bfcbde32eac5fd9d4ed72d394  Helm-android.apk\n"),
            AppUpdate.parseSignature(
                "4d18823dcc8d94c8968005e3dce1f76418db9a14dae8358a6d954ea4b9c89e22cfb3f47fff4a51f6eb61015ed9c1a9c6a5b409ba4776ed87c7975b34dacce801\n".toByteArray(),
            ),
        )
        val key = AppUpdate.parsePublicKey("ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")!!
        assertEquals(testPub, "ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c")
        AppUpdate.verifyFile(apk, "Helm-android.apk", sidecars, key)
    }

    @Test fun signatureIsBoundToContentNameAndKey() {
        val key = AppUpdate.parsePublicKey(testPub)!!
        val digest = sha("apk".toByteArray())
        val sig = sign(testKey, "Helm-android.apk", digest)
        assertTrue(AppUpdate.verifySignature(key, "Helm-android.apk", digest, sig))
        assertFalse(AppUpdate.verifySignature(key, "Helm-android.apk", sha("evil".toByteArray()), sig))
        assertFalse(AppUpdate.verifySignature(key, "Helm-other.apk", digest, sig))
        assertFalse(AppUpdate.verifySignature(key, "Helm-android.apk", digest, sign(attacker, "Helm-android.apk", digest)))
        assertFalse(AppUpdate.verifySignature(key, "Helm-android.apk", digest, ByteArray(64)))
        assertFalse(AppUpdate.verifySignature(key, "Helm-android.apk", digest, ByteArray(5)))
    }

    @Test fun sidecarParsing() {
        val d = sha("x".toByteArray())
        assertTrue(d.contentEquals(AppUpdate.parseChecksum(hex(d))))
        assertTrue(d.contentEquals(AppUpdate.parseChecksum("${hex(d)}  file.apk\n")))
        listOf("zz", "", hex(d).drop(2)).forEach { bad ->
            assertThrows(UpdateException::class.java) { AppUpdate.parseChecksum(bad) }
        }
        val sig = sign(testKey, "f", d)
        assertTrue(sig.contentEquals(AppUpdate.parseSignature(sig)))
        assertTrue(sig.contentEquals(AppUpdate.parseSignature("${hex(sig)}\n".toByteArray())))
        assertThrows(UpdateException::class.java) { AppUpdate.parseSignature("short".toByteArray()) }
    }

    @Test fun noOrBadKeyMeansNoUpdater() {
        val f = Fetcher(emptyMap())
        for (key in listOf(null, "", "abcd", "zz".repeat(32), "00".repeat(32))) {
            assertNull("key=$key", ApkUpdater.create(key, f, v("0.2.0")))
        }
        assertNotNull(ApkUpdater.create(testPub, f, v("0.2.0")))
        assertTrue("nothing fetched", f.log.isEmpty())
    }

    @Test fun committedPublicKeyIsValid() {
        // Gradle runs unit tests from android/app.
        val text = File("../../keys/update-ed25519.pub").readText()
        assertNotNull(AppUpdate.parsePublicKey(text))
    }

    // ------------------------------------------------------------------ end to end, no network

    private class Fetcher(private val files: Map<String, ByteArray>) : UpdateFetcher {
        val log = mutableListOf<String>()
        override fun open(url: String): InputStream {
            log += url
            return ByteArrayInputStream(files[url] ?: throw IOException("404 $url"))
        }
    }

    private val tag = "v0.3.0"
    private val apkBytes = ByteArray(4096) { (it * 31 % 251).toByte() }

    private fun fetcher(
        served: ByteArray = apkBytes,
        signedAs: ByteArray = apkBytes,
        signer: Ed25519PrivateKeyParameters = testKey,
        json: String = fixture,
    ): Fetcher {
        val digest = sha(signedAs)
        return Fetcher(
            mapOf(
                AppUpdate.RELEASES_URL to json.toByteArray(),
                url(tag, "Helm-android.apk") to served,
                url(tag, "Helm-android.apk.sha256") to "${hex(digest)}  Helm-android.apk\n".toByteArray(),
                url(tag, "Helm-android.apk.sig") to "${hex(sign(signer, "Helm-android.apk", digest))}\n".toByteArray(),
            ),
        )
    }

    private fun updater(f: Fetcher) = ApkUpdater.create(testPub, f, v("0.2.0"))!!

    private fun assertNothingLeft(dir: File) = assertEquals(emptyList<String>(), dir.list()?.toList().orEmpty())

    @Test fun checkDownloadVerifySuccessWithProgress() {
        val dir = tmp.newFolder()
        val up = updater(fetcher())
        val update = up.check()!!
        val seen = mutableListOf<Long>()
        val apk = up.downloadAndVerify(update, dir) { done, total ->
            seen += done
            assertEquals(4096, total)
        }
        assertEquals(File(dir, "Helm-android.apk"), apk.file)
        assertTrue(apkBytes.contentEquals(apk.file.readBytes()))
        assertEquals(4096L, seen.last())
        assertFalse(File(dir, "Helm-android.apk.part").exists())
        up.reverify(apk)
    }

    @Test fun tamperedDownloadIsRefusedAndDeleted() {
        val dir = tmp.newFolder()
        val bad = apkBytes.clone().also { it[100] = (it[100] + 1).toByte() }
        val up = updater(fetcher(served = bad))
        val e = assertThrows(UpdateException::class.java) { up.downloadAndVerify(up.check()!!, dir) }
        assertEquals(UpdateFailure.CHECKSUM_MISMATCH, e.failure)
        assertNothingLeft(dir)
    }

    @Test fun rightChecksumWrongSignerIsRefusedAndDeleted() {
        val dir = tmp.newFolder()
        val up = updater(fetcher(signer = attacker))
        val e = assertThrows(UpdateException::class.java) { up.downloadAndVerify(up.check()!!, dir) }
        assertEquals(UpdateFailure.SIGNATURE_MISMATCH, e.failure)
        assertNothingLeft(dir)
    }

    @Test fun anEarlierDownloadDoesNotSurviveAFailedOne() {
        val dir = tmp.newFolder()
        File(dir, "Helm-android.apk").writeBytes("left over".toByteArray())
        val up = updater(fetcher(signer = attacker))
        assertThrows(UpdateException::class.java) { up.downloadAndVerify(up.check()!!, dir) }
        assertNothingLeft(dir)
    }

    @Test fun sizeMismatchesAreRefused() {
        for ((served, failure) in listOf(apkBytes.copyOf(1000) to UpdateFailure.SIZE_MISMATCH, apkBytes.copyOf(5000) to UpdateFailure.TOO_LARGE)) {
            val dir = tmp.newFolder()
            val up = updater(fetcher(served = served, signedAs = served))
            val e = assertThrows(UpdateException::class.java) { up.downloadAndVerify(up.check()!!, dir) }
            assertEquals(failure, e.failure)
            assertNothingLeft(dir)
        }
    }

    @Test fun fileChangedAfterVerificationIsCaughtBeforeInstall() {
        val dir = tmp.newFolder()
        val up = updater(fetcher())
        val apk = up.downloadAndVerify(up.check()!!, dir)
        apk.file.writeBytes("swapped".toByteArray())
        val e = assertThrows(UpdateException::class.java) { up.reverify(apk) }
        assertEquals(UpdateFailure.CHECKSUM_MISMATCH, e.failure)
        assertFalse(apk.file.exists())
    }

    @Test fun plainHttpIsRefusedBeforeAnyFetch() {
        val f = fetcher()
        val up = updater(f)
        val update = up.check()!!
        f.log.clear()
        val http = update.copy(apk = update.apk.copy(url = update.apk.url.replace("https://", "http://")))
        val e = assertThrows(UpdateException::class.java) { up.downloadAndVerify(http, tmp.newFolder()) }
        assertEquals(UpdateFailure.INSECURE_URL, e.failure)
        assertTrue(f.log.isEmpty())
    }

    @Test fun oversizedReleaseJsonIsRefused() {
        val up = updater(fetcher(json = " ".repeat(5 * 1024 * 1024)))
        assertEquals(UpdateFailure.TOO_LARGE, assertThrows(UpdateException::class.java) { up.check() }.failure)
    }

    @Test fun noNetworkSurfacesAsIoFailure() {
        val up = ApkUpdater.create(testPub, Fetcher(emptyMap()), v("0.2.0"))!!
        assertThrows(IOException::class.java) { up.check() }
    }

    // ------------------------------------------------------------------ system installer results

    @Test fun installerStatusesInPlainTerms() {
        assertEquals(InstallOutcome.NeedsUserAction, InstallOutcome.fromStatus(InstallOutcome.STATUS_PENDING_USER_ACTION, null))
        assertEquals(InstallOutcome.Success, InstallOutcome.fromStatus(InstallOutcome.STATUS_SUCCESS, null))
        assertEquals(InstallOutcome.Cancelled, InstallOutcome.fromStatus(InstallOutcome.STATUS_FAILURE_ABORTED, "user said no"))

        val signature = InstallOutcome.Failed(InstallOutcome.SIGNATURE_MESSAGE)
        assertEquals(signature, InstallOutcome.fromStatus(InstallOutcome.STATUS_FAILURE_CONFLICT, null))
        assertEquals(
            signature,
            InstallOutcome.fromStatus(InstallOutcome.STATUS_FAILURE, "INSTALL_FAILED_UPDATE_INCOMPATIBLE: signatures do not match"),
        )
        assertEquals(
            InstallOutcome.Failed("A newer Helm is already installed."),
            InstallOutcome.fromStatus(InstallOutcome.STATUS_FAILURE_CONFLICT, "INSTALL_FAILED_VERSION_DOWNGRADE"),
        )
        assertTrue(InstallOutcome.fromStatus(InstallOutcome.STATUS_FAILURE_STORAGE, null) is InstallOutcome.Failed)
        assertEquals(InstallOutcome.Failed("The install failed: boom"), InstallOutcome.fromStatus(99, "boom"))
    }
}
