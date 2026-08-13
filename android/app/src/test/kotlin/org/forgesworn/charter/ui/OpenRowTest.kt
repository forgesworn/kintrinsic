package org.forgesworn.charter.ui

import org.forgesworn.charter.enforce.openRowEntries
import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenRowTest {

    private val inventory = listOf(
        "com.book" to "Voice",
        "com.pods" to "AntennaPod",
        "com.game" to "Some Game",
    )

    /**
     * A family that never sets this clause must see a shade byte-identical to
     * the one they see today. No empty row, no header, nothing.
     */
    @Test
    fun `nothing named means no row at all`() {
        assertTrue(openRowEntries(emptySet(), inventory).isEmpty())
    }

    @Test
    fun `named apps render with their friendly labels, sorted`() {
        val out = openRowEntries(setOf("com.book", "com.pods"), inventory)
        assertEquals(listOf("com.pods" to "AntennaPod", "com.book" to "Voice"), out)
    }

    /**
     * The clause can name an app that has since been uninstalled. Offering a
     * button that opens nothing is worse than offering no button.
     */
    @Test
    fun `an app missing from the inventory is not offered`() {
        assertTrue(openRowEntries(setOf("com.gone"), inventory).isEmpty())
    }

    // ── review finding I3 (2026-08-04): the row must never offer a button
    // appSuspendSet is refusing to open behind it — a named app that is
    // blocklisted, outside an allowlist, or whose own bucket is spent must
    // each be dropped, exactly like AppSuspendSetTest pins the enforcement
    // side of the same union. ────────────────────────────────────────────

    @Test
    fun `a named app the guardian has since blocklisted is not offered`() {
        val policy = CharterCore.AppPolicy(posture = "blocklist", blocked = listOf("com.book"), allowed = emptyList())
        val out = openRowEntries(setOf("com.book"), inventory, appPolicy = policy)
        assertTrue(out.isEmpty())
    }

    @Test
    fun `a named app left off an active allowlist is not offered`() {
        val policy = CharterCore.AppPolicy(posture = "allowlist", allowed = listOf("com.school"), blocked = emptyList())
        val out = openRowEntries(setOf("com.book"), inventory, appPolicy = policy)
        assertTrue(out.isEmpty())
    }

    @Test
    fun `a named app ON an active allowlist is still offered`() {
        val policy = CharterCore.AppPolicy(posture = "allowlist", allowed = listOf("com.book"), blocked = emptyList())
        val out = openRowEntries(setOf("com.book"), inventory, appPolicy = policy)
        assertEquals(listOf("com.book" to "Voice"), out)
    }

    @Test
    fun `a named app whose own bucket is spent is not offered`() {
        val out = openRowEntries(setOf("com.book"), inventory, bucketSuspensions = setOf("com.book"))
        assertTrue(out.isEmpty())
    }

    @Test
    fun `a named app the schedule-driven per-app rule blocks right now is not offered`() {
        val out = openRowEntries(setOf("com.book"), inventory, ruleSuspensions = setOf("com.book"))
        assertTrue(out.isEmpty())
    }

    @Test
    fun `an unrelated named app is unaffected by another app's suspension`() {
        val policy = CharterCore.AppPolicy(posture = "blocklist", blocked = listOf("com.game"), allowed = emptyList())
        val out = openRowEntries(setOf("com.book", "com.game"), inventory, appPolicy = policy)
        assertEquals(listOf("com.book" to "Voice"), out)
        assertFalse(out.any { it.first == "com.game" })
    }

    @Test
    fun `no policy or suspensions at all behaves exactly like the original signature`() {
        val out = openRowEntries(setOf("com.book", "com.pods"), inventory)
        assertEquals(listOf("com.pods" to "AntennaPod", "com.book" to "Voice"), out)
    }
}
