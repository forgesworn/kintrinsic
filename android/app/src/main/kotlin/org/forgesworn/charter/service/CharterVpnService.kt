package org.forgesworn.charter.service

import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.util.Log
import org.forgesworn.charter.enforce.dns.DnsDecision
import org.forgesworn.charter.enforce.dns.DnsResolver
import org.forgesworn.charter.enforce.dns.buildAnswer
import org.forgesworn.charter.enforce.dns.buildNxdomain
import org.forgesworn.charter.enforce.dns.parseQuestion
import org.forgesworn.charter.native.CharterCore
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.util.concurrent.ArrayBlockingQueue
import java.util.concurrent.RejectedExecutionHandler
import java.util.concurrent.ThreadPoolExecutor
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicLong

/**
 * The DNS-filtering TUN. Only the virtual DNS server addresses, plus the
 * well-known public resolvers in [KnownResolvers] (05-G4), are routed into the
 * tunnel, so every other packet flows over the real network untouched (split
 * tunnel). Plain DNS an app aims straight at one of those resolvers is answered
 * here under the same plan; anything else sent to them (DoH on 443, DoT on 853,
 * DNS over TCP) is dropped, so the app falls back to the system resolver, which
 * is this filter. Each captured DNS query is decided by the shared plan:
 *   Block -> NXDOMAIN; Rewrite -> resolve the controlled target and answer
 *   under the queried name; PassThrough -> relay verbatim upstream.
 * The DO pins this always-on WITHOUT lockdown (WardenController.init → fail-soft):
 * lockdown + a DNS-only split tunnel was proven on-metal to kill all non-DNS
 * traffic, so instead the OS keeps the filter running and restarts it on crash
 * (a seconds-long unfiltered-DNS window at worst). It stays tamper-resistant
 * because the ward still cannot disable the VPN or set a private DoH resolver
 * (DISALLOW_CONFIG_VPN / DISALLOW_CONFIG_PRIVATE_DNS) and the DoH canary is
 * NXDOMAIN'd. See VpnDnsFilterOps.pinAlwaysOn for the full rationale.
 */
class CharterVpnService : VpnService() {
    @Volatile private var tun: ParcelFileDescriptor? = null
    @Volatile private var worker: Thread? = null
    @Volatile private var resolver: DnsResolver? = null
    @Volatile private var output: FileOutputStream? = null
    // Worker pool for decide+relay so one slow upstream lookup (soTimeout up
    // to 4s) can never stall the read loop — and with it all DNS device-wide.
    // Its queue is BOUNDED (05b-B1): it used to be unbounded, so any app could
    // write queries faster than a slow upstream drained them and grow it until
    // the process died, taking the filter and the warden with it. Past the
    // bound a query is dropped, which the querier treats as a lost packet and
    // retries; the filter's decisions are never skipped, only its backlog.
    private val dropped = AtomicLong()
    /** The last thousand of [dropped] that was logged, so a count that sits
     *  on a multiple of 1000 is logged once, not on every packet read. */
    private var droppedLoggedThousands = 0L
    private val pool = boundedDnsPool(DNS_WORKERS, DNS_QUEUE) { dropped.incrementAndGet() }
    // Multiple workers write to the single tun output; each datagram must be
    // written+flushed atomically or replies would interleave and corrupt.
    private val writeLock = Any()

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Debug builds only: tear the tunnel down while the service, and with it
        // the always-on pin, stays up — the state a dead read loop leaves. The
        // liveness round's STOP_DNS used to call stopService, which is a no-op
        // on a VPN service the platform holds bound, so 05-B7 was never
        // exercised. The service is not exported, so only this app can send it.
        if (intent?.action == ACTION_DEBUG_KILL_TUNNEL &&
            (applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0
        ) {
            Log.w(TAG, "debug: tunnel torn down, service and pin left in place")
            stopTunnel()
            return START_STICKY
        }
        // Always re-read the plan on (re)start; apply calls just restart us.
        // A throw (a refused native core, F3/N4) is not "no policy": keep the
        // tunnel up and block everything rather than crash-loop or pass through.
        val plan = runCatching { CharterCore.dnsPlan() }.getOrElse {
            Log.e(TAG, "web plan unavailable: failing closed", it)
            CharterCore.DnsPlan(
                "", "locked", emptyList(), emptyList(),
                emptyList(), emptyList(), true, "off", emptyList(),
            )
        }
        if (plan == null) {
            // Not paired / no policy: keep the tunnel UP (fail-closed under
            // lockdown) but pass everything through — resolver = unrestricted.
            resolver = DnsResolver(
                CharterCore.DnsPlan("", "unrestricted", emptyList(), emptyList(),
                    emptyList(), emptyList(), false, "off", emptyList())
            )
        } else {
            resolver = DnsResolver(plan)
        }
        // A tunnel whose read loop has ended is as good as none: rebuild it
        // rather than trust the stale descriptor (05-B7).
        if (tun == null || !tunnelUp) {
            stopTunnel()
            startTunnel()
        }
        // The platform restarts an always-on VPN on its own after the process
        // dies, which makes this one more edge that brings the warden back
        // (05-G1). Idempotent: a running warden only re-arms its alarm.
        runCatching {
            if (org.forgesworn.charter.admin.Provisioning.isDeviceOwner(this)) CharterService.start(this)
        }.onFailure { Log.w(TAG, "could not start the warden from the filter", it) }
        return START_STICKY
    }

    /** The ward's VPN was taken away (another VPN, or a revoke). Mark the
     *  tunnel down so the warden's liveness check restarts it (05-B7). */
    override fun onRevoke() {
        Log.e(TAG, "VPN revoked: the warden will restart the filter")
        stopTunnel()
        super.onRevoke()
    }

    private fun startTunnel() {
        val builder = Builder()
            .setSession("Kintrinsic")
            .setBlocking(true)
            .addAddress("10.111.0.2", 32)
            .addAddress("fd00:6368:6172:74::2", 128)
            // Route ONLY resolvers into the tunnel (split tunnel): our virtual
            // ones, and the well-known public ones an app might aim at
            // directly (05-G4).
            .addRoute(DNS_V4, 32)
            .addRoute(DNS_V6, 128)
            .addDnsServer(DNS_V4)
            .addDnsServer(DNS_V6)
        for (r in KnownResolvers.routes) {
            runCatching { builder.addRoute(r.address, r.prefix) }
                .onFailure { Log.w(TAG, "could not route ${r.address}/${r.prefix}", it) }
        }
        // Never route Kintrinsic's own package through the tunnel (defence-in-depth;
        // the relay traffic is not DNS-to-our-IP anyway, but be explicit).
        runCatching { builder.addDisallowedApplication(packageName) }
        val fd = builder.establish() ?: run {
            Log.e(TAG, "establish() returned null — VPN not permitted?")
            stopSelf(); return
        }
        tun = fd
        output = FileOutputStream(fd.fileDescriptor)
        tunnelUp = true
        worker = Thread({ pump(fd) }, "charter-dns").also { it.start() }
    }

    private fun stopTunnel() {
        tunnelUp = false
        worker?.interrupt()
        runCatching { tun?.close() }
        tun = null
        output = null
    }

    private fun pump(fd: ParcelFileDescriptor) {
        val input = FileInputStream(fd.fileDescriptor)
        val buf = ByteArray(32767)
        while (!Thread.currentThread().isInterrupted) {
            val n = try { input.read(buf) } catch (t: Throwable) { break }
            // End of stream is a dead tunnel, not an empty read: spinning on
            // it would keep [tunnelUp] true over a filter that sees nothing.
            if (n < 0) break
            if (n == 0) continue
            // No DNS query is bigger than this; a bigger packet is not one we
            // answer, so it is not copied or queued either.
            if (n > MAX_QUERY_PACKET) continue
            val packet = buf.copyOf(n)
            // Hand off decide+relay to the pool — the read loop must never
            // block on an upstream lookup. Exception-safe: a malformed packet
            // (or any bug in parse/decide/build) must not kill enforcement.
            pool.execute {
                try {
                    val reply = handleIpPacket(packet) ?: return@execute
                    val out = output ?: return@execute
                    synchronized(writeLock) { out.write(reply); out.flush() }
                } catch (t: Throwable) {
                    // Drop the packet; the querier retries. Never propagate.
                }
            }
            val thousands = dropped.get() / DROP_LOG_EVERY
            if (thousands > droppedLoggedThousands) {
                droppedLoggedThousands = thousands
                Log.w(TAG, "DNS backlog full: ${dropped.get()} queries dropped so far")
            }
        }
        // The read loop only ends when the tunnel is gone: say so, so the
        // warden's liveness check restarts it rather than trusting the pin.
        if (tun === fd) tunnelUp = false
    }

    /** Parse an IPv4/IPv6 + UDP/53 DNS query, decide, and synthesize an IP reply. */
    private fun handleIpPacket(packet: ByteArray): ByteArray? {
        val ip = IpUdpDatagram.parse(packet) ?: return null
        if (ip.dstPort != 53) return null
        val q = parseQuestion(ip.payload) ?: return null
        queriesSeen.incrementAndGet()
        val dns = when (val d = resolver!!.decide(q)) {
            is DnsDecision.Block -> { queriesBlocked.incrementAndGet(); buildNxdomain(q) }
            is DnsDecision.PassThrough -> relay(ip.payload) ?: buildNxdomain(q)
            is DnsDecision.Rewrite -> {
                val ips = resolveProtected(d.target)
                if (ips.isEmpty()) buildNxdomain(q) else buildAnswer(q, ips)
            }
        }
        return ip.swapAndWrapUdp(dns)
    }

    /** Relay a raw DNS query to the real upstream resolver over a protected socket. */
    private fun relay(query: ByteArray): ByteArray? = runCatching {
        DatagramSocket().use { s ->
            protect(s)
            s.soTimeout = 4000
            val upstream = InetAddress.getByName(UPSTREAM)
            s.send(DatagramPacket(query, query.size, upstream, 53))
            val resp = ByteArray(4096)
            val dp = DatagramPacket(resp, resp.size)
            s.receive(dp)
            resp.copyOf(dp.length)
        }
    }.getOrNull()

    /** Resolve a controlled rewrite target via a protected DNS lookup. */
    private fun resolveProtected(host: String): List<InetAddress> = runCatching {
        // A minimal A-record query for `host` to the upstream, protected.
        val q = org.forgesworn.charter.enforce.dns.buildQuery(host)
        val resp = relay(q) ?: return emptyList()
        org.forgesworn.charter.enforce.dns.parseAddresses(resp)
    }.getOrElse { emptyList() }

    override fun onDestroy() {
        stopTunnel()
        pool.shutdownNow()
        super.onDestroy()
    }

    companion object {
        private const val TAG = "CharterVpn"
        const val ACTION_APPLY = "org.forgesworn.charter.APPLY_DNS"
        /** Debug builds only; see [onStartCommand]. */
        const val ACTION_DEBUG_KILL_TUNNEL = "org.forgesworn.charter.DEBUG_KILL_TUNNEL"

        /** Queries this process's filter has decided, and how many it refused:
         *  lets a test round tell "the filter allowed it" from "the lookup
         *  never reached the filter". */
        val queriesSeen = AtomicLong()
        val queriesBlocked = AtomicLong()
        const val EXTRA_REVISION = "revision"
        const val DNS_V4 = "10.111.0.53"
        const val DNS_V6 = "fd00:6368:6172:74::53"
        // The real resolver to relay pass-through + rewrite lookups to. A public
        // resolver keeps this independent of the underlying network's DNS (which
        // GrapheneOS may set per-network); revisit if we want the link's own DNS.
        const val UPSTREAM = "9.9.9.9"

        /** Worker threads relaying queries upstream. */
        private const val DNS_WORKERS = 8
        /** Queries that may wait for a worker before new ones are dropped
         *  (05b-B1). 256 × a query-sized packet is well under a megabyte. */
        internal const val DNS_QUEUE = 256
        /** An IPv6 + UDP header and the largest EDNS payload we relay. */
        internal const val MAX_QUERY_PACKET = 40 + 8 + 4096
        private const val DROP_LOG_EVERY = 1000L

        /** Whether this process's tunnel is established and being read
         *  (05-B7) — see [org.forgesworn.charter.enforce.DnsFilterOps.isLive]. */
        @Volatile var tunnelUp = false
            private set

        fun applyIntent(context: Context, revision: String): Intent =
            Intent(context, CharterVpnService::class.java)
                .setAction(ACTION_APPLY)
                .putExtra(EXTRA_REVISION, revision)
    }
}

/**
 * The DNS worker pool: [threads] workers over a queue of at most [queue]
 * waiting queries. A query that arrives when the queue is full is dropped and
 * [onDrop] is told; it never throws into the read loop and never grows memory
 * (05b-B1).
 */
internal fun boundedDnsPool(threads: Int, queue: Int, onDrop: () -> Unit): ThreadPoolExecutor =
    ThreadPoolExecutor(
        threads,
        threads,
        0L,
        TimeUnit.MILLISECONDS,
        ArrayBlockingQueue(queue),
        RejectedExecutionHandler { _, _ -> onDrop() },
    )
