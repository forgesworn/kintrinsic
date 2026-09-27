package org.forgesworn.charter.native

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * F3: the native core is checked against the ABI this build expects before
 * anything else is called into it. A mismatch, or no core at all, is an init
 * failure and every later call is refused on the Kotlin side.
 */
class AbiCheckTest {

    @Test
    fun theExpectedAbiIsAccepted() {
        assertNull(CharterCore.abiRefusal(Result.success(CharterCore.EXPECTED_ABI_VERSION)))
    }

    @Test
    fun anyOtherAbiIsRefused() {
        for (v in listOf(0, 1, CharterCore.EXPECTED_ABI_VERSION + 1, -1)) {
            assertNotNull("abi $v", CharterCore.abiRefusal(Result.success(v)))
        }
    }

    @Test
    fun aCoreThatWillNotLoadIsRefused() {
        assertNotNull(CharterCore.abiRefusal(Result.failure(UnsatisfiedLinkError("no charter_jni"))))
    }

    /** On the host there is no `.so`, so this is the refused path end to end:
     *  init reports a failure instead of throwing, and no JNI follows it. */
    @Test
    fun aRefusedCoreFailsInitAndIsNeverCalledAgain() {
        val r = CharterCore.init("/nonexistent", "enforce", 1L, "test")
        assertNotNull(r.error)
        assertFalse(r.paired)
        assertEquals("enforce", r.enforceMode)
        val e = assertThrows(CharterCore.AbiMismatch::class.java) {
            CharterCore.appRuleSuspensions(0L)
        }
        assertTrue(e.message!!.isNotBlank())
        assertThrows(CharterCore.AbiMismatch::class.java) { CharterCore.pairingState() }
    }
}
