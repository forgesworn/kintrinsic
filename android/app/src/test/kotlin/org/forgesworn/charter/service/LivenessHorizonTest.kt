package org.forgesworn.charter.service

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The next-boundary search: pure, so the world is a lambda. */
class LivenessHorizonTest {
    private fun first(changeAt: Long?, calls: MutableList<Long>? = null) =
        LivenessHorizon.firstChangeSecs(300, 60, 2) { t ->
            calls?.add(t)
            changeAt != null && t >= changeAt
        }

    @Test
    fun `nothing changing within the horizon gives no boundary`() {
        assertNull(first(null))
        assertNull(first(301))
    }

    @Test
    fun `a change is found to within the resolution and never reported early`() {
        for (c in listOf(1L, 59L, 60L, 61L, 137L, 299L, 300L)) {
            val got = first(c)!!
            assertTrue("change at $c, got $got", got >= c)
            assertTrue("change at $c, got $got", got - c <= 2)
        }
    }

    @Test
    fun `a boundary just inside the horizon is found`() {
        assertEquals(300L, first(300))
    }

    @Test
    fun `the search stays small`() {
        val calls = mutableListOf<Long>()
        first(137, calls)
        assertTrue(calls.size <= 12)
    }

    @Test
    fun `a listening grace ending soon is a boundary one second after it runs out`() {
        assertEquals(31L, LivenessHorizon.earliest(null, null, null, listeningSecs = 30))
        // The earliest of everything wins, including the probe.
        assertEquals(11L, LivenessHorizon.earliest(probe = 90, scheduleSecs = null, debuggingSecs = null, listeningSecs = 10))
        assertEquals(31L, LivenessHorizon.earliest(probe = 90, scheduleSecs = 200, debuggingSecs = null, listeningSecs = 30))
    }

    @Test
    fun `a grace beyond the horizon, or none, adds nothing`() {
        assertNull(LivenessHorizon.earliest(null, null, null, listeningSecs = 301))
        assertNull(LivenessHorizon.earliest(null, null, null, listeningSecs = null))
        assertNull(LivenessHorizon.earliest(null, null, null, listeningSecs = -1))
    }

    @Test
    fun `the schedule allowance is a boundary but an unbounded one is not`() {
        assertEquals(121L, LivenessHorizon.earliest(null, scheduleSecs = 120, debuggingSecs = null, listeningSecs = null))
        assertNull(LivenessHorizon.earliest(null, scheduleSecs = -1, debuggingSecs = null, listeningSecs = null))
        assertNull(LivenessHorizon.earliest(null, scheduleSecs = 4_000, debuggingSecs = null, listeningSecs = null))
    }

    @Test
    fun `a low budget alone adds no boundary`() {
        // The budget is not an input at all: with the schedule unbounded (-1)
        // and nothing else due, a ward with 2 minutes of budget left and the
        // screen dark has no boundary, so the alarm is not woken for it.
        val tl = org.forgesworn.charter.native.CharterCore.parseTimeLeft(
            """{"known":true,"locked":false,"reason":"none","effectiveSecs":120,"scheduleSecs":-1,"budgetSecs":120,"extensionSecs":0}""",
        )
        assertEquals(-1L, tl.scheduleSecs)
        assertNull(LivenessHorizon.earliest(null, tl.scheduleSecs, null, null))
    }

    @Test
    fun `an extension running out while the schedule is closed is read from the schedule allowance`() {
        val tl = org.forgesworn.charter.native.CharterCore.parseTimeLeft(
            """{"known":true,"locked":false,"reason":"none","effectiveSecs":90,"scheduleSecs":90,"budgetSecs":-1,"extensionSecs":90}""",
        )
        assertEquals(91L, LivenessHorizon.earliest(null, tl.scheduleSecs, null, null))
    }

    @Test
    fun `an always-available entry expiring is a change the search finds`() {
        fun snap(open: List<String>) = LivenessHorizon.Snapshot(
            known = true, locked = true, reason = "schedule", activeTime = null, appPolicy = null,
            appRules = emptyList(), buckets = emptyList(), tethering = "blocked",
            maintenanceOpen = false, alwaysOpen = open,
        )
        val got = LivenessHorizon.firstChangeSecs(300, 60, 2) { t ->
            if (t < 100) snap(listOf("com.example.calls")) else snap(emptyList())
        }!!
        assertTrue("got $got", got in 100..102)
    }
}
