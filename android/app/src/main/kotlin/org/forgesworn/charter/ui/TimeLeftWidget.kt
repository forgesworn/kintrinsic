package org.forgesworn.charter.ui

import android.appwidget.AppWidgetManager
import android.appwidget.AppWidgetProvider
import android.content.Context
import android.widget.RemoteViews
import org.forgesworn.charter.R
import org.forgesworn.charter.native.CharterCore

/**
 * The ward's at-a-glance mirror (spec D8): a home-screen widget with the
 * minutes left today. Fed level-triggered from the CharterService tick (only
 * repaints when the text changes) plus the launcher's own onUpdate. JNI reads
 * happen off the calling thread when it might be main.
 */
class TimeLeftWidget : AppWidgetProvider() {

    override fun onUpdate(
        context: Context,
        appWidgetManager: AppWidgetManager,
        appWidgetIds: IntArray,
    ) {
        // Launcher callbacks arrive on the main thread — JNI stays off it.
        Thread { push(context, force = true) }.start()
    }

    companion object {
        @Volatile private var lastShown: String? = null

        /**
         * Repaint every widget instance with the current time-left. Safe from
         * any WORKER thread (takes the warden lock via JNI). No-op when
         * nothing changed or no widgets are placed.
         */
        fun push(context: Context, force: Boolean = false) {
            val mgr = AppWidgetManager.getInstance(context) ?: return
            val ids = mgr.getAppWidgetIds(
                android.content.ComponentName(context, TimeLeftWidget::class.java),
            )
            if (ids.isEmpty()) return

            val view = runCatching {
                CharterCore.scheduleView(System.currentTimeMillis() / 1000)
            }.getOrNull()
            // One wording for the widget and the screen behind it (HomeCopy).
            val (big, small) = HomeCopy.headline(view).let { it.big to it.caption }
            val key = "$big|$small"
            if (!force && key == lastShown) return
            lastShown = key

            val rv = RemoteViews(context.packageName, R.layout.widget_time_left).apply {
                setTextViewText(R.id.widget_minutes, big)
                setTextViewText(R.id.widget_caption, small)
                // Tap → the Kintrinsic app (the D8 mirror lives on its main screen).
                setOnClickPendingIntent(
                    R.id.widget_root,
                    android.app.PendingIntent.getActivity(
                        context,
                        0,
                        android.content.Intent(context, org.forgesworn.charter.MainActivity::class.java)
                            .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
                        android.app.PendingIntent.FLAG_IMMUTABLE,
                    ),
                )
            }
            for (id in ids) runCatching { mgr.updateAppWidget(id, rv) }
        }
    }
}
