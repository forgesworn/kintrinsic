package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * F1: a per-app input that could not be read keeps the last good answer and
 * runs the tick degraded (suspend-only, hidden set untouched); it never becomes
 * "no policy" or "no suspensions".
 */
class AppGateMemoryTest {

    private val blocklist = CharterCore.AppPolicy(
        posture = "blocklist",
        blocked = listOf("app.block"),
        allowed = emptyList(),
        hidden = listOf("app.bloat"),
    )
    private fun fail(): Result<Nothing> = Result.failure(CharterCore.Unreadable("store EIO"))

    @Test
    fun healthyReadsPassStraightThrough() {
        val m = AppGateMemory()
        val i = m.resolve(Result.success(blocklist), Result.success(listOf("r")), Result.success(listOf("b")))
        assertEquals(blocklist, i.appPolicy)
        assertEquals(setOf("r"), i.ruleSuspensions)
        assertEquals(setOf("b"), i.bucketSuspensions)
        assertFalse(i.degraded)
        assertTrue(i.appPolicyFresh)
    }

    @Test
    fun aFailedReadKeepsTheLastGoodAnswer() {
        val m = AppGateMemory()
        m.resolve(Result.success(blocklist), Result.success(listOf("r")), Result.success(listOf("b")))
        val i = m.resolve(fail(), fail(), fail())
        assertEquals("the blocklist stays", blocklist, i.appPolicy)
        assertEquals(setOf("r"), i.ruleSuspensions)
        assertEquals(setOf("b"), i.bucketSuspensions)
        assertTrue("suspend-only", i.degraded)
        assertFalse("hidden set left alone", i.appPolicyFresh)
    }

    @Test
    fun oneFailedInputDegradesTheWholeTick() {
        val m = AppGateMemory()
        m.resolve(Result.success(blocklist), Result.success(listOf("r")), Result.success(emptyList()))
        val i = m.resolve(Result.success(blocklist), fail(), Result.success(emptyList()))
        assertTrue(i.degraded)
        assertTrue(i.appPolicyFresh)
        assertEquals(setOf("r"), i.ruleSuspensions)
    }

    @Test
    fun beforeAnyGoodReadAFailureLiftsNothing() {
        val m = AppGateMemory()
        val i = m.resolve(fail(), fail(), fail())
        // Nothing to add, and degraded so nothing is taken away either: the
        // device stays exactly as the platform kept it.
        assertNull(i.appPolicy)
        assertTrue(i.ruleSuspensions.isEmpty() && i.bucketSuspensions.isEmpty())
        assertTrue(i.degraded)
        assertFalse(i.appPolicyFresh)
    }

    @Test
    fun aGoodNoneIsRememberedAsNone() {
        val m = AppGateMemory()
        m.resolve(Result.success(blocklist), Result.success(emptyList()), Result.success(emptyList()))
        m.resolve(Result.success(null), Result.success(emptyList()), Result.success(emptyList()))
        val i = m.resolve(fail(), Result.success(emptyList()), Result.success(emptyList()))
        assertNull("the guardian really lifted it", i.appPolicy)
    }

    @Test
    fun onlyTheEdgesAreReported() {
        val m = AppGateMemory()
        val ok = { m.resolve(Result.success(null), Result.success(emptyList()), Result.success(emptyList())) }
        ok(); assertFalse(m.transitioned)
        m.resolve(fail(), fail(), fail()); assertTrue(m.transitioned)
        m.resolve(fail(), fail(), fail()); assertFalse(m.transitioned)
        ok(); assertTrue(m.transitioned)
        ok(); assertFalse(m.transitioned)
    }
}
