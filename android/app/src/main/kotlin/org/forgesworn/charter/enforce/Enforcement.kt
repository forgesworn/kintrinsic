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

    /**
     * Level-triggered "remove from device" (2026-08-27). [desired] is the set
     * that must be HIDDEN right now — gone from the launcher, the drawer and
     * Settings as if uninstalled ([android.app.admin.DevicePolicyManager.setApplicationHidden]).
     * Every package the warden hid that is NOT in [desired] is unhidden, so a
     * guardian dropping a name from the clause puts the app back.
     *
     * Its own dimension, deliberately NOT unioned into [reconcile]: hiding is a
     * standing tidy of the device, not a time-of-day posture. It survives a
     * pause, an unlock and an always-available exemption, because none of those
     * are answers to "should this app be on this phone at all?".
     *
     * Idempotent and difference-only: a tick where nothing moved must make no
     * binder calls at all (the [syncOnChange] discipline). Returns the packages
     * it could not change (never fatal).
     */
    fun reconcileHidden(desired: Set<String>): List<String>
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
 * The packages to HIDE — the `apps` clause's "remove from device" list,
 * narrowed to what this phone actually has and widened by nothing.
 *
 * Origin (2026-08-27): a ward's Samsung tablet arrived carrying a screenful of
 * OEM bloatware nobody in the family wanted, and since 0.6.9 locks USB
 * debugging by design there is no cable path to sweep it off — the tidy has to
 * come down the charter or not at all. Suspending those apps was the wrong
 * shape: a suspended app still sits in the drawer wearing a grey badge,
 * advertising itself and inviting a tap that goes nowhere.
 *
 * Deliberately unlike [appSuspendSet]: this reads NOTHING but
 * [org.forgesworn.charter.native.CharterCore.AppPolicy.hidden]. Not the
 * posture, not `blocked`, not `allowed`, not the lock, not a bucket — hiding
 * is a standing statement about which apps belong on the device, so no clock
 * and no exemption may move it. A pause lifts BLOCKING and leaves this exactly
 * where it stands (the pure logic here cannot even see a pause; see the call
 * site in `WardenController` for the contract that keeps it that way).
 *
 * Two narrowings, both non-negotiable:
 *  - `∩ installed` — a named package the phone never had is silently ignored,
 *    so a guardian pasting a list of bloatware package names that only half
 *    matches this model does not produce a tick of failure spam forever.
 *  - `− deny` — [denyListPackages] may NEVER be hidden. Hiding the launcher,
 *    the IME, Settings, the dialer or Kintrinsic itself would strand the ward on
 *    a phone she cannot use or call out of, and unlike a suspension there is no
 *    grey badge to explain it. The clause is not permitted to brick the device.
 *
 * Empty when there is no policy at all — nothing hidden, everything the warden
 * previously hid comes back.
 *
 * Pure set logic — the JVM-testable seam [AppGateOps.reconcileHidden] applies.
 */
fun appHideSet(
    policy: org.forgesworn.charter.native.CharterCore.AppPolicy?,
    installed: Collection<String>,
    deny: Set<String>,
): Set<String> {
    if (policy == null) return emptySet()
    return policy.hidden.toSet().intersect(installed.toSet()) - deny
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
 * What the warden has HIDDEN, remembered across ticks and reboots.
 *
 * This has to be persisted, and the reason is the whole trick: a hidden package
 * drops out of `queryIntentActivities` — the device stops admitting it exists.
 * So the warden cannot re-derive "what did I hide?" by looking; it would find
 * nothing, unhide nothing, and a package dropped from the clause would stay
 * invisible forever with no way back short of a factory reset. The memory IS
 * the undo.
 *
 * A plain string-set under one key: this is a small set that is rewritten
 * whole, never merged.
 */
class HiddenAppsStore(context: Context) {
    // Lazy so merely CONSTRUCTING the ops object never touches storage — the
    // production wiring builds it eagerly and a first tick is not guaranteed.
    private val prefs by lazy {
        context.getSharedPreferences("charter-hidden-apps", Context.MODE_PRIVATE)
    }

    fun read(): Set<String> = prefs.getStringSet(KEY, emptySet())?.toSet() ?: emptySet()

    /** Writes ONLY on a real change — a quiet tick must not touch flash (this
     *  runs on every enforcement tick). A defensive copy goes in, because
     *  SharedPreferences does not copy the set it is handed. */
    fun write(next: Set<String>, previous: Set<String>) {
        if (next == previous) return
        prefs.edit().putStringSet(KEY, HashSet(next)).apply()
    }

    companion object {
        private const val KEY = "charter.hiddenApps"
    }
}

/**
 * Which of [packages] this device actually has, hidden ones included.
 *
 * `MATCH_UNINSTALLED_PACKAGES` is the point: a package the warden has hidden
 * reads as uninstalled to every ordinary query, so without this flag the
 * hide-set would empty itself the tick after it took effect and the warden
 * would immediately unhide everything it just hid.
 *
 * Probes only the named packages rather than enumerating the whole device —
 * `getInstalledApplications` costs real time on a phone (the same reason the
 * install account refreshes on a cadence instead of every tick), and the clause
 * names a handful of packages, not thousands.
 */
fun installedAmong(context: Context, packages: Collection<String>): Set<String> {
    if (packages.isEmpty()) return emptySet()
    val pm = context.packageManager
    return packages.filterTo(HashSet()) { installedInfo(pm, it) != null }
}

/**
 * The [android.content.pm.ApplicationInfo] for [pkg] if the device really has
 * it, hidden included; null otherwise.
 *
 * `MATCH_UNINSTALLED_PACKAGES` widens the query far enough to see a hidden
 * package — but it also drags in the ghosts of packages removed with their data
 * kept, so `FLAG_INSTALLED` narrows it back. A hidden package keeps that flag
 * (hiding marks the package hidden for the user, not uninstalled), a ghost does
 * not: the flag is exactly the line between "here but invisible" and "gone".
 * Chasing a ghost would mean a `setApplicationHidden` call that fails every
 * tick forever, which is the failure spam this whole path must not produce.
 */
private fun installedInfo(
    pm: PackageManager,
    pkg: String,
): android.content.pm.ApplicationInfo? {
    val info = runCatching {
        pm.getApplicationInfo(pkg, PackageManager.MATCH_UNINSTALLED_PACKAGES)
    }.getOrNull() ?: return null
    val installed = info.flags and android.content.pm.ApplicationInfo.FLAG_INSTALLED != 0
    return if (installed) info else null
}

/**
 * Labels for packages the warden has HIDDEN, as `(pkg, label)` — the inventory
 * entries no launcher query can produce any more, since hiding removes the
 * package from `queryIntentActivities`.
 *
 * Without this the guardian's app picker would lose an app the moment she hid
 * it, leaving her no row to un-hide from: the tidy would be one-way, which is
 * exactly the trap Kintrinsic must never build. Deliberately kept OUT of
 * [launchableAppsPairs] — the shade's Open row is built from that list, and a
 * button that opens a hidden app is a button that does nothing.
 *
 * A package that is genuinely gone (uninstalled while hidden) is dropped; a
 * package whose label cannot be read falls back to its own name.
 */
fun hiddenAppsPairs(context: Context, hidden: Set<String>): List<Pair<String, String>> {
    if (hidden.isEmpty()) return emptyList()
    val pm = context.packageManager
    val deny = denyListPackages(context)
    return hidden.filterNot { it in deny }.sorted().mapNotNull { pkg ->
        val info = installedInfo(pm, pkg) ?: return@mapNotNull null
        pkg to runCatching { pm.getApplicationLabel(info).toString() }.getOrDefault(pkg)
    }
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
 *  package; labels are the friendly display names.
 *
 *  Packages the warden has HIDDEN are appended with `"hidden": true` (2026-08-27).
 *  They must appear, or the picker loses the app the moment it is hidden and
 *  the guardian has nothing to un-hide from — a one-way tidy. The flag is
 *  emitted ONLY on those entries: an ordinary app's object is byte-for-byte
 *  what every shipped guardian already parses, so nothing older has to change
 *  to keep working. */
fun launchableAppsJson(context: Context): String {
    val arr = org.json.JSONArray()
    val seen = HashSet<String>()
    for ((pkg, label) in launchableAppsPairs(context)) {
        if (!seen.add(pkg)) continue
        arr.put(org.json.JSONObject().put("pkg", pkg).put("label", label))
    }
    // `seen` guards the seam between the two sources: a package the warden
    // believes it hid but that is somehow still launchable (a hide that failed,
    // a stale memory) must appear once, not twice.
    for ((pkg, label) in hiddenAppsPairs(context, HiddenAppsStore(context).read())) {
        if (!seen.add(pkg)) continue
        arr.put(org.json.JSONObject().put("pkg", pkg).put("label", label).put("hidden", true))
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

    /** What we hid, so we can put it back — a hidden package stops answering
     *  launcher queries, so this memory is the only route home. */
    private val hiddenStore = HiddenAppsStore(context)

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

    override fun reconcileHidden(desired: Set<String>): List<String> {
        // Belt and braces over [appHideSet]'s own subtraction: whatever route a
        // package took to get here, the deny-list is never hidden. Stranding the
        // ward on a phone with no launcher, no keyboard or no dialer is not a
        // failure mode worth one binder call's saving.
        val target = desired - denyListPackages(context)
        val remembered = hiddenStore.read()
        val toHide = target - remembered
        val toUnhide = remembered - target
        // The difference discipline (see [syncOnChange]): a settled device makes
        // NO calls here. Hiding is level-triggered like the suspend reconcile,
        // and this runs every tick — re-asserting an unchanged set would be the
        // ~68-redundant-changes-a-minute mistake again, on a heavier call.
        if (toHide.isEmpty() && toUnhide.isEmpty()) return emptyList()
        val failed = mutableListOf<String>()
        val next = remembered.toMutableSet()
        for (pkg in toHide) {
            if (setHidden(pkg, true)) next.add(pkg) else failed.add(pkg)
        }
        for (pkg in toUnhide) {
            // A failed unhide stays REMEMBERED so the next tick retries it —
            // forgetting it would leave the app invisible with nothing left on
            // the device that knows to bring it back. The one exception is a
            // package that is simply gone: nothing to unhide and nothing to
            // retry, so it leaves the memory rather than failing forever.
            when {
                setHidden(pkg, false) -> next.remove(pkg)
                installedAmong(context, listOf(pkg)).isEmpty() -> next.remove(pkg)
                else -> failed.add(pkg)
            }
        }
        hiddenStore.write(next, remembered)
        if (failed.isNotEmpty()) Log.w(TAG, "could not hide/unhide: ${failed.joinToString()}")
        return failed
    }

    /** One [DevicePolicyManager.setApplicationHidden] call, never fatal: it
     *  throws for a package this user does not have, and a single bad name in
     *  the clause must not take the rest of the reconcile down with it. */
    private fun setHidden(pkg: String, hide: Boolean): Boolean =
        runCatching { dpm.setApplicationHidden(admin, pkg, hide) }.getOrDefault(false)

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
