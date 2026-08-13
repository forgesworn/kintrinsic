package org.forgesworn.charter.enforce.hotspot

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import java.net.Inet4Address
import java.net.InetAddress
import java.net.NetworkInterface
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/** A live Kintrinsic Hotspot session: the join credentials (for the guardian's QR)
 *  and where the filtering proxy listens on the AP interface. */
data class HotspotSession(
    val ssid: String,
    val passphrase: String,
    val proxyHost: String,
    val proxyPort: Int,
)

/** What the guests are doing right now, for the idle auto-off ([HotspotIdle]). */
data class GuestActivity(
    val liveSocketCount: Int,
    val lastActivityNanos: Long,
)

/**
 * Orchestrates filtered tethering: an app-hosted local-only Wi-Fi AP (the OS
 * forwards none of its traffic — fail-closed) plus a [HotspotProxy] bound to
 * that AP's interface. Guests point at the proxy and get FILTERED internet;
 * anything not going through the proxy simply has no route. The proxy's own
 * egress rides the phone's upstream because the Kintrinsic app is excluded from
 * the on-device VPN tun (CharterVpnService.addDisallowedApplication), so no
 * per-socket protect() is needed.
 *
 * Caller sets the restriction posture first (filtered ⇒ DISALLOW_WIFI_TETHERING,
 * NOT DISALLOW_CONFIG_TETHERING, which would also block our own LOHS).
 */
class CharterHotspot(
    private val context: Context,
    private val proxyPort: Int = DEFAULT_PROXY_PORT,
    private val filterProvider: () -> HotspotFilter,
) {
    constructor(context: Context, filterProvider: () -> HotspotFilter) :
        this(context, DEFAULT_PROXY_PORT, filterProvider)

    private val wifi = context.getSystemService(Context.WIFI_SERVICE) as WifiManager
    // Guarded by `lock`, which is NEVER held across the AP-bring-up await, so
    // stop() (called on the main thread) can never block behind an in-flight
    // start() (called on a worker) — avoids an ANR on grant revocation.
    private val lock = Any()
    private var reservation: WifiManager.LocalOnlyHotspotReservation? = null
    private var proxy: HotspotProxy? = null
    private var hotspotThread: HandlerThread? = null
    @Volatile private var stopped = false
    private val starting = java.util.concurrent.atomic.AtomicBoolean(false)

    /** Bring up the AP + proxy, blocking up to [timeoutMs] for the AP. Returns
     *  the session, or null if the AP could not start (or was stopped meanwhile). */
    fun start(timeoutMs: Long = DEFAULT_TIMEOUT_MS): HotspotSession? {
        if (!starting.compareAndSet(false, true)) return null
        try {
            stopped = false
            val latch = CountDownLatch(1)
            // The reservation handoff: the callback deposits here, and whoever
            // holds abandoned=true closes whatever lands. Covers the race the
            // old code leaked on — onStarted firing AFTER the await timed out
            // left a live AP nobody owned (or could ever close).
            val resRef = java.util.concurrent.atomic.AtomicReference<WifiManager.LocalOnlyHotspotReservation?>(null)
            val abandoned = java.util.concurrent.atomic.AtomicBoolean(false)
            val thread = HandlerThread("charter-hotspot").apply { start() }
            val handler = Handler(thread.looper)
            synchronized(lock) { hotspotThread = thread }
            wifi.startLocalOnlyHotspot(
                object : WifiManager.LocalOnlyHotspotCallback() {
                    override fun onStarted(r: WifiManager.LocalOnlyHotspotReservation) {
                        resRef.set(r)
                        // Late delivery into an abandoned attempt: close it HERE
                        // (the CAS decides exactly one closer in every interleaving).
                        if (abandoned.get() && resRef.compareAndSet(r, null)) {
                            Log.w(TAG, "LOHS started after abandon — closing")
                            runCatching { r.close() }
                        }
                        latch.countDown()
                    }
                    override fun onFailed(reason: Int) {
                        Log.w(TAG, "LOHS failed: $reason"); latch.countDown()
                    }
                },
                handler,
            )

            // Abandon this attempt: close any reservation that has landed (or
            // will land — see the callback), then quit the thread AFTER a grace
            // window so a late onStarted is still delivered and self-closes.
            // (quitSafely() now would silently drop that callback: stuck AP.)
            fun abandon() {
                abandoned.set(true)
                resRef.getAndSet(null)?.let { runCatching { it.close() } }
                handler.postDelayed({ runCatching { thread.quitSafely() } }, ABANDON_GRACE_MS)
                synchronized(lock) { if (hotspotThread === thread) hotspotThread = null }
            }

            // Await WITHOUT holding the lock (this can take up to timeoutMs).
            val ok = latch.await(timeoutMs, TimeUnit.MILLISECONDS) && resRef.get() != null
            if (!ok || stopped) {
                abandon()
                return null
            }
            val apAddr = apInterfaceAddress() ?: run {
                Log.w(TAG, "no AP interface address"); abandon(); return null
            }
            val p = try {
                startProxy(apAddr)
            } catch (t: Throwable) {
                // No proxy ⇒ no session. Guests would get a dead AP (LOHS
                // forwards nothing — still fail-closed), but never leave it up.
                Log.w(TAG, "proxy failed to start", t)
                abandon()
                return null
            }
            val res = synchronized(lock) {
                // stop() may have landed since the await; deciding INSIDE the
                // lock means stop()'s snapshot and this store can't miss each
                // other (the old pre-store check left exactly that window).
                if (stopped) null else resRef.get()?.also { r ->
                    reservation = r
                    proxy = p
                }
            }
            if (res == null) {
                runCatching { p.stop() }
                abandon()
                return null
            }
            val cfg = softApConfig(res)
            return HotspotSession(
                ssid = cfg.first,
                passphrase = cfg.second,
                proxyHost = apAddr.hostAddress ?: "",
                proxyPort = p.actualPort,
            )
        } finally {
            starting.set(false)
        }
    }

    /** The filtering proxy on its preferred port, falling back to an ephemeral
     *  one if something squats it — the session (QR) carries the actual port. */
    private fun startProxy(apAddr: InetAddress): HotspotProxy =
        try {
            HotspotProxy(apAddr, proxyPort, filterProvider).also { it.start() }
        } catch (e: java.io.IOException) {
            Log.w(TAG, "proxy port $proxyPort unavailable, using ephemeral", e)
            HotspotProxy(apAddr, 0, filterProvider).also { it.start() }
        }

    /** Guest activity on the live proxy, or null when no session is up. */
    fun guestActivity(): GuestActivity? {
        val p = synchronized(lock) { proxy } ?: return null
        return GuestActivity(p.liveSocketCount, p.lastActivityNanos)
    }

    fun stop() {
        stopped = true
        val inFlight = starting.get()
        val (p, r, t) = synchronized(lock) {
            val snapshot = Triple(proxy, reservation, hotspotThread)
            proxy = null; reservation = null
            // An in-flight start() owns its handler thread (its abandon path
            // quits it after the grace window) — quitting it here could drop
            // the LOHS callback and strand a live AP.
            if (!inFlight) hotspotThread = null
            snapshot
        }
        runCatching { p?.stop() }
        runCatching { r?.close() }
        if (!inFlight) runCatching { t?.quitSafely() }
    }

    /** SSID + passphrase from the reservation, across API levels. */
    private fun softApConfig(r: WifiManager.LocalOnlyHotspotReservation): Pair<String, String> {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            val c = r.softApConfiguration
            val ssid = c.wifiSsid?.toString()?.trim('"') ?: ""
            return ssid to (c.passphrase ?: "")
        }
        @Suppress("DEPRECATION")
        val wc = r.wifiConfiguration
        @Suppress("DEPRECATION")
        return (wc?.SSID ?: "") to (wc?.preSharedKey ?: "")
    }

    /** The AP's own IPv4 (guests reach the proxy here). The local-only AP is a
     *  SoftAp gateway (an address ending in `.1`, historically 192.168.x.1), so
     *  we PREFER that shape over other site-local addresses; the phone's STA
     *  (home-Wi-Fi client) address and the VPN tun are excluded. Logged so the
     *  guest-device hardware round can confirm the right interface was chosen. */
    private fun apInterfaceAddress(): InetAddress? {
        val staIp = @Suppress("DEPRECATION") wifi.connectionInfo?.ipAddress ?: 0
        val staAddr = if (staIp != 0)
            "%d.%d.%d.%d".format(staIp and 0xff, staIp shr 8 and 0xff, staIp shr 16 and 0xff, staIp shr 24 and 0xff)
        else null

        val candidates = mutableListOf<Inet4Address>()
        for (nif in NetworkInterface.getNetworkInterfaces()) {
            // Skip loopback and point-to-point links: the Kintrinsic DNS-filter VPN
            // is a point-to-point tun with a private IPv4, and during a filtered
            // session it is up — we must bind the proxy to the AP, not the tun.
            if (!nif.isUp || nif.isLoopback || nif.isPointToPoint) continue
            for (addr in nif.inetAddresses) {
                if (addr is Inet4Address && addr.isSiteLocalAddress && addr.hostAddress != staAddr) {
                    candidates.add(addr)
                }
            }
        }
        // A SoftAp gateway address (…​.1, ideally 192.168.x.1) is the AP; prefer it.
        val chosen = candidates.firstOrNull { it.hostAddress?.endsWith(".1") == true }
            ?: candidates.firstOrNull()
        Log.i(TAG, "AP interface address: ${chosen?.hostAddress} (candidates=${candidates.map { it.hostAddress }})")
        return chosen
    }

    private companion object {
        const val TAG = "CharterHotspot"
        const val DEFAULT_PROXY_PORT = 8888
        const val DEFAULT_TIMEOUT_MS = 20_000L
        // How long an abandoned start keeps its handler thread alive so a
        // late-arriving LOHS callback can still be delivered (and self-close).
        const val ABANDON_GRACE_MS = 30_000L
    }
}
