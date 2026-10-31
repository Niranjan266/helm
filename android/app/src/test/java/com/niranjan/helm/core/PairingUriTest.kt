package com.niranjan.helm.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test

class PairingUriTest {
    private val id = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f"
    private val fp = "a1".repeat(32)
    private val token = "Ab3" + "x".repeat(18) + "-_" + "Z9".repeat(10) // 43 base64url chars

    private fun link(
        v: String? = "1",
        idV: String? = id,
        n: String? = "DESKTOP-1",
        a: String? = "192.168.1.20,10.0.0.5",
        p: String? = "47820",
        fpV: String? = fp,
        t: String? = token,
    ): String {
        val parts = listOfNotNull(
            v?.let { "v=$it" },
            idV?.let { "id=$it" },
            n?.let { "n=$it" },
            a?.let { "a=$it" },
            p?.let { "p=$it" },
            fpV?.let { "fp=$it" },
            t?.let { "t=$it" },
        )
        return "helm://pair?" + parts.joinToString("&")
    }

    private fun assertRejected(raw: String) {
        try {
            PairingUri.parse(raw)
            fail("expected rejection: $raw")
        } catch (e: PairingUriException) {
            assertTrue(e.message!!.isNotBlank())
        }
    }

    @Test
    fun parsesValidLink() {
        val u = PairingUri.parse(link())
        assertEquals(id, u.hostId)
        assertEquals("DESKTOP-1", u.hostName)
        assertEquals(listOf("192.168.1.20", "10.0.0.5"), u.addresses)
        assertEquals(47820, u.port)
        assertEquals(fp, u.fingerprint)
        assertEquals(token, u.token)
    }

    @Test
    fun decodesPercentEncodedName() {
        val u = PairingUri.parse(link(n = "Niranjan%27s%20Gaming%20PC%20%E2%9C%93"))
        assertEquals("Niranjan's Gaming PC ✓", u.hostName)
    }

    @Test
    fun decodesPlusAsSpaceInName() {
        assertEquals("My PC", PairingUri.parse(link(n = "My+PC")).hostName)
    }

    @Test
    fun decodesEncodedCommaInAddressList() {
        val u = PairingUri.parse(link(a = "192.168.1.20%2C10.0.0.5"))
        assertEquals(listOf("192.168.1.20", "10.0.0.5"), u.addresses)
    }

    @Test
    fun normalisesUppercaseFingerprint() {
        val upper = "ABCDEF0123456789".repeat(4)
        assertEquals(upper.lowercase(), PairingUri.parse(link(fpV = upper)).fingerprint)
    }

    @Test
    fun acceptsUppercaseSchemeTrimsAndIgnoresOrder() {
        val raw = "  HELM://pair?t=$token&fp=$fp&p=1&a=1.2.3.4&n=X&id=$id&v=1  "
        val u = PairingUri.parse(raw)
        assertEquals(1, u.port)
        assertEquals(listOf("1.2.3.4"), u.addresses)
    }

    @Test
    fun singleAddressAndDuplicatesAndBlanksAreHandled() {
        val u = PairingUri.parse(link(a = "192.168.1.20,,192.168.1.20, "))
        assertEquals(listOf("192.168.1.20"), u.addresses)
    }

    @Test
    fun missingNameFallsBack() {
        val u = PairingUri.parse(link(n = null))
        assertTrue(u.hostName.isNotBlank())
    }

    @Test
    fun portBoundsAccepted() {
        assertEquals(1, PairingUri.parse(link(p = "1")).port)
        assertEquals(65535, PairingUri.parse(link(p = "65535")).port)
    }

    @Test
    fun toStringHidesToken() {
        assertFalse(PairingUri.parse(link()).toString().contains(token))
    }

    @Test
    fun rejectsWrongSchemeOrHost() {
        assertRejected(link().replace("helm://", "http://"))
        assertRejected(link().replace("helm://pair", "helm://pairing"))
        assertRejected("helm:pair?v=1")
        assertRejected("helm://pair")
        assertRejected("")
        assertRejected("not a link at all")
    }

    @Test
    fun rejectsBadVersion() {
        assertRejected(link(v = null))
        assertRejected(link(v = "2"))
        assertRejected(link(v = ""))
    }

    @Test
    fun rejectsBadFingerprint() {
        assertRejected(link(fpV = null))
        assertRejected(link(fpV = "a1".repeat(31)))
        assertRejected(link(fpV = "a1".repeat(33)))
        assertRejected(link(fpV = "zz".repeat(32)))
        assertRejected(link(fpV = "a1:".repeat(32)))
    }

    @Test
    fun rejectsBadPort() {
        assertRejected(link(p = null))
        assertRejected(link(p = "0"))
        assertRejected(link(p = "65536"))
        assertRejected(link(p = "-1"))
        assertRejected(link(p = "http"))
    }

    @Test
    fun rejectsMissingOrBadAddresses() {
        assertRejected(link(a = null))
        assertRejected(link(a = ""))
        assertRejected(link(a = ",,"))
        assertRejected(link(a = "192.168.1.300"))
        assertRejected(link(a = "my-pc.local"))
    }

    @Test
    fun rejectsMissingOrBadToken() {
        assertRejected(link(t = null))
        assertRejected(link(t = ""))
        assertRejected(link(t = "abc%20def"))
    }

    @Test
    fun rejectsMissingOrBadHostId() {
        assertRejected(link(idV = null))
        assertRejected(link(idV = "not-a-uuid"))
    }

    @Test
    fun rejectsBrokenPercentEncoding() {
        assertRejected(link(n = "bad%zzname"))
    }

    @Test
    fun parseOrNullReturnsNullOnBadInput() {
        assertNull(PairingUri.parseOrNull("helm://pair?v=9"))
    }
}
