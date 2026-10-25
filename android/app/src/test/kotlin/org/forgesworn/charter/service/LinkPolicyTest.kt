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
                "Kintrinsic a1b2" to listOf("192.168.1.9:47615", "192.168.1.5:47615"),
                "Kintrinsic c3d4" to listOf("192.168.1.5:47615"),
            ),
        )
        assertEquals(listOf("192.168.1.5:47615", "192.168.1.9:47615"), merged)
        val many = (1..20).associate { "s$it" to listOf("10.0.0.$it:1") }
        assertEquals(LinkPolicy.MAX_ADDRS, LinkPolicy.merge(many).size)
        assertTrue(LinkPolicy.merge(emptyMap()).isEmpty())
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
                """"lastError":null,"running":true,"appliedSeq":7}""",
        )
        assertTrue(s.running)
        assertTrue(s.connected)
        assertEquals(7L, s.appliedSeq)
        assertEquals(null, s.lastError)
        val off = CharterCore.parseLinkState("""{"running":false,"connected":false,"appliedSeq":0}""")
        assertFalse(off.running)
        assertFalse(off.connected)
        val junk = CharterCore.parseLinkState("""{"error":"not initialized"}""")
        assertFalse(junk.running)
        assertFalse(CharterCore.parseLinkState("not json").running)
    }
}
