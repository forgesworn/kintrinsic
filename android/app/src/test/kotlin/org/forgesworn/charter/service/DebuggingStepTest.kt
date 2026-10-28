package org.forgesworn.charter.service

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The per-tick debugging step's decision: blocked unless the clause allows,
 * held (nothing written) only on a store read error, and cleared on a device
 * known to be unpaired so a release leaves no restriction behind.
 */
class DebuggingStepTest {

    private fun state(paired: Boolean, unreadable: Boolean = false) =
        CharterCore.PairingState(paired, null, null, pairingUnreadable = unreadable)

    @Test
    fun theSentinelsMatchTheNativeCore() {
        // `DEBUGGING_BLOCKED` / `DEBUGGING_HOLD` in android/jni/src/lib.rs.
        assertEquals(-1L, CharterCore.DEBUGGING_BLOCKED)
        assertEquals(-2L, CharterCore.DEBUGGING_HOLD)
    }

    @Test
    fun aPairedWardFollowsTheClause() {
        assertEquals(true, debuggingWrite(state(true), 0L))
        assertEquals(true, debuggingWrite(state(true), 1_800_000_000L))
        assertEquals(false, debuggingWrite(state(true), CharterCore.DEBUGGING_BLOCKED))
    }

    @Test
    fun anUnreadableStoreWritesNothing() {
        assertNull(debuggingWrite(state(true), CharterCore.DEBUGGING_HOLD))
    }

    @Test
    fun anyOtherNegativeAnswerBlocks() {
        assertEquals(false, debuggingWrite(state(true), -3L))
        assertEquals(false, debuggingWrite(state(true), Long.MIN_VALUE))
    }

    @Test
    fun noPairingAnswerOrAnUnreadableOneIsHeldAsPairedAndBlocks() {
        // A failed init, a refused `.so`, an error reply: paired, and the
        // native answer there is the blocked default.
        assertTrue(debuggingPaired(null))
        assertEquals(false, debuggingWrite(null, CharterCore.DEBUGGING_BLOCKED))
        assertTrue(debuggingPaired(state(true, unreadable = true)))
        assertTrue(debuggingPaired(state(false, unreadable = true)))
        assertEquals(false, debuggingWrite(state(false, unreadable = true), CharterCore.DEBUGGING_BLOCKED))
    }

    @Test
    fun aKnownUnpairedDeviceIsClearedWhateverTheCoreSays() {
        assertFalse(debuggingPaired(state(false)))
        assertEquals(true, debuggingWrite(state(false), CharterCore.DEBUGGING_BLOCKED))
        assertEquals(true, debuggingWrite(state(false), CharterCore.DEBUGGING_HOLD))
    }
}
