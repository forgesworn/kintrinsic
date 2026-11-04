package org.forgesworn.charter.ui

/**
 * How the shade starts a call without depending on whichever dialer is default.
 *
 * It used to send an implicit `ACTION_CALL`. Telecom answers that, but a
 * third-party dialer may declare the same filter, and then the intent resolves
 * to that app or to a chooser. Since the shade launches it into its own pinned
 * task, LockTask does not check it: the app opens over the lock, and the ward's
 * call now depends on it. So the routes are, in order:
 *
 *  - [Route.TELECOM]: `TelecomManager.placeCall`. A direct call to the system
 *    service, with no activity to resolve.
 *  - [Route.SYSTEM_DIAL_PAD]: the system dialer, named explicitly, with the
 *    number filled in.
 *  - [Route.ANY_CALL_HANDLER]: the old implicit intent, as the last resort. It
 *    is reached only when both of the others failed, or the device names no
 *    system dialer, and then getting a call out matters more than who answers
 *    the intent.
 *
 * An emergency call starts at the system dial pad. Telecom would send it there
 * in any case, since only the default or system dialer may place one directly
 * (a non-dialer's call to an emergency number is cancelled and the dial pad
 * opened instead). Going straight there means a failure is seen here and can
 * fall through, rather than being lost inside Telecom. The ward presses call on
 * the dial pad.
 */
internal object LockCall {

    enum class Route { TELECOM, SYSTEM_DIAL_PAD, ANY_CALL_HANDLER }

    fun routes(emergency: Boolean, systemDialer: String?): List<Route> {
        val ordered = if (emergency) {
            listOf(Route.SYSTEM_DIAL_PAD, Route.TELECOM, Route.ANY_CALL_HANDLER)
        } else {
            listOf(Route.TELECOM, Route.SYSTEM_DIAL_PAD, Route.ANY_CALL_HANDLER)
        }
        return if (systemDialer.isNullOrEmpty()) ordered - Route.SYSTEM_DIAL_PAD else ordered
    }

    /**
     * Tries each route until one starts, and returns it; null when none did.
     * Every failure moves on to the next route, so no single one can strand the
     * ward.
     */
    fun placeFirst(routes: List<Route>, attempt: (Route) -> Unit): Route? =
        routes.firstOrNull { route -> runCatching { attempt(route) }.isSuccess }
}
