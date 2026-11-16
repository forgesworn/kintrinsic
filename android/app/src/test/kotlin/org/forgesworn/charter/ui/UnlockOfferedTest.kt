package org.forgesworn.charter.ui

import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The shade's "Unlock phone": only while the phone's own lock is up. */
class UnlockOfferedTest {
    @Test
    fun `offered only with a screen lock that is locked`() {
        assertTrue(unlockOffered(secure = true, keyguardLocked = true))
        assertFalse(unlockOffered(secure = true, keyguardLocked = false))
        assertFalse(unlockOffered(secure = false, keyguardLocked = true))
        assertFalse(unlockOffered(secure = false, keyguardLocked = false))
    }
}
