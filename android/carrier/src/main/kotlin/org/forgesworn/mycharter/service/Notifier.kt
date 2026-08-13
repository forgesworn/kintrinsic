package org.forgesworn.mycharter.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import org.forgesworn.mycharter.MainActivity
import org.forgesworn.mycharter.R
import org.forgesworn.mycharter.carrier.RosterEntry
import org.forgesworn.mycharter.carrier.WardName
import org.forgesworn.mycharter.carrier.describeWard

/**
 * Notification surfaces. Two channels: the quiet persistent one the foreground
 * service is required to show, and the URGENT one a ward's ask rides in
 * (heads-up + sound + lockscreen). Copy uses the wardship lexicon.
 */
object Notifier {
    private const val CH_SERVICE = "carrier.service"
    private const val CH_APPROVALS = "carrier.approvals"
    const val SERVICE_NOTIF_ID = 1

    fun ensureChannels(ctx: Context) {
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(
                CH_SERVICE,
                ctx.getString(R.string.notif_channel_service),
                NotificationManager.IMPORTANCE_MIN,
            ),
        )
        nm.createNotificationChannel(
            NotificationChannel(
                CH_APPROVALS,
                ctx.getString(R.string.notif_channel_approvals),
                NotificationManager.IMPORTANCE_HIGH,
            ).apply {
                lockscreenVisibility = Notification.VISIBILITY_PUBLIC
                enableVibration(true)
            },
        )
    }

    fun serviceNotification(ctx: Context): Notification =
        Notification.Builder(ctx, CH_SERVICE)
            .setSmallIcon(android.R.drawable.stat_notify_sync_noanim)
            .setContentTitle("Kintrinsic is listening for ward requests")
            // Tap → the console (a persistent row you can't act on is a dead
            // end; same rule as the ask notification's Approvals deep-link).
            .setContentIntent(
                PendingIntent.getActivity(
                    ctx,
                    0,
                    Intent(ctx, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                ),
            )
            .setOngoing(true)
            .build()

    /**
     * The ward used their emergency unlock. Not an approval request — there
     * is nothing to approve, it already happened — this is the LOUD half of
     * the break-glass bargain (design memo 2026-07-24). Warm, not alarming:
     * the ward may be in trouble, and the product's answer to overuse is a
     * conversation, never a lockout.
     */
    fun notifyOverride(
        ctx: Context,
        machine: String,
        scope: String,
        durationSecs: Long,
        roster: List<RosterEntry> = emptyList(),
    ) {
        val tap = PendingIntent.getActivity(
            ctx,
            "override".hashCode(),
            Intent(ctx, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_ROUTE, MainActivity.ROUTE_ACTIVITY)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val mins = (durationSecs / 60).coerceAtLeast(1)
        val n = Notification.Builder(ctx, CH_APPROVALS)
            .setSmallIcon(android.R.drawable.stat_sys_warning)
            .setContentTitle("Emergency unlock used")
            .setContentText(overrideText(scope, mins, describeWard(machine, roster)))
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setFullScreenIntent(tap, true)
            .build()
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        // Keyed by machine: a second override replaces the row rather than
        // stacking — one live "this is happening now" per phone.
        nm.notify("override-$machine".hashCode(), n)
    }

    /** The ask itself. Tap → the console's Approvals screen. */
    fun notifyRequest(
        ctx: Context,
        reqId: String,
        op: String,
        minutes: Long?,
        machine: String,
        roster: List<RosterEntry> = emptyList(),
    ) {
        val tap = PendingIntent.getActivity(
            ctx,
            reqId.hashCode(),
            Intent(ctx, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_ROUTE, MainActivity.ROUTE_APPROVALS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val text = requestText(op, minutes, describeWard(machine, roster))
        val n = Notification.Builder(ctx, CH_APPROVALS)
            .setSmallIcon(android.R.drawable.stat_notify_more)
            .setContentTitle("Kintrinsic request")
            .setContentText(text)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_MESSAGE)
            // Full-screen intent: lights the screen where permitted (Android
            // 14+ may quietly downgrade to heads-up — acceptable fallback; a
            // dedicated keyguard-safe alert activity is a hardware-round item).
            .setFullScreenIntent(tap, true)
            .build()
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        // One notification per reqId — a replayed wrap never re-alerts anyway
        // (store dedupe), but distinct asks each get their own row.
        nm.notify(reqId.hashCode(), n)
    }

    /**
     * Pure text for [notifyOverride] — split out so the no-hit path (today's
     * exact wording) is JVM-testable without a [Context] to build a real
     * [Notification]. `who == null` (empty/stale/no-hit roster) MUST render
     * verbatim what shipped before the roster existed — never a guessed name.
     */
    internal fun overrideText(scope: String, mins: Long, who: WardName?): String {
        val what = if (scope == "full") "their phone" else "calls"
        return if (who != null) {
            "${who.childName} opened $what (${who.deviceLabel}) for $mins minutes."
        } else {
            "Your ward opened $what for $mins minutes."
        }
    }

    /** Pure text for [notifyRequest] — see [overrideText]. */
    internal fun requestText(op: String, minutes: Long?, who: WardName?): String {
        val subject = if (who != null) "${who.childName} (${who.deviceLabel})" else "Your ward"
        return when {
            op == "time.extend" && minutes != null -> "$subject asks for $minutes more minutes"
            op == "install.apk" -> "$subject asks to install an app"
            else -> "$subject sent a request"
        }
    }
}
