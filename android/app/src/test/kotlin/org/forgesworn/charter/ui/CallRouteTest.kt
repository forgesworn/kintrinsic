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

    private val platform = "com.android.phone" to "com.android.phone.EmergencyDialer"

    private fun pick(
        vararg candidates: Pair<String, String>,
        dialers: List<String?> = listOf("com.google.android.dialer"),
        allowed: Set<String> = emptySet(),
    ) = CallRoute.pickEmergencyDialler(
        candidates = candidates.toList(),
        platformPackage = CallRoute.PLATFORM_EMERGENCY_DIALLER_PACKAGE,
        platformClass = CallRoute.PLATFORM_EMERGENCY_DIALLER_CLASS,
        dialerPackages = dialers,
        lockTaskPermitted = { it in allowed },
    )

    @Test
    fun `the platform component is the one named by the constants`() {
        assertEquals("com.android.phone", CallRoute.PLATFORM_EMERGENCY_DIALLER_PACKAGE)
        assertEquals("com.android.phone.EmergencyDialer", CallRoute.PLATFORM_EMERGENCY_DIALLER_CLASS)
    }

    @Test
    fun `one platform candidate is picked`() {
        assertEquals(platform, pick(platform))
    }

    @Test
    fun `the platform component wins over another system candidate, in either order`() {
        val oem = "com.google.android.dialer" to "com.google.android.dialer.Emergency"
        assertEquals(platform, pick(oem, platform))
        assertEquals(platform, pick(platform, oem))
    }

    @Test
    fun `with two system packages claiming the action and no platform component, the first usable one is picked`() {
        val chooser = "android" to "com.android.internal.app.ResolverActivity"
        val dialler = "com.google.android.dialer" to "x.Emergency"
        assertEquals(dialler, pick(chooser, dialler))
        assertNull("a chooser alone never qualifies", pick(chooser))
    }

    @Test
    fun `an unlisted candidate is skipped unless the lock task allowlist has it`() {
        val oem = "com.oem.emergency" to "com.oem.emergency.Main"
        assertNull(pick(oem))
        assertEquals(oem, pick(oem, allowed = setOf("com.oem.emergency")))
    }

    @Test
    fun `no candidates picks nothing`() {
        assertNull(pick())
    }

    // ---- the check after a launch, and the status line ---------------------------

    @Test
    fun `the check falls through only from a clear shade, and a covered one is opened unless asleep or ringing`() {
        for (screenOn in bools) {
            for (ringing in bools) {
                // Clear includes the screen being on, but the verdict holds whatever it is handed.
                assertEquals(CallRoute.Check.NEXT_ROUTE, CallRoute.check(true, screenOn, ringing))
            }
        }
        assertEquals(CallRoute.Check.OPENED, CallRoute.check(clear = false, screenOn = true, ringing = false))
        assertEquals(CallRoute.Check.FAILED, CallRoute.check(clear = false, screenOn = false, ringing = false))
        assertEquals(CallRoute.Check.FAILED, CallRoute.check(clear = false, screenOn = true, ringing = true))
        assertEquals(CallRoute.Check.FAILED, CallRoute.check(clear = false, screenOn = false, ringing = true))
    }

    @Test
    fun `an emergency press keeps the advice whatever it settles on, and only a failure with the shade clear says so`() {
        for (outcome in CallSequencer.Outcome.values()) {
            for (clear in bools) {
                val expected = if (outcome == CallSequencer.Outcome.FAILED && clear) {
                    CallRoute.Line.NOTHING_OPENED
                } else {
                    CallRoute.Line.ADVICE
                }
                assertEquals("$outcome clear=$clear", expected, CallRoute.settledLine(true, outcome, clear))
            }
        }
    }

    @Test
    fun `a guardian's number says nothing unless its call could not be started`() {
        for (clear in bools) {
            assertEquals(CallRoute.Line.BLANK, CallRoute.settledLine(false, CallSequencer.Outcome.OPENED, clear))
            assertEquals(CallRoute.Line.BLANK, CallRoute.settledLine(false, CallSequencer.Outcome.CANCELLED, clear))
            assertEquals(CallRoute.Line.CALL_FAILED, CallRoute.settledLine(false, CallSequencer.Outcome.FAILED, clear))
        }
    }

    @Test
    fun `the status lines, verbatim, and none claims a call started`() {
        assertEquals("", CallRoute.Line.BLANK.text)
        assertEquals(
            "If you still need help, press the power button five times for Emergency SOS, or use another phone.",
            CallRoute.Line.ADVICE.text,
        )
        assertEquals("Tap Emergency call on the next screen", CallRoute.Line.BOUNCER.text)
        assertEquals(
            "Couldn't open the emergency dialler. Press the power button five times for Emergency SOS, " +
                "or use another phone.",
            CallRoute.Line.NOTHING_OPENED.text,
        )
        assertEquals("Couldn't start the call — tell your guardian.", CallRoute.Line.CALL_FAILED.text)
        for (line in CallRoute.Line.values()) {
            val t = line.text.lowercase()
            assertFalse(line.name, "calling" in t || "call started" in t || "connected" in t || "opened" in t)
        }
    }
}
