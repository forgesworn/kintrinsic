package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.CallRoute.Route
import org.junit.Assert.assertEquals
import org.junit.Test

class CallRouteTest {

    @Test
    fun `an emergency number prefers the emergency dialler, locked or not`() {
        assertEquals(Route.EMERGENCY_DIALLER, CallRoute.choose(true, emergencyDiallerResolves = true, systemDialResolves = true))
        assertEquals(Route.EMERGENCY_DIALLER, CallRoute.choose(true, emergencyDiallerResolves = true, systemDialResolves = false))
    }

    @Test
    fun `without an emergency dialler it falls back to ACTION_DIAL, then ACTION_CALL`() {
        assertEquals(Route.SYSTEM_DIAL, CallRoute.choose(true, emergencyDiallerResolves = false, systemDialResolves = true))
        assertEquals(Route.CALL, CallRoute.choose(true, emergencyDiallerResolves = false, systemDialResolves = false))
    }

    @Test
    fun `a guardian's number is always ACTION_CALL`() {
        for (e in listOf(true, false)) {
            for (s in listOf(true, false)) {
                assertEquals(Route.CALL, CallRoute.choose(false, e, s))
            }
        }
    }
}
