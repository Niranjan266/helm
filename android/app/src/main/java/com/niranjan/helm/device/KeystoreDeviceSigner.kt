package com.niranjan.helm.device

import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import com.niranjan.helm.core.DeviceSigner
import java.security.KeyPairGenerator
import java.security.KeyStore
import java.security.PrivateKey
import java.security.Signature
import java.security.spec.ECGenParameterSpec

/**
 * EC P-256 key in the Android Keystore (non-exportable), used only for signing handshake
 * challenges with SHA256withECDSA.
 */
class KeystoreDeviceSigner(private val alias: String = ALIAS) : DeviceSigner {
    private val keyStore: KeyStore = KeyStore.getInstance(PROVIDER).apply { load(null) }

    init {
        if (!keyStore.containsAlias(alias)) generate()
    }

    private fun generate() {
        val spec = KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_SIGN)
            .setAlgorithmParameterSpec(ECGenParameterSpec("secp256r1"))
            .setDigests(KeyProperties.DIGEST_SHA256)
            .build()
        val gen = KeyPairGenerator.getInstance(KeyProperties.KEY_ALGORITHM_EC, PROVIDER)
        gen.initialize(spec)
        gen.generateKeyPair()
    }

    override fun publicKeySpki(): ByteArray =
        keyStore.getCertificate(alias).publicKey.encoded

    override fun sign(data: ByteArray): ByteArray {
        val key = keyStore.getKey(alias, null) as PrivateKey
        val s = Signature.getInstance("SHA256withECDSA")
        s.initSign(key)
        s.update(data)
        return s.sign()
    }

    companion object {
        const val ALIAS = "helm_device_key"
        private const val PROVIDER = "AndroidKeyStore"
    }
}
