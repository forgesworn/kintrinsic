package org.forgesworn.charter.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.PowerManager
import android.os.SystemClock
import android.util.Log
import org.forgesworn.charter.MainActivity

/**
 * The loop host (port-spec §3.3) — the Android analog of charterd's run loop.
 * A foreground service that drives [WardenController.tickAndApply] on a worker
 * thread. Started by [BootReceiver] on boot/replace and by the DPC on provision;
 * enforcement re-arms level-triggered on every (re)start, so a crash/restart
 * self-heals rather than losing (or getting stuck in) a lock.
 *
 * ## Why the loop watches the screen
 *
 * A ward's phone spends most of the day in a pocket, and a dark screen is the
 * one state in which Kintrinsic has nothing to decide: no app is being used, so no
 * time accrues, nobody is waiting at a lock screen for an answer, and no
 * warning is worth showing. Ticking twice a second and opening a fresh relay
 * connection four times a minute through all of that was the whole background
 * drain — thousands of needless CPU and radio wake-ups a day on a phone doing
 * nothing (decented, 2026-08-01).
 *
 * So both loops slow right down while the screen is off and snap back the
 * instant it lights, before the ward can reach anything. Nothing here changes
 * *what* is enforced: every decision stays level-triggered off the stored
 * charter, so a slow tick lands the same lock a fast one would, only later —
 * and "later" is bounded by the screen coming on, which is the first moment a
 * lock could matter. The relay poll's own cursor self-heals a missed window.
 *
 * ## Why an alarm as well
 *
 * A `Handler` timer runs on the uptime clock, which stops while the CPU
 * sleeps, so in Doze the dark tick and the dark poll did not happen for hours
 * (05-B2), and nothing brought the warden back if the process was killed
 * (05-G1). [LivenessAlarm] fires through Doze every minute and [kick]s the
 * loops: it runs a tick or a poll that is overdue, holding a wake lock only
 * for as long as that takes, and restarts the service if it had died. It is
 * also the watchdog the port-spec names: a tick wedged for
 * [LivenessPolicy.WEDGED_MS] restarts the process.
 */
class CharterService : Service() {

    private lateinit var worker: HandlerThread
    private lateinit var handler: Handler
    private lateinit var slowWorker: HandlerThread
    private lateinit var slowHandler: Handler
    private lateinit var controller: WardenController
    @Volatile private var running = false
    /** The screen's state as the LOOP understands it — see [screenReceiver]. */
    @Volatile private var interactive = true
    /** Both loops are running and may be re-paced — see [repace]. */
    @Volatile private var armed = false

    /** When the last enforcement tick / relay poll finished, and when the
     *  running tick (if any) started — `elapsedRealtime`, which counts deep
     *  sleep. 0 = never / none. Read by [kick]. */
    @Volatile private var lastTickAt = 0L
    @Volatile private var tickStartedAt = 0L
    @Volatile private var lastPollAt = 0L
    @Volatile private var pollStartedAt = 0L

    private var tickWake: PowerManager.WakeLock? = null
    private var pollWake: PowerManager.WakeLock? = null

    /** One enforcement tick, timed for the watchdog. Worker thread only. */
    private fun runTick(screenWas: Boolean? = null) {
        tickStartedAt = SystemClock.elapsedRealtime()
        try {
            controller.tickAndApply(screenWas = screenWas)
            // The D8 widget rides the tick (worker thread — JNI-safe);
            // level-triggered inside push(), so unchanged minutes are free.
            if (screenWas == null) org.forgesworn.charter.ui.TimeLeftWidget.push(this)
        } catch (t: Throwable) {
            Log.e(TAG, "tick failed", t)
        } finally {
            tickStartedAt = 0L
            lastTickAt = SystemClock.elapsedRealtime()
        }
    }

    /** One relay round plus the install drain. Slow worker only. */
    private fun runPoll() {
        pollStartedAt = SystemClock.elapsedRealtime()
        try {
            val r = controller.pollOnce()
            // Every round logged: this is the bring-up visibility for the
            // on-metal gates (offline reasons are routine, not silent).
            Log.i(TAG, "poll: $r")
            // Same worker (both are blocking IO): enact any guardian-approved
            // installs the poll just delivered a grant for.
            val installed = controller.drainAndInstall()
            if (installed > 0) Log.i(TAG, "drained $installed install(s)")
        } catch (t: Throwable) {
            Log.e(TAG, "poll failed", t)
        } finally {
            pollStartedAt = 0L
            lastPollAt = SystemClock.elapsedRealtime()
        }
    }

    private val tick = object : Runnable {
        override fun run() {
            if (!running) return
            runTick()
            handler.postDelayed(this, if (interactive) TICK_MS else DARK_TICK_MS)
        }
    }

    /**
     * Screen on/off is the pace-setter for both loops. These are protected
     * system broadcasts and cannot be declared in the manifest, so the service
     * holds the registration for its own lifetime.
     *
     * Each edge banks the interval that just ended with the state it was
     * actually spent in ([WardenController.tickAndApply]'s `screenWas`), THEN
     * re-paces. Without that, waking the phone would credit the whole dark
     * minute since the last slow tick as screen time and quietly bill the ward
     * a minute of their day for every glance at the clock.
     */
    private val screenReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            when (intent?.action) {
                Intent.ACTION_SCREEN_ON -> repace(nowInteractive = true)
                Intent.ACTION_SCREEN_OFF -> repace(nowInteractive = false)
            }
        }
    }

    /**
     * Close the books on the interval that just ended and restart both loops at
     * the new pace. Ordering is guaranteed by the single-threaded loopers: the
     * banking tick is queued ahead of the re-armed loop, so the core sees the
     * dark interval before it sees the lit one.
     */
    private fun repace(nowInteractive: Boolean) {
        val wasInteractive = interactive
        interactive = nowInteractive
        // Before the loops are armed there is nothing to re-pace and nothing to
        // bank — and re-posting `tick` here would race onCreate's own arming
        // into TWO self-perpetuating chains, doubling the drain this change
        // exists to remove. Recording the state is enough: the arming that
        // follows reads it.
        if (!running || !armed) return
        handler.removeCallbacks(tick)
        handler.post {
            // Bank the closing interval as what it WAS, not what it is now.
            runTick(screenWas = wasInteractive)
        }
        handler.post(tick)
        slowHandler.removeCallbacks(slowTick)
        if (nowInteractive) {
            // Waking up is exactly when a guardian's answer, gift or lock is
            // worth having: poll at once rather than up to two minutes late.
            slowHandler.post(slowTick)
        } else {
            slowHandler.postDelayed(slowTick, DARK_SLOW_TICK_MS)
        }
    }

    // The slow (network) tick on its OWN thread: charterPollOnce blocks for
    // seconds on relay IO, and the Rust side releases the warden lock around
    // the network phases — so a slow relay costs a poll, never an enforcement
    // tick (the two-thread rule, port-spec §3.2).
    private val slowTick = object : Runnable {
        override fun run() {
            if (!running) return
            runPoll()
            slowHandler.postDelayed(this, if (interactive) SLOW_TICK_MS else DARK_SLOW_TICK_MS)
        }
    }

    override fun onCreate() {
        super.onCreate()
        startForeground(NOTIF_ID, notification())
        worker = HandlerThread("charter-worker").apply { start() }
        handler = Handler(worker.looper)
        slowWorker = HandlerThread("charter-slow-worker").apply { start() }
        slowHandler = Handler(slowWorker.looper)
        controller = WardenController.real(this)
        // Seed from the real screen rather than assuming lit: a service started
        // by BootReceiver or restarted by START_STICKY overnight would
        // otherwise spend the rest of the night at the fast pace.
        interactive = runCatching {
            (getSystemService(Context.POWER_SERVICE) as PowerManager).isInteractive
        }.getOrDefault(true)
        registerReceiver(
            screenReceiver,
            IntentFilter().apply {
                addAction(Intent.ACTION_SCREEN_ON)
                addAction(Intent.ACTION_SCREEN_OFF)
            },
            Context.RECEIVER_NOT_EXPORTED,
        )
        val pm = getSystemService(PowerManager::class.java)
        tickWake = runCatching {
            pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "charter:tick").apply { setReferenceCounted(false) }
        }.getOrNull()
        pollWake = runCatching {
            pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "charter:poll").apply { setReferenceCounted(false) }
        }.getOrNull()
        // Outside the power-save allowlist Doze rate-limits the liveness
        // alarm to roughly one every nine minutes. Said once, loudly, so a
        // phone test can tell the two cases apart.
        if (runCatching { pm.isIgnoringBatteryOptimizations(packageName) }.getOrDefault(false)) {
            Log.i(TAG, "on the power-save allowlist: the liveness alarm runs every minute in Doze")
        } else {
            Log.w(TAG, "NOT on the power-save allowlist: Doze may pace the liveness alarm to ~9 min")
        }
        running = true
        LivenessAlarm.arm(this)
        handler.post {
            // A native-load failure (a missing or mismatched library) throws
            // out of init; it must not kill the process into a START_STICKY
            // crash loop with the loops never armed (05-B13). The tick retries
            // a failed init on its own pace.
            tickStartedAt = SystemClock.elapsedRealtime()
            try {
                controller.init()
            } catch (t: Throwable) {
                Log.e(TAG, "warden init threw (the tick retries it)", t)
            } finally {
                tickStartedAt = 0L
            }
            armed = true
            handler.post(tick)
            slowHandler.post(slowTick)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Every start may arrive through startForegroundService (the alarm,
        // the boot receiver, the DNS filter), so the foreground promise is
        // kept on every one, not only the first.
        runCatching { startForeground(NOTIF_ID, notification()) }
            .onFailure { Log.e(TAG, "startForeground failed", it) }
        LivenessAlarm.arm(this)
        if (intent?.action == ACTION_KICK) kick()
        return START_STICKY
    }

    /**
     * The liveness alarm's visit (05-B2/05-G1): run whatever the frozen
     * `Handler` loops have missed, each under a wake lock held only until it
     * finishes, and restart a wedged process. Main thread; the work itself is
     * posted to the loops' own workers, so it is serialised with them.
     */
    private fun kick() {
        try {
            val now = SystemClock.elapsedRealtime()
            if (LivenessPolicy.wedged(now, tickStartedAt)) {
                // A tick stuck this long holds every lock and decision where it
                // was. Restarting is the only move left: START_STICKY and the
                // alarm bring the service back, and it re-applies everything.
                Log.e(TAG, "enforcement tick wedged for ${(now - tickStartedAt) / 1000}s: restarting the process")
                android.os.Process.killProcess(android.os.Process.myPid())
                return
            }
            if (LivenessPolicy.pollWedged(now, pollStartedAt)) {
                // Same for the slow worker: a poll hung in relay IO stops every
                // later poll (and with it every guardian clause), and the
                // queue behind it never drains. Restart, as for the tick.
                Log.e(TAG, "relay poll hung for ${(now - pollStartedAt) / 1000}s: restarting the process")
                android.os.Process.killProcess(android.os.Process.myPid())
                return
            }
            if (!running || !armed) return
            val tickDue = LivenessPolicy.tickDue(now, lastTickAt, interactive, TICK_MS)
            val pollDue = LivenessPolicy.pollDue(now, lastPollAt, interactive, SLOW_TICK_MS, DARK_SLOW_TICK_MS)
            Log.i(TAG, "liveness kick: tick due=$tickDue poll due=$pollDue screen=${if (interactive) "on" else "off"}")
            if (tickDue) {
                val wl = tickWake
                runCatching { wl?.acquire(TICK_WAKE_MS) }
                val posted = handler.post {
                    try {
                        val n = SystemClock.elapsedRealtime()
                        if (LivenessPolicy.tickDue(n, lastTickAt, interactive, TICK_MS)) runTick()
                    } finally {
                        runCatching { wl?.takeIf { it.isHeld }?.release() }
                    }
                }
                if (!posted) runCatching { wl?.takeIf { it.isHeld }?.release() }
            }
            if (pollDue) {
                val wl = pollWake
                runCatching { wl?.acquire(POLL_WAKE_MS) }
                val posted = slowHandler.post {
                    try {
                        val n = SystemClock.elapsedRealtime()
                        if (LivenessPolicy.pollDue(n, lastPollAt, interactive, SLOW_TICK_MS, DARK_SLOW_TICK_MS)) {
                            runPoll()
                        }
                    } finally {
                        runCatching { wl?.takeIf { it.isHeld }?.release() }
                    }
                }
                if (!posted) runCatching { wl?.takeIf { it.isHeld }?.release() }
            }
        } finally {
            LivenessReceiver.releaseBridge()
        }
    }

    override fun onDestroy() {
        running = false
        armed = false
        runCatching { unregisterReceiver(screenReceiver) }
        runCatching { tickWake?.takeIf { it.isHeld }?.release() }
        runCatching { pollWake?.takeIf { it.isHeld }?.release() }
        if (::worker.isInitialized) worker.quitSafely()
        if (::slowWorker.isInitialized) slowWorker.quitSafely()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    private fun notification(): Notification {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        val channel = NotificationChannel(CHANNEL, "Kintrinsic", NotificationManager.IMPORTANCE_LOW).apply {
            description = "Keeps screen-time limits running"
        }
        nm.createNotificationChannel(channel)
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Kintrinsic")
            .setContentText("Screen-time limits are active")
            .setSmallIcon(android.R.drawable.ic_lock_idle_lock)
            .setContentIntent(openApp(this))
            .setOngoing(true)
            .build()
    }

    companion object {
        private const val TAG = "CharterService"
        private const val CHANNEL = "charter-warden"
        private const val NOTIF_ID = 1001
        internal const val TICK_MS = 2_000L
        const val ACTION_KICK = "org.forgesworn.charter.KICK"
        /** Upper bounds on the kick's wake locks; each is released as soon as
         *  its tick or poll finishes. A poll is several relay round trips. */
        private const val TICK_WAKE_MS = 20_000L
        private const val POLL_WAKE_MS = 90_000L

        /**
         * The enforcement pace while the screen is off. Nothing accrues in the
         * dark and no lock can be met, so this only has to be short enough that
         * the books are never badly stale — and it MUST stay well under the
         * core's 300s per-tick accrual clamp, or a long dark stretch would
         * silently drop the interval it is meant to credit as idle.
         */
        internal const val DARK_TICK_MS = 60_000L

        /** Every Kintrinsic notification taps through to the app (ward request:
         *  a notification you can't act on is a dead end). */
        fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        /** Relay poll cadence (D4: connection-per-poll; 2-day cursor self-heals). */
        internal const val SLOW_TICK_MS = 15_000L

        /**
         * The relay pace while the screen is off. Each poll is a fresh
         * connection, and a radio brought up four times a minute all night is
         * the single most expensive thing this app did. A phone in a pocket has
         * nobody waiting on the answer: what matters is that the poll happens
         * the moment the screen lights, which [repace] guarantees.
         */
        internal const val DARK_SLOW_TICK_MS = 120_000L

        fun start(context: Context) {
            val intent = Intent(context, CharterService::class.java)
            context.startForegroundService(intent)
        }

        /** The liveness alarm's entry: starts the service if it is not
         *  running, and asks it to run whatever is overdue. */
        fun kick(context: Context) {
            context.startForegroundService(
                Intent(context, CharterService::class.java).setAction(ACTION_KICK),
            )
        }
    }
}
