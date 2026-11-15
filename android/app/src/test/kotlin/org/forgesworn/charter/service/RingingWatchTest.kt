package org.forgesworn.charter.service

import android.telephony.TelephonyManager.CALL_STATE_IDLE
import android.telephony.TelephonyManager.CALL_STATE_OFFHOOK
import android.telephony.TelephonyManager.CALL_STATE_RINGING
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** A call that rings during a lockout gets its in-call screen, once per ring. */
class RingingWatchTest {

    @Test
    fun `the ringing edge during a lockout brings the in-call screen up`() {
        assertTrue(inCallScreenWanted(CALL_STATE_IDLE, CALL_STATE_RINGING, lockedOut = true))
        // A second call waiting while one is up rings too.
        assertTrue(inCallScreenWanted(CALL_STATE_OFFHOOK, CALL_STATE_RINGING, lockedOut = true))
    }

    @Test
    fun `nothing outside a lockout, and nothing but the edge`() {
        assertFalse(inCallScreenWanted(CALL_STATE_IDLE, CALL_STATE_RINGING, lockedOut = false))
        assertFalse(inCallScreenWanted(CALL_STATE_RINGING, CALL_STATE_RINGING, lockedOut = true))
        assertFalse(inCallScreenWanted(CALL_STATE_RINGING, CALL_STATE_OFFHOOK, lockedOut = true))
        assertFalse(inCallScreenWanted(CALL_STATE_RINGING, CALL_STATE_IDLE, lockedOut = true))
    }
}
