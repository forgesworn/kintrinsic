package org.forgesworn.charter.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** 05-G2 posture pacing and 05-B7 DNS-tunnel liveness. */
class PostureAndDnsLivenessTest {

    @Test
    fun `the posture pass is due first time, on its interval, and after the clock goes back`() {
        assertTrue(postureDue(1_000, 0, 30))
        assertFalse(postureDue(1_029, 1_000, 30))
        assertTrue(postureDue(1_030, 1_000, 30))
        assertTrue(postureDue(900, 1_000, 30))
    }

    @Test
    fun `a new plan revision is always dispatched`() {
        assertTrue(dnsDispatchWanted("r2", "r1", tunnelLive = true, nowUnix = 10, lastRedispatchUnix = 9, redispatchSecs = 30))
        assertTrue(dnsDispatchWanted("r1", null, tunnelLive = true, nowUnix = 10, lastRedispatchUnix = 9, redispatchSecs = 30))
    }

    @Test
    fun `an applied revision with a live tunnel is left alone`() {
        assertFalse(dnsDispatchWanted("r1", "r1", tunnelLive = true, nowUnix = 1_000, lastRedispatchUnix = 0, redispatchSecs = 30))
    }

    @Test
    fun `a pinned filter whose tunnel is down is restarted, paced`() {
        assertTrue(dnsDispatchWanted("r1", "r1", tunnelLive = false, nowUnix = 1_000, lastRedispatchUnix = 0, redispatchSecs = 30))
        assertFalse(dnsDispatchWanted("r1", "r1", tunnelLive = false, nowUnix = 1_010, lastRedispatchUnix = 1_000, redispatchSecs = 30))
        assertTrue(dnsDispatchWanted("r1", "r1", tunnelLive = false, nowUnix = 1_030, lastRedispatchUnix = 1_000, redispatchSecs = 30))
    }
}
