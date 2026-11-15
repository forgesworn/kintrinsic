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
        at: Long? = null,
        wrapId: String? = null,
    ) = postEmergencyAlert(
        ctx,
        key = alertKey("override", machine, at, wrapId),
        title = "Emergency unlock used",
        text = overrideText(
            scope,
            (durationSecs / 60).coerceAtLeast(1),
            describeWard(machine, roster),
        ),
        whenMillis = plausibleWhenMillis(at, System.currentTimeMillis() / 1000),
    )

    /**
     * The ward used the emergency-call button on their lock screen (device
     * audit `outcome=override`, `op=call.emergency`). It means the emergency
     * dialler opened, not that a call was placed, so the words say exactly that.
     * Same loud high-importance channel and public lockscreen as [notifyOverride].
     */
    fun notifyEmergencyCall(
        ctx: Context,
        machine: String,
        roster: List<RosterEntry> = emptyList(),
        at: Long? = null,
        wrapId: String? = null,
    ) = postEmergencyAlert(
        ctx,
        key = alertKey("emergency-call", machine, at, wrapId),
        title = EMERGENCY_CALL_TITLE,
        text = emergencyCallText(describeWard(machine, roster)),
        whenMillis = plausibleWhenMillis(at, System.currentTimeMillis() / 1000),
    )

    /**
     * Any other `outcome=override` op from a newer ward: honest and generic,
     * never the unlock wording (nothing was unlocked as far as we know).
     */
    fun notifyEmergencyControl(
        ctx: Context,
        machine: String,
        roster: List<RosterEntry> = emptyList(),
        at: Long? = null,
        wrapId: String? = null,
    ) = postEmergencyAlert(
        ctx,
        key = alertKey("emergency-control", machine, at, wrapId),
        title = EMERGENCY_CONTROL_TITLE,
        text = emergencyControlText(describeWard(machine, roster)),
        whenMillis = plausibleWhenMillis(at, System.currentTimeMillis() / 1000),
    )

    /**
     * One loud alert per PRESS: the row id is per press (see [alertKey]), so two
     * presses stack, and a rebuilt wrap for the same press reuses its row and,
     * with `setOnlyAlertOnce`, does not buzz again.
     */
    private fun postEmergencyAlert(
        ctx: Context,
        key: String,
        title: String,
        text: String,
        whenMillis: Long?,
    ) {
        val tap = PendingIntent.getActivity(
            ctx,
            key.hashCode(),
            Intent(ctx, MainActivity::class.java)
                .putExtra(MainActivity.EXTRA_ROUTE, MainActivity.ROUTE_ACTIVITY)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val b = Notification.Builder(ctx, CH_APPROVALS)
            .setSmallIcon(android.R.drawable.stat_sys_warning)
            .setContentTitle(title)
            .setContentText(text)
            // The whole sentence, not a truncated line.
            .setStyle(Notification.BigTextStyle().bigText(text))
            // Shown in full on the lock screen: a guardian must be able to read
            // an emergency without unlocking their own phone.
            .setVisibility(Notification.VISIBILITY_PUBLIC)
            .setOnlyAlertOnce(true)
            .setContentIntent(tap)
            .setAutoCancel(true)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setFullScreenIntent(tap, true)
        // The press time, not the time the guardian's phone heard of it.
        if (whenMillis != null) b.setWhen(whenMillis).setShowWhen(true)
        val nm = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(key.hashCode(), b.build())
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

    internal const val EMERGENCY_CALL_TITLE = "Emergency call button used"
    internal const val EMERGENCY_CONTROL_TITLE = "Emergency control used"

    /** A press timestamp is believed only when it is not in the future and not older than this. */
    internal const val MAX_PRESS_AGE_SECS = 30L * 24 * 60 * 60

    /**
     * The row key for one press: `{kind}-{machine}-{at}` when the ward stamped a
     * press time (a wrap rebuilt after a day carries the same `at`, so it reuses
     * its row), else `{kind}-{machine}-wrap-{wrapId}`, else per machine.
     */
    internal fun alertKey(kind: String, machine: String, at: Long?, wrapId: String?): String = when {
        at != null -> "$kind-$machine-$at"
        wrapId != null -> "$kind-$machine-wrap-$wrapId"
        else -> "$kind-$machine"
    }

    /**
     * The notification's `when` in milliseconds: the press time when it is
     * plausible (not later than now, not older than [MAX_PRESS_AGE_SECS]), else
     * null (the platform stamps it on arrival).
     */
    internal fun plausibleWhenMillis(at: Long?, nowSecs: Long): Long? =
        if (at != null && at in 0..nowSecs && nowSecs - at <= MAX_PRESS_AGE_SECS) at * 1000 else null

    /** What an audit wrap asks of the carrier. */
    internal enum class AuditAction { UNLOCK, EMERGENCY_CALL, OTHER_CONTROL, IGNORE }

    /**
     * Pure dispatch for a device audit, so it is JVM-testable. Only
     * `outcome=override` alerts (anything else, such as a screen-lock reset
     * result, is dropped). Within it: an empty op or `unlock.breakglass` is the
     * emergency unlock (an older ward sent no op), `call.emergency` is the
     * emergency-call button, and any other op is a generic honest alert rather
     * than the unlock wording.
     */
    internal fun auditAction(outcome: String, op: String): AuditAction {
        if (outcome != "override") return AuditAction.IGNORE
        return when (op) {
            "", "unlock.breakglass" -> AuditAction.UNLOCK
            "call.emergency" -> AuditAction.EMERGENCY_CALL
            else -> AuditAction.OTHER_CONTROL
        }
    }

    /** Pure text for [notifyEmergencyCall] — see [overrideText]. */
    internal fun emergencyCallText(who: WardName?): String =
        if (who != null) {
            "${who.childName} used the emergency call button on ${who.deviceLabel}."
        } else {
            "Your ward used the emergency call button."
        }

    /** Pure text for [notifyEmergencyControl] — see [overrideText]. */
    internal fun emergencyControlText(who: WardName?): String =
        if (who != null) {
            "${who.childName}: an emergency control was used on ${who.deviceLabel}"
        } else {
            "Your ward: an emergency control was used"
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
