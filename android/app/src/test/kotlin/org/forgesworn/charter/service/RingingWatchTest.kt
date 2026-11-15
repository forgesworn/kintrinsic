package org.forgesworn.charter.service

import android.telephony.TelephonyManager.CALL_STATE_IDLE
import android.telephony.TelephonyManager.CALL_STATE_OFFHOOK
import android.telephony.TelephonyManager.CALL_STATE_RINGING
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicBoolean

/** A call that rings during a lockout gets its in-call screen, once per ring. */
class RingingWatchTest {

    @Test
    fun `the ringing edge during a lockout brings the in-call screen up`() {
        assertTrue(inCallScreenWanted(CALL_STATE_IDLE, CALL_STATE_RINGING, lockedOut = true))
        // A second call waiting while one is up rings too.
        assertTrue(inCallScreenWanted(CALL_STATE_OFFHOOK, CALL_STATE_RINGING, lockedOut = true))
    }

    @Test
    fun `nothing registers after a stop`() {
        val r = RegisterUntilStopped()
        var registers = 0
        var unregisters = 0
        r.stop { unregisters++ }
        r.ensure { registers++; true }
        assertEquals(0, registers)
        assertEquals(0, unregisters)
        val r2 = RegisterUntilStopped()
        r2.ensure { registers++; true }
        r2.ensure { registers++; true }
        assertEquals("once", 1, registers)
        r2.stop { unregisters++ }
        r2.stop { unregisters++ }
        r2.ensure { registers++; true }
        assertEquals(1, registers)
        assertEquals(1, unregisters)
        // A registration that did not take is tried again.
        val r3 = RegisterUntilStopped()
        var tries = 0
        r3.ensure { tries++; false }
        r3.ensure { tries++; throw SecurityException("no READ_PHONE_STATE") }
        r3.ensure { tries++; true }
        assertEquals(3, tries)
    }

    @Test
    fun `a stop during a registration waits for it and undoes it`() {
        val r = RegisterUntilStopped()
        val inRegister = CountDownLatch(1)
        val release = CountDownLatch(1)
        val unregistered = AtomicBoolean(false)
        val ticker = Thread { r.ensure { inRegister.countDown(); release.await(); true } }
        ticker.start()
        assertTrue(inRegister.await(5, TimeUnit.SECONDS))
        val main = Thread { r.stop { unregistered.set(true) } }
        main.start()
        Thread.sleep(100)
        assertFalse("stop waits for the registration under way", unregistered.get())
        release.countDown()
        ticker.join(5000)
        main.join(5000)
        assertTrue("and then undoes it", unregistered.get())
        assertTrue(r.stopped)
    }

    @Test
    fun `nothing outside a lockout, and nothing but the edge`() {
        assertFalse(inCallScreenWanted(CALL_STATE_IDLE, CALL_STATE_RINGING, lockedOut = false))
        assertFalse(inCallScreenWanted(CALL_STATE_RINGING, CALL_STATE_RINGING, lockedOut = true))
        assertFalse(inCallScreenWanted(CALL_STATE_RINGING, CALL_STATE_OFFHOOK, lockedOut = true))
        assertFalse(inCallScreenWanted(CALL_STATE_RINGING, CALL_STATE_IDLE, lockedOut = true))
    }
}
