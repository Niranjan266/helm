package com.niranjan.helm.device

import android.app.PendingIntent
import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.widget.RemoteViews
import com.niranjan.helm.HelmApp
import com.niranjan.helm.R
import com.niranjan.helm.core.WidgetPower
import com.niranjan.helm.ui.MainActivity
import com.niranjan.helm.ui.PowerActionActivity

/**
 * Home-screen power widget (RemoteViews): the saved PC's name and whether it is connected, plus
 * Lock, Sleep, Shut down and a shortcut to the Screen tab.
 *
 * Power buttons open [PowerActionActivity], a see-through screen that asks before shutting down
 * and runs the per-PC fingerprint check when it is on; the command itself is then sent by
 * [QuickActionReceiver] over the saved-PC connection, with a toast for the result.
 */
class PowerWidget : AppWidgetProvider() {
    override fun onUpdate(context: Context, manager: AppWidgetManager, ids: IntArray) {
        val views = render(context, HelmBackground.state())
        for (id in ids) manager.updateAppWidget(id, views)
    }

    companion object {
        private const val REQ_BASE = 140
        private const val REQ_SCREEN = 150
        private const val REQ_OPEN = 151

        fun updateAll(context: Context, st: BackgroundState) {
            val manager = AppWidgetManager.getInstance(context) ?: return
            val ids = runCatching { manager.getAppWidgetIds(ComponentName(context, PowerWidget::class.java)) }.getOrNull() ?: return
            if (ids.isEmpty()) return
            val views = render(context, st)
            for (id in ids) runCatching { manager.updateAppWidget(id, views) }
        }

        private fun render(context: Context, st: BackgroundState): RemoteViews {
            val v = RemoteViews(context.packageName, R.layout.widget_power)
            val saved = (context.applicationContext as? HelmApp)?.controller?.savedPcs?.pcs?.value?.firstOrNull()?.name
            val header = WidgetPower.header(st.pcName, saved, st.connected)
            v.setTextViewText(R.id.wp_title, header.title)
            v.setTextViewText(R.id.wp_status, header.status)
            v.setImageViewResource(R.id.wp_dot, if (header.online) R.drawable.widget_dot_online else R.drawable.widget_dot_offline)

            val buttons = mapOf(
                WidgetPower.LOCK to R.id.wp_lock,
                WidgetPower.SLEEP to R.id.wp_sleep,
                WidgetPower.SHUTDOWN to R.id.wp_shutdown,
            )
            WidgetPower.ACTIONS.forEachIndexed { i, action ->
                val viewId = buttons.getValue(action)
                v.setTextViewText(viewId, WidgetPower.label(action))
                v.setOnClickPendingIntent(viewId, powerIntent(context, action, REQ_BASE + i))
            }
            v.setOnClickPendingIntent(R.id.wp_screen, openApp(context, MainActivity.screenIntent(context, "SCREEN"), REQ_SCREEN))
            v.setOnClickPendingIntent(
                R.id.wp_header,
                openApp(context, Intent(context, MainActivity::class.java), REQ_OPEN),
            )
            return v
        }

        private fun powerIntent(context: Context, action: String, req: Int): PendingIntent =
            PendingIntent.getActivity(
                context, req,
                Intent(context, PowerActionActivity::class.java)
                    .putExtra(PowerActionActivity.EXTRA_ACTION, action)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK or Intent.FLAG_ACTIVITY_NO_ANIMATION),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )

        private fun openApp(context: Context, intent: Intent, req: Int): PendingIntent =
            PendingIntent.getActivity(
                context, req,
                intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
                PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
            )
    }
}
