package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

/** A lock shade never headlines "Unlocked" (emulator round, 2026-09-28). */
class LockShadeCopyTest {
    private fun info(locked: Boolean, reason: String, title: String) =
        CharterCore.LockInfo(locked, reason, title, "", "up to 2h 00m a day — 8m used today", false)

    @Test fun a_core_lock_keeps_the_cores_copy() {
        val core = info(true, "schedule", "Outside allowed hours")
        assertSame(core, shadeInfo(core, standIn = false))
        assertSame(core, shadeInfo(core, standIn = true))
    }

    @Test fun a_stand_in_lock_over_an_unlocked_core_is_not_headlined_unlocked() {
        val shade = shadeInfo(info(false, "none", "Unlocked"), standIn = true)!!
        assertTrue(shade.locked)
        assertEquals("Locked for now", shade.title)
        assertTrue(shade.comeBack.isNotEmpty())
        // The used line and the reason the ask routes on are the core's.
        assertEquals("none", shade.reason)
        assertEquals("up to 2h 00m a day — 8m used today", shade.usedLine)
    }

    @Test fun a_shade_the_core_has_just_released_still_reads_locked_until_it_goes() {
        val shade = shadeInfo(info(false, "none", "Unlocked"), standIn = false)!!
        assertTrue(shade.locked)
        assertEquals("Locked", shade.title)
    }

    @Test fun an_unreadable_core_keeps_the_shades_own_fallback() {
        assertNull(shadeInfo(null, standIn = true))
    }
}
