package org.forgesworn.mycharter.carrier

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class LinkNamesTest {
    @Test fun instanceNameIsKintrinsicAndFourLowercaseHex() {
        assertEquals("Kintrinsic 00ff", LinkNames.instanceName(byteArrayOf(0, -1)))
        assertEquals("Kintrinsic a10b", LinkNames.instanceName(byteArrayOf(0xa1.toByte(), 0x0b, 0x33)))
        assertTrue(LinkNames.isInstanceName(LinkNames.instanceName(byteArrayOf(0x12, 0x34))))
    }

    @Test fun instanceNameValidation() {
        assertTrue(LinkNames.isInstanceName("Kintrinsic 1a2b"))
        assertFalse(LinkNames.isInstanceName("Kintrinsic 1A2B"))
        assertFalse(LinkNames.isInstanceName("Kintrinsic 1a2"))
        assertFalse(LinkNames.isInstanceName("Kintrinsic (2) 1a2b"))
        assertFalse(LinkNames.isInstanceName(""))
    }

    @Test fun hexToKeyBytes() {
        val k = hexToKeyBytes("0f".repeat(32))!!
        assertArrayEquals(ByteArray(32) { 0x0f }, k)
        assertNull(hexToKeyBytes("0f".repeat(31)))
        assertNull(hexToKeyBytes("zz" + "0f".repeat(31)))
    }
}
