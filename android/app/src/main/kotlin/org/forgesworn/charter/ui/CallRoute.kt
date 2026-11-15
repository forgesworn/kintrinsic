package org.forgesworn.charter.ui

/**
 * Which routes a hold-to-call button on the lock shade tries, and in what
 * order. Pure, so the choice is host-testable; [LockActivity] builds and
 * launches the intents, and [CallSequencer] walks the routes.
 *
 * Whatever the route, a press of the shade's own emergency control is reported
 * to the guardian once its hold and cancel window complete, before any route
 * is tried (see `EmergencyUse.pressed`); a route that is refused changes
 * nothing about that.
 *
 * - A guardian's number (not an emergency number): [Route.CALL] (ACTION_CALL),
 *   which places the call straight away. Unchanged, and the only route.
 * - An emergency number (the shade's Emergency button, or a lifeline entry the
 *   platform counts as one): the platform will not let an app that is not the
 *   dialer place it. Telecom turns ACTION_CALL into the system dialler's
 *   keypad, and during a lockout that launch can be refused by LockTask. So the
 *   order is:
 *   1. [Route.EMERGENCY_DIALLER], when one resolved from the system image and
 *      LockTask will let it through (see [emergencyDiallerUsable]);
 *   2. with the phone's own lock showing, [Route.BOUNCER]: that lock's own
 *      screen, which carries an Emergency call button of its own;
 *   3. with it not showing, [Route.SYSTEM_DIAL] (ACTION_DIAL with the number),
 *      when the system dialler resolves;
 *   4. [Route.CALL] (ACTION_CALL), last.
 *   A route that throws or is refused falls through to the next.
 */
object CallRoute {
    enum class Route { EMERGENCY_DIALLER, BOUNCER, SYSTEM_DIAL, CALL }

    /** Every route to try, in order. */
    fun sequence(
        isEmergency: Boolean,
        locked: Boolean,
        emergencyDiallerOk: Boolean,
        systemDialOk: Boolean,
    ): List<Route> {
        if (!isEmergency) return listOf(Route.CALL)
        return buildList {
            if (emergencyDiallerOk) add(Route.EMERGENCY_DIALLER)
            if (locked) add(Route.BOUNCER) else if (systemDialOk) add(Route.SYSTEM_DIAL)
            add(Route.CALL)
        }
    }

    /** The route for attempt number [attempt] (0 = the first), or null once
     *  every route has been tried. */
    fun choose(
        isEmergency: Boolean,
        locked: Boolean,
        emergencyDiallerOk: Boolean,
        systemDialOk: Boolean,
        attempt: Int,
    ): Route? = sequence(isEmergency, locked, emergencyDiallerOk, systemDialOk).getOrNull(attempt)

    /** The platform's emergency dialler component (AOSP's `com.android.phone/.EmergencyDialer`,
     *  the one LockTask itself exempts). The framework has no public constant for it. */
    const val PLATFORM_EMERGENCY_DIALLER_PACKAGE = "com.android.phone"
    const val PLATFORM_EMERGENCY_DIALLER_CLASS = "com.android.phone.EmergencyDialer"

    /**
     * Whether an emergency dialler that resolved (from the system image only)
     * is one LockTask will let a lockout launch: it is the platform's own
     * emergency dialler component, or its package is the system or default
     * dialler's, or it is on the LockTask allowlist ([lockTaskPermitted]).
     * Anything else, such as an OEM's own screen, a chooser (package `android`)
     * or a component not resolved at all, is skipped, so the next route runs.
     */
    fun emergencyDiallerUsable(
        resolvedPackage: String?,
        resolvedClass: String?,
        platformPackage: String?,
        platformClass: String?,
        dialerPackages: Collection<String?>,
        lockTaskPermitted: Boolean,
    ): Boolean {
        if (resolvedPackage.isNullOrEmpty() || resolvedClass.isNullOrEmpty()) return false
        if (resolvedPackage == platformPackage && resolvedClass == platformClass) return true
        if (dialerPackages.any { it != null && it == resolvedPackage }) return true
        return lockTaskPermitted
    }

    /**
     * Which of the system-image activities that answer the emergency dialler
     * action to launch, as (package, class), or null when none qualifies. The
     * platform's own component wins whenever it is among them; otherwise the
     * first candidate [emergencyDiallerUsable] accepts. A phone on which two
     * system packages claim the action therefore still gets its emergency
     * dialler, instead of a chooser (package `android`) that would make the
     * route skip.
     */
    fun pickEmergencyDialler(
        candidates: List<Pair<String, String>>,
        platformPackage: String?,
        platformClass: String?,
        dialerPackages: Collection<String?>,
        lockTaskPermitted: (String) -> Boolean,
    ): Pair<String, String>? {
        val platform = candidates.firstOrNull { it.first == platformPackage && it.second == platformClass }
        if (platform != null) return platform
        return candidates.firstOrNull { (pkg, cls) ->
            emergencyDiallerUsable(pkg, cls, platformPackage, platformClass, dialerPackages, lockTaskPermitted(pkg))
        }
    }

    /** What a started intent route's one check makes of the shade. */
    enum class Check {
        /** The shade is clear: this route showed nothing, so the next one runs. */
        NEXT_ROUTE,

        /** Covered, but by the screen going off or a ringing call: not a dialler. */
        FAILED,

        /** Covered with the screen on and no call ringing: something opened. */
        OPENED,
    }

    /**
     * The check [CallSequencer.CHECK_MS] after an intent route's
     * `startActivity`. [clear] is the shade resumed, focused and on screen;
     * [screenOn] is `PowerManager.isInteractive`; [ringing] is the call state.
     * Detection only decides whether to fall through: the ward is shown what
     * to do whichever way it goes (see [settledLine]).
     */
    fun check(clear: Boolean, screenOn: Boolean, ringing: Boolean): Check = when {
        clear -> Check.NEXT_ROUTE
        !screenOn -> Check.FAILED
        ringing -> Check.FAILED
        else -> Check.OPENED
    }

    /**
     * The status line under the lifeline buttons. Every line but [BLANK] is
     * sticky: it stays until the next press, or the end of the lockout.
     */
    enum class Line(val text: String) {
        BLANK(""),

        /** From the start of an emergency press's routing, whatever opens. */
        ADVICE(
            "If you still need help, press the power button five times for Emergency SOS, " +
                "or use another phone.",
        ),

        /** While the phone's own lock is up. */
        BOUNCER("Tap Emergency call on the next screen"),

        /** Every route was tried and nothing covered the shade. */
        NOTHING_OPENED(
            "Couldn't open the emergency dialler. Press the power button five times " +
                "for Emergency SOS, or use another phone.",
        ),

        /** A guardian's number whose call could not be started. */
        CALL_FAILED("Couldn't start the call — tell your guardian."),
    }

    /**
     * The line a press settles on. An emergency press keeps [Line.ADVICE]
     * through OPENED, CANCELLED and a FAILED with the shade covered (something
     * may be up, or the screen is off), and only a FAILED with the shade clear
     * says the dialler could not be opened. A guardian's number says nothing
     * unless its call could not be started. Nothing ever says a call started.
     */
    fun settledLine(isEmergency: Boolean, outcome: CallSequencer.Outcome, shadeClear: Boolean): Line = when {
        !isEmergency -> if (outcome == CallSequencer.Outcome.FAILED) Line.CALL_FAILED else Line.BLANK
        outcome == CallSequencer.Outcome.FAILED && shadeClear -> Line.NOTHING_OPENED
        else -> Line.ADVICE
    }
}

/**
 * Walks [CallRoute]'s routes for one press, judging each by the shade's STATE
 * at fixed points rather than by lifecycle edges. Every collaborator is
 * injected, so it is host-testable, and it holds no Android types.
 * Single-threaded: every call comes from the main thread.
 *
 * The shade is CLEAR when it is resumed, has the window focus and the screen is
 * on ([Shade.clear]); the activity calls [onClear] whenever it may have become
 * clear (from `onResume` and `onWindowFocusChanged(true)`).
 *
 * - **A route launches only from a clear shade.** When one is due and the
 *   shade is not clear, it waits for [onClear], bounded at [CLEAR_WAIT_MS]; at
 *   the bound the press settles FAILED and nothing is launched. ACTION_CALL,
 *   which targets the shade's own task, is therefore never started over
 *   whatever is covering the shade.
 * - **An intent route** (the emergency dialler, ACTION_DIAL, ACTION_CALL) is
 *   checked once, [CHECK_MS] after `startActivity` (see [CallRoute.check]):
 *   clear means it showed nothing and the next route runs; covered with the
 *   screen off, or by a ringing call, settles FAILED; otherwise OPENED. A
 *   launch that throws runs the next route at once.
 * - **The phone's own lock** ([CallRoute.Route.BOUNCER]) is requested from a
 *   clear shade. Dismissed (now unlocked) or an error: the next route, once
 *   clear. Cancelled: settles CANCELLED. Still clear [CHECK_MS] after the
 *   request: it never appeared, and the next route runs (a late answer is
 *   ignored). No answer at all within [BOUNCER_WAIT_MS]: FAILED.
 *
 * The status line does not depend on any of this being right: an emergency
 * press shows [CallRoute.Line.ADVICE] from the start, and only a FAILED with
 * the shade clear says nothing opened (see [CallRoute.settledLine]). A
 * guardian's number is one ACTION_CALL, launched from a clear shade like every
 * route and settled when it starts.
 *
 * One settlement per press. Settlement, a new press (the activity abandons the
 * old sequencer) and [abandon] invalidate every timer and every late answer.
 */
class CallSequencer(
    private val isEmergency: Boolean,
    private var locked: Boolean,
    private var emergencyDiallerOk: Boolean,
    private val systemDialOk: Boolean,
    private val shade: Shade,
    private val launcher: Launcher,
    private val timer: Timer,
    private val listener: Listener,
) {
    /** How one route's launch went. */
    enum class Launch {
        /** `startActivity` returned (not yet known to have taken the screen). */
        STARTED,

        /** It threw or could not be made; for the phone's own lock, it could
         *  not be requested or answered with an error. */
        FAILED,

        /** The phone's own lock was dismissed (no credential was needed, or it was entered). */
        DISMISSED,

        /** The phone's own lock went away unlocked (cancelled, or left for its own emergency screen). */
        CANCELLED,
    }

    /** How the press ended. */
    enum class Outcome { OPENED, CANCELLED, FAILED }

    /** The shade's state, read when it matters, never remembered. */
    interface Shade {
        /** Resumed, holding the window focus, and the screen on. */
        fun clear(): Boolean

        /** `PowerManager.isInteractive`. */
        fun screenOn(): Boolean

        /** The phone's call state is ringing. */
        fun ringing(): Boolean
    }

    fun interface Launcher {
        fun launch(route: CallRoute.Route, result: (Launch) -> Unit)
    }

    interface Timer {
        fun postDelayed(task: Runnable, delayMs: Long)
        fun cancel(task: Runnable)
    }

    interface Listener {
        /** About to try [route]. */
        fun onRoute(route: CallRoute.Route)

        /** The status line changed to [line]. */
        fun onLine(line: CallRoute.Line)

        /** Settled, once. */
        fun onSettled(outcome: Outcome)
    }

    private enum class State {
        /** Not begun. */
        IDLE,

        /** A route is due and the shade is not clear. */
        WAITING_FOR_CLEAR,

        /** An intent route was started; its check is pending. */
        CHECKING,

        /** The phone's own lock was requested; its answer is pending. */
        BOUNCER,

        /** Settled or abandoned: nothing more happens. */
        SETTLED,
    }

    private var state = State.IDLE
    private var attempt = 0

    /** Bumped by every launch and every answer taken: an answer from a
     *  launch that is no longer current is ignored. */
    private var token = 0
    private var pending: Runnable? = null
    private var line: CallRoute.Line? = null

    fun begin() {
        if (state != State.IDLE) return
        show(if (isEmergency) CallRoute.Line.ADVICE else CallRoute.Line.BLANK)
        due()
    }

    /** The shade may have become clear (from `onResume` and
     *  `onWindowFocusChanged(true)`): a route waiting for it runs now. */
    fun onClear() {
        if (state == State.WAITING_FOR_CLEAR && shade.clear()) due()
    }

    /** The shade is going away, or a new press replaces this one: nothing
     *  more is tried and nothing more is said. */
    fun abandon() {
        state = State.SETTLED
        token++
        cancelPending()
    }

    private fun show(next: CallRoute.Line) {
        if (next == line) return
        line = next
        listener.onLine(next)
    }

    private fun cancelPending() {
        pending?.let { timer.cancel(it) }
        pending = null
    }

    private fun arm(delayMs: Long, onFire: () -> Unit) {
        cancelPending()
        val task = object : Runnable {
            override fun run() {
                // A task cancelled too late to stop it running still does nothing.
                if (pending !== this || state == State.SETTLED) return
                pending = null
                onFire()
            }
        }
        pending = task
        timer.postDelayed(task, delayMs)
    }

    private fun settle(outcome: Outcome) {
        if (state == State.SETTLED) return
        state = State.SETTLED
        token++
        cancelPending()
        show(CallRoute.settledLine(isEmergency, outcome, shade.clear()))
        listener.onSettled(outcome)
    }

    /** The next route is due: launched now if the shade is clear, otherwise
     *  once it is, within [CLEAR_WAIT_MS]. None left: FAILED. */
    private fun due() {
        if (state == State.SETTLED) return
        val route = CallRoute.choose(isEmergency, locked, emergencyDiallerOk, systemDialOk, attempt)
        if (route == null) {
            settle(Outcome.FAILED)
            return
        }
        if (shade.clear()) {
            cancelPending()
            launch(route)
        } else if (state != State.WAITING_FOR_CLEAR) {
            // The bound runs from when the route fell due, not from the last
            // time the shade looked.
            state = State.WAITING_FOR_CLEAR
            arm(CLEAR_WAIT_MS) { settle(Outcome.FAILED) }
        }
    }

    private fun launch(route: CallRoute.Route) {
        attempt++
        val mine = ++token
        listener.onRoute(route)
        if (route == CallRoute.Route.BOUNCER) {
            state = State.BOUNCER
            show(CallRoute.Line.BOUNCER)
            // Armed before the request, so an answer given at once cancels it.
            arm(CHECK_MS) { bouncerCheck() }
        } else {
            state = State.CHECKING
        }
        launcher.launch(route) { result ->
            if (mine == token && state != State.SETTLED) answered(route, result)
        }
    }

    private fun answered(route: CallRoute.Route, result: Launch) {
        if (route == CallRoute.Route.BOUNCER) {
            if (result == Launch.STARTED) return // not an answer the lock gives
            token++
            cancelPending()
            when (result) {
                Launch.CANCELLED -> settle(Outcome.CANCELLED)
                Launch.DISMISSED -> {
                    // Unlocked now: the lock's own screen is spent, and so is the
                    // emergency dialler if it was ever going to be tried. The
                    // unlocked routes run, once the shade is clear.
                    locked = false
                    emergencyDiallerOk = false
                    attempt = 0
                    show(CallRoute.Line.ADVICE)
                    due()
                }
                else -> {
                    show(CallRoute.Line.ADVICE)
                    due()
                }
            }
            return
        }
        token++
        when (result) {
            Launch.STARTED -> if (isEmergency) {
                arm(CHECK_MS) { intentCheck() }
            } else {
                settle(Outcome.OPENED)
            }
            // A throw, or nothing to start: the next route at once.
            else -> if (isEmergency) due() else settle(Outcome.FAILED)
        }
    }

    private fun intentCheck() {
        when (CallRoute.check(shade.clear(), shade.screenOn(), shade.ringing())) {
            CallRoute.Check.NEXT_ROUTE -> due()
            CallRoute.Check.FAILED -> settle(Outcome.FAILED)
            CallRoute.Check.OPENED -> settle(Outcome.OPENED)
        }
    }

    private fun bouncerCheck() {
        if (shade.clear()) {
            // The phone's own lock never covered the shade: its late answer is
            // ignored, and the next route runs.
            token++
            show(CallRoute.Line.ADVICE)
            due()
        } else {
            arm(BOUNCER_WAIT_MS - CHECK_MS) { settle(Outcome.FAILED) }
        }
    }

    companion object {
        /** How long after an intent route's launch, or the lock's request, the shade is checked. */
        const val CHECK_MS = 2_500L

        /** How long a due route waits for a clear shade before the press settles FAILED. */
        const val CLEAR_WAIT_MS = 3_000L

        /** How long, from the request, the phone's own lock has to answer. */
        const val BOUNCER_WAIT_MS = 60_000L
    }
}
