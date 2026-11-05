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
import org.forgesworn.mycharter.carrier.hexToKeyBytes
import org.forgesworn.mycharter.link.LocalLink
import org.forgesworn.mycharter.native.GuardianNative
import org.forgesworn.mycharter.relay.RelayFraming
import org.forgesworn.mycharter.relay.RelayRoute
import org.forgesworn.mycharter.relay.RelayRoutes
import org.json.JSONObject
import org.json.JSONArray
import java.util.concurrent.TimeUnit

/**
 * The carrier's whole point: a foreground service holding one websocket per
 * relay for each key it listens on (see [RelayRoute]; one key, one socket per
 * relay), classifying every gift-wrap addressed to the guardian, and raising
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
    /** The home-network listener (local link v1); runs while provisioned. */
    private lateinit var localLink: LocalLink
    /** One socket per relay and key; a key or relay dropped from the
     *  provision closes its sockets. */
    private val sockets = mutableMapOf<RelayRoute, WebSocket>()
    private val backoffs = mutableMapOf<RelayRoute, Long>()
    /** When each socket opened (monotonic) — how [scheduleReconnect] judges it. */
    private val openedAt = mutableMapOf<RelayRoute, Long>()
    /** Sockets with a reconnect already queued, so a socket reported dead twice
     *  does not end up with two of them. */
    private val reconnecting = mutableSetOf<RelayRoute>()
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
        localLink = LocalLink(this, store, handler) { wrapJson ->
            handler.post { if (running) classifyAndNotify(wrapJson) }
        }
        running = true
        handler.post { connectAll() }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        handler.post { connectAll() }
        return START_STICKY
    }

    override fun onDestroy() {
        running = false
        sockets.values.forEach { it.close(1000, "service stopped") }
        sockets.clear()
        openedAt.clear()
        reconnecting.clear()
        // On the worker, like every other LocalLink call; quitSafely still
        // delivers it.
        handler.post { localLink.stop() }
        worker.quitSafely()
        super.onDestroy()
    }

    override fun onBind(intent: Intent?): IBinder? = null

    // ---- relay plumbing (worker thread) ----------------------------------

    private fun connectAll() {
        val p = store.provision() ?: run {
            Log.i(TAG, "not provisioned; stopping")
            localLink.stop()
            stopSelf()
            return
        }
        val abi = GuardianNative.guardianAbiVersion()
        check(abi == 3) { "guardian JNI ABI $abi != 3" }
        // Idempotent: same key + directory keeps the running listener; a new
        // guardian key restarts it.
        localLink.start()
        // A key or relay no longer provisioned closes its sockets; a key added
        // (a working key minted) opens its own beside the others.
        RelayRoutes.stale(sockets.keys, p.relays, p.pubkeys).forEach { route ->
            sockets.remove(route)?.close(1000, "relay configuration changed")
            openedAt.remove(route)
            backoffs.remove(route)
        }
        // Repeated service starts must respect an outage's queued retry,
        // rather than repeatedly waking the radio before its backoff expires.
        RelayRoutes.wanted(p.relays, p.pubkeys).forEach { route ->
            if (ReconnectPolicy.shouldConnect(route in sockets, route in reconnecting)) connect(route)
        }
    }

    private fun connect(route: RelayRoute) {
        reconnecting.remove(route)
        if (!running) return
        val p = store.provision() ?: return
        if (route !in RelayRoutes.wanted(p.relays, p.pubkeys) || route in sockets) return
        val relay = route.relay
        // This socket subscribes for, and signs in as, its own key alone.
        val listenOn = listOf(route.pubkeyHex)
        val since = System.currentTimeMillis() / 1000 - RelayFraming.SINCE_WINDOW_SECS
        val ws = client.newWebSocket(
            Request.Builder().url(relay).build(),
            object : WebSocketListener() {
                private var authId: String? = null
                private var authenticated = false
                override fun onOpen(webSocket: WebSocket, response: Response) {
                    Log.i(TAG, "open $relay")
                    // Note when it opened, but do NOT call it a success yet:
                    // see scheduleReconnect.
                    handler.post { openedAt[route] = SystemClock.elapsedRealtime() }
                    webSocket.send(RelayFraming.reqMessage("carrier", listenOn, since))
                }

                override fun onMessage(webSocket: WebSocket, text: String) {
                    handler.post {
                        val frame = runCatching { JSONArray(text) }.getOrNull() ?: return@post
                        when (frame.optString(0)) {
                            "AUTH" -> {
                                if (authId != null) return@post
                                val challenge = frame.optString(1)
                                if (challenge.isEmpty() || challenge.length > 4096) return@post
                                // The socket's own key: a relay that hands wraps
                                // only to the signed-in recipient then gives
                                // this socket the wraps to that key.
                                val sk = p.secretOf(route.pubkeyHex)?.let { hexToKeyBytes(it) } ?: return@post
                                val proof = try {
                                    JSONObject(GuardianNative.guardianRelayAuth(relay, challenge, sk, System.currentTimeMillis() / 1000))
                                } catch (_: Throwable) { return@post } finally { sk.fill(0) }
                                if (proof.has("error")) return@post
                                authId = proof.optString("id")
                                webSocket.send(JSONArray().put("AUTH").put(proof).toString())
                            }
                            "OK" -> if (!authenticated && authId != null && frame.optString(1) == authId) {
                                if (frame.optBoolean(2)) {
                                    authenticated = true
                                    webSocket.send(RelayFraming.reqMessage("carrier", listenOn, since))
                                } else {
                                    webSocket.cancel()
                                    scheduleReconnect(route, webSocket)
                                }
                            }
                            "CLOSED" -> {
                                val reason = frame.optString(2)
                                if (!reason.startsWith("auth-required:") || authenticated) {
                                    webSocket.cancel()
                                    scheduleReconnect(route, webSocket)
                                }
                            }
                            "EVENT" -> RelayFraming.parseEvent(text)?.let { classifyAndNotify(it) }
                        }
                    }
                }

                override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                    Log.w(TAG, "socket failed $relay: ${t.message}")
                    scheduleReconnect(route, webSocket)
                }

                override fun onClosed(webSocket: WebSocket, code: Int, reason: String) {
                    Log.i(TAG, "closed $relay ($code)")
                    scheduleReconnect(route, webSocket)
                }
            },
        )
        sockets[route] = ws
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
    private fun scheduleReconnect(route: RelayRoute, expectedSocket: WebSocket) {
        if (!running) return
        // These callbacks arrive on OkHttp's threads; every bit of bookkeeping
        // below is plain HashMap state owned by the worker.
        handler.post {
            if (!running || sockets[route] !== expectedSocket) return@post
            // OkHttp can report a dying socket more than once (failure after
            // close). Without this, each report would start its own reconnect
            // chain and the relay would end up with several live sockets for one key.
            if (!reconnecting.add(route)) return@post
            sockets.remove(route)
            val lived = openedAt.remove(route)
                ?.let { SystemClock.elapsedRealtime() - it }
                ?: 0L
            val next = ReconnectPolicy.next(backoffs[route] ?: 0L, lived, store.reconnectCap())
            backoffs[route] = next
            handler.postDelayed({ connect(route) }, next * 1000)
        }
    }

    private fun classifyAndNotify(eventJson: String) {
        val p = store.provision() ?: return
        val now = System.currentTimeMillis() / 1000
        // Hand the key over as bytes we can WIPE, and wipe them (S12). It used
        // to go across as the stored hex String — immutable, interned, one
        // fresh uncollectable copy of the guardian's root key per delivered
        // wrap. The array below lives for the length of one classify call.
        // The wrap is encrypted to the key its `p` tag names: a working key's
        // secret for a wrap to the working key, the root's for one to the root.
        val sk = hexToKeyBytes(p.secretFor(RelayFraming.recipient(eventJson))) ?: return
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
