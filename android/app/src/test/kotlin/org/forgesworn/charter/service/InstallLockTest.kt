package org.forgesworn.charter.service

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** F2: a pairing read failure never stands the install lock down. */
class InstallLockTest {

    private fun state(paired: Boolean, unreadable: Boolean = false) =
        CharterCore.PairingState(paired, null, null, pairingUnreadable = unreadable)

    @Test
    fun aPairedDeviceIsLocked() {
        assertTrue(installLockWanted(state(true), maintenanceOpen = false))
    }

    @Test
    fun anUnreadablePairingIsHeldAsPaired() {
        // The core reports an unreadable record as paired.
        assertTrue(installLockWanted(state(true, unreadable = true), maintenanceOpen = false))
    }

    @Test
    fun noPairingAnswerAtAllIsHeldAsPaired() {
        assertTrue(installLockWanted(null, maintenanceOpen = false))
    }

    @Test
    fun onlyAReadableUnpairedOrAnOpenWindowStandsItDown() {
        assertFalse(installLockWanted(state(false), maintenanceOpen = false))
        assertFalse(installLockWanted(state(true), maintenanceOpen = true))
    }
}
