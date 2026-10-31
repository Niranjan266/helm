package com.niranjan.helm.core

import java.security.KeyFactory
import java.security.Signature
import java.security.interfaces.ECPublicKey
import java.security.spec.X509EncodedKeySpec
import java.util.Base64
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SigningTest {
    private val hostId = "3f2b8c1e-9a4d-4e7b-8c2f-1a2b3c4d5e6f"
    private val nonce = "q+/w7Zk3pQ1a2b3c4d5e6f7g8h9i0jKLmnOPqrSTuvw="

    @Test
    fun signedStringFormatIsExact() {
        assertEquals("helm-v1|pair|$hostId|$nonce", HelmSigning.signedString("pair", hostId, nonce))
        assertEquals("helm-v1|auth|$hostId|$nonce", HelmSigning.signedString(HelmSigning.PURPOSE_AUTH, hostId, nonce))
    }

    @Test
    fun publicKeyIsP256SubjectPublicKeyInfo() {
        val signer = SoftwareDeviceSigner()
        val spki = signer.publicKeySpki()
        val key = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(spki)) as ECPublicKey
        assertEquals(256, key.params.curve.field.fieldSize)
        // SPKI DER is a SEQUENCE.
        assertEquals(0x30, spki[0].toInt() and 0xff)
        assertEquals(91, spki.size) // uncompressed P-256 SPKI
    }

    @Test
    fun signatureIsDerAndVerifiesOverTheSignedString() {
        val signer = SoftwareDeviceSigner()
        val sigB64 = HelmSigning.signatureBase64(signer, HelmSigning.PURPOSE_PAIR, hostId, nonce)
        val sig = Base64.getDecoder().decode(sigB64) // standard alphabet, padded
        assertEquals(0x30, sig[0].toInt() and 0xff)

        val pub = KeyFactory.getInstance("EC").generatePublic(X509EncodedKeySpec(signer.publicKeySpki()))
        val v = Signature.getInstance("SHA256withECDSA")
        v.initVerify(pub)
        v.update("helm-v1|pair|$hostId|$nonce".toByteArray(Charsets.UTF_8))
        assertTrue(v.verify(sig))

        // Wrong purpose must not verify.
        v.initVerify(pub)
        v.update("helm-v1|auth|$hostId|$nonce".toByteArray(Charsets.UTF_8))
        assertFalse(v.verify(sig))
    }
}
