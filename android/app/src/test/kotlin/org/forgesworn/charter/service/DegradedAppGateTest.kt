package org.forgesworn.charter.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** An app gate failing for a minute on end is stood in for by the lock. */
class DegradedAppGateTest {
    private val gate = DegradedAppGate(thresholdSecs = 60)

    @Test
    fun `a failure shorter than the threshold does not lock`() {
        assertFalse(gate.record("w", ok = false, nowUnix = 1_000))
        assertFalse(gate.record("w", ok = false, nowUnix = 1_059))
    }

    @Test
    fun `sixty seconds of failures in a row locks, and one success clears it`() {
        gate.record("w", ok = false, nowUnix = 1_000)
        assertTrue(gate.record("w", ok = false, nowUnix = 1_060))
        assertTrue(gate.record("w", ok = false, nowUnix = 1_062))
        assertFalse(gate.record("w", ok = true, nowUnix = 1_064))
        // A new spell starts from scratch.
        assertFalse(gate.record("w", ok = false, nowUnix = 1_066))
    }

    @Test
    fun `a success in between restarts the count`() {
        gate.record("w", ok = false, nowUnix = 1_000)
        gate.record("w", ok = true, nowUnix = 1_030)
        assertFalse(gate.record("w", ok = false, nowUnix = 1_070))
    }

    @Test
    fun `each ward has its own spell`() {
        gate.record("a", ok = false, nowUnix = 1_000)
        assertFalse(gate.record("b", ok = false, nowUnix = 1_060))
        assertTrue(gate.record("a", ok = false, nowUnix = 1_060))
        assertFalse(gate.record("b", ok = true, nowUnix = 1_062))
        assertTrue(gate.record("a", ok = false, nowUnix = 1_062))
    }

    @Test
    fun `a clock that went back restarts the spell rather than locking at once`() {
        gate.record("w", ok = false, nowUnix = 1_000)
        assertFalse(gate.record("w", ok = false, nowUnix = 500))
        assertTrue(gate.record("w", ok = false, nowUnix = 560))
    }
}
