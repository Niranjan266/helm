package com.niranjan.helm.core

import java.util.Base64
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put

/** G5 calls (Docs/protocol/dock-deck-2.md). Older agents still get Phase 1 dock/deck calls. */
val HelmConnection.supportsG5: Boolean get() = G5.FEATURE in hostFeatures

/** Dock with folders on a G5 agent; the flat Phase 1 dock otherwise. */
suspend fun HelmApi.dock2(): List<List<DockTile>> =
    if (conn.supportsG5) DockCodec.decode(call("dock.get", buildJsonObject { put("v", 2) })) else dock()

/** Deck with pages (a Phase 1 agent's deck is one "Main" page). */
suspend fun HelmApi.deck2(): DeckLayout =
    if (conn.supportsG5) DeckPages.decode(call("deck.get", buildJsonObject { put("v", 2) }))
    else deck().let { DeckLayout(it, DeckPages.normalize(emptyList(), it)) }

suspend fun HelmApi.setDeck2(layout: DeckLayout) {
    if (conn.supportsG5) call("deck.set", DeckPages.encode(layout)) else setDeck(layout.buttons)
}

/** Most used apps first. */
suspend fun HelmApi.appUsage(limit: Int = 500): List<AppUsage> =
    AppUsage.listFrom(gated(G5.FEATURE, "apps.usage", buildJsonObject { put("limit", limit) }))

/** Uploads a PNG/JPEG; returns the icon reference (`c:<hash>`). */
suspend fun HelmApi.putIcon(image: ByteArray): String? =
    gated(G5.FEATURE, "deck.icon.put", buildJsonObject { put("png", Base64.getEncoder().encodeToString(image)) }, timeoutMs = 30_000)
        .optStr("icon")

suspend fun HelmApi.getIcon(icon: String): ByteArray? =
    gated(G5.FEATURE, "deck.icon.get", buildJsonObject { put("icon", icon) }).optStr("png")?.let {
        try {
            Base64.getDecoder().decode(it)
        } catch (_: IllegalArgumentException) {
            null
        }
    }
