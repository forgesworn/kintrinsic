package org.forgesworn.charter.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

/** 05b-B1 (bounded DNS backlog) and 05-G4 (resolvers routed into the tunnel). */
class DnsTunnelBoundsTest {

    @Test
    fun `a flood past the bound is dropped, not queued, and never throws`() {
        val drops = AtomicInteger()
        val pool = boundedDnsPool(threads = 2, queue = 4) { drops.incrementAndGet() }
        val release = CountDownLatch(1)
        val ran = AtomicInteger()
        try {
            repeat(1_000) {
                pool.execute {
                    release.await(5, TimeUnit.SECONDS)
                    ran.incrementAndGet()
                }
            }
            // Two running, four waiting, the rest dropped.
            assertTrue(pool.queue.size <= 4)
            assertEquals(1_000 - 2 - 4, drops.get())
        } finally {
            release.countDown()
            pool.shutdown()
            pool.awaitTermination(5, TimeUnit.SECONDS)
        }
        assertEquals(6, ran.get())
    }

    @Test
    fun `the production bound keeps the backlog small`() {
        assertTrue(CharterVpnService.DNS_QUEUE in 1..1024)
        // Largest query packet we copy: an IPv6 + UDP header and an EDNS payload.
        assertEquals(40 + 8 + 4096, CharterVpnService.MAX_QUERY_PACKET)
    }

    @Test
    fun `every routed resolver is a literal address with a sane prefix`() {
        val routes = KnownResolvers.routes
        assertTrue(routes.size > 20)
        for (r in routes) {
            val addr = InetAddress.getByName(r.address) // literal: no lookup
            val max = addr.address.size * 8
            assertTrue("${r.address}/${r.prefix}", r.prefix in 16..max)
        }
    }

    @Test
    fun `the big public resolvers are routed, on both families`() {
        val addrs = KnownResolvers.routes.map { it.address }.toSet()
        for (a in listOf("8.8.8.8", "1.1.1.1", "9.9.9.9", "2001:4860:4860::8888", "2606:4700:4700::1111")) {
            assertTrue(a, a in addrs)
        }
    }

    @Test
    fun `routes are not duplicated`() {
        val routes = KnownResolvers.routes
        assertEquals(routes.size, routes.toSet().size)
    }
}
