package org.forgesworn.charter.service

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.util.Log
import org.forgesworn.charter.enforce.hotspot.CharterHotspot
import org.forgesworn.charter.enforce.hotspot.HotspotFilter
import org.forgesworn.charter.enforce.hotspot.HotspotIdle
import org.forgesworn.charter.enforce.hotspot.HotspotWish
import org.forgesworn.charter.native.CharterCore

/**
 * The foreground service that hosts a filtered Kintrinsic Hotspot for the duration
 * of a `filtered` tethering grant. The [WardenController] starts it when the
 * posture becomes "filtered" and stops it otherwise (level-triggered, so grant
 * expiry tears it down). A foreground service is required to (a) hold the
 * local-only AP across backgrounding and (b) stay exempt from Doze network
 * limits while guests are connected.
 *
 * The filter is read fresh from the core per guest request, so a mid-session
 * web-policy change is honoured immediately.
 */
class CharterHotspotService : android.app.Service() {

    private var hotspot: CharterHotspot? = null
    private var worker: HandlerThread? = null
    private var handler: Handler? = null
    @Volatile private var destroyed = false
    /** A bring-up attempt has failed and we are in the retry loop — surfaced in
     *  the notification so the ward isn't staring at "Starting…" forever. */
    @Volatile private var failed = false

    override fun onCreate() {
        super.onCreate()
        startForeground(NOTIF_ID, notification())
        val w = HandlerThread("charter-hotspot-svc").apply { start() }
        worker = w
        handler = Handler(w.looper)
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // The notification's own "Turn off" — the ward shouldn't have to open an
        // app to put their hotspot away. Clears the wish so the controller's
        // next tick agrees with us instead of restarting the AP.
        if (intent?.action == ACTION_TURN_OFF) {
            Log.i(TAG, "ward switched the guest hotspot off from the notification")
            HotspotWish.turnOff()
            stopSelf()
            return START_NOT_STICKY
        }
        if (hotspot == null) {
            hotspot = CharterHotspot(applicationContext) { currentFilter() }
            // start() blocks up to ~20s for the AP — worker thread, never main.
            handler?.post { attemptStart() }
            // Fail-closed watchdog, independent of the controller: if the core
            // no longer says "filtered" (grant expired/revoked, charter gone,
            // or a restarted process whose controller lost track of us), tear
            // ourselves down rather than keep an AP up on a stale say-so.
            handler?.postDelayed(::watchdog, WATCHDOG_MS)
        }
        return START_STICKY
    }

    /** One bring-up attempt; a transient failure (Wi-Fi off, radio busy) retries
     *  on a timer — the guardian's window shouldn't die on a slow radio. The
     *  watchdog is the loop's bound: posture != filtered stops the service. */
    private fun attemptStart() {
        val h = hotspot ?: return
        if (destroyed) return
        val outcome = runCatching { h.start() }
        val session = outcome.getOrNull()
        if (session == null) {
            if (!destroyed) {
                // Say so ON THE PHONE. A silent 30s retry loop behind a
                // "Starting…" notification is indistinguishable from a broken
                // hotspot — and while filtered is the posture the SYSTEM
                // hotspot is locked too, so there is no other way in.
                Log.w(TAG, "hotspot failed to start; retrying in ${RETRY_MS / 1000}s", outcome.exceptionOrNull())
                failed = true
                val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
                nm.notify(NOTIF_ID, notification())
                handler?.postDelayed(::attemptStart, RETRY_MS)
            }
        } else {
            failed = false
            Log.i(TAG, "charter hotspot up: ssid=${session.ssid} proxy=${session.proxyHost}:${session.proxyPort}")
            // The credentials must reach a HUMAN (found on-metal 2026-07-23:
            // an AP whose name/password live only in logcat is invisible —
            // "there is no option to set it up"). Notification + a companion
            // getter for the ward's main-screen mirror.
            liveSession = session
            val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
            nm.notify(NOTIF_ID, notification())
        }
    }

    private fun watchdog() {
        if (destroyed) return
        val mode = runCatching { CharterCore.tetheringMode(System.currentTimeMillis() / 1000) }
            .getOrDefault("blocked")
        if (mode != "filtered") {
            Log.w(TAG, "posture is '$mode', not filtered — self-stopping")
            stopSelf()
            return
        }
        // An AP with nobody on it is pure battery burn, and a ward who forgot is
        // the ordinary case — so switch ourselves off and SAY SO, rather than
        // quietly holding the radio all day.
        val activity = hotspot?.guestActivity()
        if (activity != null) {
            val idleMs = (System.nanoTime() - activity.lastActivityNanos) / 1_000_000
            if (HotspotIdle.shouldSwitchOff(activity.liveSocketCount, idleMs)) {
                Log.i(TAG, "guest hotspot idle ${idleMs / 1000}s with no guests — switching off")
                HotspotWish.turnOff(HotspotIdle.REASON)
                notifySwitchedOff()
                stopSelf()
                return
            }
        }
        handler?.postDelayed(::watchdog, WATCHDOG_MS)
    }

    /** A separate, dismissable note: the ongoing notification dies with the
     *  service, so without this the hotspot would just silently vanish. */
    private fun notifySwitchedOff() {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.notify(
            OFF_NOTIF_ID,
            Notification.Builder(this, CHANNEL)
                .setContentTitle("Guest hotspot switched off")
                .setContentText("Nobody was using it. Turn it back on in Kintrinsic when you need it.")
                .setSmallIcon(android.R.drawable.ic_menu_share)
                .setContentIntent(CharterService.openApp(this))
                .setAutoCancel(true)
                .build(),
        )
    }

    override fun onDestroy() {
        liveSession = null
        destroyed = true
        // Synchronous, not posted: stop() never blocks (it flags an in-flight
        // start() to abandon and closes sockets), while the worker may be
        // parked in a 20s bring-up — a posted stop would queue BEHIND it.
        runCatching { hotspot?.stop() }
        hotspot = null
        worker?.quitSafely()
        worker = null
        handler = null
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    /** The ward's current web policy as the guest doorman; locked (fail-closed)
     *  when no plan is available. */
    private fun currentFilter(): HotspotFilter {
        val plan = runCatching { CharterCore.dnsPlan() }.getOrNull()
            ?: CharterCore.DnsPlan(
                revision = "", mode = "locked",
                allowDomains = emptyList(), blockDomains = emptyList(),
                blockCategories = emptyList(), allowExceptions = emptyList(),
                safeSearch = true, youtubeRestrict = "off", rewrites = emptyList(),
            )
        return HotspotFilter(plan)
    }

    private fun notification(): Notification {
        val nm = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Kintrinsic Hotspot", NotificationManager.IMPORTANCE_LOW).apply {
                description = "Filtered guest hotspot"
            },
        )
        val live = liveSession
        val text = when {
            live != null -> "Network “${live.ssid}” — password ${live.passphrase}"
            failed -> "Couldn’t start the guest hotspot — trying again."
            else -> "Starting the filtered hotspot…"
        }
        val turnOff = android.app.PendingIntent.getForegroundService(
            this,
            0,
            Intent(this, CharterHotspotService::class.java).setAction(ACTION_TURN_OFF),
            android.app.PendingIntent.FLAG_IMMUTABLE or android.app.PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return Notification.Builder(this, CHANNEL)
            .setContentTitle("Kintrinsic Hotspot")
            .setContentText(text)
            .setStyle(Notification.BigTextStyle().bigText(
                when {
                    live != null ->
                        "$text\nGuests who join share this phone’s web filter. " +
                            "It switches itself off when nobody’s using it."
                    failed -> "$text\nTell your guardian if it keeps saying this."
                    else -> text
                },
            ))
            .setSmallIcon(android.R.drawable.ic_menu_share)
            .setContentIntent(CharterService.openApp(this))
            .addAction(
                Notification.Action.Builder(null as android.graphics.drawable.Icon?, "Turn off", turnOff).build(),
            )
            .setOngoing(true)
            .build()
    }

    companion object {
        /** The live AP credentials, for the ward-facing mirror ("" = not up). */
        @Volatile var liveSession: org.forgesworn.charter.enforce.hotspot.HotspotSession? = null
            private set

        private const val TAG = "CharterHotspotSvc"
        private const val CHANNEL = "charter-hotspot"
        private const val NOTIF_ID = 1002
        /** Distinct from the ongoing one, which is torn down with the service. */
        private const val OFF_NOTIF_ID = 1003
        const val ACTION_TURN_OFF = "org.forgesworn.charter.HOTSPOT_TURN_OFF"
        private const val RETRY_MS = 30_000L
        private const val WATCHDOG_MS = 60_000L

        fun start(context: Context) {
            context.startForegroundService(Intent(context, CharterHotspotService::class.java))
        }

        fun stop(context: Context) {
            context.stopService(Intent(context, CharterHotspotService::class.java))
        }
    }
}
