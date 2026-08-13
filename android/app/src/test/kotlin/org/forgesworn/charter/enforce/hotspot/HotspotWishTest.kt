package org.forgesworn.charter.enforce.hotspot

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The ward's guest-hotspot switch + the idle auto-off policy. */
class HotspotWishTest {

    // A process-wide object: leave it as the app starts, or one test's wish
    // leaks into the next.
    @After
    fun tearDown() = HotspotWish.reset()

    @Test
    fun defaultsOff() {
        assertFalse("a fresh process must never want an AP up", HotspotWish.on)
        assertNull(HotspotWish.switchedOffReason)
    }

    @Test
    fun wardTurnsItOnAndOff() {
        HotspotWish.turnOn()
        assertTrue(HotspotWish.on)
        // The ward's own off carries no explanation — they know why.
        HotspotWish.turnOff()
        assertFalse(HotspotWish.on)
        assertNull(HotspotWish.switchedOffReason)
    }

    @Test
    fun anAutomaticOffKeepsItsReasonUntilTheNextOn() {
        HotspotWish.turnOn()
        HotspotWish.turnOff(HotspotIdle.REASON)
        assertFalse(HotspotWish.on)
        assertEquals(HotspotIdle.REASON, HotspotWish.switchedOffReason)

        // Turning it back on clears the stale explanation.
        HotspotWish.turnOn()
        assertNull(HotspotWish.switchedOffReason)
    }

    @Test
    fun resetForgetsTheWish() {
        // The clause going away must not leave a wish that would resurrect the
        // AP the instant a guardian re-granted it.
        HotspotWish.turnOn()
        HotspotWish.reset()
        assertFalse(HotspotWish.on)
        assertNull(HotspotWish.switchedOffReason)
    }

    @Test
    fun idleWithNoGuestsSwitchesOffOnlyAfterTheGrace() {
        assertFalse(
            "still inside the grace window",
            HotspotIdle.shouldSwitchOff(liveSocketCount = 0, idleMs = HotspotIdle.GRACE_MS - 1),
        )
        assertTrue(
            "grace elapsed with nobody on it",
            HotspotIdle.shouldSwitchOff(liveSocketCount = 0, idleMs = HotspotIdle.GRACE_MS),
        )
    }

    @Test
    fun aConnectedGuestIsNeverCutOff() {
        // Somebody is on it: idle time is irrelevant, however long the last
        // accept was ago (a long-lived tunnel accepts once and then just pumps).
        assertFalse(
            HotspotIdle.shouldSwitchOff(liveSocketCount = 2, idleMs = HotspotIdle.GRACE_MS * 10),
        )
    }
}
