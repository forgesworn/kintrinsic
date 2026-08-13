package org.forgesworn.mycharter.service

import android.app.Service
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.SystemClock
import android.util.Log
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.native.GuardianNative
import org.forgesworn.mycharter.relay.RelayFraming
import org.json.JSONObject
import java.util.concurrent.TimeUnit

/**
 * The carrier's whole point: a foreground service holding one websocket per
 * relay, classifying every gift-wrap addressed to the guardian, and raising
 * an URGENT notification for each unseen ward REQUEST. No FCM — the socket
 * IS the push channel, which keeps de-Googled guardian phones first-class.
 *
 * Reliability posture: START_STICKY + BootReceiver + exponential-backoff
 * reconnect (1s→64s cap) + OkHttp pings. Classification runs on a worker
 * thread (the JNI rule). Everything is level-triggered off the persisted
 * provision — a restart resubscribes from (now - 48h) and the seen-reqId LRU
 * absorbs the replay.
 */
class CarrierService : Service() {

    private lateinit var store: CarrierStore
    private lateinit var worker: HandlerThread
    private lateinit var handler: Handler
    private val sockets = mutableMapOf<String, WebSocket>()
    private val backoffs = mutableMapOf<String, Long>()
    /** When each socket opened (monotonic) — how [scheduleReconnect] judges it. */
    private val openedAt = mutableMapOf<String, Long>()
    /** Relays with a reconnect already queued, so a socket reported dead twice
     *  does not end up with two of them. */
    private val reconnecting = mutableSetOf<String>()
    @Volatile private var running = false

    /**
     * The ping is what keeps the socket (and the carrier NAT binding behind it)
     * alive between wards' asks — which on a quiet day is all day. Every one of
     * them wakes the radio, so a 30s ping was ~2,900 wake-ups a day to carry a
     * handful of real messages: the carrier's whole background cost, paid to
     * learn nothing. Four minutes is comfortably inside the idle timeouts that
     * mobile networks and relays actually apply, and if a socket does get
     * dropped the reconnect below picks it straight back up — the failure mode
     * is a reconnect, not a missed ask.
     */
    private val client = OkHttpClient.Builder()
        .pingInterval(4, TimeUnit.MINUTES)
        .build()

    override fun onCreate() {
        super.onCreate()
        Notifier.ensureChannels(this)
        startForeground(Notifier.SERVICE_NOTIF_ID, Notifier.serviceNotification(this))
        store = CarrierStore(this)
        worker = HandlerThread("carrier-worker").apply { start() }
        handler = Handler(worker.looper)
        running = true
        handler.post { connectAll() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int = START_STICKY

    override fun onDestroy() {
        running = false
        sockets.values.forEach { it.close(1000, "service stopped") }
        sockets.clear()
        openedAt.clear()
        reconnecting.clear()
        worker.quitSafely()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    // ---- relay plumbing (worker thread) ----------------------------------

    private fun connectAll() {
        val p = store.provision() ?: run {
            Log.i(TAG, "not provisioned; stopping")
            stopSelf()
            return
        }
        val abi = GuardianNative.guardianAbiVersion()
        check(abi == 2) { "guardian JNI ABI $abi != 2" }
        p.relays.forEach { relay -> if (relay !in sockets) connect(relay) }
    }

    private fun connect(relay: String) {
        reconnecting.remove(relay)
        if (!running) return
        val p = store.provision() ?: return
        val since = System.currentTimeMillis() / 1000 - RelayFraming.SINCE_WINDOW_SECS
        val ws = client.newWebSocket(
            Request.Builder().url(relay).build(),
            object : WebSocketListener() {
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    Log.i(TAG, "open $relay")
                    // Note when it opened, but do NOT call it a success yet:
                    // see scheduleReconnect.
                    handler.post { openedAt[relay] = SystemClock.elapsedRealtime() }
                    webSocket.send(RelayFraming.reqMessage("carrier", p.guardianPubkeyHex, since))
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    val eventJson = RelayFraming.parseEvent(text) ?: return
                    handler.post { classifyAndNotify(eventJson) }
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    Log.w(TAG, "socket failed $relay: ${t.message}")
                    scheduleReconnect(relay)
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    Log.i(TAG, "closed $relay ($code)")
                    scheduleReconnect(relay)
                }
            },
        )
        sockets[relay] = ws
    }

    /**
     * Back off after a dropped socket — measuring success by how long the
     * connection actually HELD, not by the fact it opened.
     *
     * Resetting the backoff in onOpen meant a relay that accepts a connection
     * and immediately drops it (a full relay, a broken deploy, a captive
     * portal answering every port) never backed off at all: reconnect at one
     * second, open, drop, reset, forever. That is a hot loop on the radio all
     * day, and it looks healthy in the log because every round logs "open".
     * Only a connection that survived [STABLE_MS] earns a fresh start.
     */
    private fun scheduleReconnect(relay: String) {
        if (!running) return
        // These callbacks arrive on OkHttp's threads; every bit of bookkeeping
        // below is plain HashMap state owned by the worker.
        handler.post {
            if (!running) return@post
            // OkHttp can report a dying socket more than once (failure after
            // close). Without this, each report would start its own reconnect
            // chain and the relay would end up with several live sockets.
            if (!reconnecting.add(relay)) return@post
            sockets.remove(relay)
            val lived = openedAt.remove(relay)
                ?.let { SystemClock.elapsedRealtime() - it }
                ?: 0L
            val prev = if (lived >= STABLE_MS) 0L else (backoffs[relay] ?: 0L)
            val next = (prev * 2).coerceIn(1L, MAX_BACKOFF_SECS)
            backoffs[relay] = next
            handler.postDelayed({ connect(relay) }, next * 1000)
        }
    }

    private fun classifyAndNotify(eventJson: String) {
        val p = store.provision() ?: return
        val now = System.currentTimeMillis() / 1000
        // Hand the key over as bytes we can WIPE, and wipe them (S12). It used
        // to go across as the stored hex String — immutable, interned, one
        // fresh uncollectable copy of the guardian's root key per delivered
        // wrap. The array below lives for the length of one classify call.
        val sk = hexToBytes(p.guardianSkHex) ?: return
        val verdict = try {
            JSONObject(GuardianNative.guardianClassifyWrap(eventJson, sk, now))
        } catch (t: Throwable) {
            Log.w(TAG, "classify failed: ${t.message}")
            return
        } finally {
            sk.fill(0)
        }
        // The break-glass override: the ward opened their phone in an
        // emergency. Alert NOW — the whole design trades prevention for
        // immediate transparency, so a late notification breaks the deal.
        if (verdict.optString("type") == "audit") {
            if (verdict.optString("outcome") == "override") {
                // Alert ONCE. Every reconnect resubscribes 48h back and the
                // relay replays the same wrap, so without this the guardian is
                // re-buzzed about one unlock for two days (seen live: 6 alerts
                // for one press, 2026-08-09). The audit rumor carries no id of
                // its own, so the key is the outer wrap id — one wrap is built
                // per audit and published to every relay, so it also folds the
                // per-relay copies into one alert. An unreadable id falls
                // through to notify: a repeated alarm beats a missed emergency.
                val wrapId = RelayFraming.eventId(eventJson)
                if (wrapId != null && !store.markSeen("audit:$wrapId")) return
                Notifier.notifyOverride(
                    this,
                    machine = verdict.optString("machine", ""),
                    scope = verdict.optString("scope", ""),
                    durationSecs = verdict.optString("durationSecs", "").toLongOrNull() ?: 0L,
                    roster = store.roster(),
                )
            }
            return
        }
        if (verdict.optString("type") != "request") return
        val reqId = verdict.optString("reqId", "")
        if (reqId.isEmpty() || !store.markSeen(reqId)) return
        val minutes = verdict.optJSONObject("params")
            ?.optLong("minutesRequested", -1L)
            ?.takeIf { it >= 0 }
        Log.i(TAG, "ward request $reqId (${verdict.optString("op")})")
        Notifier.notifyRequest(
            this,
            reqId,
            verdict.optString("op"),
            minutes,
            machine = verdict.optString("machine", ""),
            roster = store.roster(),
        )
    }

    /** 64 hex chars -> 32 bytes, or null. Deliberately not a String
     *  intermediate: the point is to hand the JNI something wipeable. */
    private fun hexToBytes(hex: String): ByteArray? {
        if (hex.length != 64) return null
        val out = ByteArray(32)
        for (i in 0 until 32) {
            val hi = Character.digit(hex[i * 2], 16)
            val lo = Character.digit(hex[i * 2 + 1], 16)
            if (hi < 0 || lo < 0) return null
            out[i] = ((hi shl 4) or lo).toByte()
        }
        return out
    }

    companion object {
        private const val TAG = "CarrierService"

        /** How long a socket must hold before a drop is treated as bad luck
         *  rather than a relay that cannot carry us — see [scheduleReconnect]. */
        private const val STABLE_MS = 60_000L

        /**
         * The reconnect ceiling. A guardian out of coverage — a tunnel, a hill,
         * a flight — used to have their phone try a relay every 64s for the
         * whole outage. Five minutes still reconnects promptly on the guardian's
         * timescale (an ask that arrives meanwhile is waiting on the relay when
         * we get back; the resubscribe reaches back 48h), for a fifth of the
         * attempts.
         */
        private const val MAX_BACKOFF_SECS = 300L

        /** Idempotent start; safe from any thread; no-op unless provisioned. */
        fun start(context: Context) {
            if (CarrierStore(context).provision() == null) return
            context.startForegroundService(Intent(context, CarrierService::class.java))
        }
    }
}
