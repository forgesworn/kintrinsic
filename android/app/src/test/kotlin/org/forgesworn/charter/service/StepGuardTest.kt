package org.forgesworn.charter.service

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 05-B1: a step of the tick that throws must never stop the steps after it,
 * above all the lock surface, which comes last.
 */
class StepGuardTest {
    private val logs = mutableListOf<String>()
    private val guard = StepGuard { message, _ -> logs += message }

    @After fun clearFaults() {
        FaultInjection.failing = emptySet()
    }

    @Test
    fun `a throwing step does not stop the lock step after it`() {
        val ran = mutableListOf<String>()
        guard.run("baseline") { throw SecurityException("not device owner yet") }
        guard.run("appGate") { throw IllegalStateException("dead binder") }
        guard.run("tether") { throw IllegalArgumentException("OEM refuses key") }
        guard.run("hotspot") { throw IllegalStateException("fgs start not allowed") }
        val lockRan = guard.run("lock") { ran += "lock" }
        assertTrue(lockRan)
        assertEquals(listOf("lock"), ran)
        assertEquals(setOf("baseline", "appGate", "tether", "hotspot"), guard.failing)
    }

    @Test
    fun `a failing step reports false and never propagates`() {
        assertFalse(guard.run("x") { throw Error("even an Error") })
    }

    @Test
    fun `a persistent failure is logged once per spell, and its recovery once`() {
        repeat(5) { guard.run("tether") { throw IllegalStateException("again") } }
        assertEquals(1, logs.size)
        guard.run("tether") { }
        guard.run("tether") { }
        assertEquals(2, logs.size)
        assertTrue(logs[1].contains("recovered"))
        assertTrue(guard.failing.isEmpty())
    }

    @Test
    fun `one ward failing and another succeeding does not flap the log`() {
        repeat(10) {
            guard.run("appGate", "ward-a") { throw IllegalStateException("a fails") }
            guard.run("appGate", "ward-b") { }
        }
        assertEquals(1, logs.size)
        assertEquals(setOf("appGate@ward-a"), guard.failing)
        guard.run("appGate", "ward-a") { }
        assertEquals(2, logs.size)
        assertTrue(logs[1].contains("recovered"))
    }

    @Test
    fun `injected faults fail the named steps only`() {
        FaultInjection.failing = setOf("appGate")
        assertFalse(guard.run("appGate") { })
        assertTrue(guard.run("lock") { })
    }

    @Test
    fun `an injected all fails every step, and clearing it recovers`() {
        FaultInjection.failing = setOf("all")
        assertFalse(guard.run("baseline") { })
        assertFalse(guard.run("lock") { })
        FaultInjection.failing = emptySet()
        assertTrue(guard.run("lock") { })
    }
}
