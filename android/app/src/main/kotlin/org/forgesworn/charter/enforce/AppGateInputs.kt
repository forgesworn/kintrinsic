package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore

/**
 * The per-app inputs to one app-gate reconcile, after [AppGateMemory] has
 * resolved any that could not be read (F1).
 *
 * [degraded] means at least one input failed this tick: the reconcile must
 * then only ADD suspensions ([AppGateOps.reconcile]'s `suspendOnly`), the hidden
 * set must be left exactly as it stands, and nothing may tell the ward an app
 * opened, because nothing did.
 */
data class AppGateInputs(
    val appPolicy: CharterCore.AppPolicy?,
    val ruleSuspensions: Set<String>,
    val bucketSuspensions: Set<String>,
    val degraded: Boolean,
    /** The standing policy was read fresh this tick, so `hidden` may be applied. */
    val appPolicyFresh: Boolean,
)

/**
 * F1: the last per-app inputs that were READ successfully in this process.
 *
 * The core answers "none" and "could not read" differently now, and this is
 * where the difference is honoured. A failed read never becomes "no policy" or
 * "no suspensions" — that is what lifted every block and unhid every app for a
 * tick. It falls back to the last good answer instead, and the tick runs
 * degraded (suspend-only). Before any good answer at all this process there is
 * nothing to fall back to: the input contributes nothing, and suspend-only
 * leaves the device exactly as it was, which is what the app does before its
 * first pairing too (it never touches per-app state then).
 *
 * Pure and single-threaded (the enforcement tick's own worker), so it is
 * host-testable without a device or a native core.
 */
class AppGateMemory {
    private var policy: CharterCore.AppPolicy? = null
    private var policyKnown = false
    private var rules: Set<String>? = null
    private var buckets: Set<String>? = null
    private var wasDegraded = false

    /** Whether the last [resolve] flipped degraded ↔ healthy — the caller logs
     *  on that edge only, never once a tick. */
    var transitioned: Boolean = false
        private set

    fun resolve(
        appPolicy: Result<CharterCore.AppPolicy?>,
        ruleSuspensions: Result<List<String>>,
        bucketSuspensions: Result<List<String>>,
    ): AppGateInputs {
        appPolicy.onSuccess {
            policy = it
            policyKnown = true
        }
        ruleSuspensions.onSuccess { rules = it.toSet() }
        bucketSuspensions.onSuccess { buckets = it.toSet() }
        val degraded =
            appPolicy.isFailure || ruleSuspensions.isFailure || bucketSuspensions.isFailure
        transitioned = degraded != wasDegraded
        wasDegraded = degraded
        return AppGateInputs(
            appPolicy = if (policyKnown) policy else null,
            ruleSuspensions = rules.orEmpty(),
            bucketSuspensions = buckets.orEmpty(),
            degraded = degraded,
            appPolicyFresh = appPolicy.isSuccess,
        )
    }
}
