package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.CallCountdown.Step
import org.forgesworn.charter.ui.CallSequencer.Companion.LATE_MS
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The 3-2-1 cancel window, held by the activity so a render rebuilds a
 * counting button still counting (review 10, LO-2), and the status line after
 * a cancel (LO-1). A countdown the ward cannot see never completes: the screen
 * going off, a tick the device slept through, or a render that drops its
 * button calls it off; and one press counts at a time (final review, F1, F4,
 * F5).
 */
class CallCountdownTest {

    private fun countdown() = CallCountdown("Emergency 999", "emergency services", "999", emergency = true, left = 3)

    @Test
    fun `it counts 3, 2, 1 and the call is placed on the third tick`() {
        val c = countdown()
        assertEquals("Calling emergency services in 3… (tap to cancel)", c.text())
        assertEquals(Step.COUNT, c.tick(c.dueAt, inSight = true))
        assertEquals("Calling emergency services in 2… (tap to cancel)", c.text())
        assertEquals(Step.COUNT, c.tick(c.dueAt, inSight = true))
        assertEquals("Calling emergency services in 1… (tap to cancel)", c.text())
        assertEquals(Step.PLACE, c.tick(c.dueAt, inSight = true))
    }

    @Test
    fun `a tick up to LATE_MS after its time still counts, and one later calls the press off at any number`() {
        val c = countdown()
        c.dueAt = 5_000L
        assertFalse(c.late(5_000L + LATE_MS))
        assertTrue(c.late(5_000L + LATE_MS + 1))
        assertEquals(Step.COUNT, c.tick(5_000L + LATE_MS, inSight = true))
        assertEquals(Step.CANCEL, c.tick(5_000L + LATE_MS + 1, inSight = true))
        assertEquals("a cancelled tick does not count down", 2, c.left)
        c.left = 1
        assertEquals("not even the last", Step.CANCEL, c.tick(2 * 60 * 60_000L, inSight = true))
    }

    @Test
    fun `a tick out of sight calls the press off, even the last`() {
        val c = countdown()
        c.left = 1
        assertEquals(Step.CANCEL, c.tick(c.dueAt, inSight = false))
        assertEquals(1, c.left)
    }

    @Test
    fun `a button rebuilt mid-countdown shows the countdown where it is, and one with none shows its label`() {
        val c = countdown()
        c.tick(c.dueAt, inSight = true)
        assertEquals("Calling emergency services in 2… (tap to cancel)", CallCountdown.buttonText("Emergency 999", c))
        assertEquals("Call Mum", CallCountdown.buttonText("Call Mum", null))
    }

    @Test
    fun `each lifeline button has its own key`() {
        val emergency = CallCountdown.key("Emergency 999", "999", true)
        assertEquals(emergency, countdown().key)
        assertNotEquals(emergency, CallCountdown.key("Call 999", "999", false))
        assertNotEquals(CallCountdown.key("Call Mum", "0700", false), CallCountdown.key("Call Dad", "0700", false))
    }

    @Test
    fun `cancelling never wipes a sticky line, and says so only when there is none`() {
        val advice = CallRoute.Line.ADVICE.text
        assertEquals(advice, CallCountdown.lineAfterCancel(advice))
        assertEquals("Call cancelled.", CallCountdown.lineAfterCancel(""))
    }

    // ---- CallCountdowns: the shade's countdowns ---------------------------------

    /**
     * The holder, with a timer on two fake clocks (as in CallSequencerTest): a
     * posted tick runs only when the test moves the awake clock past it
     * ([advance]); [sleep] moves only the real-time one, as a device asleep
     * does. The host records what it is asked to do; a status line it keeps
     * stands in for the activity's, with [CallCountdown.lineAfterCancel].
     */
    private class Board {
        var clock = 0L
        var realtime = 0L
        var inSight = true
        val active = mutableListOf<Pair<Runnable, Long>>()

        /** The text on each button, by key, as last painted. */
        val buttons = mutableMapOf<String, String>()
        val completed = mutableListOf<CallCountdown>()

        /** The status line under the buttons, and whether it is sticky. */
        var line = ""
        var sticky = false
        var cancels = 0

        val timer = object : CallSequencer.Timer {
            override fun postDelayed(task: Runnable, delayMs: Long) {
                active += task to clock + delayMs
            }

            override fun cancel(task: Runnable) {
                active.removeAll { it.first === task }
            }

            override fun now(): Long = realtime
        }

        val countdowns = CallCountdowns(
            timer,
            object : CallCountdowns.Host {
                override fun paint(key: String, text: String) {
                    buttons[key] = text
                }

                override fun inSight(): Boolean = this@Board.inSight

                override fun complete(c: CallCountdown) {
                    completed += c
                }

                override fun cancelled() {
                    cancels++
                    val kept = if (sticky) line else ""
                    line = CallCountdown.lineAfterCancel(kept)
                    sticky = kept.isNotEmpty()
                }
            },
        )

        fun advance(ms: Long) {
            val target = clock + ms
            while (true) {
                val due = active.filter { it.second <= target }.minByOrNull { it.second } ?: break
                active.remove(due)
                realtime += due.second - clock
                clock = due.second
                due.first.run()
            }
            realtime += target - clock
            clock = target
        }

        fun sleep(ms: Long) {
            realtime += ms
        }
    }

    private val emergencyKey = CallCountdown.key("Emergency 999", "999", true)
    private fun mum() = CallCountdown("Call Mum", "Mum", "0700", emergency = false, left = 3)
    private val mumKey = CallCountdown.key("Call Mum", "0700", false)

    @Test
    fun `a countdown completes after three on-time ticks, once, and gives the button its label back`() {
        val b = Board()
        assertTrue(b.countdowns.start(countdown()))
        assertEquals("Calling emergency services in 3… (tap to cancel)", b.buttons[emergencyKey])
        b.advance(2_999)
        assertTrue(b.completed.isEmpty())
        assertEquals("Calling emergency services in 1… (tap to cancel)", b.buttons[emergencyKey])
        b.advance(1)
        assertEquals(1, b.completed.size)
        assertEquals("Emergency 999", b.buttons[emergencyKey])
        assertNull(b.countdowns[emergencyKey])
        assertTrue(b.active.isEmpty())
        b.advance(10_000)
        assertEquals(1, b.completed.size)
        assertEquals(0, b.cancels)
    }

    @Test
    fun `a countdown the device slept through is called off, never completed, and the label comes back`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.advance(500)
        b.sleep(2 * 60 * 60_000L) // two hours with no awake time
        b.advance(2_500) // every tick then runs, late
        assertTrue("nothing reported, no route", b.completed.isEmpty())
        assertEquals("Emergency 999", b.buttons[emergencyKey])
        assertNull(b.countdowns[emergencyKey])
        assertEquals(CallCountdown.CANCELLED, b.line)
        assertEquals(1, b.cancels)
        assertTrue(b.active.isEmpty())
    }

    @Test
    fun `a late tick keeps a sticky line that was showing`() {
        val b = Board()
        b.line = CallRoute.Line.ADVICE.text
        b.sticky = true
        b.countdowns.start(countdown())
        b.advance(1_000)
        b.sleep(60_000L)
        b.advance(1_000)
        assertTrue(b.completed.isEmpty())
        assertEquals(CallRoute.Line.ADVICE.text, b.line)
        assertTrue(b.sticky)
    }

    @Test
    fun `a tick a little late, within the tolerance, still counts`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.sleep(LATE_MS)
        b.advance(3_000)
        assertEquals(1, b.completed.size)
    }

    @Test
    fun `the screen going off calls off every countdown, as a tap would, and nothing runs after`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.advance(1_500)
        b.inSight = false
        b.countdowns.cancelAll()
        assertEquals("Emergency 999", b.buttons[emergencyKey])
        assertEquals(CallCountdown.CANCELLED, b.line)
        assertEquals(1, b.cancels)
        assertTrue(b.active.isEmpty())
        b.inSight = true
        b.advance(10_000)
        assertTrue("nothing reported, no route", b.completed.isEmpty())

        // With the advice showing, the advice stays.
        val kept = Board()
        kept.line = CallRoute.Line.ADVICE.text
        kept.sticky = true
        kept.countdowns.start(countdown())
        kept.countdowns.cancelAll()
        assertEquals(CallRoute.Line.ADVICE.text, kept.line)

        // Nothing counting: nothing said.
        val idle = Board()
        idle.countdowns.cancelAll()
        assertEquals(0, idle.cancels)
        assertEquals("", idle.line)
    }

    @Test
    fun `a tick that finds the shade out of sight calls the press off`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.advance(2_000)
        b.inSight = false // covered or screen off, the broadcast or callback not here yet
        b.advance(1_000)
        assertTrue(b.completed.isEmpty())
        assertEquals("Emergency 999", b.buttons[emergencyKey])
        assertEquals(1, b.cancels)
    }

    @Test
    fun `the shade losing the focus mid-countdown calls the press off at once, keeping a sticky line`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.advance(1_500)
        b.inSight = false // covered: the activity calls cancelAll beside the sequencer
        b.countdowns.cancelAll()
        assertEquals("Emergency 999", b.buttons[emergencyKey])
        assertEquals(CallCountdown.CANCELLED, b.line)
        assertEquals(1, b.cancels)
        assertTrue(b.active.isEmpty())
        b.inSight = true // the cover leaves
        b.advance(10_000)
        assertTrue("nothing reported, no route", b.completed.isEmpty())

        val kept = Board()
        kept.line = CallRoute.Line.ADVICE.text
        kept.sticky = true
        kept.countdowns.start(countdown())
        kept.advance(1_000)
        kept.inSight = false
        kept.countdowns.cancelAll()
        assertEquals(CallRoute.Line.ADVICE.text, kept.line)
        assertTrue(kept.sticky)
        assertEquals("Emergency 999", kept.buttons[emergencyKey])
        assertTrue(kept.completed.isEmpty())
    }

    @Test
    fun `a tap cancels its own countdown, with the same line`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.advance(1_000)
        b.countdowns.cancel(emergencyKey)
        assertEquals(CallCountdown.CANCELLED, b.line)
        b.advance(10_000)
        assertTrue(b.completed.isEmpty())
        b.countdowns.cancel(emergencyKey)
        assertEquals("cancelling twice says nothing more", 1, b.cancels)
    }

    @Test
    fun `a render that does not rebuild a counting button calls its countdown off, and one that does keeps it`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.advance(1_000)
        b.countdowns.keepOnly(setOf(emergencyKey, mumKey))
        assertEquals("rebuilt: still counting", 0, b.cancels)
        b.countdowns.keepOnly(setOf(mumKey)) // emergency services switched off, or the number changed
        assertNull(b.countdowns[emergencyKey])
        assertEquals(1, b.cancels)
        assertEquals(CallCountdown.CANCELLED, b.line)
        b.advance(10_000)
        assertTrue(b.completed.isEmpty())
        b.countdowns.keepOnly(emptySet())
        assertEquals("nothing counting: nothing said", 1, b.cancels)
    }

    @Test
    fun `while one press counts, another button cannot start a second`() {
        val b = Board()
        assertTrue(b.countdowns.start(countdown()))
        assertTrue(b.countdowns.any())
        assertFalse(b.countdowns.start(mum()))
        assertNull(b.countdowns[mumKey])
        assertFalse("the refused button is not painted", mumKey in b.buttons)
        b.advance(3_000)
        assertEquals(listOf("999"), b.completed.map { it.number })
        assertFalse(b.countdowns.any())
        assertTrue("once it is done, another may start", b.countdowns.start(mum()))
        b.countdowns.cancel(mumKey)
        assertTrue("and once one is cancelled", b.countdowns.start(countdown()))
    }

    @Test
    fun `abandoning drops every tick and says nothing`() {
        val b = Board()
        b.countdowns.start(countdown())
        b.countdowns.abandon()
        assertTrue(b.active.isEmpty())
        b.advance(10_000)
        assertTrue(b.completed.isEmpty())
        assertEquals(0, b.cancels)
        assertFalse(b.countdowns.any())
    }
}
