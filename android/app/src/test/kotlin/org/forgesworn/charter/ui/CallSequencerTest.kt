package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.CallRoute.Line
import org.forgesworn.charter.ui.CallRoute.Route
import org.forgesworn.charter.ui.CallSequencer.Companion.BOUNCER_WAIT_MS
import org.forgesworn.charter.ui.CallSequencer.Companion.CHECK_MS
import org.forgesworn.charter.ui.CallSequencer.Companion.CLEAR_WAIT_MS
import org.forgesworn.charter.ui.CallSequencer.Companion.LATE_MS
import org.forgesworn.charter.ui.CallSequencer.Companion.LOCK_GONE_MS
import org.forgesworn.charter.ui.CallSequencer.Launch
import org.forgesworn.charter.ui.CallSequencer.Outcome
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CallSequencerTest {

    /**
     * A shade whose state the test sets, a launcher that records each route and
     * answers as [onLaunch] says, and a timer on two fake clocks: a posted task
     * runs only when the test moves the awake clock past it ([advance]), and
     * [sleep] moves only the real-time one, as a device asleep does.
     */
    private class Rig(
        val isEmergency: Boolean = true,
        val locked: Boolean,
        val ed: Boolean,
        val dial: Boolean,
    ) {
        /** Something is over the shade, or it has lost the focus. */
        var covered = false
        var screenOn = true
        var ringing = false

        val launched = mutableListOf<Route>()
        val lines = mutableListOf<Line>()
        val settled = mutableListOf<Outcome>()

        /** Whether the shade was clear at each launch. */
        val clearAtLaunch = mutableListOf<Boolean>()

        /** The answer callback of each route's latest launch. */
        val answers = mutableMapOf<Route, (Launch) -> Unit>()

        /** What a launch does, and the answer it gives at once (null: none
         *  yet). By default an intent route starts and the phone's own lock
         *  waits for the ward. */
        var onLaunch: (Route) -> Launch? = { if (it == Route.BOUNCER) null else Launch.STARTED }

        /** The awake clock the timer's delays run on. */
        var clock = 0L

        /** `elapsedRealtime`: moves with [clock], and on its own in [sleep]. */
        var realtime = 0L

        /** Tasks posted and neither run nor cancelled: (task, due time). */
        val active = mutableListOf<Pair<Runnable, Long>>()

        private val shade = object : CallSequencer.Shade {
            override fun clear(): Boolean = !covered && this@Rig.screenOn
            override fun screenOn(): Boolean = this@Rig.screenOn
            override fun ringing(): Boolean = this@Rig.ringing
        }

        private val timer = object : CallSequencer.Timer {
            override fun postDelayed(task: Runnable, delayMs: Long) {
                active += task to clock + delayMs
            }

            override fun cancel(task: Runnable) {
                active.removeAll { it.first === task }
            }

            override fun now(): Long = realtime
        }

        private val listener = object : CallSequencer.Listener {
            override fun onRoute(route: Route) = Unit

            override fun onLine(line: Line) {
                lines += line
            }

            override fun onSettled(outcome: Outcome) {
                settled += outcome
            }
        }

        private fun build() = CallSequencer(
            isEmergency = isEmergency,
            locked = locked,
            emergencyDiallerOk = ed,
            systemDialOk = dial,
            shade = shade,
            launcher = { route, result ->
                launched += route
                clearAtLaunch += shade.clear()
                answers[route] = result
                onLaunch(route)?.let(result)
            },
            timer = timer,
            listener = listener,
        )

        var sequencer = build()

        /** A new press, as LockActivity.startCall makes one. */
        fun press() {
            sequencer.abandon()
            sequencer = build()
            sequencer.begin()
        }

        /** Move both clocks forward, running whatever falls due, in order. */
        fun advance(ms: Long) {
            val target = clock + ms
            while (true) {
                val due = active.filter { it.second <= target }.minByOrNull { it.second } ?: break
                active.remove(due)
                moveTo(maxOf(clock, due.second))
                due.first.run()
            }
            moveTo(target)
        }

        private fun moveTo(t: Long) {
            realtime += t - clock
            clock = t
        }

        /** The device sleeps for [ms]: real time passes, and no timer runs. */
        fun sleep(ms: Long) {
            realtime += ms
        }

        /** How long until the next thing falls due, or null. */
        fun nextDueIn(): Long? = active.minOfOrNull { it.second }?.minus(clock)

        /** The route launched [index]th (0 = the first) covers the shade. */
        fun opensAt(index: Int) {
            onLaunch = { route ->
                if (launched.size - 1 == index) covered = true
                if (route == Route.BOUNCER) null else Launch.STARTED
            }
        }
    }

    private fun rig(locked: Boolean, ed: Boolean, dial: Boolean) = Rig(locked = locked, ed = ed, dial = dial)

    @Test
    fun `the timings are the named constants`() {
        assertEquals(2_500L, CHECK_MS)
        assertEquals(3_000L, CLEAR_WAIT_MS)
        assertEquals(60_000L, BOUNCER_WAIT_MS)
        assertEquals(3_000L, LOCK_GONE_MS)
        assertEquals(1_000L, LATE_MS)
    }

    // ---- an intent route's one check -------------------------------------------

    @Test
    fun `an emergency press shows the advice from the start, before any route is judged`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        assertEquals(listOf(Line.ADVICE), r.lines)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertTrue(r.settled.isEmpty())
        assertEquals(CHECK_MS, r.nextDueIn())
    }

    @Test
    fun `a route that leaves the shade clear at the check showed nothing, and the next route runs`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.advance(CHECK_MS - 1)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        r.advance(1)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.settled.isEmpty())
        assertEquals(listOf(Line.ADVICE), r.lines)
    }

    @Test
    fun `a route that covers the shade with the screen on settles opened, and the advice stays`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.opensAt(0)
        r.sequencer.begin()
        r.advance(CHECK_MS - 1)
        assertTrue("judged once, at the check", r.settled.isEmpty())
        r.advance(1)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertEquals(listOf(Line.ADVICE), r.lines)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertTrue("every timer is gone", r.active.isEmpty())
    }

    @Test
    fun `covered with the screen off settles failed, and the advice stays to be seen on waking`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.advance(500)
        r.screenOn = false // the power button, not a dialler
        r.advance(CHECK_MS)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals("not the stronger line: the shade was not clear", listOf(Line.ADVICE), r.lines)
        assertEquals("nothing more from a sleeping shade", listOf(Route.EMERGENCY_DIALLER), r.launched)
        r.screenOn = true
        r.sequencer.onClear()
        r.advance(60_000)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
    }

    @Test
    fun `covered by a ringing call settles failed, and the advice is there after the call`() {
        val r = rig(locked = false, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.ringing = true
        r.advance(CHECK_MS)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Line.ADVICE), r.lines)
        assertEquals("ACTION_CALL is not started into the call", listOf(Route.SYSTEM_DIAL), r.launched)
    }

    @Test
    fun `the shade coming back during the check cuts nothing short, and the check still decides`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.covered = true // a window takes the focus, with no pause ...
        r.advance(300)
        r.covered = false // ... and gives it back
        r.sequencer.onClear()
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        r.advance(CHECK_MS - 300)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
    }

    @Test
    fun `a launch that throws runs the next route at once`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.onLaunch = { route -> if (route == Route.CALL) Launch.STARTED else Launch.FAILED }
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL, Route.CALL), r.launched)
        assertEquals(0L, r.clock)
        assertTrue(r.settled.isEmpty())
        assertEquals(CHECK_MS, r.nextDueIn())
    }

    @Test
    fun `the last route throwing with the shade clear says the dialler could not be opened`() {
        val r = rig(locked = false, ed = false, dial = false)
        r.onLaunch = { Launch.FAILED }
        r.sequencer.begin()
        assertEquals(listOf(Route.CALL), r.launched)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Line.ADVICE, Line.NOTHING_OPENED), r.lines)
        assertTrue(r.active.isEmpty())
    }

    // ---- launching only from a clear shade -------------------------------------

    @Test
    fun `a route due while the shade is not clear waits for it, then launches`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.covered = true
        r.sequencer.begin()
        assertTrue(r.launched.isEmpty())
        assertEquals(listOf(Line.ADVICE), r.lines)
        assertEquals(CLEAR_WAIT_MS, r.nextDueIn())
        r.advance(1_000)
        r.sequencer.onClear() // resumed, but the focus has not come back
        assertTrue(r.launched.isEmpty())
        r.covered = false
        r.sequencer.onClear()
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertEquals("the wait's bound is gone; the check is pending", CHECK_MS, r.nextDueIn())
        assertEquals(1, r.active.size)
    }

    @Test
    fun `the wait for a clear shade is bounded, and settles failed with the advice and nothing launched`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.covered = true
        r.sequencer.begin()
        r.advance(CLEAR_WAIT_MS - 1)
        assertTrue(r.settled.isEmpty())
        r.advance(1)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertTrue(r.launched.isEmpty())
        assertEquals(listOf(Line.ADVICE), r.lines)
        r.covered = false
        r.sequencer.onClear()
        assertTrue("nothing after settling", r.launched.isEmpty())
    }

    @Test
    fun `ACTION_CALL is never started from a shade that is not clear`() {
        // The phone's own lock errors while its window still covers the shade.
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        assertEquals(listOf(Route.BOUNCER), r.launched)
        r.covered = true
        r.answers.getValue(Route.BOUNCER)(Launch.FAILED)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        r.advance(CLEAR_WAIT_MS)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
    }

    // ---- the phone's own lock --------------------------------------------------

    @Test
    fun `the phone's own lock shows its instruction, then the advice again when it is done`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER), r.lines)
        r.covered = true
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
    }

    @Test
    fun `the phone's own lock dismissed runs the unlocked routes once the shade is clear, without the lock or the emergency dialler`() {
        val r = rig(locked = true, ed = true, dial = true)
        r.onLaunch = { route ->
            when (route) {
                Route.EMERGENCY_DIALLER -> Launch.FAILED
                Route.BOUNCER -> null
                else -> Launch.STARTED
            }
        }
        r.sequencer.begin()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        r.covered = true // the PIN screen is up
        r.advance(1_000)
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED) // before the shade is back
        assertEquals("waits for a clear shade", listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER), r.launched)
        assertEquals(CLEAR_WAIT_MS, r.nextDueIn())
        r.advance(400)
        r.covered = false
        r.sequencer.onClear()
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.BOUNCER, Route.SYSTEM_DIAL), r.launched)
        r.covered = true // the system dialler
        r.advance(CHECK_MS)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
    }

    @Test
    fun `the phone's own lock dismissed while the shade stays covered settles failed and never covers what opened`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        r.advance(CLEAR_WAIT_MS)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertEquals(Line.ADVICE, r.lines.last())
    }

    @Test
    fun `the phone's own lock answering with an error runs the next route`() {
        val r = rig(locked = true, ed = false, dial = false)
        r.sequencer.begin()
        r.advance(1_000)
        r.answers.getValue(Route.BOUNCER)(Launch.FAILED)
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
        assertEquals("the lock's own check is gone; ACTION_CALL's is pending", 1, r.active.size)
        assertEquals(CHECK_MS, r.nextDueIn())
    }

    @Test
    fun `the phone's own lock that cannot be requested runs the next route at once`() {
        val r = rig(locked = true, ed = false, dial = false)
        r.onLaunch = { route -> if (route == Route.BOUNCER) Launch.FAILED else Launch.STARTED }
        r.sequencer.begin()
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
        assertEquals(1, r.active.size)
        r.advance(CHECK_MS)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(Line.NOTHING_OPENED, r.lines.last())
    }

    @Test
    fun `the phone's own lock cancelled settles cancelled with the advice, and tries nothing more`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS) // it covered the shade: waiting on the ward
        r.covered = false
        r.answers.getValue(Route.BOUNCER)(Launch.CANCELLED)
        assertEquals(listOf(Outcome.CANCELLED), r.settled)
        assertEquals("the softer line, never the stronger", listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertTrue(r.active.isEmpty())
    }

    @Test
    fun `the phone's own lock that never covers the shade is given up at the check, and its late answer is ignored`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        assertEquals(CHECK_MS, r.nextDueIn())
        r.advance(CHECK_MS - 1)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        r.advance(1)
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        r.answers.getValue(Route.BOUNCER)(Launch.CANCELLED)
        assertEquals(listOf(Route.BOUNCER, Route.CALL), r.launched)
        assertTrue(r.settled.isEmpty())
    }

    @Test
    fun `the phone's own lock that covers the shade has 60 s to answer, then the press settles failed`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertEquals("60 s from the request", BOUNCER_WAIT_MS - CHECK_MS, r.nextDueIn())
        r.advance(BOUNCER_WAIT_MS - CHECK_MS - 1)
        assertTrue(r.settled.isEmpty())
        r.advance(1)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertEquals("covered at the bound: the advice", listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals("a late answer changes nothing", listOf(Route.BOUNCER), r.launched)
        assertEquals(listOf(Outcome.FAILED), r.settled)
    }

    @Test
    fun `the phone's own lock gone without an answer brings the advice back after 3 s, launches nothing, and fails at 60 s`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS)
        r.covered = false
        r.sequencer.onClear()
        assertEquals(LOCK_GONE_MS, r.nextDueIn())
        r.advance(LOCK_GONE_MS - 1)
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER), r.lines)
        r.advance(1)
        assertEquals(listOf(Line.ADVICE, Line.BOUNCER, Line.ADVICE), r.lines)
        assertEquals("nothing launched: a dismissal may still be on its way", listOf(Route.BOUNCER), r.launched)
        assertTrue(r.settled.isEmpty())
        assertEquals("the bound still runs from the request", BOUNCER_WAIT_MS - CHECK_MS - LOCK_GONE_MS, r.nextDueIn())
        r.advance(BOUNCER_WAIT_MS)
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals("the advice at the bound, never the stronger line", Line.ADVICE, r.lines.last())
        assertTrue(Line.NOTHING_OPENED !in r.lines)
    }

    @Test
    fun `a dismissal that arrives after the lock looked gone still runs the unlocked routes`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS)
        r.covered = false
        r.sequencer.onClear()
        r.advance(LOCK_GONE_MS + 1_000)
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Route.BOUNCER, Route.SYSTEM_DIAL), r.launched)
    }

    @Test
    fun `an answer inside the lock-gone wait is taken as usual`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS)
        r.covered = false
        r.sequencer.onClear()
        r.advance(1_000)
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Route.BOUNCER, Route.SYSTEM_DIAL), r.launched)
        assertEquals("only the dialler's check", 1, r.active.size)
        assertEquals(CHECK_MS, r.nextDueIn())
    }

    @Test
    fun `a lock that comes back over the shade inside the wait keeps its instruction, and its 60 s bound`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS)
        r.covered = false
        r.sequencer.onClear()
        r.advance(1_000)
        r.covered = true // the PIN screen is back
        r.advance(LOCK_GONE_MS)
        assertEquals(Line.BOUNCER, r.lines.last())
        r.covered = false
        r.sequencer.onClear() // gone again: a new 3 s wait
        r.advance(LOCK_GONE_MS)
        assertEquals(Line.ADVICE, r.lines.last())
        r.covered = true
        val elapsed = r.clock
        r.advance(BOUNCER_WAIT_MS - elapsed - 1)
        assertTrue(r.settled.isEmpty())
        r.advance(1)
        assertEquals("60 s from the request, whatever happened in between", listOf(Outcome.FAILED), r.settled)
        assertEquals(BOUNCER_WAIT_MS, r.clock)
        assertEquals(listOf(Route.BOUNCER), r.launched)
    }

    // ---- one settlement, and every timer invalidated ---------------------------

    @Test
    fun `settling cancels every timer, and nothing later moves it`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.opensAt(0)
        r.sequencer.begin()
        r.advance(CHECK_MS)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertTrue(r.active.isEmpty())
        r.covered = false
        r.sequencer.onClear()
        r.answers.getValue(Route.EMERGENCY_DIALLER)(Launch.FAILED)
        r.advance(BOUNCER_WAIT_MS)
        assertEquals(listOf(Outcome.OPENED), r.settled)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertEquals(listOf(Line.ADVICE), r.lines)
    }

    @Test
    fun `a new press invalidates the last press's timers and late answers`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.press()
        val firstAnswer = r.answers.getValue(Route.BOUNCER)
        assertEquals(1, r.active.size)
        r.advance(1_000)
        r.press()
        assertEquals("only the new press's check", 1, r.active.size)
        assertEquals(CHECK_MS, r.nextDueIn())
        firstAnswer(Launch.CANCELLED)
        firstAnswer(Launch.DISMISSED)
        assertTrue(r.settled.isEmpty())
        r.advance(CHECK_MS)
        assertEquals("one ACTION_CALL, from the new press", listOf(Route.BOUNCER, Route.BOUNCER, Route.CALL), r.launched)
    }

    @Test
    fun `a new press while the last one waits for a clear shade replaces that wait`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.covered = true
        r.press()
        r.advance(2_000)
        r.press()
        r.advance(CLEAR_WAIT_MS - 1)
        assertTrue("the first wait's bound is gone", r.settled.isEmpty())
        r.advance(1)
        assertEquals(listOf(Outcome.FAILED), r.settled)
    }

    @Test
    fun `abandoning cancels every timer and ignores late answers and a clear shade`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.sequencer.abandon()
        assertTrue(r.active.isEmpty())
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        r.sequencer.onClear()
        r.advance(BOUNCER_WAIT_MS)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertTrue(r.settled.isEmpty())
        assertEquals("nothing more is said", listOf(Line.ADVICE, Line.BOUNCER), r.lines)

        val waiting = rig(locked = false, ed = true, dial = true)
        waiting.covered = true
        waiting.sequencer.begin()
        waiting.sequencer.abandon()
        assertTrue(waiting.active.isEmpty())
        waiting.covered = false
        waiting.sequencer.onClear()
        assertTrue(waiting.launched.isEmpty())
    }

    @Test
    fun `a stale answer from an earlier route is ignored`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.onLaunch = { null }
        r.sequencer.begin()
        val first = r.answers.getValue(Route.EMERGENCY_DIALLER)
        first(Launch.FAILED)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        first(Launch.FAILED)
        first(Launch.STARTED)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.active.isEmpty())
    }

    // ---- the ward closing what opened (no escalation) ------------------------------

    @Test
    fun `the emergency dialler closed by the ward before the check settles failed with the advice, and ACTION_CALL is never tried`() {
        for (dial in bools) {
            val r = rig(locked = false, ed = true, dial = dial)
            r.sequencer.begin()
            r.advance(40)
            r.covered = true
            r.sequencer.onPaused() // the dialler shows
            r.advance(1_460)
            r.covered = false // the ward backs out at +1.5 s: resumed, focus back
            r.sequencer.onClear()
            r.advance(CHECK_MS - 1_500)
            assertEquals("dial=$dial", listOf(Outcome.FAILED), r.settled)
            assertEquals("dial=$dial", listOf(Line.ADVICE), r.lines)
            assertEquals("dial=$dial: nothing further", listOf(Route.EMERGENCY_DIALLER), r.launched)
            assertTrue(r.active.isEmpty())
        }
    }

    @Test
    fun `a route that never paused the shade still falls through`() {
        val r = rig(locked = false, ed = true, dial = false)
        r.sequencer.begin()
        r.advance(CHECK_MS) // refused by LockTask: nothing showed
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.CALL), r.launched)
    }

    @Test
    fun `a pause from before the route launched does not count against it`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.covered = true
        r.sequencer.begin() // due, waiting for a clear shade
        r.sequencer.onPaused()
        r.covered = false
        r.sequencer.onClear()
        r.advance(CHECK_MS)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
    }

    @Test
    fun `a pause on one route is forgotten when the next launches`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        r.sequencer.onPaused() // the lock screen going away
        r.covered = false
        r.sequencer.onClear()
        assertEquals(listOf(Route.BOUNCER, Route.SYSTEM_DIAL), r.launched)
        r.advance(CHECK_MS) // the system dialler was refused: it never paused the shade
        assertEquals(listOf(Route.BOUNCER, Route.SYSTEM_DIAL, Route.CALL), r.launched)
    }

    @Test
    fun `a dialler that paused the shade and is still in front at the check is opened`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.sequencer.onPaused()
        r.advance(CHECK_MS)
        assertEquals(listOf(Outcome.OPENED), r.settled)
    }

    // ---- sleep and the screen going off ------------------------------------------

    @Test
    fun `the screen going off during the check settles failed, and nothing launches when the shade is clear again`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.advance(1_000)
        r.screenOn = false
        r.sequencer.onScreenOff()
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Line.ADVICE), r.lines)
        assertTrue(r.active.isEmpty())
        r.screenOn = true // woken within the check, and the shade clear
        r.sequencer.onClear()
        r.advance(CHECK_MS)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
    }

    @Test
    fun `the screen going off settles a press in any wait, and is ignored once settled or before it begins`() {
        val waiting = rig(locked = false, ed = true, dial = true)
        waiting.sequencer.onScreenOff()
        waiting.covered = true
        waiting.sequencer.begin()
        assertTrue("before it began, nothing", waiting.settled.isEmpty())
        waiting.sequencer.onScreenOff()
        assertEquals(listOf(Outcome.FAILED), waiting.settled)
        assertTrue(waiting.launched.isEmpty())

        val lock = rig(locked = true, ed = false, dial = true)
        lock.sequencer.begin()
        lock.covered = true
        lock.advance(CHECK_MS)
        lock.sequencer.onScreenOff()
        assertEquals(listOf(Outcome.FAILED), lock.settled)
        assertEquals(Line.ADVICE, lock.lines.last())
        lock.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        assertEquals(listOf(Route.BOUNCER), lock.launched)

        val opened = rig(locked = false, ed = true, dial = true)
        opened.opensAt(0)
        opened.sequencer.begin()
        opened.advance(CHECK_MS)
        opened.sequencer.onScreenOff()
        assertEquals(listOf(Outcome.OPENED), opened.settled)
    }

    @Test
    fun `waiting for a clear shade past the bound in real time, then clear, settles failed with nothing launched`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.covered = true
        r.sequencer.begin()
        r.advance(500)
        r.sleep(10 * 60_000L) // asleep: the bound's timer never ran
        r.covered = false
        r.sequencer.onClear()
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertTrue(r.launched.isEmpty())
        assertEquals(listOf(Line.ADVICE), r.lines)
        assertTrue(r.active.isEmpty())
    }

    @Test
    fun `a check that runs long after its time, the device having slept, settles failed and launches nothing`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.advance(1_000)
        r.sleep(2 * 60 * 60_000L)
        r.advance(CHECK_MS) // the check runs at last, with the shade clear
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Route.EMERGENCY_DIALLER), r.launched)
        assertEquals(listOf(Line.ADVICE), r.lines)
    }

    @Test
    fun `a check a little late, but within the tolerance, still decides`() {
        val r = rig(locked = false, ed = true, dial = true)
        r.sequencer.begin()
        r.sleep(LATE_MS)
        r.advance(CHECK_MS)
        assertEquals(listOf(Route.EMERGENCY_DIALLER, Route.SYSTEM_DIAL), r.launched)
        assertTrue(r.settled.isEmpty())
    }

    @Test
    fun `the phone's own lock answering after its 60 s in real time settles failed with nothing launched`() {
        val r = rig(locked = true, ed = false, dial = true)
        r.sequencer.begin()
        r.covered = true
        r.advance(CHECK_MS)
        r.sleep(BOUNCER_WAIT_MS) // the bound's timer has not run
        r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
        r.covered = false
        r.sequencer.onClear()
        assertEquals(listOf(Outcome.FAILED), r.settled)
        assertEquals(listOf(Route.BOUNCER), r.launched)
        assertEquals(Line.ADVICE, r.lines.last())
    }

    // ---- a guardian's number ---------------------------------------------------

    @Test
    fun `a guardian's number is one ACTION_CALL, settled when it starts, with no advice`() {
        val ok = Rig(isEmergency = false, locked = true, ed = true, dial = true)
        ok.sequencer.begin()
        assertEquals(listOf(Route.CALL), ok.launched)
        assertEquals(listOf(Outcome.OPENED), ok.settled)
        assertEquals(listOf(Line.BLANK), ok.lines)
        assertTrue(ok.active.isEmpty())

        val bad = Rig(isEmergency = false, locked = false, ed = false, dial = false)
        bad.onLaunch = { Launch.FAILED }
        bad.sequencer.begin()
        assertEquals(listOf(Route.CALL), bad.launched)
        assertEquals(listOf(Outcome.FAILED), bad.settled)
        assertEquals(listOf(Line.BLANK, Line.CALL_FAILED), bad.lines)
    }

    @Test
    fun `a guardian's number also launches only from a clear shade`() {
        val r = Rig(isEmergency = false, locked = false, ed = false, dial = false)
        r.covered = true
        r.sequencer.begin()
        assertTrue(r.launched.isEmpty())
        r.covered = false
        r.sequencer.onClear()
        assertEquals(listOf(Route.CALL), r.launched)
        assertEquals(listOf(Outcome.OPENED), r.settled)

        val never = Rig(isEmergency = false, locked = false, ed = false, dial = false)
        never.covered = true
        never.sequencer.begin()
        never.advance(CLEAR_WAIT_MS)
        assertTrue(never.launched.isEmpty())
        assertEquals(listOf(Outcome.FAILED), never.settled)
        assertEquals(listOf(Line.BLANK, Line.CALL_FAILED), never.lines)
    }

    // ---- full sequences, every locked / emergency dialler / dial cell ----------

    private val bools = listOf(true, false)

    @Test
    fun `every cell, nothing ever covering the shade, tries every route in order and ends on the stronger line`() {
        for (locked in bools) for (ed in bools) for (dial in bools) {
            val cell = "locked=$locked ed=$ed dial=$dial"
            val seq = CallRoute.sequence(true, locked, ed, dial)
            val r = rig(locked, ed, dial)
            r.sequencer.begin()
            r.advance(CHECK_MS * seq.size)
            assertEquals(cell, seq, r.launched)
            assertEquals(cell, listOf(Outcome.FAILED), r.settled)
            assertEquals(cell, Line.NOTHING_OPENED, r.lines.last())
            assertEquals(cell, CHECK_MS * seq.size, r.clock)
            assertTrue(cell, r.active.isEmpty())
            assertTrue(cell, r.clearAtLaunch.all { it })
        }
    }

    @Test
    fun `every cell, the first route that covers the shade settles it opened, with nothing after it`() {
        for (locked in bools) for (ed in bools) for (dial in bools) {
            val seq = CallRoute.sequence(true, locked, ed, dial)
            for ((index, route) in seq.withIndex()) {
                if (route == Route.BOUNCER) continue // the lock answers; it is not judged opened
                val cell = "locked=$locked ed=$ed dial=$dial opens at $route"
                val r = rig(locked, ed, dial)
                r.opensAt(index)
                r.sequencer.begin()
                r.advance(CHECK_MS * seq.size + BOUNCER_WAIT_MS)
                assertEquals(cell, seq.take(index + 1), r.launched)
                assertEquals(cell, listOf(Outcome.OPENED), r.settled)
                assertTrue(cell, Line.NOTHING_OPENED !in r.lines)
                assertEquals(cell, Line.ADVICE, r.lines.last())
                assertTrue(cell, r.active.isEmpty())
                assertTrue(cell, r.clearAtLaunch.all { it })
            }
        }
    }

    @Test
    fun `every locked cell, the PIN entered at the phone's own lock, ends at the unlocked routes`() {
        for (ed in bools) for (dial in bools) {
            val cell = "ed=$ed dial=$dial"
            val r = rig(locked = true, ed = ed, dial = dial)
            r.sequencer.begin()
            r.advance(if (ed) CHECK_MS else 0) // the emergency dialler showed nothing
            assertEquals(cell, Route.BOUNCER, r.launched.last())
            r.covered = true
            r.answers.getValue(Route.BOUNCER)(Launch.DISMISSED)
            r.covered = false
            r.sequencer.onClear()
            val unlocked = CallRoute.sequence(true, locked = false, emergencyDiallerOk = false, systemDialOk = dial)
            assertEquals(cell, unlocked.first(), r.launched.last())
            r.covered = true // it opened
            r.advance(CHECK_MS)
            assertEquals(cell, listOf(Outcome.OPENED), r.settled)
            assertTrue(cell, Route.EMERGENCY_DIALLER !in r.launched.drop(r.launched.indexOf(Route.BOUNCER)))
        }
    }
}
