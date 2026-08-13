package org.forgesworn.charter.enforce.hotspot

import org.forgesworn.charter.native.CharterCore.DnsPlan
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.InetAddress
import java.net.ServerSocket
import java.net.Socket

/**
 * Host-JVM tests for the proxy's second gate (public-internet-only egress) and
 * its abuse caps. The filter vets the NAME a guest asked for; these prove the
 * proxy also vets the ADDRESS the dial actually landed on (DNS rebinding /
 * unrestricted-mode raw IPs), bounds request parsing, and refuses denied ports.
 */
class HotspotProxyTest {

    private var proxy: HotspotProxy? = null
    private var origin: ServerSocket? = null

    // Unrestricted plan: the DOMAIN filter passes everything, so whatever gets
    // refused in these tests is refused by the proxy's own gates.
    private fun openPlan() = DnsPlan(
        revision = "r1", mode = "unrestricted",
        allowDomains = emptyList(), blockDomains = emptyList(),
        blockCategories = emptyList(), allowExceptions = emptyList(),
        safeSearch = false, youtubeRestrict = "off", rewrites = emptyList(),
    )

    @After
    fun tearDown() {
        proxy?.stop()
        origin?.close()
    }

    private fun startProxy(
        egressGuard: ((InetAddress) -> Boolean)? = null,
        dial: ((String, Int) -> Socket)? = null,
    ): HotspotProxy {
        val p = when {
            egressGuard != null && dial != null -> HotspotProxy(
                bind = InetAddress.getLoopbackAddress(), port = 0,
                filterProvider = { HotspotFilter(openPlan()) },
                dial = dial, egressGuard = egressGuard,
            )
            dial != null -> HotspotProxy(
                bind = InetAddress.getLoopbackAddress(), port = 0,
                filterProvider = { HotspotFilter(openPlan()) },
                dial = dial,
            )
            else -> HotspotProxy(
                bind = InetAddress.getLoopbackAddress(), port = 0,
                filterProvider = { HotspotFilter(openPlan()) },
            )
        }
        p.start()
        proxy = p
        return p
    }

    /** Send one CONNECT and return the status line (null = connection closed). */
    private fun connectVia(p: HotspotProxy, target: String, headers: String = ""): String? =
        Socket(InetAddress.getLoopbackAddress(), p.actualPort).use { s ->
            val out = OutputStreamWriter(s.getOutputStream())
            out.write("CONNECT $target HTTP/1.1\r\n$headers\r\n")
            out.flush()
            BufferedReader(InputStreamReader(s.getInputStream())).readLine()
        }

    // --- the egress guard classifier ----------------------------------------

    @Test fun guard_refuses_every_non_public_address_class() {
        for (
            bad in listOf(
                "127.0.0.1", // loopback
                "0.0.0.0", // any-local
                "169.254.10.10", // IPv4 link-local
                "10.0.0.5", "172.16.0.5", "192.168.1.1", // RFC1918
                "100.64.0.1", "100.127.255.254", // CGNAT 100.64/10
                "224.0.0.1", // multicast
                "::1", "::", "fe80::1", "fc00::1", "fd12:3456::1", // v6 classes
            )
        ) {
            assertFalse(bad, isPublicInternetAddress(InetAddress.getByName(bad)))
        }
    }

    @Test fun guard_allows_public_addresses() {
        for (good in listOf("93.184.216.34", "1.1.1.1", "100.63.0.1", "100.128.0.1", "2606:4700::1")) {
            assertTrue(good, isPublicInternetAddress(InetAddress.getByName(good)))
        }
    }

    // --- the guard applied to a live tunnel ---------------------------------

    @Test fun default_guard_refuses_a_dial_landing_on_loopback() {
        // Stand-in for DNS rebinding: the name passes the (unrestricted) filter
        // but the dial lands on 127.0.0.1 — the ward's phone itself.
        val v4 = InetAddress.getByName("127.0.0.1")
        val srv = ServerSocket(0, 1, v4)
        origin = srv
        val p = startProxy(dial = { _, _ -> Socket(v4, srv.localPort) })
        val status = connectVia(p, "rebound.example:443")
        assertTrue("expected 403 for loopback egress, got: $status", status != null && status!!.contains("403"))
    }

    @Test fun permissive_guard_tunnels_to_the_origin() {
        val v4 = InetAddress.getByName("127.0.0.1")
        val srv = ServerSocket(0, 1, v4)
        origin = srv
        Thread {
            runCatching {
                srv.accept().use { c ->
                    val r = BufferedReader(InputStreamReader(c.getInputStream()))
                    val w = OutputStreamWriter(c.getOutputStream())
                    w.write("echo:${r.readLine()}\n"); w.flush()
                }
            }
        }.apply { isDaemon = true }.start()

        val p = startProxy(egressGuard = { true }, dial = { _, _ -> Socket(v4, srv.localPort) })
        Socket(InetAddress.getLoopbackAddress(), p.actualPort).use { s ->
            val out = OutputStreamWriter(s.getOutputStream())
            out.write("CONNECT news.example:443 HTTP/1.1\r\n\r\n")
            out.flush()
            val reader = BufferedReader(InputStreamReader(s.getInputStream()))
            val status = reader.readLine()
            assertTrue("expected 200, got: $status", status != null && status.contains("200"))
            while (true) { val l = reader.readLine(); if (l.isNullOrEmpty()) break }
            out.write("ping\n"); out.flush()
            assertEquals("echo:ping", reader.readLine())
        }
    }

    // --- ports and parsing caps ---------------------------------------------

    @Test fun smtp_port_25_is_refused_before_any_dial() {
        val p = startProxy(dial = { _, _ -> throw AssertionError("must not dial port 25") })
        val status = connectVia(p, "mail.example:25")
        assertTrue("expected 403 for port 25, got: $status", status != null && status!!.contains("403"))
    }

    @Test fun out_of_range_port_is_a_bad_request() {
        val p = startProxy(dial = { _, _ -> throw AssertionError("must not dial") })
        assertTrue(connectVia(p, "a.example:0")!!.contains("400"))
        assertTrue(connectVia(p, "a.example:99999")!!.contains("400"))
    }

    @Test fun oversized_request_line_is_dropped_not_buffered() {
        val p = startProxy(dial = { _, _ -> throw AssertionError("must not dial") })
        Socket(InetAddress.getLoopbackAddress(), p.actualPort).use { s ->
            val out = s.getOutputStream()
            // One endless "line", far past the cap: the proxy must close, not
            // buffer it into the warden app's heap.
            val chunk = ByteArray(8 * 1024) { 'a'.code.toByte() }
            runCatching { repeat(4) { out.write(chunk) }; out.flush() }
            val got = runCatching {
                BufferedReader(InputStreamReader(s.getInputStream())).readLine()
            }.getOrNull()
            assertNull("connection must just close on an over-cap line, got: $got", got)
        }
    }

    @Test fun header_flood_is_rejected() {
        val p = startProxy(dial = { _, _ -> throw AssertionError("must not dial") })
        val flood = StringBuilder()
        repeat(200) { flood.append("X-Pad-$it: x\r\n") }
        val status = connectVia(p, "a.example:443", headers = flood.toString())
        assertTrue("expected 400 for header flood, got: $status", status != null && status!!.contains("400"))
    }
}
