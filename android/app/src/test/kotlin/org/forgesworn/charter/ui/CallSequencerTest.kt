package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.CallRoute.Route
import org.forgesworn.charter.ui.CallSequencer.Launch
import org.forgesworn.charter.ui.CallSequencer.Outcome
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CallSequencerTest {

    /** A launcher that answers each route from a script, and a timer on a fake
     *  clock: a posted task runs only when the test moves the clock past it. */
    private class Rig(
        isEmergency: Boolean = true,
        locked: Boolean,
        ed: Boolean,
        dial: Boolean,
        private val script: MutableMap<Route, Launch> = mutableMapOf(),
    ) {
        val launched = mutableListOf<Route>()
        val announced = mutableListOf<Route>()
        val settled = mutableListOf<Outcome>()
        val pendingResults = mutableMapOf<Route, (Launch) -> Unit>()
        var clock = 0L

        /** Tasks posted and neither run nor cancelled: (task, due time). */
        val active = mutableListOf<Pair<Runnable, Long>>()

        val sequencer = CallSequencer(
            isEmergency = isEmergency,
            locked = locked,
            emergencyDiallerOk = ed,
            systemDialOk = dial,
            launcher = { route, result ->
                launched += route
                val answer = script[route]
                if (answer != null) result(answer) else pendingResults[route] = result
            },
            timer = object : CallSequencer.Timer {
                override fun postDelayed(task: Runnable, delayMs: Long) {
                    active += task to clock + delayMs
                }

                override fun cancel(task: Runnable) {
                    active.removeAll { it.first === task }
                }

                override fun now(): Long = clock
            },
            listener = object : CallSequencer.Listener {
                override fun onRoute(route: Route) {
                    announced += route
                }

                override fun onSettled(outcome: Outcome) {
                    settled += outcome
                }
            },
        )

        /** Move the clock forward, running whatever falls due, in order. */
        fun advance(ms: Long) {
            val target = clock + ms
            while (true) {
                val due = active.filter { it.second <= target }.minByOrNull { it.second } ?: break
                active.remove(due)
                clock = maxOf(clock, due.second)
                due.first.run()
            }
            clock = target
        }

        /** How long until the next thing falls due, or null. */
        fun nextDueIn(): Long? = active.minOfOrNull { it.second }?.minus(clock)
    }

    private fun rig(locked: Boolean, ed: Boolean, dial: Boolean, vararg answers: Pair<Route, Launch>) =
        Rig(locked = locked, ed = ed, dial = dial, script = mutableMapOf(*answers))

    @Test
    fun `an opaque dialler that pauses then stops the shade settles the press, with no fallthrough`() {
        val r = rig(locked = false, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertTrue("not settled until the shade is stopped", r.settled.isEmpty())
        assertEquals(1_500L, r.nextDueIn())
        assertEquals(1_500L, CallSequencer.PAUSE_MS)
        assertEquals(3_000L, CallSequencer.STOP_MS)
        r.advance(200)
        r.sequencer.onPaused()
        assertTrue("a pause alone confirms nothing", r.settled.isEmpty())
        assertEquals("up to 3 s in total from the launch", 2_800L, r.nextDueIn())
        r.advance(100)
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertTrue("the wait is cancelled", r.active.isEmpty())
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
    }

    @Test
    fun `a pause then a resume with no stop is not a confirmation, and the next route runs`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.STARTED,
            Route.SYSTEM_DIAL to Launch.STARTED,
        )
        r.sequencer.begin()
        r.advance(50)
        r.sequencer.onPaused()
        r.advance(50)
        r.sequencer.onResumed() // Telecom's trampoline handed the screen back
        r.advance(2_899)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertTrue(r.settled.isEmpty())
        r.advance(1) // 3 s from the launch
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.settled.isEmpty())
    }

    @Test
    fun `a pause then a resume then the timeout on the last route settles failed with the advice`() {
        val r = rig(locked = false, ed = false, dial = false, Route.CALL to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(listOf(Route.CALL), r.launched)
        r.advance(10)
        r.sequencer.onPaused()
        r.sequencer.onResumed()
        r.advance(3_000)
        assertEquals(listOf(Outcome.FAILED), r.settled)
    }

    @Test
    fun `a launch the shade never pauses for is refused after 1_5 s, and the next route runs`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.STARTED,
            Route.SYSTEM_DIAL to Launch.STARTED,
        )
        r.sequencer.begin()
        r.advance(1_499)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        r.advance(1)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.settled.isEmpty())
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.OPENED), r.settled)
    }

    @Test
    fun `a stop with the screen off confirms nothing, and the sequence carries on`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.STARTED,
            Route.SYSTEM_DIAL to Launch.STARTED,
        )
        r.sequencer.begin()
        r.advance(100)
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = false) // the power button, not a dialler
        assertTrue(r.settled.isEmpty())
        r.advance(2_900)
        assertEquals("the sequence carries on", listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.settled.isEmpty())
    }

    @Test
    fun `a stop with the screen off is never remembered as a stop`() {
        val r = rig(locked = false, ed = true, dial = false, Route.EMERGENCY_DIALLER to Launch.STARTED, Route.CALL to Launch.STARTED)
        r.sequencer.begin()
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = false)
        r.advance(3_000)
        // Nothing took the screen, so ACTION_CALL is still tried.
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.CALL), r.launched)
    }

    @Test
    fun `a late stop after the route was given up on settles opened, and never starts ACTION_CALL`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.STARTED,
            Route.SYSTEM_DIAL to Launch.STARTED,
            Route.CALL to Launch.STARTED,
        )
        r.sequencer.begin()
        r.advance(1_500) // no pause in time: ACTION_DIAL is tried
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        // The emergency dialler turns up late and takes the screen.
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        r.advance(10_000)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
    }

    @Test
    fun `a stop seen while no route was waiting turns the next timer into opened, not ACTION_CALL`() {
        val r = rig(
            locked = true, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.STARTED,
        )
        r.sequencer.begin()
        r.advance(1_500) // the dialler was too slow: the phone's lock is requested
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        r.sequencer.onFocusChanged(false) // the lock screen is up
        r.sequencer.onStopped(screenOn = true) // ... and the dialler's late stop is in
        assertTrue("still waiting on the lock screen's answer", r.settled.isEmpty())
        r.pendingResults.getValue(Route.BOUNCER)(Launch.FAILED)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertEquals("ACTION_CALL is never started after a stop", listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
    }

    @Test
    fun `a cancelled lock screen after a stop settles opened`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.sequencer.onFocusChanged(false)
        r.sequencer.onStopped(screenOn = true)
        r.pendingResults.getValue(Route.BOUNCER)(Launch.CANCELLED)
        assertEquals(listOf(Outcome.OPENED), r.settled)
    }

    @Test
    fun `a route that throws falls through at once, to ACTION_DIAL, then ACTION_CALL`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.FAILED,
            Route.SYSTEM_DIAL to Launch.FAILED,
            Route.CALL to Launch.STARTED,
        )
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL, Route.CALL), r.launched)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL, Route.CALL), r.announced)
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.OPENED), r.settled)
    }

    @Test
    fun `every route failing settles as failed, once, after trying them all`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.FAILED,
            Route.SYSTEM_DIAL to Launch.STARTED,
            Route.CALL to Launch.STARTED,
        )
        r.sequencer.begin()
        r.advance(1_500) // ACTION_DIAL refused silently
        r.advance(1_500) // ACTION_CALL refused silently
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL, Route.CALL), r.launched)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        // Late events change nothing.
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.FAILED), r.settled)
    }

    @Test
    fun `an unlocked phone with no emergency dialler goes straight to ACTION_DIAL`() {
        val r = rig(locked = false, ed = false, dial = true, Route.SYSTEM_DIAL to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(listOf(Route.SYSTEM_DIAL), r.launched)
    }

    @Test
    fun `locked, a refused emergency dialler falls to the phone's own lock, which is announced`() {
        val r = rig(locked = true, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.STARTED)
        r.sequencer.begin()
        r.advance(1_500)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.announced)
        assertTrue("waiting on the ward, not settled", r.settled.isEmpty())
    }

    @Test
    fun `locked with no emergency dialler starts at the phone's own lock, never ACTION_DIAL`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        assertEquals(listOf(Route.BOUNCER), r.launched)
        r.pendingResults.getValue(Route.BOUNCER)(Launch.FAILED)
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
    }

    @Test
    fun `the phone's own lock that never takes the focus is failed after 1_5 s, and ACTION_CALL runs`() {
        val r = rig(locked = true, ed = false, dial = true, Route.CALL to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(1_500L, r.nextDueIn())
        r.advance(1_499)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        r.advance(1)
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
        // The lock screen answering after that is ignored.
        r.pendingResults.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
    }

    @Test
    fun `the phone's own lock that took the focus waits for its answer, however long`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.advance(200)
        r.sequencer.onFocusChanged(false)
        assertTrue("no timer once the lock is showing", r.active.isEmpty())
        r.advance(60_000)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertTrue(r.settled.isEmpty())
        r.pendingResults.getValue(Route.BOUNCER)(Launch.CANCELLED)
        assertEquals(listOf(Outcome.BOUNCER_CANCELLED), r.settled)
    }

    @Test
    fun `focus lost before the lock route began does not count for it`() {
        val r = rig(locked = true, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.STARTED)
        r.sequencer.begin()
        r.sequencer.onFocusChanged(false) // the emergency dialler's window
        r.advance(1_500)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        r.advance(1_500)
        assertEquals("the lock screen never showed: ACTION_CALL", listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER, Route.CALL), r.launched)
    }

    @Test
    fun `the phone's own lock dismissed carries on as unlocked, without the lock or the emergency dialler again`() {
        val r = rig(locked = true, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.FAILED, Route.SYSTEM_DIAL to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        r.sequencer.onFocusChanged(false)
        r.sequencer.onStopped(screenOn = true) // the lock screen's own stop
        r.pendingResults.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER, Route.SYSTEM_DIAL), r.launched)
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.OPENED), r.settled)
    }

    @Test
    fun `the phone's own lock cancelled settles as cancelled and tries nothing more`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.pendingResults.getValue(Route.BOUNCER)(Launch.CANCELLED)
        assertEquals(listOf(Outcome.BOUNCER_CANCELLED), r.settled)
        assertEquals(listOf(Route.BOUNCER), r.launched)
    }

    @Test
    fun `the phone's own lock that cannot be shown falls through to ACTION_CALL, and failing that settles failed`() {
        val r = rig(locked = true, ed = false, dial = false, Route.BOUNCER to Launch.FAILED, Route.CALL to Launch.FAILED)
        r.sequencer.begin()
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
        assertEquals(listOf(Outcome.FAILED), r.settled)
    }

    @Test
    fun `a pause that lands before the launch is reported waits for the stop`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.sequencer.onPaused()
        r.pendingResults.getValue(Route.EMERGENCY_DIALLER)(Launch.STARTED)
        assertTrue("a pause alone is not enough", r.settled.isEmpty())
        assertEquals(3_000L, r.nextDueIn())
        r.sequencer.onStopped(screenOn = true)
        assertEquals(listOf(Outcome.OPENED), r.settled)
    }

    @Test
    fun `a stop that lands before the launch is reported settles it`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        r.pendingResults.getValue(Route.EMERGENCY_DIALLER)(Launch.STARTED)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertTrue("no timer needed", r.active.isEmpty())
    }

    @Test
    fun `a guardian's number is one ACTION_CALL, settled on start with no confirmation wait`() {
        val ok = Rig(isEmergency = false, locked = true, ed = true, dial = true, script = mutableMapOf(Route.CALL to Launch.STARTED))
        ok.sequencer.begin()
        assertEquals(listOf(Route.CALL), ok.launched)
        assertEquals(listOf(Outcome.OPENED), ok.settled)
        assertTrue(ok.active.isEmpty())

        val bad = Rig(isEmergency = false, locked = false, ed = false, dial = false, script = mutableMapOf(Route.CALL to Launch.FAILED))
        bad.sequencer.begin()
        assertEquals(listOf(Route.CALL), bad.launched)
        assertEquals(listOf(Outcome.FAILED), bad.settled)
    }

    @Test
    fun `abandoning cancels the wait and ignores late results`() {
        val r = rig(locked = true, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.STARTED)
        r.sequencer.begin()
        r.sequencer.abandon()
        assertTrue(r.active.isEmpty())
        r.sequencer.onPaused()
        r.sequencer.onStopped(screenOn = true)
        r.advance(10_000)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertTrue(r.settled.isEmpty())
    }

    @Test
    fun `a stale result from an earlier route is ignored`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        val first = r.pendingResults.getValue(Route.EMERGENCY_DIALLER)
        first(Launch.FAILED)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        // The first route answers a second time, late: nothing moves.
        first(Launch.FAILED)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
    }
}
