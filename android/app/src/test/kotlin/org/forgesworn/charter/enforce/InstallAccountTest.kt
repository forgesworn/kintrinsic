package org.forgesworn.charter.enforce

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The account of a guardian-opened install window. What makes this worth
 * testing on the JVM rather than only on a phone: the instrumented suite is a
 * hardware gate that runs when someone remembers, and the rule deciding whether
 * a child's app appears in a report to their guardian should not wait on that.
 *
 * (`installWindowJson` is deliberately not covered here — it builds `org.json`,
 * which is an android.jar stub under `isReturnDefaultValues`. It is asserted in
 * the instrumented suite instead.)
 */
class InstallAccountTest {

    private val start = 1_000_000L
    private val end = 2_000_000L

    @Test
    fun `an app that arrived during the window is reported as installed`() {
        assertEquals(
            InstallChangeKind.INSTALLED to 1_500_000L,
            classifyChange(firstInstallMs = 1_500_000L, lastUpdateMs = 1_500_000L, start, end),
        )
    }

    /**
     * A fresh install stamps firstInstallTime and lastUpdateTime with the SAME
     * instant. Reading that as "updated" would tell a guardian a brand-new app
     * was one the child already had — the reassuring answer, and the wrong one.
     */
    @Test
    fun `a brand-new app is never mistaken for an update of one already there`() {
        val (kind, _) = classifyChange(1_500_000L, 1_500_000L, start, end)!!
        assertEquals(InstallChangeKind.INSTALLED, kind)
    }

    @Test
    fun `an app the child already had, updated in the window, is an update`() {
        assertEquals(
            InstallChangeKind.UPDATED to 1_500_000L,
            classifyChange(firstInstallMs = 5_000L, lastUpdateMs = 1_500_000L, start, end),
        )
    }

    /**
     * THE privacy invariant. Everything a child has ever installed sits in the
     * package list; only what moved inside the window the guardian themselves
     * opened may be reported. An off-by-one here turns an account into a feed.
     */
    @Test
    fun `nothing from outside the window is ever reported`() {
        // Installed and last updated well before the window opened.
        assertNull(classifyChange(5_000L, 900_000L, start, end))
        // Installed after it shut.
        assertNull(classifyChange(2_500_000L, 2_500_000L, start, end))
        // Updated after it shut, installed long before.
        assertNull(classifyChange(5_000L, 2_000_001L, start, end))
    }

    @Test
    fun `both bounds are inclusive — the very second counts as through it`() {
        assertEquals(
            InstallChangeKind.INSTALLED to start,
            classifyChange(start, start, start, end),
        )
        assertEquals(
            InstallChangeKind.INSTALLED to end,
            classifyChange(end, end, start, end),
        )
    }

    @Test
    fun `an impossible span reports nothing rather than guessing`() {
        assertNull(classifyChange(1_500_000L, 1_500_000L, end, start))
    }

    @Test
    fun `changes read newest first`() {
        val older = InstallChange("a", "A", InstallChangeKind.INSTALLED, 1_100_000L)
        val newer = InstallChange("b", "B", InstallChangeKind.UPDATED, 1_900_000L)
        assertEquals(listOf(newer, older), sortChanges(listOf(older, newer)))
    }

    @Test
    fun `an empty account is a real answer, not an error`() {
        assertEquals(emptyList<InstallChange>(), sortChanges(emptyList()))
    }
}
