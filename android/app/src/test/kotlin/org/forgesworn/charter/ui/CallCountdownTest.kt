package org.forgesworn.charter.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The 3-2-1 cancel window, held by the activity so a render rebuilds a
 * counting button still counting (review 10, LO-2), and the status line after
 * a cancel (LO-1).
 */
class CallCountdownTest {

    private fun countdown() = CallCountdown("Emergency 999", "emergency services", "999", emergency = true, left = 3)

    @Test
    fun `it counts 3, 2, 1 and the call is placed on the third tick`() {
        val c = countdown()
        assertEquals("Calling emergency services in 3… (tap to cancel)", c.text())
        assertFalse(c.tick())
        assertEquals("Calling emergency services in 2… (tap to cancel)", c.text())
        assertFalse(c.tick())
        assertEquals("Calling emergency services in 1… (tap to cancel)", c.text())
        assertTrue(c.tick())
    }

    @Test
    fun `a button rebuilt mid-countdown shows the countdown where it is, and one with none shows its label`() {
        val c = countdown()
        c.tick()
        assertEquals("Calling emergency services in 2… (tap to cancel)", CallCountdown.buttonText("Emergency 999", c))
        assertEquals("Call Mum", CallCountdown.buttonText("Call Mum", null))
    }

    @Test
    fun `each lifeline button has its own key`() {
        val emergency = CallCountdown.key("Emergency 999", "999", true)
        assertEquals(emergency, countdown().key)
        assertNotEquals(emergency, CallCountdown.key("Call 999", "999", false))
        assertNotEquals(CallCountdown.key("Call Mum", "0700", false), CallCountdown.key("Call Dad", "0700", false))
    }

    @Test
    fun `cancelling never wipes a sticky line, and says so only when there is none`() {
        val advice = CallRoute.Line.ADVICE.text
        assertEquals(advice, CallCountdown.lineAfterCancel(advice))
        assertEquals("Call cancelled.", CallCountdown.lineAfterCancel(""))
    }
}
