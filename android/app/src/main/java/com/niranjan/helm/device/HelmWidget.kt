package com.niranjan.helm.device

import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.graphics.Color
import android.view.View
import android.widget.RemoteViews
import com.niranjan.helm.QUICK_DECK
import com.niranjan.helm.QUICK_MEDIA
import com.niranjan.helm.R

/** Home-screen widget (RemoteViews): now playing + media keys + the first four deck buttons. */
class HelmWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        val views = render(context, HelmBackground.state())
        for (id in ids) manager.updateAppWidget(id, views)
    }

    companion object {
        private val DECK_IDS = intArrayOf(R.id.w_d0, R.id.w_d1, R.id.w_d2, R.id.w_d3)

        fun updateAll(context: Context, st: BackgroundState) {
            val manager = AppWidgetManager.getInstance(context) ?: return
            val ids = runCatching { manager.getAppWidgetIds(ComponentName(context, HelmWidget::class.java)) }.getOrNull() ?: return
            if (ids.isEmpty()) return
            val views = render(context, st)
            for (id in ids) runCatching { manager.updateAppWidget(id, views) }
        }

        private fun render(context: Context, st: BackgroundState): RemoteViews {
            val v = RemoteViews(context.packageName, R.layout.widget_helm)
            val m = st.media
            v.setTextViewText(R.id.w_title, m?.title?.ifBlank { null } ?: st.pcName ?: "Helm")
            v.setTextViewText(
                R.id.w_sub,
                when {
                    m != null -> m.artist
                    st.connected -> "Connected"
                    else -> "Tap a button to connect"
                },
            )
            v.setImageViewResource(R.id.w_play, if (m?.isPlaying == true) android.R.drawable.ic_media_pause else android.R.drawable.ic_media_play)
            v.setOnClickPendingIntent(R.id.w_prev, QuickActionReceiver.pending(context, QUICK_MEDIA, "previous", 101))
            v.setOnClickPendingIntent(R.id.w_play, QuickActionReceiver.pending(context, QUICK_MEDIA, "toggle_play_pause", 102))
            v.setOnClickPendingIntent(R.id.w_next, QuickActionReceiver.pending(context, QUICK_MEDIA, "next", 103))
            DECK_IDS.forEachIndexed { i, viewId ->
                val b = st.deck.getOrNull(i)
                if (b == null) {
                    v.setViewVisibility(viewId, View.INVISIBLE)
                } else {
                    v.setViewVisibility(viewId, View.VISIBLE)
                    v.setTextViewText(viewId, b.label.ifBlank { "Button ${i + 1}" })
                    v.setInt(viewId, "setBackgroundColor", runCatching { Color.parseColor(b.color) }.getOrDefault(0xFF3B82F6.toInt()))
                    v.setOnClickPendingIntent(viewId, QuickActionReceiver.pending(context, QUICK_DECK, b.id, 110 + i))
                }
            }
            return v
        }
    }
}
