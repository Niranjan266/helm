package com.niranjan.helm.core

import java.net.InetAddress
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import javax.net.ssl.SSLException
import okhttp3.Request
import okhttp3.mockwebserver.MockResponse
import okhttp3.mockwebserver.MockWebServer
import okhttp3.tls.HandshakeCertificates
import okhttp3.tls.HeldCertificate
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class PinnedTlsTest {
    private fun cert(): HeldCertificate = HeldCertificate.Builder()
        .commonName("helm-agent")
        .ecdsa256()
        .build()

    @Test
    fun sha256HexKnownVector() {
        // SHA-256("abc") from FIPS 180-2.
        assertEquals(
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
            Fingerprints.sha256Hex("abc".toByteArray()),
        )
    }

    @Test
    fun hexRoundTrip() {
        val bytes = ByteArray(32) { it.toByte() }
        assertEquals(bytes.toList(), Fingerprints.fromHex(Fingerprints.toHex(bytes)).toList())
    }

    @Test
    fun acceptsOnlyThePinnedLeaf() {
        val good = cert().certificate
        val other = cert().certificate
        val fp = Fingerprints.sha256Hex(good.encoded)
        assertEquals(64, fp.length)
        assertEquals(Fingerprints.toHex(MessageDigest.getInstance("SHA-256").digest(good.encoded)), fp)

        val tm = PinnedTrustManager(fp)
        tm.checkServerTrusted(arrayOf(good), "ECDHE_ECDSA") // no throw
        tm.checkServerTrusted(arrayOf(good, other), "ECDHE_ECDSA") // only the leaf matters

        try {
            tm.checkServerTrusted(arrayOf(other), "ECDHE_ECDSA")
            fail("other cert must be rejected")
        } catch (_: PinMismatchException) {
        }
        try {
            tm.checkServerTrusted(arrayOf(other, good), "ECDHE_ECDSA")
            fail("pinned cert in a non-leaf position must be rejected")
        } catch (_: PinMismatchException) {
        }
        try {
            tm.checkServerTrusted(emptyArray<X509Certificate>(), "ECDHE_ECDSA")
            fail("empty chain must be rejected")
        } catch (_: CertificateException) {
        }
        try {
            tm.checkClientTrusted(arrayOf(good), "ECDHE_ECDSA")
            fail("client certs are not accepted")
        } catch (_: CertificateException) {
        }
    }

    @Test
    fun uppercaseFingerprintIsAccepted() {
        val good = cert().certificate
        PinnedTrustManager(Fingerprints.sha256Hex(good.encoded).uppercase()).checkServerTrusted(arrayOf(good), "x")
    }

    @Test(expected = IllegalArgumentException::class)
    fun rejectsShortFingerprint() {
        PinnedTrustManager("abcd")
    }

    @Test
    fun realTlsConnectionSucceedsWithPinAndFailsWithoutIt() {
        val held = cert()
        val server = MockWebServer()
        server.useHttps(HandshakeCertificates.Builder().heldCertificate(held).build().sslSocketFactory(), false)
        server.enqueue(MockResponse().setBody("ok"))
        server.enqueue(MockResponse().setBody("ok"))
        server.start(InetAddress.getByName("127.0.0.1"), 0)
        try {
            val url = "https://127.0.0.1:${server.port}/"
            val goodFp = Fingerprints.sha256Hex(held.certificate.encoded)
            val good = PinnedTlsTransport().client(goodFp)
            good.newCall(Request.Builder().url(url).build()).execute().use { r ->
                assertEquals("ok", r.body!!.string())
            }

            val bad = PinnedTlsTransport().client("00".repeat(32))
            try {
                bad.newCall(Request.Builder().url(url).build()).execute().close()
                fail("wrong pin must fail the TLS handshake")
            } catch (e: SSLException) {
                var c: Throwable? = e
                var found = false
                while (c != null) {
                    if (c is PinMismatchException) found = true
                    c = c.cause
                }
                assertTrue("cause should be PinMismatchException: $e", found)
            }
        } finally {
            server.shutdown()
        }
    }
}
