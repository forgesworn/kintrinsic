package org.forgesworn.charter.ui

import org.junit.Assert.assertEquals
import org.junit.Test

/** The 10-/1-minute warnings name the time and what follows (§Times). */
class WarnCopyTest {

    @Test
    fun `a ward that uses no times hears exactly the words it always did`() {
        assertEquals("10 minutes left", WarnCopy.text("ten", null, null))
        assertEquals("1 minute left — save your game", WarnCopy.text("one", null, null))
    }

    @Test
    fun `a named time is named and what follows is said`() {
        assertEquals(
            "10 minutes of Learning time left — then Free time",
            WarnCopy.text("ten", "Learning time", "Free time"),
        )
        assertEquals(
            "10 minutes of Learning time left — then locked",
            WarnCopy.text("ten", "Learning time", "locked"),
        )
        assertEquals(
            "1 minute of Learning time left — then locked. Save your game.",
            WarnCopy.text("one", "Learning time", "locked"),
        )
    }

    @Test
    fun `free time running into a named time says so`() {
        assertEquals("10 minutes left — then Learning time", WarnCopy.text("ten", null, "Learning time"))
    }
}
