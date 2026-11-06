package org.forgesworn.charter.service

import org.forgesworn.charter.service.CharterService.Companion.DARK_SLOW_TICK_MS
import org.forgesworn.charter.service.CharterService.Companion.DARK_TICK_MS
import org.forgesworn.charter.service.CharterService.Companion.SLOW_TICK_MS
import org.forgesworn.charter.service.CharterService.Companion.TICK_MS
import org.forgesworn.charter.service.LivenessPolicy.ALARM_MS
import org.forgesworn.charter.service.LivenessPolicy.DARK_KICK_GAP_MS
import org.forgesworn.charter.service.LivenessPolicy.WEDGED_MS
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 05-B2/05-G1: the liveness alarm's pacing. Times are elapsedRealtime, which
 * keeps counting through deep sleep, so a Doze stretch shows up as a gap.
 */
class LivenessPolicyTest {
    private val t0 = 1_000_000L

    @Test
    fun `a tick that never ran is due`() {
        assertTrue(LivenessPolicy.tickDue(t0, 0L, interactive = false, tickMs = TICK_MS))
        assertTrue(LivenessPolicy.pollDue(t0, 0L, false, SLOW_TICK_MS, DARK_SLOW_TICK_MS))
    }

    @Test
    fun `in the dark every alarm ticks, unless a tick has only just run`() {
        assertTrue(LivenessPolicy.tickDue(t0 + ALARM_MS, t0, interactive = false, tickMs = TICK_MS))
        assertFalse(LivenessPolicy.tickDue(t0 + 5_000, t0, interactive = false, tickMs = TICK_MS))
    }

    @Test
    fun `a Doze stretch of hours is due at the first alarm`() {
        assertTrue(LivenessPolicy.tickDue(t0 + 3 * 3_600_000L, t0, interactive = false, tickMs = TICK_MS))
        assertTrue(LivenessPolicy.pollDue(t0 + 3 * 3_600_000L, t0, false, SLOW_TICK_MS, DARK_SLOW_TICK_MS))
    }

    @Test
    fun `while lit the alarm is only a watchdog on three missed beats`() {
        assertFalse(LivenessPolicy.tickDue(t0 + 2 * TICK_MS, t0, interactive = true, tickMs = TICK_MS))
        assertTrue(LivenessPolicy.tickDue(t0 + 3 * TICK_MS, t0, interactive = true, tickMs = TICK_MS))
        assertFalse(LivenessPolicy.pollDue(t0 + 2 * SLOW_TICK_MS, t0, true, SLOW_TICK_MS, DARK_SLOW_TICK_MS))
        assertTrue(LivenessPolicy.pollDue(t0 + 3 * SLOW_TICK_MS, t0, true, SLOW_TICK_MS, DARK_SLOW_TICK_MS))
    }

    @Test
    fun `the dark poll keeps its two-minute pace`() {
        assertFalse(LivenessPolicy.pollDue(t0 + ALARM_MS, t0, false, SLOW_TICK_MS, DARK_SLOW_TICK_MS))
        assertTrue(LivenessPolicy.pollDue(t0 + DARK_SLOW_TICK_MS, t0, false, SLOW_TICK_MS, DARK_SLOW_TICK_MS))
    }

    @Test
    fun `a tick is wedged only after the watchdog limit, and none running is never wedged`() {
        assertFalse(LivenessPolicy.wedged(t0 + WEDGED_MS, 0L))
        assertFalse(LivenessPolicy.wedged(t0 + WEDGED_MS - 1, t0))
        assertTrue(LivenessPolicy.wedged(t0 + WEDGED_MS, t0))
    }

    @Test
    fun `a hung relay poll is detected on its own, longer limit`() {
        assertFalse(LivenessPolicy.pollWedged(t0 + LivenessPolicy.POLL_WEDGED_MS, 0L))
        // A slow but healthy poll (several relays, 16 s timeouts) is not hung.
        assertFalse(LivenessPolicy.pollWedged(t0 + 3 * 60_000L, t0))
        assertFalse(LivenessPolicy.pollWedged(t0 + LivenessPolicy.POLL_WEDGED_MS - 1, t0))
        assertTrue(LivenessPolicy.pollWedged(t0 + LivenessPolicy.POLL_WEDGED_MS, t0))
        assertTrue(LivenessPolicy.POLL_WEDGED_MS >= WEDGED_MS)
    }

    @Test
    fun `the cadences keep the core's per-tick accrual clamp out of reach`() {
        // The core clamps a single tick's accrual at 300 s; the alarm and the
        // dark handler pace must both stay well under it.
        assertTrue(ALARM_MS < 300_000L / 2)
        assertTrue(DARK_TICK_MS < 300_000L / 2)
        assertTrue(DARK_KICK_GAP_MS < ALARM_MS)
    }

    private val five = LivenessPolicy.LONGER_BATTERY_ALARM_MS

    @Test
    fun `longer battery with the screen off waits five minutes when no boundary is near`() {
        assertEquals(five, LivenessPolicy.nextAlarmDelayMs(true, false, t0, null))
        assertEquals(five, LivenessPolicy.nextAlarmDelayMs(true, false, t0, t0 + 3_600_000L))
        assertEquals(five, LivenessPolicy.nextAlarmDelayMs(true, false, t0, t0 + five))
    }

    @Test
    fun `a boundary sooner than five minutes pulls the alarm forward to it`() {
        assertEquals(90_000L, LivenessPolicy.nextAlarmDelayMs(true, false, t0, t0 + 90_000L))
        assertEquals(1_000L, LivenessPolicy.nextAlarmDelayMs(true, false, t0, t0 + 1_000L))
    }

    @Test
    fun `every other mode, and a lit screen, keep the minute alarm`() {
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(false, false, t0, null))
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(false, false, t0, t0 + 10_000L))
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(true, true, t0, null))
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(true, true, t0, t0 + 10_000L))
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(false, true, t0, null))
    }

    @Test
    fun `a boundary that has already passed is stale and keeps the minute alarm`() {
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(true, false, t0, t0))
        assertEquals(ALARM_MS, LivenessPolicy.nextAlarmDelayMs(true, false, t0, t0 - 5_000L))
    }

    @Test
    fun `the stretched alarm is never later than five minutes`() {
        for (b in listOf(null, t0 - 1, t0, t0 + 1, t0 + five, Long.MAX_VALUE)) {
            assertTrue(LivenessPolicy.nextAlarmDelayMs(true, false, t0, b) <= five)
        }
    }
}
