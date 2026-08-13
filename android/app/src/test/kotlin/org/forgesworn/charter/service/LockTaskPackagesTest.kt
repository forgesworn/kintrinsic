package org.forgesworn.charter.service

import org.forgesworn.charter.enforce.lockTaskAllowlist
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LockTaskPackagesTest {

    @Test
    fun `the phone stack is always allowed, so a lifeline call has a face`() {
        val out = lockTaskAllowlist("org.forgesworn.charter", listOf("com.android.dialer"), emptySet())
        assertEquals(listOf("org.forgesworn.charter", "com.android.dialer"), out)
    }

    @Test
    fun `always-available apps join the allowlist`() {
        val out = lockTaskAllowlist("org.forgesworn.charter", emptyList(), setOf("com.book"))
        assertTrue(out.contains("com.book"))
    }

    @Test
    fun `a duplicate dialer is not listed twice`() {
        val out = lockTaskAllowlist(
            "org.forgesworn.charter",
            listOf("com.android.dialer", "com.android.dialer"),
            setOf("com.android.dialer"),
        )
        assertEquals(1, out.count { it == "com.android.dialer" })
    }

    /** Order-stable, so the level-triggered caller's equality check does not
     *  churn a DPM call every tick over set iteration order. */
    @Test
    fun `the same inputs give a byte-identical list`() {
        val a = lockTaskAllowlist("self", listOf("d"), setOf("com.b", "com.a"))
        val b = lockTaskAllowlist("self", listOf("d"), setOf("com.a", "com.b"))
        assertEquals(a, b)
    }
}
