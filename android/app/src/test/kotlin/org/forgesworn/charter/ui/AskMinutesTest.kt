package org.forgesworn.charter.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The ward picks how much more time to ask for: 10 min, 20 min, 1 hour, or a typed amount. */
class AskMinutesTest {

    @Test
    fun `the chooser offers the three presets then a custom row`() {
        assertEquals(listOf("10 min", "20 min", "1 hour", "Custom…"), AskMinutes.chooserLabels())
        assertEquals(3, AskMinutes.CUSTOM_INDEX)
    }

    @Test
    fun `labels read like a clock`() {
        assertEquals("5 min", AskMinutes.label(5))
        assertEquals("45 min", AskMinutes.label(45))
        assertEquals("1 hour", AskMinutes.label(60))
        assertEquals("2 hours", AskMinutes.label(120))
        assertEquals("1 h 30 min", AskMinutes.label(90))
    }

    @Test
    fun `the chooser opens on the default, then on the last pick, and on custom for a typed amount`() {
        assertEquals(1, AskMinutes.checkedIndex(null)) // 20 min
        assertEquals(0, AskMinutes.checkedIndex(10))
        assertEquals(2, AskMinutes.checkedIndex(60))
        assertEquals(AskMinutes.CUSTOM_INDEX, AskMinutes.checkedIndex(45))
    }

    @Test
    fun `a typed amount must be a whole number of minutes the wire can carry`() {
        assertEquals(45, AskMinutes.parseCustom("45"))
        assertEquals(45, AskMinutes.parseCustom(" 45 "))
        assertEquals(1, AskMinutes.parseCustom("1"))
        assertEquals(1440, AskMinutes.parseCustom("1440"))
        assertNull(AskMinutes.parseCustom("0"))
        assertNull(AskMinutes.parseCustom("-5"))
        assertNull(AskMinutes.parseCustom("1441"))
        assertNull(AskMinutes.parseCustom("1.5"))
        assertNull(AskMinutes.parseCustom("ten"))
        assertNull(AskMinutes.parseCustom(""))
        assertNull(AskMinutes.parseCustom(null))
    }

    @Test
    fun `the amount rides the request as minutesRequested`() {
        assertEquals(
            """{"minutesRequested":60,"reason":"","limitHit":"budget"}""",
            AskType.params(60, "budget", null),
        )
    }
}
