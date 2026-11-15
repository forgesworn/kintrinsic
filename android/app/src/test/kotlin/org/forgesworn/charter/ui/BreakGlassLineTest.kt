package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * What the shade says after the emergency unlock's hold: "switched off" only
 * when the family switched it off, and "your guardian has been told" only when
 * the core journaled the press for them (review 9, LO-1 and LO-7).
 */
class BreakGlassLineTest {

    private val told = "Your guardian has been told you need them. You can call the numbers above now."
    private val notTold = "Your guardian may not have been told. Call them if you can."
    private val switchedOff = "Emergency unlock isn't switched on for this phone."
    private val tryAgain = "Couldn't use the emergency unlock. Try again."

    private fun lineFor(raw: String) = breakGlassLine(CharterCore.parseBreakGlass(raw))

    @Test
    fun `the calls scope says the guardian was told only when the press was journaled`() {
        assertEquals(told, lineFor("""{"allowed":true,"scope":"calls","secsLeft":600,"journaled":true}"""))
        assertEquals(notTold, lineFor("""{"allowed":true,"scope":"calls","secsLeft":600,"journaled":false}"""))
    }

    @Test
    fun `a core that does not say whether it journaled is taken as having done so`() {
        val res = CharterCore.parseBreakGlass("""{"allowed":true,"scope":"calls","secsLeft":600}""")
        assertTrue(res.journaled)
        assertEquals(told, breakGlassLine(res))
    }

    @Test
    fun `the full scope unlocks, journaled or not`() {
        assertEquals("Unlocked.", lineFor("""{"allowed":true,"scope":"full","secsLeft":600,"journaled":true}"""))
        assertEquals("Unlocked.", lineFor("""{"allowed":true,"scope":"full","secsLeft":600,"journaled":false}"""))
    }

    @Test
    fun `only not-enabled says it is switched off`() {
        assertEquals(switchedOff, lineFor("""{"allowed":false,"reason":"not-enabled"}"""))
    }

    @Test
    fun `an error, an empty answer, an unparseable one or any other refusal is a fault to retry`() {
        for (raw in listOf(
            """{"error":"not initialized"}""",
            """{"error":"internal error (panic recovered)"}""",
            "",
            "not json",
            "{}",
            """{"allowed":false}""",
            """{"allowed":false,"reason":"something-new"}""",
        )) {
            assertEquals(raw, tryAgain, lineFor(raw))
        }
        assertEquals(tryAgain, breakGlassLine(null))
    }

    @Test
    fun `an error key is read as an error, never as allowed`() {
        val res = CharterCore.parseBreakGlass("""{"error":"not initialized","allowed":true}""")
        assertFalse(res.allowed)
        assertEquals("error", res.reason)
    }

    @Test
    fun `a parsed answer keeps its fields`() {
        val res = CharterCore.parseBreakGlass("""{"allowed":true,"scope":"calls","secsLeft":900,"journaled":false}""")
        assertEquals(CharterCore.BreakGlassResult(true, "calls", 900, "", journaled = false), res)
    }
}
