package org.forgesworn.mycharter.service
import org.junit.Assert.*
import org.junit.Test

class ReconnectPolicyTest {
    @Test fun `outage doubles up to selected cap and stable socket resets`() {
        for (cap in listOf(60L, 300L, 600L)) {
            var previous = 0L
            repeat(20) { previous = ReconnectPolicy.next(previous, 0, cap) }
            assertEquals(cap, previous)
            assertEquals(1L, ReconnectPolicy.next(previous, 60000, cap))
        }
        assertEquals(300L, ReconnectPolicy.cap(-1))
    }
    @Test fun `service restarts respect queued retries and connecting sockets`() {
        assertFalse(ReconnectPolicy.shouldConnect(false, true))
        assertFalse(ReconnectPolicy.shouldConnect(true, false))
        assertTrue(ReconnectPolicy.shouldConnect(false, false))
    }
}
