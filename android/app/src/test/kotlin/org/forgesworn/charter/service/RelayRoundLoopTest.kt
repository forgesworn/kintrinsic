package org.forgesworn.charter.service

import org.junit.Assert.*
import org.junit.Test

class RelayRoundLoopTest {
    private class Harness {
        var now = 1000L
        var lit = true
        var running = true
        var rounds = 0
        val policy = RelaySchedule()
        val queue = mutableMapOf<Runnable, Long>()
        var duringWork: (() -> Unit)? = null
        val loop = RelayRoundLoop(
            { now }, { running }, { lit }, { policy.interval(lit, now) },
            { r, delay -> queue[r] = now + delay }, { queue.remove(it); Unit },
            { rounds++; duringWork?.invoke(); now += 1000 },
        )
        fun next() {
            val entry = queue.minBy { it.value }
            now = maxOf(now, entry.value); queue.remove(entry.key); entry.key.run()
        }
    }
    @Test fun `many opens coalesce and refresh during work uses completed round`() {
        val h = Harness()
        repeat(20) { h.loop.refresh(true) }
        assertEquals(1, h.queue.size)
        h.next(); h.next()
        assertEquals(1, h.rounds); assertEquals(1, h.queue.size)
        h.duringWork = { repeat(20) { h.loop.refresh(true) } }
        h.next() // regular round; refresh queued while in flight
        h.next() // refresh observes completed round, arms one future round
        assertEquals(2, h.rounds); assertEquals(1, h.queue.size)
        assertEquals(h.now + 60000, h.queue.values.single())
    }
    @Test fun `screen transitions and settings preserve one chain and screen-off priority`() {
        val h = Harness()
        h.policy.submitted("ask", h.now)
        h.loop.refresh(true); h.next(); h.next()
        assertEquals(h.now + 5000, h.queue.values.single())
        h.lit = false; h.loop.refresh(false); h.next()
        assertEquals(h.now + 300000, h.queue.values.single())
        h.now += 130000; h.lit = true; h.loop.refresh(true); h.next(); h.next()
        assertEquals(h.now + 60000, h.queue.values.single())
        h.policy.settings = BatterySettings(600,120,false,false,60)
        h.loop.refresh(false); h.next()
        assertEquals(h.now + 120000, h.queue.values.single())
        assertEquals(1, h.queue.size)
    }
    @Test fun `wake upgrades an already queued screen-off reschedule to prompt`() {
        val h = Harness()
        h.lit = false; h.loop.refresh(false)
        h.lit = true; h.loop.refresh(true)
        h.next()
        assertEquals(h.now, h.queue.values.single())
        h.next(); assertEquals(1, h.rounds)
    }
    @Test fun `recently completed work satisfies repeated GUI refreshes`() {
        val h = Harness()
        h.loop.refresh(true); h.next(); h.next()
        h.now += 2000
        h.loop.refresh(true); h.next()
        assertEquals(1, h.rounds)
        assertEquals(h.now + 60000, h.queue.values.single())
    }
    @Test fun `watchdog respects selected schedule and re-arms a single chain`() {
        val h = Harness()
        h.lit = false
        h.policy.settings = BatterySettings(screenOffSecs = 600)
        h.loop.refresh(false); h.next()
        assertFalse(LivenessPolicy.pollDue(301000, 1000, false, 600000, 600000))
        assertTrue(LivenessPolicy.pollDue(601000, 1000, false, 600000, 600000))
        h.loop.overdueRound()
        assertEquals(1, h.rounds); assertEquals(1, h.queue.size)
        assertEquals(h.now + 600000, h.queue.values.single())
        h.running = false; h.next()
        assertEquals(1, h.rounds); assertTrue(h.queue.isEmpty())
    }
}
