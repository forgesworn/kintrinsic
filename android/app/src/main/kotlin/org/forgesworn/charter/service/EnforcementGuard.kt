package org.forgesworn.charter.service

/**
 * Runs each step of a tick on its own, so a step that throws can never stop
 * the steps after it — above all the lock surface (05-B1).
 *
 * Before this, `lock.show` was the LAST statement of `applyDecision`, behind
 * four unguarded platform calls. One `SecurityException`, a dead binder, or an
 * OEM refusing one restriction key aborted the tick before the lock, and did so
 * again on every tick, with only a logcat line to show for it. Every failure
 * here fails closed: the step is retried on the next tick (everything in the
 * warden is level-triggered), whatever it already left in force stays in
 * force, and the steps after it still run.
 *
 * A failing step is logged once when it starts failing and once when it
 * recovers, not on every 2 s tick. [failing] names the steps currently failing.
 */
internal class StepGuard(
    private val log: (message: String, error: Throwable?) -> Unit,
) {
    private val failingSteps = linkedSetOf<String>()

    /** The steps that failed on their last run, in the order they began failing. */
    val failing: Set<String> get() = failingSteps.toSet()

    /** Run [block] as [step]. True when it completed; false when it threw. */
    fun run(step: String, block: () -> Unit): Boolean = try {
        FaultInjection.check(step)
        block()
        if (failingSteps.remove(step)) log("enforcement step '$step' recovered", null)
        true
    } catch (t: Throwable) {
        if (failingSteps.add(step)) {
            log("enforcement step '$step' failed; the rest of the tick still runs and it is retried", t)
        }
        false
    }
}

/**
 * Debug-build fault injection for the on-metal liveness round. Nothing in a
 * release build sets [failing]: the only writer is the debug-only
 * `DebugReceiver` (source set `src/debug`), so in release [check] is a read of
 * an empty set.
 */
object FaultInjection {
    /** Step names to fail (see [StepGuard.run] call sites), or "all". */
    @Volatile var failing: Set<String> = emptySet()

    fun check(step: String) {
        val f = failing
        if (f.isNotEmpty() && (step in f || "all" in f)) {
            throw IllegalStateException("injected fault: $step")
        }
    }
}

/**
 * Whether the standing baseline is due a re-read (05-G2's `posture()`), paced
 * rather than every 2 s tick. A clock that went backwards counts as due, so a
 * clock change can never postpone the check.
 */
internal fun postureDue(nowUnix: Long, lastUnix: Long, intervalSecs: Long): Boolean =
    lastUnix == 0L || nowUnix < lastUnix || nowUnix - lastUnix >= intervalSecs

/**
 * Whether the DNS filter must be (re)dispatched this tick. A new plan revision
 * always is. So is a filter that is pinned but not actually running (05-B7:
 * the always-on setting still reads pinned while the tunnel is down, so DNS
 * flows unfiltered), paced by [redispatchSecs] so a tunnel the platform will
 * not establish is not hammered every tick.
 */
internal fun dnsDispatchWanted(
    revision: String,
    appliedRevision: String?,
    tunnelLive: Boolean,
    nowUnix: Long,
    lastRedispatchUnix: Long,
    redispatchSecs: Long,
): Boolean {
    if (revision != appliedRevision) return true
    if (tunnelLive) return false
    return postureDue(nowUnix, lastRedispatchUnix, redispatchSecs)
}
