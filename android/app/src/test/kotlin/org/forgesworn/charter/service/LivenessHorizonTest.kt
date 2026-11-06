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
}
