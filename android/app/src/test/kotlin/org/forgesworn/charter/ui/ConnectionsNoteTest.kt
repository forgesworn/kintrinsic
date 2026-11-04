package org.forgesworn.charter.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ConnectionsNoteTest {

    @Test
    fun `nothing limited says nothing`() {
        assertNull(Connections.limitNote("any", "any"))
        assertNull(Connections.limitNote(null, null))
    }

    @Test
    fun `each limit is said plainly`() {
        assertEquals(
            "Wi-Fi: only networks with a password\nBluetooth: devices already paired",
            Connections.limitNote("secured", "paired"),
        )
        assertEquals(
            "Wi-Fi: networks already saved on this phone",
            Connections.limitNote("saved", "any"),
        )
    }
}
