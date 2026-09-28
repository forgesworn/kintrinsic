package org.forgesworn.charter.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The shade is re-delivered when the lock's reason changes under it. */
class LockRelaunchTest {
    @Test fun a_live_shade_for_the_same_reason_is_left_alone() {
        assertFalse(lockLaunchWanted(shown = true, alive = true, shownReason = "schedule", reason = "schedule"))
    }

    @Test fun a_stand_in_lock_becoming_a_schedule_lock_is_redelivered() {
        assertTrue(lockLaunchWanted(true, true, DegradedAppGate.DEGRADED_REASON, "schedule"))
        assertTrue(lockLaunchWanted(true, true, "budget", "schedule"))
    }

    @Test fun a_dead_or_never_launched_shade_is_launched() {
        assertTrue(lockLaunchWanted(shown = true, alive = false, shownReason = "schedule", reason = "schedule"))
        assertTrue(lockLaunchWanted(shown = false, alive = true, shownReason = null, reason = "schedule"))
    }
}
