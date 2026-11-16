package org.forgesworn.charter.service

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** When the shade comes back in front of an app opened from it. */
class ShadeRaiseTest {
    private fun wanted(
        screenOff: Boolean = false,
        secure: Boolean = true,
        keyguardLocked: Boolean = true,
        coveredForMs: Long? = RAISE_GRACE_MS,
        inCall: Boolean = false,
    ) = shadeRaiseWanted(screenOff, secure, keyguardLocked, coveredForMs, inCall)

    @Test fun screen_off_raises_on_a_secure_phone_whatever_is_in_front() {
        assertTrue(wanted(screenOff = true, keyguardLocked = false, coveredForMs = null))
    }

    @Test fun a_phone_with_no_secure_lock_is_never_raised() {
        assertFalse(wanted(screenOff = true, secure = false))
        assertFalse(wanted(secure = false))
    }

    @Test fun never_over_a_call() {
        assertFalse(wanted(screenOff = true, inCall = true))
        assertFalse(wanted(inCall = true))
    }

    @Test fun the_tick_raises_once_the_keyguard_locks_over_an_app_the_shade_opened() {
        assertTrue(wanted())
    }

    @Test fun the_tick_leaves_an_unlocked_app_alone() {
        assertFalse(wanted(keyguardLocked = false))
    }

    @Test fun the_tick_leaves_alone_what_the_shade_did_not_open() {
        // The emergency dialer from the bouncer, or the shade itself in front.
        assertFalse(wanted(coveredForMs = null))
    }

    @Test fun the_tick_waits_out_an_app_still_opening() {
        assertFalse(wanted(coveredForMs = RAISE_GRACE_MS - 1))
        assertFalse(wanted(coveredForMs = 0))
    }
}
