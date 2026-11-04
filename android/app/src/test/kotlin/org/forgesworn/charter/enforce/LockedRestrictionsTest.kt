package org.forgesworn.charter.enforce

import android.os.UserManager
import org.forgesworn.charter.native.CharterCore.Mode
import org.forgesworn.charter.service.lockedRestrictionsWanted
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The restrictions that close Settings while the ward is locked, and only then.
 * Each pass compares against the platform's real state, so a refused write is
 * retried and never remembered as done.
 */
class LockedRestrictionsTest {

    private val locked = LOCKED_RESTRICTIONS

    @Test
    fun `the locked set is the three that were agreed`() {
        assertEquals(
            setOf(
                UserManager.DISALLOW_MODIFY_ACCOUNTS,
                UserManager.DISALLOW_APPS_CONTROL,
                UserManager.DISALLOW_CONFIG_DEFAULT_APPS,
            ),
            locked.toSet(),
        )
    }

    @Test
    fun `they hold only while locked in Enforce`() {
        assertTrue(lockedRestrictionsWanted(Mode.ENFORCE, locked = true, degraded = false))
        assertFalse(lockedRestrictionsWanted(Mode.ENFORCE, locked = false, degraded = false))
        for (mode in listOf(Mode.OBSERVE, Mode.FREEZE_ONLY)) {
            assertFalse(lockedRestrictionsWanted(mode, locked = true, degraded = false))
            assertFalse(lockedRestrictionsWanted(mode, locked = true, degraded = true))
        }
    }

    /** The stand-in lock for a failing app gate puts up the same shade and the
     *  same row, so it needs the same closure. */
    @Test
    fun `the stand-in lock is closed too`() {
        assertTrue(lockedRestrictionsWanted(Mode.ENFORCE, locked = false, degraded = true))
    }

    @Test
    fun `they are never part of the always-on baseline`() {
        assertTrue(locked.none { it in BASELINE_RESTRICTIONS })
    }

    @Test
    fun `un-chartering clears them, and everything else it always cleared`() {
        assertTrue(RESTRICTIONS_CLEARED_ON_RELEASE.containsAll(locked))
        assertTrue(RESTRICTIONS_CLEARED_ON_RELEASE.containsAll(BASELINE_RESTRICTIONS))
        assertTrue(
            RESTRICTIONS_CLEARED_ON_RELEASE.containsAll(
                listOf(
                    UserManager.DISALLOW_WIFI_TETHERING,
                    UserManager.DISALLOW_ADD_WIFI_CONFIG,
                    UserManager.DISALLOW_CONFIG_BLUETOOTH,
                    UserManager.DISALLOW_DEBUGGING_FEATURES,
                ),
            ),
        )
    }

    @Test
    fun `locking sets them all and unlocking lifts them all`() {
        val on = mutableMapOf<String, Boolean>()
        pushRestrictions(lockedRestrictionTargets(true), { false }) { k, v -> on[k] = v }
        assertEquals(locked.associateWith { true }, on)

        on.clear()
        pushRestrictions(lockedRestrictionTargets(false), { true }) { k, v -> on[k] = v }
        assertEquals(locked.associateWith { false }, on)
    }

    @Test
    fun `a settled device makes no writes`() {
        var writes = 0
        pushRestrictions(lockedRestrictionTargets(true), { true }) { _, _ -> writes++ }
        pushRestrictions(lockedRestrictionTargets(false), { false }) { _, _ -> writes++ }
        assertEquals(0, writes)
    }

    @Test
    fun `an unreadable state writes them all, never assumes they are right`() {
        val writes = mutableListOf<String>()
        pushRestrictions(lockedRestrictionTargets(true), null) { k, _ -> writes += k }
        assertEquals(locked, writes)
    }

    @Test
    fun `a refused key is reported, does not stop the rest, and is retried next pass`() {
        val state = mutableSetOf<String>()
        val refused = UserManager.DISALLOW_APPS_CONTROL
        var refuse = true
        val write = { key: String, on: Boolean ->
            if (refuse && key == refused) throw SecurityException("no")
            if (on) state += key else state -= key
            Unit
        }

        val first = pushRestrictions(lockedRestrictionTargets(true), { it in state }, write)
        assertEquals(listOf(refused), first.failed)
        assertEquals(locked.toSet() - refused, state)
        assertFalse(lockedRestrictionsInForce { it in state })

        // Nothing was cached as pushed: the next pass tries exactly that key.
        refuse = false
        val second = pushRestrictions(lockedRestrictionTargets(true), { it in state }, write)
        assertEquals(emptyList<String>(), second.failed)
        assertEquals(listOf(refused), second.reasserted)
        assertTrue(lockedRestrictionsInForce { it in state })
    }

    @Test
    fun `the row is offered only when every one is really in force`() {
        assertTrue(lockedRestrictionsInForce { it in locked })
        for (missing in locked) {
            assertFalse(lockedRestrictionsInForce { it in locked && it != missing })
        }
        assertFalse(lockedRestrictionsInForce { false })
    }

    @Test
    fun `an unreadable state keeps the row hidden`() {
        assertFalse(lockedRestrictionsInForce(null as ((String) -> Boolean)?))
    }
}
