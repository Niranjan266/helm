package com.niranjan.helm.core

import java.security.KeyPair
import java.security.KeyPairGenerator
import java.security.Signature
import java.security.spec.ECGenParameterSpec
import java.util.Base64

/** Signs handshake challenges for this phone (PROTOCOL.md §2, §5). */
interface DeviceSigner {
    /** X.509 SubjectPublicKeyInfo DER of the EC P-256 public key. */
    fun publicKeySpki(): ByteArray

    /** SHA256withECDSA signature, ASN.1 DER encoded. */
    fun sign(data: ByteArray): ByteArray
}

/** Software EC P-256 signer. Used by JVM tests; the app uses the Android Keystore signer. */
class SoftwareDeviceSigner(private val keyPair: KeyPair = generate()) : DeviceSigner {
    override fun publicKeySpki(): ByteArray = keyPair.public.encoded

    override fun sign(data: ByteArray): ByteArray {
        val s = Signature.getInstance("SHA256withECDSA")
        s.initSign(keyPair.private)
        s.update(data)
        return s.sign()
    }

    companion object {
        fun generate(): KeyPair {
            val g = KeyPairGenerator.getInstance("EC")
            g.initialize(ECGenParameterSpec("secp256r1"))
            return g.generateKeyPair()
        }
    }
}

/** Who this phone is, as presented to the PC. */
data class DeviceIdentity(
    val deviceId: String,
    val deviceName: String,
    val signer: DeviceSigner,
)

object HelmSigning {
    const val PURPOSE_PAIR = "pair"
    const val PURPOSE_AUTH = "auth"

    /** `helm-v1|<purpose>|<host_id>|<nonce>` — nonce is the exact string from `hello`. */
    fun signedString(purpose: String, hostId: String, nonce: String): String =
        "helm-v1|$purpose|$hostId|$nonce"

    /** Base64 (standard, padded) DER signature over [signedString]. */
    fun signatureBase64(signer: DeviceSigner, purpose: String, hostId: String, nonce: String): String =
        b64(signer.sign(signedString(purpose, hostId, nonce).toByteArray(Charsets.UTF_8)))

    fun b64(bytes: ByteArray): String = Base64.getEncoder().encodeToString(bytes)
}
