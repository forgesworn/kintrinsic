package org.forgesworn.charter.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class TimeTextTest {
    @Test fun hoursAndMinutes() {
        assertEquals("2h 15m", TimeText.timeLeft(2 * 3600 + 15 * 60))
        assertEquals("1h 0m", TimeText.timeLeft(3600))
    }

    @Test fun minutesOnlyAtFiveOrMore() {
        assertEquals("37m", TimeText.timeLeft(37 * 60 + 20))
        assertEquals("5m", TimeText.timeLeft(5 * 60))
    }

    @Test fun secondsShownUnderFiveMinutes() {
        assertEquals("4m 59s", TimeText.timeLeft(4 * 60 + 59))
        assertEquals("1m 0s", TimeText.timeLeft(60))
        assertEquals("48s", TimeText.timeLeft(48))
        assertEquals("0s", TimeText.timeLeft(0))
        assertEquals("0s", TimeText.timeLeft(-5))
    }
}
