package org.forgesworn.charter.enforce.hotspot

import android.util.Log
import java.io.InputStream
import java.io.OutputStream
import java.net.Inet6Address
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.ServerSocket
import java.net.Socket
import java.util.concurrent.Executors
import java.util.concurrent.atomic.AtomicBoolean

/**
 * The Kintrinsic Hotspot filtering proxy: an HTTP CONNECT proxy that guests on the
 * app-hosted local-only AP point their traffic at. Every tunnel is gated by
 * [HotspotFilter] (the SAME policy as the on-phone DNS filter), so guest devices
 * get FILTERED internet — the core of Kintrinsic's filtered-tethering, a thing no
 * shipping parental product does. Non-proxied guest traffic simply gets no
 * route (the local-only AP forwards nothing), so the network fails closed.
 *
 * Egress uses [dial], which production binds to the phone's upstream via
 * `VpnService.protect()` so the proxy's own sockets skip the on-device tun (no
 * circular route). The default dials directly — used by the loopback test.
 *
 * [filterProvider] is called per request so a mid-session policy change (or the
 * grant expiring, tearing the whole service down) is honoured immediately.
 */
class HotspotProxy(
    private val bind: InetAddress,
    private val port: Int,
    private val filterProvider: () -> HotspotFilter,
    private val dial: (host: String, port: Int) -> Socket = { h, p ->
        // Bounded connect (a black-holed IP must not pin a pool thread for the
        // OS default ~2min). InetSocketAddress resolves the name here, so the
        // egress guard below vets the address we ACTUALLY connected to.
        Socket().apply { connect(InetSocketAddress(h, p), CONNECT_MS) }
    },
    /**
     * Second gate on every tunnel, applied to the address the upstream socket
     * actually CONNECTED to (post-DNS): guests may reach the public internet
     * only. The domain filter vets the NAME a guest asked for, but the name is
     * resolved at dial time — without this, a hostname pointing at 127.0.0.1 /
     * 192.168.x.x (DNS rebinding), or a raw private IP in `unrestricted` mode,
     * would land the ward's phone connecting into itself or the home LAN on a
     * guest's behalf. Tests inject a permissive guard to tunnel over loopback.
     */
    private val egressGuard: (InetAddress) -> Boolean = ::isPublicInternetAddress,
) {
    @Volatile private var server: ServerSocket? = null
    private val running = AtomicBoolean(false)
    private val pool = Executors.newCachedThreadPool()
    private var acceptor: Thread? = null
    // Live client + upstream sockets, so stop() can force every tunnel closed
    // (a thread blocked in a raw socket read ignores interrupt).
    private val liveSockets = java.util.Collections.newSetFromMap(
        java.util.concurrent.ConcurrentHashMap<Socket, Boolean>(),
    )

    val actualPort: Int get() = server?.localPort ?: -1

    /** Open guest sockets (client + upstream halves both counted): non-zero
     *  means somebody is on the AP right now. Feeds the idle auto-off. */
    val liveSocketCount: Int get() = liveSockets.size

    /**
     * Monotonic stamp (nanoTime) of the last guest connection accepted. Set at
     * start() so an AP nobody ever joined ages from bring-up rather than
     * looking eternally fresh. nanoTime, not wall-clock: the DPM requires
     * automatic time on these phones, and an NTP correction must not read as
     * fifteen idle minutes.
     */
    @Volatile var lastActivityNanos: Long = 0L
        private set

    fun start() {
        if (running.getAndSet(true)) return
        val s = ServerSocket(port, BACKLOG, bind)
        server = s
        lastActivityNanos = System.nanoTime()
        acceptor = Thread {
            while (running.get()) {
                val client = try {
                    s.accept()
                } catch (_: Throwable) {
                    break // socket closed on stop()
                }
                pool.execute { runCatching { handle(client) } }
            }
        }.apply { isDaemon = true; start() }
    }

    fun stop() {
        if (!running.getAndSet(false)) return
        runCatching { server?.close() }
        acceptor?.interrupt()
        // Force every live tunnel closed — reads blocked on these sockets won't
        // wake for pool.shutdownNow()'s interrupt, so close the sockets directly.
        for (s in liveSockets) runCatching { s.close() }
        liveSockets.clear()
        pool.shutdownNow()
    }

    private fun handle(client: Socket) {
        lastActivityNanos = System.nanoTime()
        liveSockets.add(client)
        try {
            client.use { c ->
                // Bound the header read so a guest can't park a thread by opening a
                // socket and never sending a request line.
                runCatching { c.soTimeout = IDLE_MS }
                val requestLine = readLine(c.getInputStream()) ?: return
                if (!drainHeaders(c.getInputStream())) {
                    c.getOutputStream().write(response(400, "Bad Request"))
                    return
                }

                val target = parseConnect(requestLine)
                if (target == null) {
                    c.getOutputStream().write(response(400, "Bad Request"))
                    return
                }
                val (host, dstPort) = target
                if (dstPort in DENY_PORTS) {
                    // Port 25: an open CONNECT proxy is otherwise a spam relay
                    // riding the ward's IP. (Submission 465/587 stay open.)
                    Log.i(TAG, "refuse $host:$dstPort (denied port)")
                    c.getOutputStream().write(response(403, "Blocked by Kintrinsic"))
                    return
                }

                when (val v = filterProvider().verdict(host)) {
                    is HotspotVerdict.Refuse -> {
                        Log.i(TAG, "refuse $host")
                        c.getOutputStream().write(response(403, "Blocked by Kintrinsic"))
                    }
                    is HotspotVerdict.Dial -> {
                        val upstream = try {
                            dial(v.host, dstPort).also { it.soTimeout = IDLE_MS }
                        } catch (e: Throwable) {
                            Log.w(TAG, "dial ${v.host}:$dstPort failed", e)
                            c.getOutputStream().write(response(502, "Bad Gateway"))
                            return
                        }
                        val remote = upstream.inetAddress
                        if (remote == null || !egressGuard(remote)) {
                            Log.i(TAG, "refuse $host -> ${remote?.hostAddress} (non-public egress)")
                            runCatching { upstream.close() }
                            c.getOutputStream().write(response(403, "Blocked by Kintrinsic"))
                            return
                        }
                        liveSockets.add(upstream)
                        try {
                            upstream.use { up ->
                                c.getOutputStream().write(response(200, "Connection Established"))
                                c.getOutputStream().flush()
                                pump(c, up)
                            }
                        } finally {
                            liveSockets.remove(upstream)
                        }
                    }
                }
            }
        } finally {
            // Always drop the client — a thrown pump/read or an early return must
            // never leave a (now-closed) socket referenced in the live set until
            // stop(); over a long session that would leak unboundedly.
            liveSockets.remove(client)
        }
    }

    /** Bidirectional byte pump; returns when either side closes. */
    private fun pump(a: Socket, b: Socket) {
        val t = Thread { runCatching { a.getInputStream().copyTo(b.getOutputStream()); b.shutdownOutput() } }
            .apply { isDaemon = true; start() }
        runCatching { b.getInputStream().copyTo(a.getOutputStream()); a.shutdownOutput() }
        t.join()
    }

    /** `CONNECT host:port HTTP/1.1` → (host, port); null if not a valid CONNECT. */
    private fun parseConnect(line: String): Pair<String, Int>? {
        val parts = line.split(' ')
        if (parts.size < 2 || !parts[0].equals("CONNECT", ignoreCase = true)) return null
        val hostPort = parts[1]
        val idx = hostPort.lastIndexOf(':')
        if (idx <= 0) return null
        val host = hostPort.substring(0, idx)
        val p = hostPort.substring(idx + 1).toIntOrNull() ?: return null
        if (p !in 1..65535) return null
        return host to p
    }

    /** One CRLF line, capped at [MAX_LINE_LEN] — a guest streaming an endless
     *  "line" must exhaust its cap, not the warden app's heap. Over-cap (like
     *  EOF-before-any-bytes) reads as null = bad request, connection dropped. */
    private fun readLine(input: InputStream): String? {
        val sb = StringBuilder()
        while (true) {
            val ch = input.read()
            if (ch == -1) return if (sb.isEmpty()) null else sb.toString()
            if (ch == '\n'.code) return sb.toString().trimEnd('\r')
            if (sb.length >= MAX_LINE_LEN) return null
            sb.append(ch.toChar())
        }
    }

    /** Swallow request headers up to a sane count. False = malformed/abusive
     *  (EOF before the blank line, an over-cap header line, or too many) —
     *  never fall through to tunnelling with unconsumed request bytes. */
    private fun drainHeaders(input: InputStream): Boolean {
        var n = 0
        while (true) {
            val l = readLine(input) ?: return false
            if (l.isEmpty()) return true
            if (++n > MAX_HEADER_LINES) return false
        }
    }

    private fun response(code: Int, reason: String): ByteArray =
        "HTTP/1.1 $code $reason\r\n\r\n".toByteArray(Charsets.US_ASCII)

    private companion object {
        const val TAG = "HotspotProxy"
        const val BACKLOG = 32
        // Idle read timeout on both legs of a tunnel — an idle-but-open
        // connection is reclaimed instead of pinning two threads forever.
        const val IDLE_MS = 120_000
        // Upstream connect timeout — a black-holed dial must release its thread.
        const val CONNECT_MS = 15_000
        // Request parsing caps: CONNECT lines are tiny; anything huge is abuse.
        const val MAX_LINE_LEN = 4096
        const val MAX_HEADER_LINES = 128
        // TCP 25 (raw SMTP relay) is the one port an open tunnel proxy must
        // never offer — everything else stays open for legit guest traffic.
        val DENY_PORTS = setOf(25)
    }
}

/**
 * True only for an address on the public internet — the default egress guard.
 * Refused: loopback, any-local (0.0.0.0/::), link-local (169.254/16, fe80::/10),
 * RFC1918 site-local, IPv6 unique-local (fc00::/7), multicast, and CGNAT
 * 100.64.0.0/10 (carrier-shared space — never a legitimate guest destination).
 * Internal (not private) so the unit test pins each class explicitly.
 */
internal fun isPublicInternetAddress(a: InetAddress): Boolean {
    if (
        a.isLoopbackAddress || a.isAnyLocalAddress || a.isLinkLocalAddress ||
        a.isSiteLocalAddress || a.isMulticastAddress
    ) {
        return false
    }
    val b = a.address
    if (a is Inet6Address) {
        // fc00::/7 — RFC 4193 unique-local (isSiteLocalAddress only covers the
        // deprecated fec0::/10, so ULAs need their own check).
        if ((b[0].toInt() and 0xfe) == 0xfc) return false
    } else if (b.size == 4) {
        // 100.64.0.0/10 — RFC 6598 carrier-grade NAT shared space.
        if (b[0].toInt() and 0xff == 100 && (b[1].toInt() and 0xc0) == 0x40) return false
    }
    return true
}

private fun InputStream.copyTo(out: OutputStream) {
    val buf = ByteArray(16 * 1024)
    while (true) {
        val n = read(buf)
        if (n == -1) break
        out.write(buf, 0, n)
        out.flush()
    }
}
