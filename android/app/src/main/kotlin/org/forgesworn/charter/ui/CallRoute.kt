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
}

/**
 * Walks [CallRoute]'s routes for one press until one is confirmed, with every
 * collaborator injected so it is host-testable. Single-threaded: every call
 * comes from the main thread, and the activity feeds it the shade's lifecycle
 * through [onPaused], [onResumed], [onStopped] and [onFocusChanged].
 *
 * A launch that returns is not yet a success: LockTask can refuse a launch
 * without throwing, and Telecom's transparent trampoline pauses the shade for
 * a moment and hands it straight back. For an emergency number a started
 * launch is confirmed only when the shade is STOPPED (an opaque dialler took
 * the screen, while the screen is on). The wait has two stages:
 *
 * - no pause within [pauseMs] of the launch: the route was refused, try the
 *   next one;
 * - once a pause is seen, wait for the stop until [stopMs] in total from the
 *   launch. A pause followed by a resume, with no stop, is not a confirmation
 *   (a trampoline), and when the time runs out the next route is tried.
 *
 * A stop is remembered across routes: a dialler that took the screen late (a
 * loaded phone) settles the press as opened instead of being followed by
 * another route, and [CallRoute.Route.CALL], which targets the shade's own
 * task, is never started once a stop has been seen. The
 * [CallRoute.Route.BOUNCER] route is asynchronous: the launcher reports how
 * the phone's own lock ended, and if the shade's window has not lost focus
 * within [pauseMs] the lock screen never appeared and the route has failed.
 */
class CallSequencer(
    private val isEmergency: Boolean,
    private var locked: Boolean,
    private var emergencyDiallerOk: Boolean,
    private val systemDialOk: Boolean,
    private val launcher: Launcher,
    private val timer: Timer,
    private val listener: Listener,
    private val pauseMs: Long = PAUSE_MS,
    private val stopMs: Long = STOP_MS,
) {
    /** How one route's launch went. */
    enum class Launch {
        /** `startActivity` returned (not yet known to have taken the screen). */
        STARTED,
        /** It threw, or could not be made. */
        FAILED,
        /** The phone's own lock was dismissed (no credential was needed, or it was entered). */
        DISMISSED,
        /** The phone's own lock went away unlocked (cancelled, or left for its own emergency screen). */
        CANCELLED,
    }

    /** How the press ended. */
    enum class Outcome { OPENED, BOUNCER_CANCELLED, FAILED }

    fun interface Launcher {
        fun launch(route: CallRoute.Route, result: (Launch) -> Unit)
    }

    interface Timer {
        fun postDelayed(task: Runnable, delayMs: Long)
        fun cancel(task: Runnable)

        /** A monotonic clock in milliseconds. */
        fun now(): Long
    }

    interface Listener {
        /** About to try [route]. */
        fun onRoute(route: CallRoute.Route)

        /** Settled, once. */
        fun onSettled(outcome: Outcome)
    }

    private var attempt = 0
    private var epoch = 0
    private var settled = false
    private var launchedAt = 0L
    private var pausedSinceLaunch = false
    private var focusLostSinceLaunch = false

    /** A stop (screen on) has been seen since the first launch, on any route. */
    private var stopSeen = false
    private var waitingFor: Wait = Wait.NOTHING
    private var pending: Runnable? = null

    private enum class Wait { NOTHING, PAUSE, STOP, BOUNCER_FOCUS }

    fun begin() = next()

    /** The shade is paused (called from `onPause`). */
    fun onPaused() {
        if (settled) return
        pausedSinceLaunch = true
        if (waitingFor == Wait.PAUSE) waitForStop()
    }

    /** The shade is back in the foreground (called from `onResume`). A pause
     *  followed by a resume, with no stop, confirms nothing. */
    fun onResumed() {
        if (settled) return
        pausedSinceLaunch = false
    }

    /**
     * The shade is stopped (called from `onStop`), [screenOn] being
     * `PowerManager.isInteractive`. A stop with the screen off is the screen
     * going off, not a dialler: it confirms nothing and the sequence carries on.
     */
    fun onStopped(screenOn: Boolean) {
        if (settled || !screenOn) return
        stopSeen = true
        if (waitingFor == Wait.PAUSE || waitingFor == Wait.STOP) {
            cancelPending()
            settle(Outcome.OPENED)
        }
    }

    /** The shade's window gained or lost focus (called from `onWindowFocusChanged`). */
    fun onFocusChanged(hasFocus: Boolean) {
        if (settled || hasFocus) return
        focusLostSinceLaunch = true
        // The phone's own lock took the focus: it is showing, and the launcher
        // now reports how it ends.
        if (waitingFor == Wait.BOUNCER_FOCUS) cancelPending()
    }

    /** The shade is going away: nothing more is tried. */
    fun abandon() {
        settled = true
        cancelPending()
    }

    private fun cancelPending() {
        pending?.let { timer.cancel(it) }
        pending = null
        waitingFor = Wait.NOTHING
    }

    private fun arm(wait: Wait, delayMs: Long, onFire: () -> Unit) {
        cancelPending()
        val task = Runnable {
            pending = null
            waitingFor = Wait.NOTHING
            if (!settled) onFire()
        }
        pending = task
        waitingFor = wait
        timer.postDelayed(task, delayMs.coerceAtLeast(0L))
    }

    private fun waitForStop() {
        arm(Wait.STOP, launchedAt + stopMs - timer.now()) { advance() }
    }

    private fun settle(outcome: Outcome) {
        if (settled) return
        settled = true
        cancelPending()
        listener.onSettled(outcome)
    }

    /** The current route did not take the screen: the next one runs, unless a
     *  stop has been seen, in which case something did and the press is opened. */
    private fun advance() {
        if (stopSeen) settle(Outcome.OPENED) else next()
    }

    private fun next() {
        if (settled) return
        cancelPending()
        val route = CallRoute.choose(isEmergency, locked, emergencyDiallerOk, systemDialOk, attempt)
        if (route == null) {
            settle(Outcome.FAILED)
            return
        }
        // ACTION_CALL targets the shade's own task: after a stop it would pull
        // the shade back over whatever took the screen.
        if (route == CallRoute.Route.CALL && isEmergency && stopSeen) {
            settle(Outcome.OPENED)
            return
        }
        attempt++
        pausedSinceLaunch = false
        focusLostSinceLaunch = false
        val mine = ++epoch
        listener.onRoute(route)
        if (route == CallRoute.Route.BOUNCER) {
            arm(Wait.BOUNCER_FOCUS, pauseMs) {
                // The shade kept the focus: the phone's own lock never showed.
                if (!focusLostSinceLaunch) {
                    epoch++ // a late answer from the lock screen is ignored
                    advance()
                }
            }
        }
        launcher.launch(route) { result -> if (mine == epoch && !settled) onLaunched(result) }
    }

    private fun onLaunched(result: Launch) {
        cancelPending()
        when (result) {
            Launch.FAILED -> advance()
            Launch.CANCELLED -> if (stopSeen) settle(Outcome.OPENED) else settle(Outcome.BOUNCER_CANCELLED)
            Launch.DISMISSED -> {
                // Unlocked now: the lock's own screen is spent, and so is the
                // emergency dialler if it was ever going to be tried. The
                // lock screen's own stop is spent with it.
                locked = false
                emergencyDiallerOk = false
                attempt = 0
                stopSeen = false
                next()
            }
            Launch.STARTED -> when {
                !isEmergency -> settle(Outcome.OPENED)
                stopSeen -> settle(Outcome.OPENED)
                else -> {
                    launchedAt = timer.now()
                    if (pausedSinceLaunch) {
                        waitForStop()
                    } else {
                        arm(Wait.PAUSE, pauseMs) { advance() }
                    }
                }
            }
        }
    }

    companion object {
        /** How long a started emergency launch has to pause the shade. */
        const val PAUSE_MS = 1_500L

        /** How long, in total from the launch, a paused shade has to stop. */
        const val STOP_MS = 3_000L
    }
}
