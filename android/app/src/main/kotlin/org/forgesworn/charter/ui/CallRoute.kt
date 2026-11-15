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

    /** The platform's emergency dialler component when the framework will not
     *  say (AOSP: `TelecomManager.EMERGENCY_DIALER_COMPONENT`). */
    const val FALLBACK_EMERGENCY_DIALLER_PACKAGE = "com.android.phone"
    const val FALLBACK_EMERGENCY_DIALLER_CLASS = "com.android.phone.EmergencyDialer"

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
}

/**
 * Walks [CallRoute]'s routes for one press until one is confirmed, with every
 * collaborator injected so it is host-testable. Single-threaded: every call
 * comes from the main thread.
 *
 * A launch that returns is not yet a success: LockTask can refuse a launch
 * without throwing. For an emergency number a started launch is confirmed only
 * when the shade is paused (something else took the screen) within [confirmMs]
 * of it; otherwise the route is treated as refused and the next one runs. The
 * [CallRoute.Route.BOUNCER] route is asynchronous: the launcher reports how
 * the phone's own lock ended.
 */
class CallSequencer(
    private val isEmergency: Boolean,
    private var locked: Boolean,
    private var emergencyDiallerOk: Boolean,
    private val systemDialOk: Boolean,
    private val launcher: Launcher,
    private val timer: Timer,
    private val listener: Listener,
    private val confirmMs: Long = CONFIRM_MS,
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
    private var pausedSinceLaunch = false
    private var confirming: Runnable? = null

    fun begin() = next()

    /** The shade went to the background (called from `onPause`). */
    fun onPaused() {
        pausedSinceLaunch = true
        val waiting = confirming ?: return
        timer.cancel(waiting)
        confirming = null
        settle(Outcome.OPENED)
    }

    /** The shade is going away: nothing more is tried. */
    fun abandon() {
        settled = true
        confirming?.let { timer.cancel(it) }
        confirming = null
    }

    private fun settle(outcome: Outcome) {
        if (settled) return
        settled = true
        listener.onSettled(outcome)
    }

    private fun next() {
        if (settled) return
        val route = CallRoute.choose(isEmergency, locked, emergencyDiallerOk, systemDialOk, attempt)
        if (route == null) {
            settle(Outcome.FAILED)
            return
        }
        attempt++
        pausedSinceLaunch = false
        val mine = ++epoch
        listener.onRoute(route)
        launcher.launch(route) { result -> if (mine == epoch && !settled) onLaunched(result) }
    }

    private fun onLaunched(result: Launch) {
        when (result) {
            Launch.FAILED -> next()
            Launch.CANCELLED -> settle(Outcome.BOUNCER_CANCELLED)
            Launch.DISMISSED -> {
                // Unlocked now: the lock's own screen is spent, and so is the
                // emergency dialler if it was ever going to be tried.
                locked = false
                emergencyDiallerOk = false
                attempt = 0
                next()
            }
            Launch.STARTED -> when {
                !isEmergency || pausedSinceLaunch -> settle(Outcome.OPENED)
                else -> {
                    val check = Runnable {
                        confirming = null
                        next()
                    }
                    confirming = check
                    timer.postDelayed(check, confirmMs)
                }
            }
        }
    }

    companion object {
        /** How long a started emergency launch has to take the screen. */
        const val CONFIRM_MS = 1_500L
    }
}
