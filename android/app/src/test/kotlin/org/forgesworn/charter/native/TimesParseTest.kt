package org.forgesworn.charter.native

import org.forgesworn.charter.enforce.browserPackageOrNull
import org.forgesworn.charter.ui.HomeCopy
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The §Times fields crossing the JNI boundary. Every one is additive and
 * optional: a core that never sends them parses to the exact pre-times shape.
 */
class TimesParseTest {

    @Test
    fun `a warning carries the time and what follows, and a legacy one carries neither`() {
        val d = CharterCore.parseDecisions(
            """[{"subject":"s","locked":false,"reason":"none","effects":[
                 {"kind":"warn","level":"ten","time":"Learning time","then":"Free time"}],
                "remainingSecs":540,"configured":true,"enforceMode":"enforce",
                "activeTime":"learning","activeLabel":"Learning time"}]""",
        )
        val warn = d[0].effects.single() as CharterCore.Effect.Warn
        assertEquals("Learning time", warn.time)
        assertEquals("Free time", warn.then)
        assertEquals("learning", d[0].activeTime)
        assertEquals("Learning time", d[0].activeLabel)

        val legacy = CharterCore.parseDecisions(
            """[{"subject":"s","locked":false,"reason":"none","effects":[{"kind":"warn","level":"one"}],
                "remainingSecs":50,"configured":true,"enforceMode":"enforce"}]""",
        )
        val w = legacy[0].effects.single() as CharterCore.Effect.Warn
        assertNull(w.time)
        assertNull(w.then)
        assertNull(legacy[0].activeTime)
    }

    @Test
    fun `the lock offers a type only when the child has a named time`() {
        val typed = CharterCore.parseLockInfo(
            """{"locked":true,"reason":"schedule","title":"Locked until Learning time at 09:00",
                "comeBack":"","usedLine":"","dormant":false,
                "askTimes":[{"id":"free","label":"Free time"},{"id":"learning","label":"Learning time"}],
                "defaultAskTime":"learning"}""",
        )
        assertEquals("Locked until Learning time at 09:00", typed.title)
        assertEquals(listOf("free", "learning"), typed.askTimes.map { it.id })
        assertEquals("learning", typed.defaultAskTime)

        val legacy = CharterCore.parseLockInfo(
            """{"locked":true,"reason":"schedule","title":"Outside allowed hours","comeBack":"","usedLine":"","dormant":false}""",
        )
        assertTrue(legacy.askTimes.isEmpty())
        assertNull(legacy.defaultAskTime)
    }

    @Test
    fun `the home view names the time until when and what is allowed`() {
        val v = CharterCore.parseScheduleView(
            """{"locked":false,"minutesLeft":30,"secondsLeft":1800,"detail":null,"lines":[],
                "activeTime":"learning","activeLabel":"Learning time",
                "timeLine":"Learning time until 18:00","allowed":["Khan Academy","YouTube"]}""",
        )!!
        assertEquals(
            listOf("Learning time until 18:00", "Allowed: Khan Academy, YouTube"),
            HomeCopy.timeLines(v),
        )
        // Free time and a ward with no times: nothing added.
        val free = CharterCore.parseScheduleView(
            """{"locked":false,"minutesLeft":30,"secondsLeft":1800,"detail":null,"lines":[]}""",
        )!!
        assertTrue(HomeCopy.timeLines(free).isEmpty())
        assertTrue(HomeCopy.timeLines(null).isEmpty())
    }

    @Test
    fun `time left parses the active time and a legacy one has none`() {
        val t = CharterCore.parseTimeLeft(
            """{"known":true,"locked":false,"reason":"none","effectiveSecs":900,"activeTime":"learning"}""",
        )
        assertEquals("learning", t.activeTime)
        assertNull(
            CharterCore.parseTimeLeft(
                """{"known":true,"locked":false,"reason":"none","effectiveSecs":900}""",
            ).activeTime,
        )
    }

    @Test
    fun `no default browser chosen is unknown, never the platform chooser`() {
        assertEquals("com.browser", browserPackageOrNull("com.browser"))
        assertNull(browserPackageOrNull("android"))
        assertNull(browserPackageOrNull(""))
        assertNull(browserPackageOrNull(null))
    }
}
