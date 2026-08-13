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

    private val tick = object : Runnable {
        override fun run() {
            if (!running) return
            try {
                controller.tickAndApply()
                // The D8 widget rides the tick (worker thread — JNI-safe);
                // level-triggered inside push(), so unchanged minutes are free.
                org.forgesworn.charter.ui.TimeLeftWidget.push(this@CharterService)
            } catch (t: Throwable) {
                Log.e(TAG, "tick failed", t)
            }
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
            runCatching { controller.tickAndApply(screenWas = wasInteractive) }
                .onFailure { Log.e(TAG, "boundary tick failed", it) }
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
            }
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
        running = true
        handler.post {
            controller.init()
            armed = true
            handler.post(tick)
            slowHandler.post(slowTick)
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        running = false
        armed = false
        runCatching { unregisterReceiver(screenReceiver) }
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
        private const val TICK_MS = 2_000L

        /**
         * The enforcement pace while the screen is off. Nothing accrues in the
         * dark and no lock can be met, so this only has to be short enough that
         * the books are never badly stale — and it MUST stay well under the
         * core's 300s per-tick accrual clamp, or a long dark stretch would
         * silently drop the interval it is meant to credit as idle.
         */
        private const val DARK_TICK_MS = 60_000L

        /** Every Kintrinsic notification taps through to the app (ward request:
         *  a notification you can't act on is a dead end). */
        fun openApp(context: Context): PendingIntent = PendingIntent.getActivity(
            context,
            0,
            Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

        /** Relay poll cadence (D4: connection-per-poll; 2-day cursor self-heals). */
        private const val SLOW_TICK_MS = 15_000L

        /**
         * The relay pace while the screen is off. Each poll is a fresh
         * connection, and a radio brought up four times a minute all night is
         * the single most expensive thing this app did. A phone in a pocket has
         * nobody waiting on the answer: what matters is that the poll happens
         * the moment the screen lights, which [repace] guarantees.
         */
        private const val DARK_SLOW_TICK_MS = 120_000L

        fun start(context: Context) {
            val intent = Intent(context, CharterService::class.java)
            context.startForegroundService(intent)
        }
    }
}
