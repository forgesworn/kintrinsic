package org.forgesworn.charter

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.forgesworn.charter.native.CharterNative
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class CharterNativeTest {
    @Test
    fun rustCoreLoadsAndAnswers() {
        assertEquals(1, CharterNative.charterAbiVersion())
    }
}
