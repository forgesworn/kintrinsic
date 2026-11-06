package org.forgesworn.charter.service

import org.forgesworn.charter.native.CharterCore

/**
 * Finds the next moment the ward's rules change, so the stretched "Longer
 * battery" alarm can be pulled forward to land on it.
 *
 * Rather than re-implement every rule's calendar in Kotlin, this asks the core
 * what it WOULD enforce a minute, two minutes... five minutes from now (the
 * core's evaluators all take the time as an argument) and looks for the first
 * answer that differs from today's. A change found is then narrowed to a couple
 * of seconds by halving. Anything that moves the lock, the named time, the app
 * lists, the per-app rules or buckets, tethering or an install window is
 * caught this way, with no list of rule types to keep in step.
 *
 * Two things a "what would it be later" question cannot see are read directly:
 * the seconds left on an out-of-hours extension (it burns wall-clock), the
 * end of a debugging window, and the end of a listening grace.
 */
internal object LivenessHorizon {
    /** How far ahead to look: the stretched alarm period. */
    const val HORIZON_SECS = LivenessPolicy.LONGER_BATTERY_ALARM_MS / 1000
    /** Probe spacing; a change is then narrowed down inside one step. */
    const val STEP_SECS = 60L
    /** Narrowing stops at this width, so a boundary is met to within it. */
    const val RESOLUTION_SECS = 2L

    /**
     * Seconds from now until [at] first returns something different from
     * `at(0)`, within [horizonSecs]; null if it never does. Probes every
     * [stepSecs], then halves the step that changed down to [resolutionSecs].
     * The result is an instant at which the answer HAS changed, so an alarm
     * set for it is never early. Pure: [at] is the whole world.
     */
    fun <T> firstChangeSecs(
        horizonSecs: Long,
        stepSecs: Long,
        resolutionSecs: Long,
        at: (offsetSecs: Long) -> T,
    ): Long? {
        val base = at(0)
        var lo = 0L
        var t = stepSecs
        while (t <= horizonSecs) {
            if (at(t) != base) {
                var hi = t
                while (hi - lo > resolutionSecs) {
                    val mid = (lo + hi) / 2
                    if (at(mid) != base) hi = mid else lo = mid
                }
                return hi
            }
            lo = t
            t += stepSecs
        }
        return null
    }

    /** What the core would enforce at one instant, reduced to what can change a decision. */
    internal data class Snapshot(
        val known: Boolean,
        val locked: Boolean,
        val reason: String,
        val activeTime: String?,
        val appPolicy: CharterCore.AppPolicy?,
        val appRules: List<String>,
        val buckets: List<String>,
        val tethering: String,
        val maintenanceOpen: Boolean,
        /** Apps the family agreed are open at any hour; an entry can expire. */
        val alwaysOpen: List<String> = emptyList(),
    )

    internal fun snapshotAt(unix: Long): Snapshot {
        val tl = CharterCore.timeLeft(null, unix)
        return Snapshot(
            known = tl.known,
            locked = tl.locked,
            reason = tl.reason,
            activeTime = tl.activeTime,
            appPolicy = CharterCore.appPolicy(unix),
            appRules = CharterCore.appRuleSuspensions(unix).sorted(),
            buckets = CharterCore.bucketSuspensions(unix).sorted(),
            tethering = CharterCore.tetheringMode(unix),
            maintenanceOpen = CharterCore.maintenanceOpen(unix),
            alwaysOpen = CharterCore.alwaysAvailable(unix, tl.locked, tl.reason).sorted(),
        )
    }

    /**
     * Seconds from [nowUnix] until the earliest known change within the
     * horizon, or null when none is in reach. Throws if the core cannot be
     * read; the caller then keeps the minute cadence.
     */
    fun nextChangeSecs(nowUnix: Long, listeningSecsLeft: Long? = null): Long? {
        val probe = firstChangeSecs(HORIZON_SECS, STEP_SECS, RESOLUTION_SECS) { snapshotAt(nowUnix + it) }
        val tl = CharterCore.timeLeft(null, nowUnix)
        val dbg = CharterCore.debuggingUntil(nowUnix)
        return earliest(
            probe = probe,
            scheduleSecs = tl.scheduleSecs.takeIf { tl.known && !tl.locked },
            debuggingSecs = (dbg - nowUnix).takeIf { dbg > nowUnix },
            listeningSecs = listeningSecsLeft,
        )
    }

    /**
     * The earliest of the boundaries, in seconds from now, within the horizon.
     * Pure. [probe] is already an instant the answer HAS changed. The others
     * are "this many seconds are left", so the change is one second after:
     *  - [scheduleSecs]: the schedule's own allowance (window close, or the
     *    out-of-hours extension running out). NOT the effective figure, which
     *    folds in the screen-time budget: that does not burn while the screen
     *    is dark, so using it would wake the phone every few seconds all night
     *    once a budget ran low. -1 or null = none.
     *  - [debuggingSecs]: a debugging window's end.
     *  - [listeningSecs]: the listening grace running out, when the music that
     *    was allowed to finish must stop. The core cannot be probed for it.
     */
    fun earliest(probe: Long?, scheduleSecs: Long?, debuggingSecs: Long?, listeningSecs: Long?): Long? {
        val candidates = listOfNotNull(
            probe,
            scheduleSecs?.takeIf { it in 0..HORIZON_SECS }?.plus(1),
            debuggingSecs?.takeIf { it in 0..HORIZON_SECS }?.plus(1),
            listeningSecs?.takeIf { it in 0..HORIZON_SECS }?.plus(1),
        )
        return candidates.minOrNull()
    }
}
