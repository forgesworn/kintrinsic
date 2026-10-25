package org.forgesworn.charter.service

import java.net.InetAddress
import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The pure half of local-link discovery (the ward is a client only). */
class LinkPolicyTest {
    private fun ip(s: String): InetAddress = InetAddress.getByName(s)

    @Test
    fun `discovery runs only on a network, with a link, while not connected`() {
        assertTrue(LinkPolicy.shouldDiscover(networkUp = true, linkRunning = true, connected = false))
        assertFalse(LinkPolicy.shouldDiscover(networkUp = true, linkRunning = true, connected = true))
        assertFalse(LinkPolicy.shouldDiscover(networkUp = false, linkRunning = true, connected = false))
        assertFalse(LinkPolicy.shouldDiscover(networkUp = true, linkRunning = false, connected = false))
    }

    @Test
    fun `endpoints keep IPv4 with a real port and drop the rest`() {
        val hosts = listOf(
            ip("192.168.1.5"),
            ip("fe80::1"),
            ip("0.0.0.0"),
            ip("127.0.0.1"),
            ip("224.0.0.251"),
            ip("10.0.0.7"),
            ip("192.168.1.5"),
        )
        assertEquals(listOf("192.168.1.5:47615", "10.0.0.7:47615"), LinkPolicy.endpoints(hosts, 47_615))
        assertTrue(LinkPolicy.endpoints(hosts, 0).isEmpty())
        assertTrue(LinkPolicy.endpoints(hosts, 70_000).isEmpty())
        assertTrue(LinkPolicy.endpoints(emptyList(), 47_615).isEmpty())
    }

    @Test
    fun `merge is the full, stable, capped set`() {
        val merged = LinkPolicy.merge(
            mapOf(
                "Kintrinsic a1b2" to LinkPolicy.Found(listOf("192.168.1.9:47615", "192.168.1.5:47615"), 7),
                "Kintrinsic c3d4" to LinkPolicy.Found(listOf("192.168.1.5:47615"), 7),
            ),
        )
        assertEquals(listOf("192.168.1.5:47615", "192.168.1.9:47615"), merged)
        val many = (1..20).associate { "s$it" to LinkPolicy.Found(listOf("10.0.0.$it:1"), 0) }
        assertEquals(LinkPolicy.MAX_ADDRS, LinkPolicy.merge(many).size)
        assertTrue(LinkPolicy.merge(emptyMap()).isEmpty())
    }

    @Test
    fun `the sockets bind to the network the guardian was found on`() {
        val wifi = 432_902_426_637L
        val found = mapOf(
            "Kintrinsic a1b2" to LinkPolicy.Found(listOf("192.168.1.9:47615"), wifi),
            "Kintrinsic c3d4" to LinkPolicy.Found(listOf("10.9.0.2:47615"), 99),
        )
        // The first address of the merged set ("10.9…") decides.
        assertEquals(99L, LinkPolicy.netHandle(found, fallback = 5))
        assertEquals(wifi, LinkPolicy.netHandle(found.filterKeys { it == "Kintrinsic a1b2" }, fallback = 5))
        // Nothing found, or found on an unknown network: the current one.
        assertEquals(5L, LinkPolicy.netHandle(emptyMap(), fallback = 5))
        assertEquals(5L, LinkPolicy.netHandle(mapOf("x" to LinkPolicy.Found(listOf("192.168.1.9:1"), 0)), 5))
        val body = org.json.JSONObject(CharterCore.linkDiscoveredBody(listOf("192.168.1.9:47615"), wifi))
        assertEquals(wifi, body.getLong("netHandle"))
        assertEquals("192.168.1.9:47615", body.getJSONArray("addrs").getString(0))
    }

    @Test
    fun `a new link generation is told the discovered set again`() {
        assertTrue(LinkPolicy.mustResend(lastGen = null, gen = 1))
        assertTrue(LinkPolicy.mustResend(lastGen = 1, gen = 2))
        assertFalse(LinkPolicy.mustResend(lastGen = 2, gen = 2))
        // Not running (or an older library): nothing to tell.
        assertFalse(LinkPolicy.mustResend(lastGen = 2, gen = null))
    }

    @Test
    fun `discovery browses a minute, rests ten, and wakes at once`() {
        val d = DiscoveryDuty(windowMs = 60_000, pauseMs = 600_000)
        assertTrue(d.browse(0))
        assertEquals(60_000L, d.nextChangeIn(0))
        assertTrue(d.browse(59_999))
        // Nothing connected within the window: rest.
        assertFalse(d.browse(60_000))
        assertEquals(600_000L, d.nextChangeIn(60_000))
        assertFalse(d.browse(300_000))
        // The rest is over: a new window.
        assertTrue(d.browse(660_000))
        assertFalse(d.browse(720_000))
        // A network change, screen-on or kick browses again at once.
        d.wake()
        assertTrue(d.browse(720_001))
        assertTrue(d.browse(780_000))
        assertFalse(d.browse(780_001))
        // Not wanted (connected) resets it: the next want starts a window.
        d.idle()
        assertTrue(d.browse(800_000))
    }

    @Test
    fun `any move of the applied count ticks, the first reading does not`() {
        assertFalse(LinkPolicy.appliedMoved(null, 4))
        assertFalse(LinkPolicy.appliedMoved(4, 4))
        assertTrue(LinkPolicy.appliedMoved(4, 5))
        // The native core restarted and counts from zero again.
        assertTrue(LinkPolicy.appliedMoved(4, 0))
    }

    @Test
    fun `link state parses, and anything unreadable is not running`() {
        val s = CharterCore.parseLinkState(
            """{"connected":true,"sessions":[],"candidates":1,"remembered":null,"outbox":0,""" +
                """"lastError":null,"running":true,"appliedSeq":7,"gen":3}""",
        )
        assertEquals(3L, s.gen)
        assertEquals(1, s.candidates)
        assertTrue(s.running)
        assertTrue(s.connected)
        assertEquals(7L, s.appliedSeq)
        assertEquals(null, s.lastError)
        val off = CharterCore.parseLinkState("""{"running":false,"connected":false,"appliedSeq":0}""")
        assertFalse(off.running)
        assertFalse(off.connected)
        assertEquals(null, off.gen)
        val junk = CharterCore.parseLinkState("""{"error":"not initialized"}""")
        assertFalse(junk.running)
        assertFalse(CharterCore.parseLinkState("not json").running)
    }
}
