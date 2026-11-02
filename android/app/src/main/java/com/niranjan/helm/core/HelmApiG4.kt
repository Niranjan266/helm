package com.niranjan.helm.core

import kotlinx.serialization.json.JsonObject
import kotlinx.serialization.json.JsonPrimitive
import kotlinx.serialization.json.booleanOrNull
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

// g4 (Docs/protocol/pairing-security.md): the PC's pairing settings. Admin tier, gated on `g4`.

/** `pairing.settings.get` -> "ask on the PC before a new phone pairs". */
suspend fun HelmApi.pairingRequiresApproval(): Boolean =
    (gated(Features.G4, "pairing.settings.get")["require_approval"] as? JsonPrimitive)?.booleanOrNull == true

/** `pairing.settings.set{require_approval}`; returns the saved value. */
suspend fun HelmApi.setPairingRequiresApproval(on: Boolean): Boolean {
    val r: JsonObject = gated(Features.G4, "pairing.settings.set", buildJsonObject { put("require_approval", on) })
    return (r["require_approval"] as? JsonPrimitive)?.booleanOrNull == true
}
