package org.forgesworn.mycharter.relay

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class RelayRoutesTest {
    private val r = "a".repeat(64)
    private val w = "b".repeat(64)
    private val one = "wss://one.example"
    private val two = "wss://two.example"

    /** One key: one socket per relay, as before the key list. */
    @Test fun oneKeyIsOneSocketPerRelay() {
        assertEquals(
            listOf(RelayRoute(one, r), RelayRoute(two, r)),
            RelayRoutes.wanted(listOf(one, two), listOf(r)),
        )
    }

    /** D5: a root and its working key each get their own socket on every
     *  relay, so each signs in as itself. */
    @Test fun everyKeyGetsItsOwnSocketOnEveryRelay() {
        assertEquals(
            listOf(RelayRoute(one, r), RelayRoute(one, w), RelayRoute(two, r), RelayRoute(two, w)),
            RelayRoutes.wanted(listOf(one, two, one), listOf(r, w, r)),
        )
    }

    @Test fun aDroppedKeyClosesItsSocketsOnly() {
        val open = RelayRoutes.wanted(listOf(one, two), listOf(r, w))
        assertEquals(
            listOf(RelayRoute(one, w), RelayRoute(two, w)),
            RelayRoutes.stale(open, listOf(one, two), listOf(r)),
        )
    }

    @Test fun aDroppedRelayClosesItsSocketsOnly() {
        val open = RelayRoutes.wanted(listOf(one, two), listOf(r, w))
        assertEquals(
            listOf(RelayRoute(two, r), RelayRoute(two, w)),
            RelayRoutes.stale(open, listOf(one), listOf(r, w)),
        )
    }

    @Test fun anAddedKeyClosesNothing() {
        val open = RelayRoutes.wanted(listOf(one), listOf(r))
        assertTrue(RelayRoutes.stale(open, listOf(one), listOf(r, w)).isEmpty())
    }
}
