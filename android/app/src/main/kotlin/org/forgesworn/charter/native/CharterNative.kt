package org.forgesworn.charter.native

/**
 * Raw JNI declarations backed by libcharter_jni.so (android/jni, cargo-ndk).
 *
 * All payloads are UTF-8 JSON strings; hex is strict lowercase. The Rust core
 * owns decide/verify/schedule/crypto — single-sourced with the Linux warden.
 * Never call these from the JVM main thread: every call takes the warden lock
 * (port-spec §2.3/§3.2).
 */
object CharterNative {
    init {
        System.loadLibrary("charter_jni")
    }

    /** Surface version; Kotlin refuses to run on a mismatch. */
    external fun charterAbiVersion(): Int

    /** Initialize the warden over [baseDir]; returns init JSON. */
    external fun charterInit(baseDir: String, enforceMode: String, appVersionCode: Long): String

    /** Machine pubkey grouped 8×8 for the pairing display. */
    external fun charterDeviceCode(): String

    /** `{paired, guardianShort, guardian, subject}`. */
    external fun charterPairingState(): String

    /** Onboarding/test pairing setter; returns the new pairing state JSON. */
    external fun charterSetPairing(guardianHex: String, subjectHex: String): String

    /**
     * The REAL pairing: pin the guardian from the `bunker://` link pasted from
     * Kintrinsic. Returns the new pairing-state JSON, or `{"error": …}` with
     * parent-facing text.
     */
    external fun charterPair(bunkerUri: String, nowUnix: Long): String

    /**
     * One slow-tick relay round: pull + authenticate + store CLAUSE wraps,
     * then emit the STATUS heartbeat. BLOCKING network IO (seconds) — slow
     * worker thread only; the enforcement tick is never stalled by it.
     */
    external fun charterPollOnce(nowUnix: Long): String

    /** Authenticate + store a signed CLAUSE event; returns the result JSON. */
    external fun charterIngestClause(eventJson: String, nowUnix: Long): String

    /**
     * The ward's standing per-app policy AS ENFORCED AT [nowUnix] (`GrantApps`
     * JSON), or "" when none. Any live time-boxed hold is already folded into
     * the lists, so this stays a plain posture + lists answer. Takes the clock
     * because a hold ends at an absolute instant and is re-resolved each tick.
     */
    external fun charterAppPolicy(nowUnix: Long): String

    /**
     * The ward's LIVE app holds at [nowUnix]: a JSON array of
     * `{pkg, state, untilUnix}`, or "" when none.
     */
    external fun charterAppHolds(nowUnix: Long): String

    /**
     * The ward's per-app RULE suspensions for [nowUnix]: a JSON array of the
     * packages whose `appRules` access is Blocked right now (blocked outright,
     * or outside their allowed hours), or "[]" when none. Schedule-dependent —
     * recomputed each call; takes the warden lock, so worker thread only.
     */
    external fun charterAppRuleSuspensions(nowUnix: Long): String

    /**
     * The ward's per-bucket ("named time") suspensions for [nowUnix]: a JSON
     * array of the packages whose bucket has spent either axis (day or week)
     * right now, or "[]" when none. Extra-adjusted (a granted
     * `time.extend`/gift to a bucket lifts both walls today) and recomputed
     * each call, like [charterAppRuleSuspensions]. Spending a bucket suspends
     * only ITS apps, never the whole device.
     */
    external fun charterBucketSuspensions(nowUnix: Long): String

    /**
     * Every named bucket's day/week picture plus the labelled `askFirst`
     * list: `{"buckets":[…],"askFirst":[{"pkg","label"}]}` JSON — the ward's
     * own mirror surface (spec D-Bus parity).
     */
    external fun charterBucketViews(nowUnix: Long): String

    /**
     * The ward's tethering posture at [nowUnix]: "blocked", "raw", or
     * "filtered". Time-boxed grants end AT their `until`, so this is
     * recomputed each tick; takes the warden lock, so worker thread only.
     */
    external fun charterTetheringMode(nowUnix: Long): String

    /** The ward's effective web-content policy as a DNS plan (`{"revision","plan":{…}}` JSON), or "" when unpaired. */
    external fun charterDnsPlan(): String

    /** The ward's lifeline numbers (`[{"label","number"}]` JSON), or "" when
     *  none. One lock-screen call button per entry (spec D9). */
    external fun charterLifeline(): String

    /** The ward's emergency override: unlocks now, tells the guardian. */
    external fun charterBreakGlass(nowUnix: Long): String

    /** The ward-facing "your charter" mirror (spec D8):
     *  `{"locked","minutesLeft","detail","lines"}` JSON, or "" when no charter
     *  is configured yet. Takes the warden lock — worker thread only. */
    external fun charterScheduleView(nowUnix: Long): String

    /** Report the device's installed launchable apps (JSON `[{pkg,label}]`) —
     *  ridden on the next STATUS so the guardian can pick apps by name. */
    external fun charterSetInstalledApps(appsJson: String): String

    /** Report the platform boot counter (`Settings.Global.BOOT_COUNT`) this
     *  process is running under. The warden counts the boots it did NOT run
     *  through — the only trace safe mode leaves — and rides the tally on
     *  the next STATUS. Call once per warden start, before the first tick. */
    external fun charterNoteBoot(bootCount: Long, nowUnix: Long): String

    /** The install window as this device saw it:
     *  `{"startedAt","endedAt","open"}`, or `{}` when none. Records the
     *  open/close edges as a side effect — worker thread only. */
    external fun charterMaintenanceSpan(nowUnix: Long): String

    /** Report what changed during that span (JSON
     *  `{"startedAt","endedAt","changes":[{pkg,label,kind,at}]}`) — ridden on
     *  the next STATUS as the guardian's account of their own loosening. */
    external fun charterSetInstallWindow(reportJson: String): String

    /**
     * One enforcement tick; returns `[ChildDecision]` JSON. [foregroundPkg] is
     * the SAME single foreground-package probe the caller already made for
     * the screen credit — never a second probe — so a bucketed app's tick is
     * ALSO credited to its named-times bucket, alongside the ordinary screen
     * credit.
     */
    external fun charterTick(
        activeSubjectHex: String?,
        screenInteractive: Boolean,
        nowUnix: Long,
        foregroundPkg: String?,
    ): String

    /** `TimeLeftView` JSON (unknown → locked). */
    external fun charterTimeLeft(subjectHex: String?, nowUnix: Long): String

    /** Child-facing lock copy JSON. */
    external fun charterLockInfo(subjectHex: String?, nowUnix: Long): String

    /** Packages that may keep playing through the lock + the grace remaining.
     *  `audioPlaying` must come from the platform — the exemption is about
     *  audio actually sounding, not about an app merely being named. */
    external fun charterListeningView(
        nowUnix: Long,
        locked: Boolean,
        audioPlaying: Boolean,
    ): String

    /** Packages that may be OPENED though the device is locked. Takes the lock
     *  REASON because only "schedule" and "budget" exempt anything — a
     *  stand-down or a malformed charter never does. */
    external fun charterAlwaysAvailable(
        nowUnix: Long,
        locked: Boolean,
        lockReason: String,
    ): String

    /** Set the staged-bring-up enforcement mode (observe|freezeOnly|enforce). */
    external fun charterSetEnforceMode(mode: String)

    /**
     * Submit a brokered ask (op = "time.extend"). Persists Pending BEFORE
     * publishing; returns {"reqId": …} or {"error": …}. BLOCKING network IO —
     * worker thread only.
     */
    external fun charterSubmitRequest(op: String, paramsJson: String): String

    /** Newest-first request records (`[RequestRecord]` JSON). */
    external fun charterListRequests(limit: Int): String

    /** Records for one reqId ("" = all). */
    external fun charterQueryStatus(reqId: String): String

    /** Cancel a Pending ask; "true" iff it was Pending. */
    external fun charterCancelRequest(reqId: String): String

    /**
     * Every guardian-approved install currently parked (`[PendingInstall]`
     * JSON), oldest-first. The device performs each install — verifying signing
     * continuity against `signerCertSha256` BEFORE committing — then reports
     * each outcome via [charterInstallResult].
     */
    external fun charterDrainInstalls(nowUnix: Long): String

    /**
     * Report an install outcome: [status] ∈ {"ok","terminal","transient"}.
     * `ok`/`terminal` clear the directive; `transient` leaves it for a retry.
     */
    external fun charterInstallResult(reqId: String, status: String, reason: String): String

    /** A stuck update as JSON, or "" when nothing is failing. */
    external fun charterInstallHealth(nowUnix: Long): String

    /** `{"open":bool}` — is a guardian's maintenance window open right now? */
    external fun charterMaintenanceOpen(nowUnix: Long): String
}
