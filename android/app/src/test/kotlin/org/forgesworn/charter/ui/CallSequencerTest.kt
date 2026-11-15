package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.CallRoute.Route
import org.forgesworn.charter.ui.CallSequencer.Launch
import org.forgesworn.charter.ui.CallSequencer.Outcome
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CallSequencerTest {

    /** A launcher that answers each route from a script, and a clock-less timer
     *  that runs a posted task only when the test says the time passed. */
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
        val posted = mutableListOf<Pair<Runnable, Long>>()
        val cancelled = mutableListOf<Runnable>()
        val pendingResults = mutableMapOf<Route, (Launch) -> Unit>()

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
                    posted += task to delayMs
                }

                override fun cancel(task: Runnable) {
                    cancelled += task
                }
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

        /** The confirm window of the latest started launch runs out. */
        fun timeOut() {
            val (task, _) = posted.last { it.first !in cancelled }
            task.run()
        }
    }

    private fun rig(locked: Boolean, ed: Boolean, dial: Boolean, vararg answers: Pair<Route, Launch>) =
        Rig(locked = locked, ed = ed, dial = dial, script = mutableMapOf(*answers))

    @Test
    fun `the emergency dialler that takes the screen settles the press, with no fallthrough`() {
        val r = rig(locked = false, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertTrue("not settled until the shade is paused", r.settled.isEmpty())
        assertEquals(CallSequencer.CONFIRM_MS, r.posted.single().second)
        assertEquals(1_500L, CallSequencer.CONFIRM_MS)
        r.sequencer.onPaused()
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertEquals("the timeout is cancelled", r.posted.single().first, r.cancelled.single())
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
    }

    @Test
    fun `a launch the shade never pauses for is treated as refused, and the next route runs`() {
        val r = rig(
            locked = false, ed = true, dial = true,
            Route.EMERGENCY_DIALLER to Launch.STARTED,
            Route.SYSTEM_DIAL to Launch.STARTED,
        )
        r.sequencer.begin()
        r.timeOut()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.settled.isEmpty())
        r.sequencer.onPaused()
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
        r.timeOut() // ACTION_DIAL refused silently
        r.timeOut() // ACTION_CALL refused silently
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL, Route.CALL), r.launched)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        // Late events change nothing.
        r.sequencer.onPaused()
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
        r.timeOut()
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
    fun `the phone's own lock dismissed carries on as unlocked, without the lock or the emergency dialler again`() {
        val r = rig(locked = true, ed = true, dial = true, Route.EMERGENCY_DIALLER to Launch.FAILED, Route.SYSTEM_DIAL to Launch.STARTED)
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        r.pendingResults.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER, Route.SYSTEM_DIAL), r.launched)
        r.sequencer.onPaused()
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
    fun `a pause that lands before the launch is reported still confirms it`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.sequencer.onPaused()
        r.pendingResults.getValue(Route.EMERGENCY_DIALLER)(Launch.STARTED)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertTrue("no timer needed", r.posted.isEmpty())
    }

    @Test
    fun `a guardian's number is one ACTION_CALL, settled on start with no confirmation wait`() {
        val ok = Rig(isEmergency = false, locked = true, ed = true, dial = true, script = mutableMapOf(Route.CALL to Launch.STARTED))
        ok.sequencer.begin()
        assertEquals(listOf(Route.CALL), ok.launched)
        assertEquals(listOf(Outcome.OPENED), ok.settled)
        assertTrue(ok.posted.isEmpty())

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
        assertEquals(r.posted.single().first, r.cancelled.single())
        r.sequencer.onPaused()
        r.posted.single().first.run()
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
