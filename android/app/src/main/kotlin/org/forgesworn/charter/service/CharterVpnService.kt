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
import java.util.concurrent.Executors

/**
 * The DNS-filtering TUN. Only the virtual DNS server addresses are routed into
 * the tunnel, so every non-DNS packet flows over the real network untouched
 * (split tunnel). Each captured DNS query is decided by the shared plan:
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
    // Bounded pool for decide+relay so one slow upstream lookup (soTimeout up
    // to 4s) can never stall the read loop — and with it all DNS device-wide.
    private val pool = Executors.newFixedThreadPool(8)
    // Multiple workers write to the single tun output; each datagram must be
    // written+flushed atomically or replies would interleave and corrupt.
    private val writeLock = Any()

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Always re-read the plan on (re)start; apply calls just restart us.
        val plan = CharterCore.dnsPlan()
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
        if (tun == null) startTunnel()
        return START_STICKY
    }

    private fun startTunnel() {
        val builder = Builder()
            .setSession("Kintrinsic")
            .setBlocking(true)
            .addAddress("10.111.0.2", 32)
            .addAddress("fd00:6368:6172:74::2", 128)
            // Route ONLY our virtual resolvers into the tunnel (split tunnel).
            .addRoute(DNS_V4, 32)
            .addRoute(DNS_V6, 128)
            .addDnsServer(DNS_V4)
            .addDnsServer(DNS_V6)
        // Never route Kintrinsic's own package through the tunnel (defence-in-depth;
        // the relay traffic is not DNS-to-our-IP anyway, but be explicit).
        runCatching { builder.addDisallowedApplication(packageName) }
        val fd = builder.establish() ?: run {
            Log.e(TAG, "establish() returned null — VPN not permitted?")
            stopSelf(); return
        }
        tun = fd
        output = FileOutputStream(fd.fileDescriptor)
        worker = Thread({ pump(fd) }, "charter-dns").also { it.start() }
    }

    private fun pump(fd: ParcelFileDescriptor) {
        val input = FileInputStream(fd.fileDescriptor)
        val buf = ByteArray(32767)
        while (!Thread.currentThread().isInterrupted) {
            val n = try { input.read(buf) } catch (t: Throwable) { break }
            if (n <= 0) continue
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
        }
    }

    /** Parse an IPv4/IPv6 + UDP/53 DNS query, decide, and synthesize an IP reply. */
    private fun handleIpPacket(packet: ByteArray): ByteArray? {
        val ip = IpUdpDatagram.parse(packet) ?: return null
        if (ip.dstPort != 53) return null
        val q = parseQuestion(ip.payload) ?: return null
        val dns = when (val d = resolver!!.decide(q)) {
            is DnsDecision.Block -> buildNxdomain(q)
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
        worker?.interrupt()
        pool.shutdownNow()
        runCatching { tun?.close() }
        tun = null
        output = null
        super.onDestroy()
    }

    companion object {
        private const val TAG = "CharterVpn"
        const val ACTION_APPLY = "org.forgesworn.charter.APPLY_DNS"
        const val EXTRA_REVISION = "revision"
        const val DNS_V4 = "10.111.0.53"
        const val DNS_V6 = "fd00:6368:6172:74::53"
        // The real resolver to relay pass-through + rewrite lookups to. A public
        // resolver keeps this independent of the underlying network's DNS (which
        // GrapheneOS may set per-network); revisit if we want the link's own DNS.
        const val UPSTREAM = "9.9.9.9"

        fun applyIntent(context: Context, revision: String): Intent =
            Intent(context, CharterVpnService::class.java)
                .setAction(ACTION_APPLY)
                .putExtra(EXTRA_REVISION, revision)
    }
}
