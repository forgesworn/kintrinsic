package org.forgesworn.charter.enforce

import android.app.admin.DevicePolicyManager
import android.content.ComponentName
import android.content.Context
import android.content.pm.PackageManager
import android.os.UserManager
import android.util.Log

/**
 * The Android enforce half (port-spec §3.5). Each capability is an interface
 * with a DevicePolicyManager-backed impl and a fake for JVM tests — the
 * mock/real discipline carried to Kotlin.
 */

/** Suspend/unsuspend the ward's app surface — the budget/schedule enactor +
 *  the standing per-app policy (D3). */
interface AppGateOps {
    /**
     * Level-triggered reconcile (I12). The suspend set is the UNION of every
     * source: when [locked] (time's up / outside hours) the whole launchable
     * surface except the deny-list; PLUS the standing per-app policy's set (a
     * blocklist's blocked apps, or everything outside an allowlist); PLUS the
     * schedule-driven per-app [ruleSuspensions] (apps the `appRules` clause
     * blocks right now); PLUS the named-times [bucketSuspensions] (apps whose
     * OWN bucket allowance is spent — this closes just that bucket, never the
     * whole device). [alwaysAvailable] is the one dimension that goes the
     * OTHER way — it is subtracted from the lock posture, not unioned in, for
     * the packages the family agreed may survive the lock. A package suspended
     * by ANY source stays suspended; only packages in NONE are unsuspended —
     * so the sources never fight. Everything is best-effort over the live
     * launchable surface. Returns packages that could not be changed (never
     * fatal).
     */
    fun reconcile(
        locked: Boolean,
        appPolicy: org.forgesworn.charter.native.CharterCore.AppPolicy?,
        ruleSuspensions: Set<String> = emptySet(),
        listeningExempt: Set<String> = emptySet(),
        bucketSuspensions: Set<String> = emptySet(),
        alwaysAvailable: Set<String> = emptySet(),
    ): List<String>
    fun isSuspended(pkg: String): Boolean
}

/**
 * The set of launchable packages to suspend, the UNION of every source
 * (level-triggered, I12): the whole-device [locked] surface, the standing
 * per-app [appPolicy] posture (blocklist's blocked apps, or everything outside
 * an allowlist), the schedule-driven per-app [ruleSuspensions], and the
 * named-times [bucketSuspensions] (apps whose own bucket allowance is spent —
 * this closes just that bucket, never the whole device). A package asked for
 * by ANY source is in the set; only packages in NONE fall out (and get
 * unsuspended).
 *
 * Exemptions ([listeningExempt], [alwaysAvailable]) are subtracted from the
 * LOCK posture only, never from the standing policy or the per-app rules: they
 * are permission to survive a lock, never permission to dodge a block a
 * guardian set outright. An app blocked in the Apps clause stays blocked
 * whether it is making a noise or named as always available.
 *
 * Pure set logic — the JVM-testable seam the DPM reconcile applies.
 */
fun appSuspendSet(
    launchable: Collection<String>,
    locked: Boolean,
    appPolicy: org.forgesworn.charter.native.CharterCore.AppPolicy?,
    ruleSuspensions: Set<String>,
    listeningExempt: Set<String> = emptySet(),
    bucketSuspensions: Set<String> = emptySet(),
    alwaysAvailable: Set<String> = emptySet(),
): Set<String> {
    // The STANDING posture — what the guardian's app policy says regardless of
    // the clock. Kept separate from the lock posture below because exemptions
    // may never touch it.
    val standing: Set<String> = when {
        appPolicy == null -> emptySet()
        appPolicy.posture == "allowlist" -> launchable.toSet() - appPolicy.allowed.toSet()
        else -> appPolicy.blocked.toSet()
    }
    // The LOCK posture — the whole surface, and the only thing an exemption may
    // ever be subtracted from. Both exemptions are permission to survive a
    // LOCK: a story the family agreed may finish (`listening`, gated on audio
    // actually sounding), and an app they agreed is open at any hour
    // (`alwaysavailable`, gated on the lock reason, resolved by the caller).
    //
    // Subtracting from the lock posture ALONE is the fix for the hole this
    // function always claimed to close but did not: `union - listeningExempt`
    // also cancelled a standing blocklist, so an app the guardian blocked
    // outright came back at the lock if it was making a noise.
    val lockPosture: Set<String> =
        if (locked) launchable.toSet() - listeningExempt - alwaysAvailable else emptySet()
    // Union: a package suspended by the lock, the standing policy, a per-app
    // rule, OR its named-times bucket stays suspended — the dimensions never
    // cancel each other out.
    return lockPosture + standing + ruleSuspensions + bucketSuspensions
}

/**
 * The LockTask allowlist as pure set logic: Kintrinsic, the phone stack, and the
 * live always-available set, deduped and order-stable. The JVM-testable seam
 * behind [org.forgesworn.charter.service.WardenController.lockTaskPackages].
 */
fun lockTaskAllowlist(
    self: String,
    dialers: List<String>,
    alwaysAvailable: Set<String>,
): List<String> = (listOf(self) + dialers + alwaysAvailable.sorted()).distinct()

/**
 * The level-triggered compare-then-push idiom, as pure logic: skip [push]
 * entirely when [next] equals [lastPushed] (the discipline
 * [DpmRestrictionOps.applyTetherMode] and
 * [org.forgesworn.charter.service.WardenController.syncLockTaskPackages] both
 * follow, after a phone once logged ~68 redundant restriction changes a
 * minute, 2026-07-26); otherwise call [push] and cache [next] ONLY if it
 * reports success. A failed push must leave the cache at [lastPushed]
 * UNCHANGED — record it as pushed anyway and a transient failure (a
 * SecurityException before Device Owner status settles, an
 * IllegalArgumentException for an uninstalled package, a binder hiccup) is
 * silently never retried, because the next tick's [next] would be identical
 * to what the cache already claims succeeded.
 *
 * The JVM-testable seam behind a DPM-calling `syncX` method: [push] is a
 * plain lambda, so no `Context`/`DevicePolicyManager` is needed to exercise
 * the retry-on-failure behavior.
 */
fun <T> syncOnChange(next: T, lastPushed: T?, push: (T) -> Boolean): T? =
    if (next == lastPushed) lastPushed else if (push(next)) next else lastPushed

/** Own the install-lockdown + anti-tamper restrictions (port-spec §3.4). */
interface RestrictionOps {
    /** Apply the baseline restriction set — only once a charter exists (I17). */
    fun applyBaseline()

    /**
     * Set ONLY the install lock, level-triggered every tick and independent of
     * the one-shot baseline — so a guardian's maintenance window can stand it
     * down and, just as importantly, so it comes back the moment the window
     * shuts (including across a reboot, since it is re-derived each tick).
     *
     * "The install lock" is both install restrictions together; see the
     * implementation for why one of them is not enough.
     */
    fun setInstallLock(locked: Boolean)
    fun clearBaseline()
    fun isRestrictionActive(key: String): Boolean

    /**
     * Level-triggered tethering posture ("blocked" | "raw" | "filtered"),
     * re-asserted each tick like the app-gate reconcile. "blocked" locks all
     * tethering config (re-applying it auto-kills a live hotspot — OS
     * behavior); "raw" frees it for the granted window; "filtered" frees
     * config but locks the SYSTEM Wi-Fi hotspot, leaving only Kintrinsic's own
     * local-only AP legal. Anything unrecognized is treated as "blocked".
     */
    fun applyTetherMode(mode: String)
}

/** The budget loop's input: foreground package + screen interactivity. */
interface UsageSource {
    fun foregroundPackage(): String?
    fun screenInteractive(): Boolean
}

/**
 * The packages Kintrinsic must NEVER suspend — suspending them would break the
 * lock experience or trap the ward (I23; is_valid_freeze_target analog).
 */
fun denyListPackages(context: Context): Set<String> {
    val pm = context.packageManager
    val deny = mutableSetOf(
        context.packageName, // never suspend ourselves / the lock UI
        "com.android.settings",
        "com.android.systemui",
        "com.android.phone",
        "com.android.dialer",
        "com.android.emergency",
        "com.android.inputmethod.latin",
    )
    // The current default launcher + IME are load-bearing; add them dynamically.
    resolveDefaultLauncher(pm)?.let { deny.add(it) }
    return deny
}

private fun resolveDefaultLauncher(pm: PackageManager): String? {
    val intent = android.content.Intent(android.content.Intent.ACTION_MAIN).apply {
        addCategory(android.content.Intent.CATEGORY_HOME)
    }
    return pm.resolveActivity(intent, PackageManager.MATCH_DEFAULT_ONLY)
        ?.activityInfo?.packageName
}

/** Enumerate launchable packages in this user (live, never a stale list, I23). */
fun launchablePackages(context: Context): List<String> {
    val pm = context.packageManager
    val intent = android.content.Intent(android.content.Intent.ACTION_MAIN).apply {
        addCategory(android.content.Intent.CATEGORY_LAUNCHER)
    }
    return pm.queryIntentActivities(intent, 0)
        .map { it.activityInfo.packageName }
        .distinct()
}

/**
 * Installed launchable apps as `(pkg, label)` pairs, excluding the deny-list —
 * the shared query-and-dedupe loop behind [launchableAppsJson] (the guardian's
 * app-picker source) and the shade's Open-row inventory. Deduped by package;
 * labels are the friendly display names. The two callers must never drift on
 * the deny-list or the dedupe, so neither may duplicate this loop.
 */
fun launchableAppsPairs(context: Context): List<Pair<String, String>> {
    val pm = context.packageManager
    val deny = denyListPackages(context)
    val intent = android.content.Intent(android.content.Intent.ACTION_MAIN).apply {
        addCategory(android.content.Intent.CATEGORY_LAUNCHER)
    }
    val seen = HashSet<String>()
    val out = mutableListOf<Pair<String, String>>()
    for (ri in pm.queryIntentActivities(intent, 0)) {
        val pkg = ri.activityInfo.packageName
        if (pkg in deny || !seen.add(pkg)) continue
        val label = runCatching { ri.loadLabel(pm).toString() }.getOrDefault(pkg)
        out.add(pkg to label)
    }
    return out
}

/** Installed launchable apps as JSON `[{"pkg":…,"label":…}]`, excluding the
 *  deny-list — the guardian's app-picker source (rides STATUS). Deduped by
 *  package; labels are the friendly display names. */
fun launchableAppsJson(context: Context): String {
    val arr = org.json.JSONArray()
    for ((pkg, label) in launchableAppsPairs(context)) {
        arr.put(org.json.JSONObject().put("pkg", pkg).put("label", label))
    }
    return arr.toString()
}

/**
 * What the shade's Open row should offer: the live always-available packages
 * that the device can actually launch, as `(pkg, label)` sorted by label.
 *
 * `alwaysAvailable` only answers the LOCK question ("does this clause outlive
 * this particular lock?" — [org.forgesworn.charter.native.CharterCore.alwaysAvailable]).
 * It says nothing about the OTHER dimensions [appSuspendSet] unions in: the
 * standing per-app policy, the schedule-driven per-app rules, and the
 * named-times bucket suspensions. Before this took [appPolicy]/
 * [ruleSuspensions]/[bucketSuspensions], the row painted a button for a named
 * app on an ALLOWLIST posture that didn't name it, or that was outright
 * blocklisted, or whose own bucket was spent — [appSuspendSet] was correctly
 * keeping it suspended regardless, so the button was guaranteed to do
 * nothing at every tap, not merely stale (review finding I3, 2026-08-04).
 * Mirrors [appSuspendSet]'s non-lock terms exactly, restricted to the
 * packages that matter here — the LOCK term is not repeated because
 * `alwaysAvailable` has already resolved it.
 *
 * Defaulted so every existing call site (and test) that has no opinion on the
 * standing policy keeps behaving exactly as before: no policy, no rules, no
 * spent buckets ⇒ nothing here is filtered beyond the inventory check below.
 *
 * An app named in the clause but absent from the inventory is dropped — it was
 * uninstalled since the guardian chose it, and a button that opens nothing is
 * worse than no button. An empty result means render NO row: a family that
 * never sets this clause sees the shade exactly as it is today.
 */
fun openRowEntries(
    alwaysAvailable: Set<String>,
    inventory: List<Pair<String, String>>,
    appPolicy: org.forgesworn.charter.native.CharterCore.AppPolicy? = null,
    ruleSuspensions: Set<String> = emptySet(),
    bucketSuspensions: Set<String> = emptySet(),
): List<Pair<String, String>> {
    val standing: Set<String> = when {
        appPolicy == null -> emptySet()
        appPolicy.posture == "allowlist" -> alwaysAvailable - appPolicy.allowed.toSet()
        else -> appPolicy.blocked.toSet()
    }
    val live = alwaysAvailable - standing - ruleSuspensions - bucketSuspensions
    return inventory.filter { it.first in live }.sortedBy { it.second.lowercase() }
}

class DpmAppGateOps(
    private val context: Context,
    private val dpm: DevicePolicyManager,
    private val admin: ComponentName,
) : AppGateOps {

    override fun reconcile(
        locked: Boolean,
        appPolicy: org.forgesworn.charter.native.CharterCore.AppPolicy?,
        ruleSuspensions: Set<String>,
        listeningExempt: Set<String>,
        bucketSuspensions: Set<String>,
        alwaysAvailable: Set<String>,
    ): List<String> {
        val deny = denyListPackages(context)
        val launchable = launchablePackages(context).filter { it !in deny }
        if (launchable.isEmpty()) return emptyList()
        // The set to suspend: the UNION of the whole-device lock, the standing
        // per-app policy (blocklist ⇒ its blocked apps; allowlist ⇒ everything
        // outside allowed), the schedule-driven per-app rule suspensions, and
        // the named-times bucket suspensions — so the sources never fight (a
        // package blocked by ANY stays suspended).
        val suspendSet: Set<String> = appSuspendSet(
            launchable,
            locked,
            appPolicy,
            ruleSuspensions,
            listeningExempt,
            bucketSuspensions,
            alwaysAvailable,
        )
        val toSuspend = launchable.filter { it in suspendSet }.toTypedArray()
        val toUnsuspend = launchable.filter { it !in suspendSet }.toTypedArray()
        // setPackagesSuspended returns the packages it FAILED to change — treat
        // as "does not exist / retry next tick", log every one, never swallow.
        val failed = mutableListOf<String>()
        if (toSuspend.isNotEmpty()) failed += dpm.setPackagesSuspended(admin, toSuspend, true)
        if (toUnsuspend.isNotEmpty()) failed += dpm.setPackagesSuspended(admin, toUnsuspend, false)
        if (failed.isNotEmpty()) Log.w(TAG, "could not update: ${failed.joinToString()}")
        return failed
    }

    override fun isSuspended(pkg: String): Boolean =
        try {
            dpm.isPackageSuspended(admin, pkg)
        } catch (e: PackageManager.NameNotFoundException) {
            false
        }

    companion object {
        private const val TAG = "DpmAppGateOps"
    }
}

class DpmRestrictionOps(
    private val dpm: DevicePolicyManager,
    private val admin: ComponentName,
) : RestrictionOps {

    /**
     * The load-bearing install-lockdown (I28) + the clock-tamper close
     * (DISALLOW_CONFIG_DATE_TIME, I7) + casual-tamper guards.
     */
    private val baseline = listOf(
        UserManager.DISALLOW_INSTALL_APPS,
        UserManager.DISALLOW_INSTALL_UNKNOWN_SOURCES,
        UserManager.DISALLOW_CONFIG_DATE_TIME,
        UserManager.DISALLOW_FACTORY_RESET,
        UserManager.DISALLOW_ADD_USER,
        // The ward cannot swap in their own VPN or a private DoH resolver that
        // would bypass the Kintrinsic DNS filter (web-content enforcement).
        UserManager.DISALLOW_CONFIG_VPN,
        UserManager.DISALLOW_CONFIG_PRIVATE_DNS,
        // Tethering hands tethered clients RAW upstream internet (kernel-forwarded
        // around the VPN + Private DNS) — default-closed; a guardian grant lifts it.
        UserManager.DISALLOW_CONFIG_TETHERING,
        // Safe mode disables EVERY third-party package, a Device Owner
        // included: no CharterService, no VPN, no lock, no app suspension,
        // and — because nothing is running — no report that any of it
        // stopped. Long-press Power from the lock screen, "Reboot to safe
        // mode", and the phone is unmanaged for as long as the ward stays
        // there (S1, review 2026-08-07).
        //
        // These two were held back on purpose while there was no way out of a
        // wedged warden: taking away the OS's own escape hatches before ours
        // existed would have been a way to brick a child's phone (port-spec
        // D9/I27). Ours has since shipped — break-glass (jni/breakglass.rs +
        // LockActivity) gives a full offline unlock from the lock screen, so
        // the deferral no longer has anything to wait for.
        //
        // The boot-count tamper signal is the belt to this brace: a ward who
        // finds some other way to boot unwarded still shows up on the
        // guardian's STATUS as an unexplained gap.
        UserManager.DISALLOW_SAFE_BOOT,
        // Developer Options survives the freeze deny-list (the ward keeps
        // Settings), and USB debugging from there is adb: force-stop the
        // warden, flip the usage-stats app-op, and the day's accrual stops
        // without anything appearing to be off.
        UserManager.DISALLOW_DEBUGGING_FEATURES,
    )

    /**
     * Level-triggered, but only ACTS on a difference. This runs every tick, and
     * blindly re-adding restrictions that are already set cost a child's phone
     * ~68 redundant binder calls a minute, all day, each one logged by the
     * system (observed on Robin's phone, 2026-07-26).
     *
     * `maintenanceOpen` is the one deliberate loosening: while a guardian's
     * signed, expiring window is open, the install lock stands down so a cabled
     * phone can be repaired. Everything else in the baseline stays on, and the
     * lock is re-applied the moment the window shuts — including after a
     * reboot, because it is re-derived from the clause every tick.
     */
    override fun applyBaseline() {
        val current = runCatching { dpm.getUserRestrictions(admin) }.getOrNull()
        for (r in baseline) {
            // Unknown current state (a read that failed) → apply, never assume.
            if (current?.getBoolean(r, false) != true) dpm.addUserRestriction(admin, r)
        }
        dpm.setAutoTimeRequired(admin, true)
    }

    /**
     * The install lock is TWO restrictions, not one, and standing down only the
     * first is how a maintenance window comes to *look* open while refusing
     * every install anyway.
     *
     * `DISALLOW_INSTALL_APPS` closes every `PackageInstaller` session.
     * `DISALLOW_INSTALL_UNKNOWN_SOURCES` closes the sources an ordinary store
     * app may install FROM — and on GrapheneOS there is no privileged Play
     * Store: sandboxed Play is an unprivileged app, so it is itself an "unknown
     * source". Lifting only `DISALLOW_INSTALL_APPS` there leaves Play exactly as
     * blocked as before. So both move together, for the same signed span.
     *
     * This does NOT widen the guardian-approved staged-APK install: a Device
     * Owner's own silent session was never unknown-sources-gated, which is why
     * [ApkInstallOps] still lifts `DISALLOW_INSTALL_APPS` alone.
     */
    private val installLock = listOf(
        UserManager.DISALLOW_INSTALL_APPS,
        UserManager.DISALLOW_INSTALL_UNKNOWN_SOURCES,
    )

    override fun setInstallLock(locked: Boolean) {
        val current = runCatching { dpm.getUserRestrictions(admin) }.getOrNull()
        for (r in installLock) {
            // Fail-closed on an unreadable state: re-apply the lock rather than
            // assume it is already right.
            val isSet = current?.getBoolean(r, false)
            if (locked && isSet != true) {
                dpm.addUserRestriction(admin, r)
            } else if (!locked && isSet != false) {
                dpm.clearUserRestriction(admin, r)
            }
        }
    }

    override fun clearBaseline() {
        baseline.forEach { dpm.clearUserRestriction(admin, it) }
        // Not in the baseline list (only a filtered-tethering session sets it),
        // but un-chartering must leave NO Kintrinsic restriction behind.
        dpm.clearUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
        dpm.setAutoTimeRequired(admin, false)
    }

    /** The last mode actually pushed to the platform, so a level-triggered
     *  caller doesn't re-issue identical DPM calls every tick. Robin's phone
     *  logged ~68 redundant restriction changes a minute doing exactly that
     *  (2026-07-26) — pure binder traffic and battery on a child's device. */
    @Volatile private var lastTetherMode: String? = null

    override fun applyTetherMode(mode: String) {
        if (mode == lastTetherMode) return
        lastTetherMode = mode
        when (mode) {
            "raw" -> {
                dpm.clearUserRestriction(admin, UserManager.DISALLOW_CONFIG_TETHERING)
                dpm.clearUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
            }
            "filtered" -> {
                // Lock the SYSTEM Wi-Fi hotspot FIRST, then free config — never
                // leave a window where both are clear and raw system tethering
                // is momentarily legal.
                dpm.addUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
                dpm.clearUserRestriction(admin, UserManager.DISALLOW_CONFIG_TETHERING)
            }
            else -> {
                dpm.addUserRestriction(admin, UserManager.DISALLOW_CONFIG_TETHERING)
                dpm.clearUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
            }
        }
    }

    override fun isRestrictionActive(key: String): Boolean {
        val bundle = dpm.getUserRestrictions(admin)
        return bundle.getBoolean(key, false)
    }
}

class UsageStatsSource(private val context: Context) : UsageSource {

    @Volatile private var lastForeground: String? = null

    override fun foregroundPackage(): String? {
        val usm = context.getSystemService(Context.USAGE_STATS_SERVICE)
            as? android.app.usage.UsageStatsManager ?: return lastForeground
        return try {
            val now = System.currentTimeMillis()
            val events = usm.queryEvents(now - LOOKBACK_MS, now)
            val ev = android.app.usage.UsageEvents.Event()
            var latestPkg: String? = null
            while (events.hasNextEvent()) {
                events.getNextEvent(ev)
                if (ev.eventType == android.app.usage.UsageEvents.Event.ACTIVITY_RESUMED) {
                    latestPkg = ev.packageName
                }
            }
            // A successful query with a fresh resume updates our memory; an empty
            // window keeps the last-known (the app may have resumed before the
            // lookback and is still foreground). A read FAILURE (below) also
            // preserves it — so a transient UsageStats failure never looks like
            // "idle" and silently stops budget accrual (I22 — fail closed).
            if (latestPkg != null) lastForeground = latestPkg
            lastForeground
        } catch (t: Throwable) {
            android.util.Log.w(TAG, "UsageStats read failed; holding last foreground", t)
            lastForeground
        }
    }

    override fun screenInteractive(): Boolean {
        val pm = context.getSystemService(Context.POWER_SERVICE) as? android.os.PowerManager
        return pm?.isInteractive ?: true
    }

    private companion object {
        const val TAG = "UsageStatsSource"
        const val LOOKBACK_MS = 10_000L
    }
}
