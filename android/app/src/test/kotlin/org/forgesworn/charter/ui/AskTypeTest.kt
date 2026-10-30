package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The ward's ask for more time carries a TYPE only when the child has a named time (§Times). */
class AskTypeTest {

    private val free = CharterCore.AskTime("free", "Free time")
    private val learning = CharterCore.AskTime("learning", "Learning time")
    private val reading = CharterCore.AskTime("reading", "Reading time")
    private val offered = listOf(free, learning, reading)

    @Test
    fun `no named time means no type and the untyped ask it always was`() {
        assertNull(AskType.resolve(emptyList(), null, null))
        assertEquals(
            """{"minutesRequested":30,"reason":"","limitHit":"schedule"}""",
            AskType.params(30, "schedule", null),
        )
    }

    @Test
    fun `free time leaves the time field out so the request is byte-identical to before times`() {
        assertEquals(
            """{"minutesRequested":30,"reason":"","limitHit":"budget"}""",
            AskType.params(30, "budget", "free"),
        )
    }

    @Test
    fun `a named time rides the request as its id`() {
        assertEquals(
            """{"minutesRequested":30,"reason":"","limitHit":"schedule","time":"learning"}""",
            AskType.params(30, "schedule", "learning"),
        )
    }

    @Test
    fun `the core default applies until the ward picks, and a pick that is no longer offered falls back`() {
        assertEquals(learning, AskType.resolve(offered, null, "learning"))
        assertEquals(reading, AskType.resolve(offered, "reading", "learning"))
        // A time deleted since the pick: back to the default, never to a ghost.
        assertEquals(learning, AskType.resolve(offered, "gone", "learning"))
        // No default and no pick: Free time.
        assertEquals(free, AskType.resolve(offered, null, null))
        assertEquals(free, AskType.resolve(offered, null, "gone"))
    }

    @Test
    fun `the chooser names the choice`() {
        assertEquals("For: Learning time", AskType.chooserLabel(learning))
    }
}
