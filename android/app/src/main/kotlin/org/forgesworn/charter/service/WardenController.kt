package org.forgesworn.charter.service

import android.app.admin.DevicePolicyManager
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.util.Log
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.enforce.ApkInstallOps
import org.forgesworn.charter.enforce.AppGateOps
import org.forgesworn.charter.enforce.blockedUnder
import org.forgesworn.charter.enforce.holdNotices
import org.forgesworn.charter.enforce.memoryOf
import org.forgesworn.charter.enforce.ruleNotices
import org.forgesworn.charter.enforce.DnsFilterOps
import org.forgesworn.charter.enforce.DpmApkInstallOps
import org.forgesworn.charter.enforce.DpmAppGateOps
import org.forgesworn.charter.enforce.DpmRestrictionOps
import org.forgesworn.charter.enforce.hotspot.HotspotWish
import org.forgesworn.charter.enforce.RestrictionOps
import org.forgesworn.charter.enforce.UsageSource
import org.forgesworn.charter.enforce.UsageStatsSource
import org.forgesworn.charter.enforce.VpnDnsFilterOps
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.ui.LockActivity

/**
 * The orchestration seam between the Rust decision core and the Android enforce
 * ops (port-spec §3.3). Owns the tick→apply loop body; the service drives it on
 * a cadence and tests drive it directly. Injectable ops = mock/real discipline.
 */
class WardenController(
    private val context: Context,
    private val dpm: DevicePolicyManager,
    private val admin: ComponentName,
    private val appGate: AppGateOps,
    private val restrictions: RestrictionOps,
    private val usage: UsageSource,
    private val install: ApkInstallOps,
    private val dnsFilter: DnsFilterOps,
    private val lock: LockController,
    private val warn: WarnNotifier = object : WarnNotifier {
        override fun warn(level: String) {}
    },
    private val installAccount: org.forgesworn.charter.enforce.InstallAccountOps =
        org.forgesworn.charter.enforce.PmInstallAccountOps(context),
    private val installNotice: org.forgesworn.charter.enforce.InstallWindowNotice =
        org.forgesworn.charter.enforce.NotificationInstallWindowNotice(context),
    /** Telling the ward an app was opened or paused for a while. Null in tests
     *  (and only there): the pure diff is exercised directly in
     *  `AppHoldNoticesTest`, so nothing here needs SharedPreferences to run. */
    private val holdNotifier: org.forgesworn.charter.enforce.HoldNotifier? = null,
    private val holdMemory: org.forgesworn.charter.enforce.HoldMemoryStore? = null,
    /** The same, for apps with their own agreed hours (`appRules`). Separate
     *  store: it answers which scheduled apps are shut, not which holds run. */
    private val ruleMemory: org.forgesworn.charter.enforce.RuleMemoryStore? = null,
) {
    private var initialized = false
    private var baselineApplied = false
    /** Whether we believe the hotspot service is running. Starts null (UNKNOWN):
     *  after a process restart a sticky-restarted service may be up while a
     *  fresh controller's `false` would swallow the stop edge — an expired
     *  grant would then never tear the AP down. Unknown ⇒ the first tick sends
     *  one reconciling start/stop either way (both are idempotent). */
    private var hotspotServiceRunning: Boolean? = null
    private var appliedDnsRevision: String? = null
    /** Latched true once the usage-access app-op is VERIFIED allowed. */
    private var usageAccessOk = false

    /** Interface over the lock surface so tests don't launch a real activity. */
    interface LockController {
        fun show(reason: String)
        fun hide()
    }

    /** Delivers child-facing moments (real notifications in production). */
    interface WarnNotifier {
        fun warn(level: String)
        fun granted(minutes: Int) {}
        /** The guardian said no. A ward who asked deserves the answer wherever
         *  they are — an unseen answer is indistinguishable from being ignored. */
        fun denied() {}
    }

    /** Initialize the Rust warden over device-protected storage. */
    fun init(enforceMode: String = "enforce"): CharterCore.InitResult {
        val res = CharterCore.init(
            Provisioning.baseDir(context),
            enforceMode,
            Provisioning.ownVersionCode(context),
        )
        initialized = res.error == null
        if (initialized) {
            // Count the boots we did NOT run through (S1). Safe mode disables
            // every third-party package, this one included, so a safe-mode
            // session leaves no tick, no lock and no report — the phone comes
            // back enforcing as though it never happened. The platform's boot
            // counter kept running regardless, so the gap between the count
            // we last ran under and this one is the holiday. Before the first
            // tick, so the first STATUS after a gap already carries it.
            noteBoot()
        }
        if (initialized && Provisioning.isDeviceOwner(context)) {
            // Allow the lock activity to hold LockTask — AND the phone stack,
            // so the in-call UI can surface over the pin. Without this a
            // lifeline call runs headless behind the lock: no hang-up, no
            // in-call controls (the 2026-07-24 on-device trap: a rejected
            // call rolled to voicemail with no way out). The dialer being
            // reachable opens nothing else — every other app stays suspended;
            // a locked phone is still a phone, per the lifeline design.
            // Empty always-available set: no decision has run yet, so this is
            // purely "the phone stack is reachable from the first moment."
            // `applyDecision` re-derives the full allowlist every tick.
            syncLockTaskPackages(emptySet())
            ensureUsageAccess()
            // The 10-min/1-min warnings need POST_NOTIFICATIONS; the lock
            // screen's "Call your guardian" (spec D9) needs CALL_PHONE; the
            // shade's End-call bar + call-state awareness need
            // READ_PHONE_STATE + ANSWER_PHONE_CALLS; the filtered guest
            // hotspot's own AP needs NEARBY_WIFI_DEVICES — all plain runtime
            // permissions, so DO self-grant works.
            for (perm in arrayOf(
                android.Manifest.permission.POST_NOTIFICATIONS,
                android.Manifest.permission.CALL_PHONE,
                android.Manifest.permission.READ_PHONE_STATE,
                android.Manifest.permission.ANSWER_PHONE_CALLS,
                android.Manifest.permission.NEARBY_WIFI_DEVICES,
            )) {
                runCatching {
                    dpm.setPermissionGrantState(
                        admin,
                        context.packageName,
                        perm,
                        android.app.admin.DevicePolicyManager.PERMISSION_GRANT_STATE_GRANTED,
                    )
                }
            }
        }
        return res
    }

    /**
     * Self-grant usage access (spike T6) and VERIFY the app-op actually
     * flipped. `setPermissionGrantState` returns false — it does not throw —
     * when a build refuses an app-op permission, so an unchecked call leaves
     * the foreground read rejected, the budget silently never accrues, and
     * the daily limit is dead (fail-safe, but dead). Caught on-metal twice:
     * spike T6, then again on GrapheneOS/bramble (2026-07-21, issue #42).
     * Level-triggered from every tick until it holds; logs loudly meanwhile.
     * Manual fallback:
     * `adb shell appops set org.forgesworn.charter android:get_usage_stats allow`.
     */
    private fun ensureUsageAccess() {
        if (usageAccessOk || !Provisioning.isDeviceOwner(context)) return
        val granted = runCatching {
            dpm.setPermissionGrantState(
                admin,
                context.packageName,
                android.Manifest.permission.PACKAGE_USAGE_STATS,
                DevicePolicyManager.PERMISSION_GRANT_STATE_GRANTED,
            )
        }.getOrDefault(false)
        val appOps = context.getSystemService(Context.APP_OPS_SERVICE) as android.app.AppOpsManager
        val mode = appOps.unsafeCheckOpNoThrow(
            android.app.AppOpsManager.OPSTR_GET_USAGE_STATS,
            android.os.Process.myUid(),
            context.packageName,
        )
        usageAccessOk = mode == android.app.AppOpsManager.MODE_ALLOWED
        if (!usageAccessOk) {
            Log.e(
                TAG,
                "usage access NOT held (grant=$granted opMode=$mode) — the daily-limit " +
                    "meter cannot accrue; retrying each tick. Manual fallback: adb shell " +
                    "appops set ${context.packageName} android:get_usage_stats allow",
            )
        }
    }

    /**
     * Hand the warden the platform boot counter so it can tally the boots it
     * missed (S1) — see [CharterNative.charterNoteBoot].
     *
     * `Settings.Global.BOOT_COUNT` is a plain global setting: readable by any
     * app, no permission, API 24+. A device that does not keep one reads 0,
     * which the warden treats as "no baseline" rather than as a gap — an
     * absent counter must never manufacture an accusation.
     */
    private fun noteBoot() {
        val bootCount = runCatching {
            android.provider.Settings.Global.getInt(
                context.contentResolver,
                android.provider.Settings.Global.BOOT_COUNT,
                0,
            ).toLong()
        }.getOrDefault(0L)
        runCatching { CharterCore.noteBoot(bootCount, System.currentTimeMillis() / 1000) }
    }

    /**
     * One relay round (blocking network IO — the slow worker's job): pull
     * guardian-signed clauses, emit the STATUS heartbeat. The Rust side holds
     * the warden lock only around state transitions, so the fast tick above is
     * never stalled by a slow relay. Null until the warden is initialized.
     */
    fun pollOnce(nowUnix: Long = System.currentTimeMillis() / 1000): CharterCore.PollResult? {
        if (!initialized) return null
        // Report the device's app inventory BEFORE the poll (STATUS is built in
        // it), so the guardian can pick apps to control by name (D3).
        runCatching {
            CharterCore.setInstalledApps(org.forgesworn.charter.enforce.launchableAppsJson(context))
        }
        return CharterCore.pollOnce(nowUnix)
    }

    /**
     * Drain guardian-approved installs the core has parked and enact each one
     * (port-spec §2.4). Runs on the slow worker (installs are blocking IO). The
     * core owns single-use + signing-cert pinning; we perform the platform
     * install, verifying signing continuity BEFORE commit, and report each
     * outcome so the directive clears (ok/terminal) or retries (transient).
     * Only the Device Owner can install onto the locked-down phone.
     */
    fun drainAndInstall(nowUnix: Long = System.currentTimeMillis() / 1000): Int {
        if (!initialized || !Provisioning.isDeviceOwner(context)) return 0
        val pending = CharterCore.drainInstalls(nowUnix)
        for (p in pending) {
            var reason = ""
            val outcome = try {
                install.install(p)
            } catch (t: Throwable) {
                Log.e(TAG, "install ${p.packageName} threw", t)
                reason = t.javaClass.simpleName
                CharterCore.InstallOutcome.TRANSIENT
            }
            if (outcome != CharterCore.InstallOutcome.OK && reason.isEmpty()) {
                reason = install.lastFailure() ?: "install failed"
            }
            Log.i(TAG, "install ${p.packageName} (${p.reqId.take(8)}…): $outcome")
            // The reason rides back so the guardian sees it on STATUS — a
            // directive retrying in silence is what cost two days.
            CharterCore.installResult(p.reqId, outcome, reason)
        }
        return pending.size
    }

    /** The span whose account we last published, and when we last recomputed
     *  it. Enumerating every installed package is not free, so an OPEN window
     *  is re-read at most once a [ACCOUNT_REFRESH_SECS] rather than every tick;
     *  a window that has just SHUT is always recomputed once more, because that
     *  final pass is the one the guardian actually reads. */
    private var accountedStart: Long? = null
    private var accountedEnd: Long? = null
    private var lastAccountAt: Long = 0

    /** The minutes-left the ward's notice currently shows, or null when no
     *  notice is up. Re-posting an identical notification every tick is pure
     *  binder traffic and battery on a child's phone — the same mistake that
     *  logged ~68 redundant restriction changes a minute on Robin's (2026-07-26).
     *  Only an actual change to what the ward would READ is worth a re-post. */
    private var noticeMinutes: Int? = null

    /** Idempotent: a cancel per tick is still a binder call per tick. Starts
     *  null (UNKNOWN) rather than "not shown", so the FIRST tick after a
     *  process restart always clears a notice a killed process may have left
     *  standing — a phone that re-locked while Kintrinsic was dead must never keep
     *  telling the ward that installs are open. */
    private fun hideNotice() {
        if (noticeMinutes == null && noticeEverChecked) return
        noticeEverChecked = true
        installNotice.hide()
        noticeMinutes = null
    }

    private var noticeEverChecked = false

    /**
     * Publish what came through the guardian's install window, and tell the
     * ward their phone is open while it is.
     *
     * Both halves are level-triggered off the span the core tracks, so a reboot
     * mid-window resumes correctly and a window that expired while the phone was
     * off still gets its closing account on the next tick.
     */
    private fun accountForInstallWindow(nowUnix: Long) {
        val span = CharterCore.maintenanceSpan(nowUnix)
        if (span == null) {
            hideNotice()
            return
        }

        if (span.open) {
            // From the window's REAL expiry. Deriving it from the core's cap
            // told a child with 30 minutes that they had 60 (2026-07-30) —
            // being told you have longer than you do is the one direction a
            // ward-facing number must never round. Unknown expiry ⇒ 0, which
            // the notice renders WITHOUT a duration rather than inventing one.
            val minutes = span.untilUnix
                ?.let { ((it - nowUnix).coerceAtLeast(0) + 59) / 60 }
                ?.toInt()
                ?: 0
            if (noticeMinutes != minutes) {
                installNotice.show(minutes)
                noticeMinutes = minutes
            }
        } else {
            hideNotice()
        }

        // Recompute when the span CHANGED (a new window, or one that just shut),
        // or when an open window's periodic refresh is due.
        val spanChanged = span.startedAt != accountedStart || span.endedAt != accountedEnd
        val refreshDue = span.open && nowUnix - lastAccountAt >= ACCOUNT_REFRESH_SECS
        if (!spanChanged && !refreshDue) return

        val endMs = (span.endedAt ?: nowUnix) * 1000
        val changes = installAccount.changesIn(span.startedAt * 1000, endMs)
        CharterCore.setInstallWindow(
            org.forgesworn.charter.enforce.installWindowJson(
                span.startedAt,
                span.endedAt,
                changes,
            ),
        )
        accountedStart = span.startedAt
        accountedEnd = span.endedAt
        lastAccountAt = nowUnix
    }

    /**
     * One loop iteration: read the world, ask the core, apply the decision.
     * Level-triggered everywhere — safe to call every tick and on every restart.
     */
    fun tickAndApply(
        nowUnix: Long = System.currentTimeMillis() / 1000,
        screenWas: Boolean? = null,
    ): List<CharterCore.ChildDecision> {
        if (!initialized) return emptyList()
        ensureUsageAccess()
        // The install lock, every tick and independent of the one-shot
        // baseline: a guardian's signed, expiring maintenance window stands it
        // down so a cabled phone can be repaired without removing Device
        // Owner, and so a ward can install or update an app from their store
        // for that span — and it comes straight back when the window shuts,
        // including after a reboot, because it is re-derived here rather than
        // latched.
        //
        // And ONLY while paired. A released phone must be left with no
        // Kintrinsic restriction at all (I17, and the clearBaseline contract),
        // but this line used to re-lock install every tick regardless: a
        // guardian who disconnected a phone to repair it over a cable got adb
        // (the baseline had stood down) and `adb install` refused
        // (INSTALL_FAILED_USER_RESTRICTED) — with no window to open, because
        // an unpaired ward has no guardian to sign one (2026-09-07).
        val pairing = CharterCore.pairingState()
        if (Provisioning.isDeviceOwner(context)) {
            val locked = pairing.paired && !CharterCore.maintenanceOpen(nowUnix)
            runCatching { restrictions.setInstallLock(locked) }
            // Never let the account or the ward's notice break enforcement: a
            // PackageManager that throws must not stop the lock coming back.
            runCatching { accountForInstallWindow(nowUnix) }
        }
        val subject = pairing.subject
        // Ward-on-primary (D1): the WHOLE phone is the ward's surface, so ANY
        // foreground app while the screen is interactive is the ward using
        // their time — the subject is a pubkey and foreground is a package;
        // they are different namespaces, never compared. Suspended-by-Kintrinsic
        // time credits zero in the core (FrozenByCharter). A null foreground
        // (idle or read failure) credits nothing (I22 — never let an
        // unreadable UsageStats stall the lock or fail open the budget).
        val fg = usage.foregroundPackage()
        val active = if (fg != null && subject != null) subject else null
        // The core credits the interval SINCE THE LAST TICK, so what matters is
        // whether the screen was lit for *that interval* — not what it reads
        // now. Between the screen going off and coming back on the loop runs at
        // a minute's cadence to save the battery, which makes the two boundary
        // ticks the only places those two answers differ: on wake, reading
        // "interactive" would bill the ward for the whole dark minute they
        // spent asleep. [CharterService] hands us the interval's real state
        // there; everywhere else the live reading is the interval's reading.
        val screen = screenWas ?: usage.screenInteractive()
        // The SAME foreground-package probe above feeds the bucket credit —
        // never a second probe. The core gates crediting on genuine activity
        // (active subject + screen interactive), so passing it unconditionally
        // here is safe: an idle/frozen tick simply credits nothing.
        val decisions = CharterCore.tick(active, screen, nowUnix, fg)

        for (d in decisions) applyDecision(d, nowUnix)
        return decisions
    }

    /**
     * The LockTask allowlist: Kintrinsic itself, the phone stack, and whatever the
     * `alwaysavailable` clause opens right now.
     *
     * The phone stack is here so a lifeline call has a face — without it a call
     * runs headless behind the lock with no hang-up (the 2026-07-24 on-device
     * trap). The always-available set is here so the shade's Open row can
     * actually launch what it offers.
     *
     * RE-DERIVED EVERY TICK, never once at init: entries expire on their own
     * clock and a guardian edits the clause while the phone is locked. This was
     * a one-shot call until 2026-08-03, which would have pinned whatever the
     * set happened to be at boot.
     */
    private fun lockTaskPackages(alwaysAvailable: Set<String>): Array<String> {
        val dialers = buildList {
            runCatching {
                val telecom = context.getSystemService(Context.TELECOM_SERVICE)
                    as android.telecom.TelecomManager
                telecom.defaultDialerPackage?.let { add(it) }
                telecom.systemDialerPackage?.let { add(it) }
            }
        }
        return org.forgesworn.charter.enforce.lockTaskAllowlist(
            context.packageName,
            dialers,
            alwaysAvailable,
        ).toTypedArray()
    }

    /** The last allowlist actually pushed, so a level-triggered caller doesn't
     *  re-issue an identical DPM call every tick — the same discipline
     *  [DpmRestrictionOps.applyTetherMode] follows after Robin's phone logged
     *  ~68 redundant restriction changes a minute (2026-07-26).
     *
     *  Records ONLY what was successfully pushed: a failed `setLockTaskPackages`
     *  (a SecurityException before Device Owner status settles, an
     *  IllegalArgumentException for a clause-named package that isn't
     *  installed, a transient binder failure) leaves this untouched, so the
     *  next tick sees the same target list as "still not applied" and retries
     *  — rather than believing a failed push already landed and never trying
     *  again until the always-available set happens to change for some
     *  unrelated reason (fix round 1, 2026-08-04). */
    @Volatile private var lastLockTaskPackages: List<String>? = null

    private fun syncLockTaskPackages(alwaysAvailable: Set<String>) {
        val next = lockTaskPackages(alwaysAvailable).toList()
        lastLockTaskPackages = org.forgesworn.charter.enforce.syncOnChange(
            next,
            lastLockTaskPackages,
        ) { pkgs ->
            runCatching { dpm.setLockTaskPackages(admin, pkgs.toTypedArray()) }
                .onFailure { Log.w(TAG, "could not set LockTask packages", it) }
                .isSuccess
        }
    }

    /**
     * Apply one ward's decision — everything LEVEL-triggered off `locked`, and
     * everything gated on the staged-bring-up mode so Observe truly touches
     * nothing and FreezeOnly suspends without a lock surface (port-spec §3.3).
     */
    private fun applyDecision(d: CharterCore.ChildDecision, nowUnix: Long) {
        // Restrictions gate on a charter EXISTING, never on lock activity — a
        // charted ward keeps the install-lockdown up during allowed hours too
        // (the critical fail-open the review caught). Observe applies nothing.
        val applyRestrictions = d.configured && d.enforceMode != CharterCore.Mode.OBSERVE
        syncBaseline(applyRestrictions)

        for (e in d.effects) when (e) {
            // The core re-arms these correctly (once each; after thaw / rising
            // edge / unbounded spell) — delivery is our only job.
            is CharterCore.Effect.Warn -> {
                Log.i(TAG, "warn ${e.level}")
                warn.warn(e.level)
            }
            is CharterCore.Effect.Granted -> {
                Log.i(TAG, "granted ${e.minutes}")
                warn.granted(e.minutes)
            }
            is CharterCore.Effect.Denied -> {
                Log.i(TAG, "denied")
                warn.denied()
            }
            is CharterCore.Effect.Audit -> Log.i(TAG, "audit ${e.outcome}")
        }

        // Suspend: level-triggered from `locked` (whole device) PLUS the standing
        // per-app policy (blocked apps stay blocked even when unlocked) PLUS the
        // schedule-driven per-app RULE suspensions (apps the appRules clause
        // blocks right now — recomputed each tick since they turn on/off with the
        // clock). All unioned in ONE reconcile so they never fight; skipped
        // entirely in Observe. Each is its own charter dimension, independent of
        // schedule/budget. These JNI calls take the warden lock — they run on the
        // slow-safe worker thread (this whole tick is off the main thread).
        if (d.enforceMode != CharterCore.Mode.OBSERVE) {
            val appPolicy = runCatching { CharterCore.appPolicy(nowUnix) }.getOrNull()
            val ruleSuspensions =
                runCatching { CharterCore.appRuleSuspensions(nowUnix) }.getOrDefault(emptyList())
            // Named-times ("buckets"): apps whose OWN bucket allowance is spent
            // right now — its own charter dimension, exactly like the per-app
            // rule suspensions above. Spending a bucket closes only ITS apps,
            // never the whole device.
            val bucketSuspensions =
                runCatching { CharterCore.bucketSuspensions(nowUnix) }.getOrDefault(emptyList())
            // Audio the family agreed may finish (spec 2026-07-29). The PLATFORM
            // says whether anything is actually sounding — a named app that
            // isn't playing is suspended like everything else, so the exemption
            // can't be squatted on by playing silence.
            val audioPlaying = runCatching {
                (context.getSystemService(android.content.Context.AUDIO_SERVICE)
                    as android.media.AudioManager).isMusicActive
            }.getOrDefault(false)
            val listening = runCatching {
                CharterCore.listeningView(nowUnix, d.locked, audioPlaying)
            }.getOrNull()
            // Apps the family agreed are open at ANY hour (spec 2026-08-03).
            // Takes the lock REASON, not just `locked`: a stand-down and a
            // malformed charter must take everything, so the clause is asked
            // whether THIS lock is one it may outlive.
            val alwaysAvailable = runCatching {
                CharterCore.alwaysAvailable(nowUnix, d.locked, d.reason)
            }.getOrDefault(emptySet())
            // Level-triggered, exactly like the suspend set below: the clause
            // changes and entries expire, so the allowlist is re-derived here
            // rather than pinned at boot.
            syncLockTaskPackages(alwaysAvailable)
            appGate.reconcile(
                locked = d.locked,
                appPolicy = appPolicy,
                ruleSuspensions = ruleSuspensions.toSet(),
                listeningExempt = listening?.exempt ?: emptySet(),
                bucketSuspensions = bucketSuspensions.toSet(),
                alwaysAvailable = alwaysAvailable,
            )

            // "Remove from device" (2026-08-27): the apps clause's `hidden` list,
            // enacted OUTSIDE the union above because it answers a different
            // question. Suspension asks "may she open this now?"; hiding asks
            // "does this belong on the phone at all?" — so no lock, exemption,
            // bucket or pause moves it. It exists because a ward's Samsung
            // tablet arrived full of OEM bloatware and, since 0.6.9 locks USB
            // debugging by design, there is no cable path to sweep it off; the
            // tidy has to come down the charter.
            //
            // Level-triggered like everything else here: re-derived every tick,
            // with `reconcileHidden` making binder calls only on a difference.
            // Installedness is probed for the NAMED packages only —
            // `MATCH_UNINSTALLED_PACKAGES` under the hood, since a package we
            // already hid reads as uninstalled to any ordinary query.
            //
            // The pause contract: pausing lifts BLOCKING, it does not put
            // bloatware back. The core (`warden.rs` `app_policy`) therefore
            // still surfaces a PAUSED clause that names hidden apps — with its
            // posture lists emptied, so the suspend set above blocks nothing
            // while `hidden` here stays in force. A paused clause hiding
            // nothing arrives as null exactly as it always did.
            runCatching {
                val hidden = appPolicy?.hidden.orEmpty()
                appGate.reconcileHidden(
                    org.forgesworn.charter.enforce.appHideSet(
                        appPolicy,
                        org.forgesworn.charter.enforce.installedAmong(context, hidden),
                        org.forgesworn.charter.enforce.denyListPackages(context),
                    ),
                )
            }

            // An app that starts working and later stops working, with nothing
            // on the phone explaining either moment, is the silent change
            // Kintrinsic's transparency invariant forbids. Both edges are told.
            // `appPolicy` above has ALREADY dissolved live holds into its lists,
            // so it answers "and what is it now?" for a hold that just ended.
            val notifier = holdNotifier
            val memory = holdMemory
            if (notifier != null && memory != null) {
                runCatching {
                    val live = CharterCore.appHolds(nowUnix)
                    val previous = memory.read()
                    val notices = holdNotices(previous, live) { pkg ->
                        blockedUnder(appPolicy, pkg)
                    }
                    for (n in notices) notifier.post(n)
                    // Writes only when something actually moved — a quiet tick
                    // must not touch flash (the background-power pass, 0.6.4).
                    memory.write(memoryOf(live), previous)
                }
            }

            // The same courtesy for apps with their own agreed hours. These
            // turn on and off with the clock every tick and, until the
            // 2026-08-02 audit, said nothing at either edge — the transparency
            // invariant honoured for holds and forgotten for the clause right
            // beside it. Same notifier, so one app's notice replaces its own
            // rather than stacking two contradictory ones.
            val rules = ruleMemory
            if (notifier != null && rules != null) {
                runCatching {
                    val nowSuspended = ruleSuspensions.toSet()
                    val firstRun = rules.firstRun()
                    val previous = rules.read()
                    val notices = ruleNotices(previous, nowSuspended, firstRun) { pkg ->
                        blockedUnder(appPolicy, pkg)
                    }
                    for (n in notices) notifier.post(n)
                    rules.write(nowSuspended, previous)
                }
            }
        }

        // Tethering posture: level-triggered per tick like the app gate, so a
        // grant window ends the moment its `until` passes (re-locking auto-kills
        // a live hotspot — OS behavior). Only while a charter exists: the
        // baseline owns the un-charted "everything cleared" state (I17). The
        // "filtered" mode additionally hosts Kintrinsic's own filtered AP via the
        // hotspot service (started/stopped level-triggered on the transition).
        val tetherMode =
            if (applyRestrictions && Provisioning.isDeviceOwner(context)) {
                runCatching { CharterCore.tetheringMode(nowUnix) }.getOrDefault("blocked")
            } else {
                "blocked" // no charter / Observe ⇒ the baseline owns the state
            }
        if (applyRestrictions && Provisioning.isDeviceOwner(context)) {
            restrictions.applyTetherMode(tetherMode)
        }
        // A filtered clause is PERMISSION, not an instruction to hold an AP up
        // all day: the ward's own switch decides (see HotspotWish — the battery
        // reason is written up there). Drop the wish whenever the permission
        // isn't there, so a re-grant never resurrects an AP nobody asked for.
        if (tetherMode != "filtered") HotspotWish.reset()
        // Sync the hotspot service UNCONDITIONALLY: a filtered session must be
        // torn down when enforcement stops (charter removed / Observe), not only
        // when a grant expires — otherwise a live AP would outlive its charter.
        syncHotspotService(tetherMode == "filtered" && HotspotWish.on)

        // Web-content filter: pin the DNS filter always-on (fail-closed) exactly
        // while enforcing a PAIRED ward, and un-pin otherwise so staged bring-up
        // and an unpaired device truly touch nothing (I17: inert until paired).
        // Gating on a subject — not just mode — is load-bearing: an unpaired DO
        // device defaults to enforce mode, and pinning a lockdown VPN there would
        // (a) break "inert until charter" and (b) risk blocking ALL data on a
        // device with no guardian to fix it if the VPN ever failed to establish.
        // Reconciled against the real OS pin state (the pin persists across
        // reboots, so an in-memory flag would drift). Unlike the app gate, the
        // web filter has no ward to serve until a subject exists.
        val enforcing = d.enforceMode != CharterCore.Mode.OBSERVE && d.subject != null
        syncDnsPin(enforcing)
        if (enforcing) {
            val plan = runCatching { CharterCore.dnsPlan() }.getOrNull()
            val rev = plan?.revision ?: ""
            // Latch the revision ONLY on a successful dispatch — a swallowed
            // background-start failure must not mark an unreached plan as
            // applied, or the new clause is silently dropped until the next
            // revision change (mirrors the lock-surface success-only latch).
            if (rev != appliedDnsRevision && dnsFilter.apply(rev)) {
                appliedDnsRevision = rev
            }
        }

        // Lock surface: level-triggered too (a dropped edge can never strand the
        // ward), and only in full Enforce (FreezeOnly = suspend, no lock UI).
        if (d.enforceMode == CharterCore.Mode.ENFORCE) {
            if (d.locked) lock.show(d.reason) else lock.hide()
        }
    }

    private fun syncBaseline(apply: Boolean) {
        if (!Provisioning.isDeviceOwner(context)) return
        if (apply && !baselineApplied) {
            restrictions.applyBaseline()
            baselineApplied = true
        } else if (!apply && baselineApplied) {
            restrictions.clearBaseline()
            baselineApplied = false
        }
    }

    /** Start/stop the filtered-hotspot foreground service on the rising/falling
     *  edge of the "filtered" posture. Edge-triggered so we don't re-issue a
     *  start every 2s tick; the service itself is idempotent besides. The first
     *  tick always reconciles (state starts UNKNOWN — see the field doc). */
    private fun syncHotspotService(wanted: Boolean) {
        if (wanted == hotspotServiceRunning) return
        if (wanted) {
            ensureNearbyWifi()
            org.forgesworn.charter.service.CharterHotspotService.start(context)
        } else {
            org.forgesworn.charter.service.CharterHotspotService.stop(context)
        }
        hotspotServiceRunning = wanted
    }

    /**
     * Self-grant NEARBY_WIFI_DEVICES and VERIFY it, immediately before a
     * filtered session is brought up. `startLocalOnlyHotspot` throws
     * SecurityException without it on API 33+, and the hotspot service catches
     * that and retries every 30s — so a missing grant looked exactly like "the
     * hotspot is broken", with the system hotspot ALSO locked by the filtered
     * posture. Found on Rob's phone 2026-07-26; the emulator suite never caught
     * it because the instrumented tests grant the permission themselves.
     *
     * Verified rather than assumed, for the ensureUsageAccess reason:
     * setPermissionGrantState returns false — it does not throw — when a build
     * refuses, and GrapheneOS has refused a grant before (issue #42).
     */
    private fun ensureNearbyWifi() {
        if (!Provisioning.isDeviceOwner(context)) return
        val perm = android.Manifest.permission.NEARBY_WIFI_DEVICES
        if (context.checkSelfPermission(perm) == android.content.pm.PackageManager.PERMISSION_GRANTED) return
        runCatching {
            dpm.setPermissionGrantState(
                admin,
                context.packageName,
                perm,
                DevicePolicyManager.PERMISSION_GRANT_STATE_GRANTED,
            )
        }
        if (context.checkSelfPermission(perm) != android.content.pm.PackageManager.PERMISSION_GRANTED) {
            Log.w(
                TAG,
                "NEARBY_WIFI_DEVICES not granted — the filtered guest hotspot cannot start. " +
                    "Manual fallback: adb shell pm grant ${context.packageName} $perm",
            )
        }
    }

    private fun syncDnsPin(pin: Boolean) {
        if (!Provisioning.isDeviceOwner(context)) return
        val pinned = runCatching { dnsFilter.isPinned() }.getOrDefault(false)
        if (pin && !pinned) {
            dnsFilter.pinAlwaysOn()
        } else if (!pin && pinned) {
            dnsFilter.clear()
            appliedDnsRevision = null // re-entering enforce re-applies the plan
        }
    }

    /**
     * The production warn delivery: a high-priority notification with the
     * EXACT Linux copy (runtime.rs:350-382) — the child's heads-up moments
     * before the lock lands.
     */
    class NotificationWarnNotifier(private val context: Context) :
        WardenController.WarnNotifier {
        override fun granted(minutes: Int) {
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
                as android.app.NotificationManager
            val channel = android.app.NotificationChannel(
                "charter-warnings",
                "Time warnings",
                android.app.NotificationManager.IMPORTANCE_HIGH,
            )
            nm.createNotificationChannel(channel)
            nm.notify(
                2003,
                android.app.Notification.Builder(context, "charter-warnings")
                    .setContentTitle("Kintrinsic — more time!")
                    .setContentText("Your guardian added $minutes minutes.")
                    .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
                    .setContentIntent(CharterService.openApp(context))
                    .setAutoCancel(true)
                    .build(),
            )
        }

        override fun denied() {
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
                as android.app.NotificationManager
            nm.createNotificationChannel(
                android.app.NotificationChannel(
                    "charter-warnings",
                    "Time warnings",
                    android.app.NotificationManager.IMPORTANCE_HIGH,
                ),
            )
            nm.notify(
                2004,
                android.app.Notification.Builder(context, "charter-warnings")
                    .setContentTitle("Kintrinsic — not this time")
                    .setContentText("Your guardian said not this time.")
                    .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
                    .setContentIntent(CharterService.openApp(context))
                    .setAutoCancel(true)
                    .build(),
            )
        }

        override fun warn(level: String) {
            val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
                as android.app.NotificationManager
            val channel = android.app.NotificationChannel(
                "charter-warnings",
                "Time warnings",
                android.app.NotificationManager.IMPORTANCE_HIGH,
            ).apply { description = "Heads-up before screen time runs out" }
            nm.createNotificationChannel(channel)
            val text = if (level == "one") {
                "1 minute left — save your game"
            } else {
                "10 minutes left"
            }
            nm.notify(
                if (level == "one") 2002 else 2001,
                android.app.Notification.Builder(context, "charter-warnings")
                    .setContentTitle("Kintrinsic — time's almost up")
                    .setContentText(text)
                    .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
                    .setContentIntent(CharterService.openApp(context))
                    .setAutoCancel(true)
                    .build(),
            )
        }
    }

    companion object {
        private const val TAG = "WardenController"

        /** Enumerating every installed package costs real time on a phone, so
         *  an open window's account refreshes on this cadence rather than every
         *  tick. A window that shuts is always accounted once more regardless. */
        private const val ACCOUNT_REFRESH_SECS = 30L

        /** The production wiring: DPM-backed ops + the real lock activity. */
        fun real(context: Context): WardenController {
            val dpm = Provisioning.dpm(context)
            val admin = Provisioning.adminComponent(context)
            return WardenController(
                context = context,
                dpm = dpm,
                admin = admin,
                appGate = DpmAppGateOps(context, dpm, admin),
                restrictions = DpmRestrictionOps(dpm, admin),
                usage = UsageStatsSource(context),
                install = DpmApkInstallOps(context),
                dnsFilter = VpnDnsFilterOps(context, dpm, admin),
                lock = ActivityLockController(context),
                warn = NotificationWarnNotifier(context),
                holdNotifier =
                    org.forgesworn.charter.enforce.NotificationHoldNotifier(context),
                holdMemory = org.forgesworn.charter.enforce.HoldMemoryStore(context),
                ruleMemory = org.forgesworn.charter.enforce.RuleMemoryStore(context),
            )
        }
    }
}

/** Launches / dismisses the real [LockActivity]. */
class ActivityLockController(private val context: Context) : WardenController.LockController {
    @Volatile private var shown = false

    override fun show(reason: String) {
        if (shown) return
        val intent = Intent(context, LockActivity::class.java).apply {
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TOP)
            putExtra("reason", reason)
        }
        // Latch `shown` ONLY on a successful launch. If the start is suppressed
        // (background-activity-launch limits), leave it false so the next
        // level-triggered tick retries — the ward is never left suspended with
        // no lock UI (the review's latch bug).
        try {
            context.startActivity(intent)
            shown = true
        } catch (t: Throwable) {
            shown = false
        }
    }

    override fun hide() {
        shown = false
        // The lock activity observes HideLock via a broadcast and finishes.
        context.sendBroadcast(Intent(ACTION_HIDE_LOCK).setPackage(context.packageName))
    }

    companion object {
        const val ACTION_HIDE_LOCK = "org.forgesworn.charter.HIDE_LOCK"
    }
}
