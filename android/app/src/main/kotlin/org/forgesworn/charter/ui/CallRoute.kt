package org.forgesworn.charter.ui

/**
 * Which intent a hold-to-call button on the lock shade starts. Pure, so the
 * choice is host-testable; [LockActivity] resolves the intents and builds them.
 *
 * Whatever the route, a call to an emergency number counts as an emergency use
 * (the activity row and the guardian's report), as it is the number that makes
 * it one; the shade's own control additionally requires its own flag (see
 * `EmergencyUse.placeAndRecord`'s `plan.emergency && emergency`).
 *
 * - A guardian's number (not an emergency number): [Route.CALL] (ACTION_CALL),
 *   which places the call straight away. Unchanged.
 * - An emergency number (the shade's Emergency button, or a lifeline entry the
 *   platform counts as one): the platform will not let an app that is not the
 *   dialer place it. Telecom turns ACTION_CALL into the system dialler's
 *   keypad, and during a lockout that launch is refused by LockTask, so with the
 *   phone's own lock NOT showing (no PIN, or already unlocked) the button did
 *   nothing. So prefer the platform's emergency dialler whenever it resolves
 *   (it shows over the keyguard and LockTask lets it through, and nothing is
 *   dialled until the ward presses call), then ACTION_DIAL with the number
 *   (the system dialler is on the lockout allowlist), and only then ACTION_CALL.
 */
object CallRoute {
    enum class Route { EMERGENCY_DIALLER, SYSTEM_DIAL, CALL }

    fun choose(
        isEmergency: Boolean,
        emergencyDiallerResolves: Boolean,
        systemDialResolves: Boolean,
    ): Route = when {
        !isEmergency -> Route.CALL
        emergencyDiallerResolves -> Route.EMERGENCY_DIALLER
        systemDialResolves -> Route.SYSTEM_DIAL
        else -> Route.CALL
    }
}
