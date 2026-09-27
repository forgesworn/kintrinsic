package org.forgesworn.charter.service

import android.app.AlarmManager
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import org.forgesworn.charter.admin.Provisioning

/**
 * When the liveness alarm should make the warden act (05-B2/05-G1). Pure, so
 * the pacing is unit-tested; every time is `SystemClock.elapsedRealtime()`,
 * which keeps counting through deep sleep (a `Handler`'s uptime clock does
 * not, which is the whole bug).
 */
internal object LivenessPolicy {
    /** The alarm's period: the dark enforcement pace the port-spec asks for. */
    const val ALARM_MS = 60_000L

    /**
     * Least gap since the last enforcement tick before an alarm runs one of
     * its own while the screen is dark. Below the alarm period, so every dark
     * alarm ticks unless a tick has only just run.
     */
    const val DARK_KICK_GAP_MS = 20_000L

    /** A tick that has run this long is wedged: the process is restarted. */
    const val WEDGED_MS = 5 * 60_000L

    /**
     * A relay poll that has run this long is hung. Longer than the tick's
     * limit: a healthy poll is several relay round trips with 16 s timeouts
     * each, so minutes are legitimate.
     */
    const val POLL_WEDGED_MS = 10 * 60_000L

    /**
     * Whether the alarm should run an enforcement tick now. While lit, the
     * alarm is only a watchdog (the handler ticks every [tickMs]), so it acts
     * when the loop has missed three beats. While dark, the alarm IS the
     * cadence. A tick that never ran is always due.
     */
    fun tickDue(nowMs: Long, lastTickMs: Long, interactive: Boolean, tickMs: Long): Boolean {
        if (lastTickMs == 0L) return true
        val gap = nowMs - lastTickMs
        return gap >= if (interactive) 3 * tickMs else DARK_KICK_GAP_MS
    }

    /**
     * Whether the alarm should run a relay poll now: when the poll is a whole
     * dark period behind, or three lit periods behind. A poll that never ran
     * is due.
     */
    fun pollDue(nowMs: Long, lastPollMs: Long, interactive: Boolean, pollMs: Long, darkPollMs: Long): Boolean {
        if (lastPollMs == 0L) return true
        val gap = nowMs - lastPollMs
        return gap >= if (interactive) 3 * pollMs else darkPollMs
    }

    /** Whether the tick that started at [tickStartedMs] (0 = none running) is wedged. */
    fun wedged(nowMs: Long, tickStartedMs: Long): Boolean =
        tickStartedMs != 0L && nowMs - tickStartedMs >= WEDGED_MS

    /** Whether the poll that started at [pollStartedMs] (0 = none running) is hung. */
    fun pollWedged(nowMs: Long, pollStartedMs: Long): Boolean =
        pollStartedMs != 0L && nowMs - pollStartedMs >= POLL_WEDGED_MS
}

/**
 * The alarm that keeps the warden alive and ticking when a `Handler` cannot
 * (05-B2/05-G1). The loop's `Handler` timers stop while the CPU sleeps, and in
 * Doze that is hours, so the dark tick and the relay poll simply did not
 * happen. An `AllowWhileIdle` alarm fires through Doze and, because the
 * `PendingIntent` is held by the system, also restarts a process that was
 * killed: the receiver starts the service again.
 *
 * Exact where the platform lets us (`USE_EXACT_ALARM`); inexact otherwise.
 * Outside the power-save allowlist Doze still rate-limits allow-while-idle
 * alarms (roughly one per nine minutes), which is a slower cadence but never
 * none. See the liveness phone-test script for the allowlist step.
 *
 * A force-stop cancels every alarm and nothing an app does survives it; on a
 * release ward, Settings disables Force stop for an active Device Owner and
 * adb is off (DISALLOW_DEBUGGING_FEATURES), and the boot receiver re-arms on
 * the next boot.
 */
object LivenessAlarm {
    private const val TAG = "LivenessAlarm"
    const val ACTION_ALARM = "org.forgesworn.charter.LIVENESS"

    fun arm(context: Context) {
        runCatching {
            val am = context.getSystemService(AlarmManager::class.java)
            val at = SystemClock.elapsedRealtime() + LivenessPolicy.ALARM_MS
            val pi = pendingIntent(context)
            if (am.canScheduleExactAlarms()) {
                am.setExactAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pi)
            } else {
                am.setAndAllowWhileIdle(AlarmManager.ELAPSED_REALTIME_WAKEUP, at, pi)
            }
        }.onFailure { Log.e(TAG, "could not arm the liveness alarm", it) }
    }

    private fun pendingIntent(context: Context): PendingIntent = PendingIntent.getBroadcast(
        context,
        0,
        Intent(context, LivenessReceiver::class.java).setAction(ACTION_ALARM),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )
}

/**
 * Receives the liveness alarm: re-arms it first (so one failure below cannot
 * end the chain), then hands the service a kick. The system holds a wake lock
 * only while [onReceive] runs, so a short bridging wake lock carries the CPU
 * until the service has taken its own; the service releases it.
 */
class LivenessReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != LivenessAlarm.ACTION_ALARM) return
        // Not a Device Owner: nothing to keep alive, and the chain ends here.
        if (!Provisioning.isDeviceOwner(context)) return
        LivenessAlarm.arm(context)
        acquireBridge(context)
        runCatching { CharterService.kick(context) }.onFailure {
            releaseBridge()
            Log.e(TAG, "could not kick the warden", it)
        }
    }

    companion object {
        private const val TAG = "LivenessReceiver"
        private const val BRIDGE_MS = 10_000L
        @Volatile private var bridge: PowerManager.WakeLock? = null

        private fun acquireBridge(context: Context) {
            runCatching {
                val pm = context.getSystemService(PowerManager::class.java)
                val wl = bridge ?: pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "charter:liveness-bridge")
                    .apply { setReferenceCounted(false) }
                    .also { bridge = it }
                wl.acquire(BRIDGE_MS)
            }
        }

        /** Called by the service once it holds its own wake lock. */
        fun releaseBridge() {
            runCatching { bridge?.takeIf { it.isHeld }?.release() }
        }
    }
}
