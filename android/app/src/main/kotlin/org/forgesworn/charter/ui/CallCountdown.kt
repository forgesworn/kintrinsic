package org.forgesworn.charter.ui

/**
 * A hold-to-call press in its 3-2-1 cancel window. [CallCountdowns] holds it,
 * not the button, so a render that rebuilds the buttons (a resume, a rotation,
 * a new lock reason) rebuilds this one still counting, and a tap on the new
 * button still cancels the press. Pure, apart from the tick the holder hands
 * it to keep.
 */
internal class CallCountdown(
    val label: String,
    val who: String,
    val number: String,
    val emergency: Boolean,
    /** The number on the button: 3, 2, 1. The call is placed when it reaches 0. */
    var left: Int,
) {
    /** Which button this is (see [key]). */
    val key: String = key(label, number, emergency)

    /** The tick the holder posted for this countdown, so it can be cancelled. */
    var posted: Runnable? = null

    /** When the posted tick is due, on `elapsedRealtime` (which goes on
     *  counting while the device sleeps). */
    var dueAt: Long = 0L

    /** What a tick does. */
    enum class Step {
        /** Not yet: the button shows the next number. */
        COUNT,

        /** The window is over, on time and in sight: the press is complete. */
        PLACE,

        /** The ward could not have seen or stopped this tick: called off. */
        CANCEL,
    }

    /** The button's text while it counts. */
    fun text(): String = "Calling $who in $left… (tap to cancel)"

    /** The tick due at [dueAt] runs at [now]: more than [CallSequencer.LATE_MS]
     *  after it, the device slept through part of the window. */
    fun late(now: Long): Boolean = now > dueAt + CallSequencer.LATE_MS

    /**
     * One second has passed. A tick that runs late (the device slept, so the
     * ward never saw the countdown) or with the screen off (the ward cannot see
     * or tap it) calls the press off, at any number, never placing it. Otherwise
     * the number goes down, and the press is placed when it reaches 0.
     */
    fun tick(now: Long, screenOn: Boolean): Step {
        if (late(now) || !screenOn) return Step.CANCEL
        left--
        return if (left <= 0) Step.PLACE else Step.COUNT
    }

    companion object {
        /** One key per lifeline button. */
        fun key(label: String, number: String, emergency: Boolean): String = "$emergency|$label|$number"

        /** What a button shows when it is built: still counting, if its press is. */
        fun buttonText(label: String, countdown: CallCountdown?): String = countdown?.text() ?: label

        const val CANCELLED = "Call cancelled."

        /** Between the numbers. */
        const val TICK_MS = 1_000L

        /**
         * The status line once a countdown is cancelled. A sticky line already
         * showing (the advice an earlier emergency press left) stays: cancelling
         * a second press never wipes it. Otherwise, "Call cancelled.".
         */
        fun lineAfterCancel(stickyText: String): String = stickyText.ifEmpty { CANCELLED }
    }
}

/**
 * Every hold-to-call press in its 3-2-1 window, held by the shade rather than
 * its buttons (see [CallCountdown]). Pure: the clock and the shade are
 * injected, so it is host-testable. Single-threaded: every call comes from the
 * main thread.
 *
 * - **One at a time.** While a press is counting, no other button starts one
 *   ([start] refuses), so two countdowns never run at once, and a second one
 *   can never complete in the first one's window and abandon its press. (A
 *   press begun after a countdown has completed is a new press, as ever.)
 * - **Only in sight.** A countdown is called off, as if the ward had tapped it,
 *   when the screen goes off ([cancelAll]), when a tick runs late in real time
 *   or finds the screen off (see [CallCountdown.tick]), and when a render no
 *   longer builds its button ([keepOnly]), since no one could then cancel it.
 *   Nothing is reported for a press called off, and no route starts.
 * - **Every cancel says the same thing** ([Host.cancelled]): an existing
 *   sticky line stays, otherwise "Call cancelled." (see
 *   [CallCountdown.lineAfterCancel]).
 */
internal class CallCountdowns(
    private val timer: CallSequencer.Timer,
    private val host: Host,
) {
    interface Host {
        /** Writes [text] on the button of the current render for [key], if there is one. */
        fun paint(key: String, text: String)

        /** `PowerManager.isInteractive`; one that cannot be read is taken as on. */
        fun screenOn(): Boolean

        /** [c]'s window completed on time and in sight: report the press, then route it. */
        fun complete(c: CallCountdown)

        /** One or more countdowns were called off: the status line after a cancel. */
        fun cancelled()
    }

    private val running = mutableMapOf<String, CallCountdown>()

    /** The countdown of the button keyed [key], if it is counting. */
    operator fun get(key: String): CallCountdown? = running[key]

    /** Whether any press is counting. */
    fun any(): Boolean = running.isNotEmpty()

    /** Starts [c]'s window. Refused (false, and nothing changes) while another
     *  press is counting. */
    fun start(c: CallCountdown): Boolean {
        if (running.isNotEmpty()) return false
        running[c.key] = c
        host.paint(c.key, c.text())
        post(c)
        return true
    }

    /** A tap during the countdown: "no, stop". */
    fun cancel(key: String) = cancelWhere { it == key }

    /** The screen went off: the ward can neither see nor tap a countdown. */
    fun cancelAll() = cancelWhere { true }

    /** After a render: a countdown whose button was not rebuilt (it is no
     *  longer offered, or its number changed) can no longer be cancelled. */
    fun keepOnly(rebuilt: Set<String>) = cancelWhere { it !in rebuilt }

    /** The shade is going away: every tick is dropped, and nothing is said. */
    fun abandon() {
        running.values.forEach { c -> c.posted?.let { timer.cancel(it) } }
        running.clear()
    }

    private fun cancelWhere(match: (String) -> Boolean) {
        val gone = running.keys.filter(match)
        if (gone.isEmpty()) return
        for (key in gone) {
            val c = running.remove(key) ?: continue
            c.posted?.let { timer.cancel(it) }
            c.posted = null
            host.paint(key, c.label)
        }
        host.cancelled()
    }

    private fun post(c: CallCountdown) {
        c.dueAt = timer.now() + CallCountdown.TICK_MS
        val task = object : Runnable {
            override fun run() {
                // Cancelled, or replaced, too late to stop it running.
                if (running[c.key] !== c || c.posted !== this) return
                when (c.tick(timer.now(), host.screenOn())) {
                    CallCountdown.Step.CANCEL -> cancel(c.key)
                    CallCountdown.Step.COUNT -> {
                        host.paint(c.key, c.text())
                        post(c)
                    }
                    CallCountdown.Step.PLACE -> {
                        running.remove(c.key)
                        c.posted = null
                        host.paint(c.key, c.label)
                        host.complete(c)
                    }
                }
            }
        }
        c.posted = task
        timer.postDelayed(task, CallCountdown.TICK_MS)
    }
}
