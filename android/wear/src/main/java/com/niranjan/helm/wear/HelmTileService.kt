package com.niranjan.helm.wear

import androidx.concurrent.futures.CallbackToFutureAdapter
import androidx.wear.protolayout.ActionBuilders
import androidx.wear.protolayout.ColorBuilders.argb
import androidx.wear.protolayout.DeviceParametersBuilders.DeviceParameters
import androidx.wear.protolayout.ModifiersBuilders
import androidx.wear.protolayout.ResourceBuilders
import androidx.wear.protolayout.TimelineBuilders
import androidx.wear.protolayout.material.Button
import androidx.wear.protolayout.material.Text
import androidx.wear.protolayout.material.Typography
import androidx.wear.protolayout.material.layouts.MultiButtonLayout
import androidx.wear.protolayout.material.layouts.PrimaryLayout
import androidx.wear.tiles.RequestBuilders
import androidx.wear.tiles.TileBuilders
import androidx.wear.tiles.TileService
import com.google.common.util.concurrent.ListenableFuture
import com.niranjan.helm.wearlink.WearAction
import com.niranjan.helm.wearlink.WearStatus
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull

/**
 * "Helm PC" tile: PC name / state plus previous, play-pause, next, mute and lock. A tap is a
 * LoadAction: the tile is re-requested with the clicked id, we relay it to the phone, then
 * redraw with the phone's latest status.
 */
class HelmTileService : TileService() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun onTileRequest(requestParams: RequestBuilders.TileRequest): ListenableFuture<TileBuilders.Tile> =
        CallbackToFutureAdapter.getFuture { completer ->
            val link = PhoneLink.get(this).also { it.start() }
            scope.launch {
                var note: String? = null
                actionFor(requestParams.currentState.lastClickableId)?.let { a ->
                    val r = withTimeoutOrNull(8_000) { link.send(a) }
                    if (r != null && !r.ok) note = r.message
                }
                val status = withTimeoutOrNull(4_000) { link.peek() }
                completer.set(tile(requestParams.deviceConfiguration, status, note))
            }
            "helm-tile"
        }

    override fun onTileResourcesRequest(requestParams: RequestBuilders.ResourcesRequest): ListenableFuture<ResourceBuilders.Resources> =
        CallbackToFutureAdapter.getFuture { completer ->
            completer.set(ResourceBuilders.Resources.Builder().setVersion(RESOURCES_VERSION).build())
            "helm-tile-res"
        }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private fun tile(device: DeviceParameters, status: WearStatus?, note: String?): TileBuilders.Tile {
        val connected = status?.connected == true
        val title = when {
            status == null -> "Phone not found"
            status.pcName == null -> "No PC"
            else -> status.pcName!!
        }
        val detail = note ?: if (connected) "Connected" else "Not connected"

        val buttons = MultiButtonLayout.Builder()
        for ((id, glyph) in listOf(
            ID_PREV to "⏮",
            ID_PLAY to "⏯",
            ID_NEXT to "⏭",
            ID_MUTE to if (status?.muted == true) "🔇" else "🔈",
            ID_LOCK to "🔒",
        )) {
            buttons.addButtonContent(Button.Builder(this, clickable(id)).setTextContent(glyph).build())
        }

        val layout = PrimaryLayout.Builder(device)
            .setResponsiveContentInsetEnabled(true)
            .setPrimaryLabelTextContent(
                Text.Builder(this, title).setTypography(Typography.TYPOGRAPHY_CAPTION1).setColor(argb(0xFFD3E4FF.toInt())).build(),
            )
            .setContent(buttons.build())
            .setSecondaryLabelTextContent(
                Text.Builder(this, detail).setTypography(Typography.TYPOGRAPHY_CAPTION2).setColor(argb(0xFFAAB4C3.toInt())).build(),
            )
            .build()

        return TileBuilders.Tile.Builder()
            .setResourcesVersion(RESOURCES_VERSION)
            .setFreshnessIntervalMillis(5 * 60_000L)
            .setTileTimeline(TimelineBuilders.Timeline.fromLayoutElement(layout))
            .build()
    }

    private fun clickable(id: String) = ModifiersBuilders.Clickable.Builder()
        .setId(id)
        .setOnClick(ActionBuilders.LoadAction.Builder().build())
        .build()

    private fun actionFor(id: String?): WearAction? = when (id) {
        ID_PREV -> WearAction.PREVIOUS
        ID_PLAY -> WearAction.PLAY_PAUSE
        ID_NEXT -> WearAction.NEXT
        ID_MUTE -> WearAction.MUTE
        ID_LOCK -> WearAction.LOCK
        else -> null
    }

    private companion object {
        const val RESOURCES_VERSION = "1"
        const val ID_PREV = "prev"
        const val ID_PLAY = "play"
        const val ID_NEXT = "next"
        const val ID_MUTE = "mute"
        const val ID_LOCK = "lock"
    }
}
