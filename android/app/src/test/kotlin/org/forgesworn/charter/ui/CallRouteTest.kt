package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.CallRoute.Route
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CallRouteTest {

    private val bools = listOf(true, false)

    // ---- the whole table: (locked, ED ok, dial ok) for an emergency number -----

    @Test
    fun `every cell of the emergency table, in order`() {
        data class Cell(val locked: Boolean, val ed: Boolean, val dial: Boolean, val expected: List<Route>)
        val cells = listOf(
            // keyguard locked: ED, then the phone's own lock, then ACTION_CALL. The
            // system dialler is never tried behind a locked keyguard.
            Cell(true, true, true, listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER, Route.CALL)),
            Cell(true, true, false, listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER, Route.CALL)),
            Cell(true, false, true, listOf(Route.BOUNCER, Route.CALL)),
            Cell(true, false, false, listOf(Route.BOUNCER, Route.CALL)),
            // unlocked: ED, then ACTION_DIAL when it resolves, then ACTION_CALL.
            Cell(false, true, true, listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL, Route.CALL)),
            Cell(false, true, false, listOf(Route.EMERGENCY_DIALLER, Route.CALL)),
            Cell(false, false, true, listOf(Route.SYSTEM_DIAL, Route.CALL)),
            Cell(false, false, false, listOf(Route.CALL)),
        )
        // The table covers all eight combinations exactly once.
        assertEquals(8, cells.map { Triple(it.locked, it.ed, it.dial) }.toSet().size)
        for (c in cells) {
            assertEquals(
                "locked=${c.locked} ed=${c.ed} dial=${c.dial}",
                c.expected,
                CallRoute.sequence(true, c.locked, c.ed, c.dial),
            )
        }
    }

    @Test
    fun `choose walks the sequence by attempt and ends in null, every cell`() {
        for (locked in bools) {
            for (ed in bools) {
                for (dial in bools) {
                    val seq = CallRoute.sequence(true, locked, ed, dial)
                    for ((i, route) in seq.withIndex()) {
                        assertEquals(route, CallRoute.choose(true, locked, ed, dial, i))
                    }
                    assertNull(CallRoute.choose(true, locked, ed, dial, seq.size))
                    assertNull(CallRoute.choose(true, locked, ed, dial, seq.size + 3))
                    // ACTION_CALL is always the last resort, and appears once.
                    assertEquals(Route.CALL, seq.last())
                    assertEquals(1, seq.count { it == Route.CALL })
                }
            }
        }
    }

    @Test
    fun `an emergency number is the emergency dialler first whenever one is usable, locked or not`() {
        for (locked in bools) {
            for (dial in bools) {
                assertEquals(Route.EMERGENCY_DIALLER, CallRoute.choose(true, locked, true, dial, 0))
            }
        }
    }

    @Test
    fun `the phone's own lock is offered only while it is showing, and ACTION_DIAL only while it is not`() {
        for (ed in bools) {
            for (dial in bools) {
                assertTrue(Route.BOUNCER in CallRoute.sequence(true, true, ed, dial))
                assertFalse(Route.SYSTEM_DIAL in CallRoute.sequence(true, true, ed, dial))
                assertFalse(Route.BOUNCER in CallRoute.sequence(true, false, ed, dial))
            }
        }
    }

    @Test
    fun `a guardian's number is ACTION_CALL and nothing else, in every cell`() {
        for (locked in bools) {
            for (ed in bools) {
                for (dial in bools) {
                    assertEquals(listOf(Route.CALL), CallRoute.sequence(false, locked, ed, dial))
                    assertEquals(Route.CALL, CallRoute.choose(false, locked, ed, dial, 0))
                    assertNull(CallRoute.choose(false, locked, ed, dial, 1))
                }
            }
        }
    }

    // ---- which resolved emergency dialler LockTask will let through ------------

    private fun usable(
        pkg: String? = "com.android.phone",
        cls: String? = "com.android.phone.EmergencyDialer",
        dialers: List<String?> = listOf("com.google.android.dialer", null),
        allowed: Boolean = false,
    ) = CallRoute.emergencyDiallerUsable(
        resolvedPackage = pkg,
        resolvedClass = cls,
        platformPackage = "com.android.phone",
        platformClass = "com.android.phone.EmergencyDialer",
        dialerPackages = dialers,
        lockTaskPermitted = allowed,
    )

    @Test
    fun `the platform's own emergency dialler component is usable`() {
        assertTrue(usable())
    }

    @Test
    fun `an OEM component in an unlisted package is not, so the next route runs`() {
        assertFalse(usable(pkg = "com.samsung.android.emergency", cls = "com.samsung.android.emergency.EmergencyDialer"))
    }

    @Test
    fun `the same package but another class is not the platform component`() {
        assertFalse(usable(cls = "com.android.phone.SomethingElse"))
    }

    @Test
    fun `a package that is the system or default dialler is usable`() {
        assertTrue(usable(pkg = "com.google.android.dialer", cls = "x.Y"))
        assertTrue(usable(pkg = "com.oem.dialer", cls = "x.Y", dialers = listOf(null, "com.oem.dialer")))
    }

    @Test
    fun `a package on the lock task allowlist is usable`() {
        assertTrue(usable(pkg = "com.oem.emergency", cls = "x.Y", allowed = true))
    }

    @Test
    fun `a chooser, a store app and an unresolved component are never usable`() {
        // The chooser: package `android`, not allowlisted.
        assertFalse(usable(pkg = "android", cls = "com.android.internal.app.ResolverActivity"))
        assertFalse(usable(pkg = null, cls = null, allowed = true))
        assertFalse(usable(pkg = "", cls = "x.Y", allowed = true))
        assertFalse(usable(pkg = "com.store.app", cls = ""))
    }

    @Test
    fun `no dialler packages known and no allowlist means only the platform component passes`() {
        assertTrue(usable(dialers = emptyList()))
        assertFalse(usable(pkg = "com.x", cls = "y", dialers = listOf(null, null)))
    }
}
