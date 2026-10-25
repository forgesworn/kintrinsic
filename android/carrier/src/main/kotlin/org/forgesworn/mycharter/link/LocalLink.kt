package org.forgesworn.mycharter.link

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.os.SystemClock
import android.util.Log
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.hexToKeyBytes
import org.forgesworn.mycharter.native.GuardianNative
import java.io.File

/**
 * The home-network route (local link v1, docs/local-network-design.md): the
 * carrier is the LISTENER, wards connect out to it.
 *
 * - The listener itself (sockets, handshake, queue, inbox, delivery records)
 *   is Rust, one per process (`GuardianNative.guardianLink*`); Kotlin does no
 *   cryptography.
 * - This class runs it while the carrier is provisioned, keeps its admitted
 *   machines in step with the roster, advertises `_kintrinsic._tcp` over
 *   NsdManager only while a Wi-Fi or Ethernet network is up (holding a
 *   MulticastLock on Wi-Fi, so a pocketed phone still answers discovery), and
 *   runs the event thread that turns a ward wrap into the usual notification.
 *
 * Owned by [org.forgesworn.mycharter.service.CarrierService]: [start] and
 * [stop] and all NSD / network bookkeeping run on the service's worker
 * [handler]. The bridge reads [current], [running] and [listening] and calls
 * the native queries directly (they are quick, thread-safe, in-memory).
 */
class LocalLink(
    context: Context,
    private val store: CarrierStore,
    private val handler: Handler,
    /** A newly received ward wrap (raw gift-wrap JSON). Called on the event
     *  thread; the receiver re-posts to its own worker. */
    private val onWrap: (String) -> Unit,
) {
    private val app = context.applicationContext
    private val stateDir = File(app.filesDir, "link")
    private val nsd = app.getSystemService(Context.NSD_SERVICE) as NsdManager
    private val connectivity = app.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
    private val multicast: WifiManager.MulticastLock? =
        (app.getSystemService(Context.WIFI_SERVICE) as? WifiManager)
            ?.createMulticastLock("kintrinsic-link")
            ?.apply { setReferenceCounted(false) }

    /** The bound port while the native link runs, else 0. */
    @Volatile private var port = 0

    /** True once NsdManager confirmed the advertisement. */
    @Volatile var listening = false
        private set

    val running: Boolean get() = port > 0

    // Worker-thread state.
    private var stopped = false
    /** Usable LAN networks -> whether each is Wi-Fi. */
    private val lans = HashMap<Network, Boolean>()
    private var netCallback: ConnectivityManager.NetworkCallback? = null
    private var registration: NsdManager.RegistrationListener? = null
    private var registeredPort = 0
    private var retryPosted = false

    @Volatile private var looping = false
    private var loop: Thread? = null

    /** Start (idempotent) the native listener, the event thread and the
     *  network watch. Worker thread. */
    fun start() {
        stopped = false
        current = this
        watchNetworks()
        val p = store.provision() ?: return
        val sk = hexToKeyBytes(p.guardianSkHex) ?: return
        val raw = try {
            stateDir.mkdirs()
            GuardianNative.guardianLinkStart(
                stateDir.path,
                sk,
                LinkJson.allowedJson(store.roster()),
                PREFERRED_PORT,
            )
        } catch (t: Throwable) {
            Log.w(TAG, "link start failed: ${t.message}")
            null
        } finally {
            sk.fill(0)
        }
        val bound = raw?.let { LinkJson.startedPort(it) } ?: 0
        if (bound == 0) {
            Log.w(TAG, "link not started: ${raw?.let { LinkJson.startError(it) }}")
            port = 0
            reconcile()
            return
        }
        if (bound != port) Log.i(TAG, "link listening on port $bound")
        port = bound
        startLoop()
        reconcile()
    }

    /** Stop everything this class started. Worker thread. */
    fun stop() {
        // The native link is process-global: a newer service instance may
        // already own it (a restart in the same process), and a late stop from
        // the old one must not take it down. Everything else here is ours.
        val owner = current === this
        stopped = true
        if (owner) current = null
        looping = false
        netCallback?.let { runCatching { connectivity.unregisterNetworkCallback(it) } }
        netCallback = null
        lans.clear()
        unregister()
        releaseMulticast()
        port = 0
        // The event thread wakes from its wait and exits on `looping`; its
        // reference is kept so a quick restart reuses it rather than racing it.
        if (owner) runCatching { GuardianNative.guardianLinkStop() }
    }

    /** The roster changed: re-derive the admitted machines. Any thread. */
    fun refreshAllowed() {
        if (!running) return
        try {
            GuardianNative.guardianLinkAllow(LinkJson.allowedJson(store.roster()))
        } catch (t: Throwable) {
            Log.w(TAG, "allow failed: ${t.message}")
        }
    }

    // ---- event thread ------------------------------------------------------

    private fun startLoop() {
        looping = true
        if (loop?.isAlive == true) return
        loop = Thread({ runLoop() }, "kintrinsic-link-events").apply {
            isDaemon = true
            start()
        }
    }

    private fun runLoop() {
        while (looping) {
            val began = SystemClock.elapsedRealtime()
            val raw = try {
                GuardianNative.guardianLinkNext(NEXT_TIMEOUT_MS)
            } catch (t: Throwable) {
                Log.w(TAG, "next failed: ${t.message}")
                "[]"
            }
            val events = LinkJson.parseEvents(raw)
            for (e in events) {
                when (e) {
                    // Classification and notification dedupe (by reqId /
                    // audit wrap id) are the relay path's: a request that
                    // arrives both ways notifies once. The wrap itself is
                    // already in the native inbox for the console.
                    is LinkJson.Event.Wrap -> runCatching { onWrap(e.wrapJson) }
                    is LinkJson.Event.Session ->
                        Log.i(TAG, "ward ${e.machine.take(8)} ${if (e.up) "on" else "off"} the home network")
                }
            }
            // An immediate empty answer means no link (stopped, or a restart
            // in progress): never spin on it.
            if (events.isEmpty() && SystemClock.elapsedRealtime() - began < 100) {
                try { Thread.sleep(1000) } catch (_: InterruptedException) { return }
            }
        }
    }

    // ---- networks + NSD (worker thread) -----------------------------------

    private fun watchNetworks() {
        if (netCallback != null) return
        // No NET_CAPABILITY_INTERNET: a home Wi-Fi whose internet is down is
        // exactly when this route matters. Transports are OR'd.
        val request = NetworkRequest.Builder()
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET)
            .build()
        val cb = object : ConnectivityManager.NetworkCallback() {
            override fun onCapabilitiesChanged(network: Network, caps: NetworkCapabilities) {
                val wifi = caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI)
                if (stopped || lans[network] == wifi) return
                lans[network] = wifi
                onNetworksChanged()
            }

            override fun onLost(network: Network) {
                if (lans.remove(network) != null) onNetworksChanged()
            }
        }
        try {
            connectivity.registerNetworkCallback(request, cb, handler)
            netCallback = cb
        } catch (t: Throwable) {
            Log.w(TAG, "network watch failed: ${t.message}")
        }
    }

    private fun onNetworksChanged() {
        if (stopped) return
        // A listener that failed to start (port in use, a stale .so) gets
        // another go when the network changes.
        if (!running) start() else reconcile()
    }

    private fun reconcile() {
        if (stopped) return
        val want = running && lans.isNotEmpty()
        // Off any home network the listener refuses wards (a mobile network's
        // carrier-grade NAT can hand out 10/8 peers); sends still queue.
        // Rust starts every new link refusing, so this is what turns it on.
        if (running) {
            try {
                GuardianNative.guardianLinkAccepting(want)
            } catch (t: Throwable) {
                Log.w(TAG, "accepting failed: ${t.message}")
            }
        }
        if (registration != null && (!want || registeredPort != port)) unregister()
        if (want && registration == null) register(port)
        updateMulticast()
    }

    private fun register(onPort: Int) {
        val info = NsdServiceInfo().apply {
            serviceName = store.linkInstanceName()
            serviceType = SERVICE_TYPE
            port = onPort
            setAttribute("v", "1")
        }
        val l = object : NsdManager.RegistrationListener {
            override fun onServiceRegistered(s: NsdServiceInfo) {
                handler.post {
                    if (registration === this) {
                        listening = true
                        Log.i(TAG, "advertised as '${s.serviceName}' on port $onPort")
                    }
                }
            }

            override fun onRegistrationFailed(s: NsdServiceInfo, errorCode: Int) {
                handler.post {
                    if (registration !== this) return@post
                    Log.w(TAG, "advertise failed ($errorCode)")
                    registration = null
                    listening = false
                    updateMulticast()
                    scheduleRetry()
                }
            }

            override fun onServiceUnregistered(s: NsdServiceInfo) {}
            override fun onUnregistrationFailed(s: NsdServiceInfo, errorCode: Int) {}
        }
        try {
            nsd.registerService(info, NsdManager.PROTOCOL_DNS_SD, l)
            registration = l
            registeredPort = onPort
        } catch (t: Throwable) {
            Log.w(TAG, "advertise threw: ${t.message}")
            scheduleRetry()
        }
    }

    private fun unregister() {
        val l = registration ?: return
        registration = null
        registeredPort = 0
        listening = false
        runCatching { nsd.unregisterService(l) }
    }

    private fun scheduleRetry() {
        if (retryPosted) return
        retryPosted = true
        handler.postDelayed({
            retryPosted = false
            reconcile()
        }, RETRY_MS)
    }

    private fun updateMulticast() {
        val lock = multicast ?: return
        val want = !stopped && registration != null && lans.values.any { it }
        try {
            if (want && !lock.isHeld) lock.acquire()
            if (!want && lock.isHeld) lock.release()
        } catch (t: Throwable) {
            Log.w(TAG, "multicast lock: ${t.message}")
        }
    }

    private fun releaseMulticast() {
        runCatching { multicast?.takeIf { it.isHeld }?.release() }
    }

    companion object {
        private const val TAG = "LocalLink"
        const val SERVICE_TYPE = "_kintrinsic._tcp"
        const val PREFERRED_PORT = 47615
        private const val NEXT_TIMEOUT_MS = 30_000
        private const val RETRY_MS = 30_000L

        /** The running carrier's link, for the bridge; null when the service
         *  is not running it. */
        @Volatile var current: LocalLink? = null
            private set
    }
}
