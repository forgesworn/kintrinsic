package org.forgesworn.charter

import org.forgesworn.charter.enforce.hotspot.HotspotFilter
import org.forgesworn.charter.enforce.hotspot.HotspotProxy
import org.forgesworn.charter.native.CharterCore.DnsPlan
import org.forgesworn.charter.native.CharterCore.DnsRewrite
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket

/**
 * The Kintrinsic Hotspot proxy over a REAL loopback socket on the device: a guest
 * speaking HTTP CONNECT to a blocked host is refused (403), and to an allowed
 * host is tunnelled to an origin — proving the filtering doorman end-to-end
 * (everything a guest device does minus the physical AP association). This is
 * the heart of the world-first "filtered tethering".
 */
class HotspotProxyE2ETest {

    private var proxy: HotspotProxy? = null
    private var origin: ServerSocket? = null

    private fun plan(vararg block: String) = DnsPlan(
        revision = "r1", mode = "blocklist",
        allowDomains = emptyList(),
        blockDomains = block.toList(),
        blockCategories = emptyList(),
        allowExceptions = emptyList(),
        safeSearch = false, youtubeRestrict = "off",
        rewrites = emptyList<DnsRewrite>(),
    )

    @After
    fun tearDown() {
        proxy?.stop()
        origin?.close()
    }

    @Test
    fun blocked_host_gets_403_from_proxy() {
        val p = HotspotProxy(
            bind = InetAddress.getLoopbackAddress(),
            port = 0,
            filterProvider = { HotspotFilter(plan("blocked.example")) },
        ).also { it.start() }
        proxy = p

        Socket(InetAddress.getLoopbackAddress(), p.actualPort).use { s ->
            val out = OutputStreamWriter(s.getOutputStream())
            out.write("CONNECT blocked.example:443 HTTP/1.1\r\nHost: blocked.example:443\r\n\r\n")
            out.flush()
            val line = BufferedReader(InputStreamReader(s.getInputStream())).readLine()
            assertTrue("expected a 403 refusal, got: $line", line != null && line.contains("403"))
        }
    }

    @Test
    fun allowed_host_tunnels_bytes_to_origin() {
        // A local origin the proxy will dial (stands in for the real internet
        // host); it echoes one line so we can prove the tunnel carries bytes.
        val v4 = InetAddress.getByName("127.0.0.1")
        val srv = ServerSocket(0, 1, v4)
        origin = srv
        Thread {
            runCatching {
                srv.accept().use { c ->
                    val r = BufferedReader(InputStreamReader(c.getInputStream()))
                    val w = OutputStreamWriter(c.getOutputStream())
                    val got = r.readLine()
                    w.write("echo:$got\n"); w.flush()
                }
            }
        }.apply { isDaemon = true }.start()

        // The doorman dials by hostname; an injected dial routes the (unblocked)
        // hostname to the loopback origin so we exercise the real allow path
        // without a raw-IP CONNECT (which correctly fails closed). The egress
        // guard is opened up because this origin IS loopback — production's
        // default guard refuses non-public addresses (covered by unit tests).
        val p = HotspotProxy(
            bind = InetAddress.getLoopbackAddress(),
            port = 0,
            filterProvider = { HotspotFilter(plan("blocked.example")) },
            dial = { _, _ -> Socket(v4, srv.localPort) },
            egressGuard = { true },
        ).also { it.start() }
        proxy = p

        Socket(InetAddress.getLoopbackAddress(), p.actualPort).use { s ->
            val out = OutputStreamWriter(s.getOutputStream())
            out.write("CONNECT news.example:443 HTTP/1.1\r\n\r\n")
            out.flush()
            val reader = BufferedReader(InputStreamReader(s.getInputStream()))
            val status = reader.readLine()
            assertTrue("expected 200 tunnel established, got: $status",
                status != null && status.contains("200"))
            // Consume the blank line terminating the proxy response headers.
            while (true) { val l = reader.readLine(); if (l.isNullOrEmpty()) break }
            // Now the socket is a raw tunnel to the origin: send a line, get echo.
            out.write("ping\n"); out.flush()
            assertEquals("echo:ping", reader.readLine())
        }
    }
}
