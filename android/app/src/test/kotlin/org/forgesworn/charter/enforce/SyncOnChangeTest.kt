package org.forgesworn.charter.enforce

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The level-triggered compare-then-push idiom, pinned in isolation from any
 * DPM call. Covers the fix-round-1 finding (2026-08-04): a failed push must
 * NOT be cached as if it landed, or the next tick's identical target list
 * looks unchanged and the retry never happens.
 */
class SyncOnChangeTest {

    @Test
    fun `unchanged input never calls push`() {
        var calls = 0
        val out = syncOnChange(listOf("a", "b"), listOf("a", "b")) { calls++; true }
        assertEquals(0, calls)
        assertEquals(listOf("a", "b"), out)
    }

    @Test
    fun `a changed input that pushes successfully is cached as the new value`() {
        val out = syncOnChange(listOf("a", "b"), listOf("a")) { true }
        assertEquals(listOf("a", "b"), out)
    }

    @Test
    fun `a failed push leaves the cache at the last-pushed value, not the failed target`() {
        val out = syncOnChange(listOf("a", "b"), listOf("a")) { false }
        assertEquals(listOf("a"), out)
    }

    @Test
    fun `a failed push is retried on the next call with the same target`() {
        var succeed = false
        var calls = 0
        var cache: List<String>? = listOf("a")
        // First call: push fails, cache must NOT advance to the failed target.
        cache = syncOnChange(listOf("a", "b"), cache) { calls++; succeed }
        assertEquals(listOf("a"), cache)
        assertEquals(1, calls)
        // Second call with the SAME target: because the cache never advanced,
        // this must push again rather than treating it as already-applied.
        succeed = true
        cache = syncOnChange(listOf("a", "b"), cache) { calls++; succeed }
        assertEquals(2, calls)
        assertEquals(listOf("a", "b"), cache)
    }

    @Test
    fun `first run with no prior cache pushes and records on success`() {
        val out = syncOnChange(listOf("a"), null) { true }
        assertEquals(listOf("a"), out)
    }

    @Test
    fun `first run with no prior cache stays null on failure`() {
        val out = syncOnChange(listOf("a"), null) { false }
        assertFalse(out != null)
        assertTrue(out == null)
    }
}
