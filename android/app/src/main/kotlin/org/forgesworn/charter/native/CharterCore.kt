package org.forgesworn.charter.native

import org.json.JSONArray
import org.json.JSONObject

/**
 * Typed wrapper over [CharterNative]. Parses the JSON boundary into Kotlin data
 * classes so the rest of the app never touches raw JSON. Pure translation — the
 * decisions themselves come from the shared Rust core.
 */
object CharterCore {

    data class InitResult(
        val machinePubkey: String,
        val paired: Boolean,
        val guardian: String?,
        val subject: String?,
        val enforceMode: String,
        val error: String?,
    )

    data class PairingState(
        val paired: Boolean,
        val guardianShort: String?,
        val subject: String?,
        val relays: List<String> = emptyList(),
        /** Parent-facing rejection text when a `pair` call was refused. */
        val error: String? = null,
    )

    data class PollResult(
        /** False when nothing could be polled (unpaired / no relays / not init). */
        val polled: Boolean,
        val clausesSeen: Int,
        val clausesAccepted: Int,
        val statusEmitted: Boolean,
        val publishFailures: Int,
        /** Why nothing was polled / the transport error (offline is routine). */
        val reason: String,
    )

    data class ClauseResult(val accepted: Boolean, val kind: String?, val reason: String)

    sealed interface Effect {
        data class Warn(val level: String) : Effect
        data class Granted(val minutes: Int) : Effect
        data class Audit(val outcome: String) : Effect
        /** The guardian answered no — announced once per ask. */
        data object Denied : Effect
    }

    enum class Mode { OBSERVE, FREEZE_ONLY, ENFORCE;
        companion object {
            fun parse(s: String) = when (s) {
                "observe" -> OBSERVE
                "freezeOnly" -> FREEZE_ONLY
                else -> ENFORCE
            }
        }
    }

    data class ChildDecision(
        val subject: String?,
        val locked: Boolean,
        val reason: String,
        val effects: List<Effect>,
        val remainingSecs: Long,
        /** A signed charter EXISTS for this ward — the restriction gate (I17/I28). */
        val configured: Boolean,
        val enforceMode: Mode,
    )

    data class TimeLeft(
        val known: Boolean,
        val locked: Boolean,
        val reason: String,
        val effectiveSecs: Long,
        val nextOpenSecs: Long?,
    )

    data class LockInfo(
        val locked: Boolean,
        val reason: String,
        val title: String,
        val comeBack: String,
        val usedLine: String,
        /** Off until a guardian opens it — no allowed hours at all, not merely
         *  a closed window. Same `reason` as any schedule lock (the grant
         *  routing needs that), so the shade tells them apart by this. */
        val dormant: Boolean,
    )

    fun abiVersion(): Int = CharterNative.charterAbiVersion()

    fun init(baseDir: String, enforceMode: String, appVersionCode: Long): InitResult {
        val o = JSONObject(CharterNative.charterInit(baseDir, enforceMode, appVersionCode))
        return InitResult(
            machinePubkey = o.optString("machine_pubkey", o.optString("machinePubkey", "")),
            paired = o.optBoolean("paired", false),
            guardian = o.optStringOrNull("guardian"),
            subject = o.optStringOrNull("subject"),
            enforceMode = o.optString("enforce_mode", o.optString("enforceMode", "enforce")),
            error = o.optStringOrNull("error"),
        )
    }

    fun deviceCode(): String = CharterNative.charterDeviceCode()

    fun pairingState(): PairingState =
        parsePairingState(JSONObject(CharterNative.charterPairingState()))

    fun setPairing(guardianHex: String, subjectHex: String): PairingState =
        parsePairingState(JSONObject(CharterNative.charterSetPairing(guardianHex, subjectHex)))

    /**
     * Pin the guardian from a pasted `bunker://` link. On refusal the returned
     * state carries [PairingState.error] (parent-facing text) and `paired`
     * reflects the unchanged prior state.
     */
    fun pair(bunkerUri: String, nowUnix: Long): PairingState {
        val o = JSONObject(CharterNative.charterPair(bunkerUri, nowUnix))
        return if (o.has("error")) {
            pairingState().copy(error = o.optString("error"))
        } else {
            parsePairingState(o)
        }
    }

    /** One relay round. Blocking network IO — call from the slow worker only. */
    fun pollOnce(nowUnix: Long): PollResult {
        val o = JSONObject(CharterNative.charterPollOnce(nowUnix))
        return PollResult(
            polled = o.optBoolean("polled", false),
            clausesSeen = o.optInt("clauses_seen", 0),
            clausesAccepted = o.optInt("clauses_accepted", 0),
            statusEmitted = o.optBoolean("status_emitted", false),
            publishFailures = o.optInt("publish_failures", 0),
            reason = o.optString("reason", ""),
        )
    }

    private fun parsePairingState(o: JSONObject): PairingState {
        val relaysArr = o.optJSONArray("relays") ?: JSONArray()
        return PairingState(
            paired = o.optBoolean("paired", false),
            guardianShort = o.optStringOrNull("guardian_short"),
            subject = o.optStringOrNull("subject"),
            relays = (0 until relaysArr.length()).map { relaysArr.getString(it) },
            error = o.optStringOrNull("error"),
        )
    }

    /** The ward's standing per-app policy. `posture` = "blocklist" | "allowlist". */
    data class AppPolicy(
        val posture: String,
        val blocked: List<String>,
        val allowed: List<String>,
    )

    /**
     * One app held away from its standing rule until [untilUnix] (absolute unix
     * seconds). `state` is "allowed" or "blocked".
     */
    data class AppHold(val pkg: String, val state: String, val untilUnix: Long)

    /**
     * The holds actually in force at [nowUnix]. Empty when none / no clause /
     * malformed. Only for TELLING the ward — enforcement reads [appPolicy],
     * which has already dissolved these into its lists.
     */
    fun appHolds(nowUnix: Long): List<AppHold> {
        val raw = CharterNative.charterAppHolds(nowUnix)
        if (raw.isBlank()) return emptyList()
        return try {
            val a = JSONArray(raw)
            (0 until a.length()).mapNotNull {
                val o = a.optJSONObject(it) ?: return@mapNotNull null
                val pkg = o.optString("pkg", "")
                if (pkg.isBlank()) return@mapNotNull null
                AppHold(pkg, o.optString("state", "allowed"), o.optLong("untilUnix", 0L))
            }
        } catch (_: Throwable) {
            emptyList()
        }
    }

    /**
     * Parse the per-app policy AS ENFORCED AT [nowUnix]; null when none applies
     * (empty / paused). Live holds are already folded into the lists.
     */
    fun appPolicy(nowUnix: Long): AppPolicy? {
        val raw = CharterNative.charterAppPolicy(nowUnix)
        if (raw.isBlank()) return null
        return try {
            val o = JSONObject(raw)
            fun list(key: String): List<String> {
                val a = o.optJSONArray(key) ?: return emptyList()
                return (0 until a.length()).map { a.getString(it) }
            }
            AppPolicy(
                posture = o.optString("posture", "blocklist"),
                blocked = list("blocked"),
                allowed = list("allowed"),
            )
        } catch (_: Throwable) {
            null
        }
    }

    /**
     * The packages the per-app RULE clause blocks RIGHT NOW at [nowUnix] —
     * blocked outright, or outside their allowed hours. Schedule-dependent, so
     * it is recomputed each tick. Empty when none apply / no clause / malformed
     * (fail-safe: never suspend on a parse error).
     */
    fun appRuleSuspensions(nowUnix: Long): List<String> {
        val raw = CharterNative.charterAppRuleSuspensions(nowUnix)
        if (raw.isBlank()) return emptyList()
        return try {
            val a = JSONArray(raw)
            (0 until a.length()).map { a.getString(it) }
        } catch (_: Throwable) {
            emptyList()
        }
    }

    /**
     * The packages a named-times bucket has spent (either axis) right now —
     * empty when none apply / no clause / malformed (fail-safe: never
     * suspend on a parse error). Spending a bucket suspends only ITS apps,
     * never the whole device.
     */
    fun bucketSuspensions(nowUnix: Long): List<String> {
        val raw = CharterNative.charterBucketSuspensions(nowUnix)
        if (raw.isBlank()) return emptyList()
        return try {
            val a = JSONArray(raw)
            (0 until a.length()).map { a.getString(it) }
        } catch (_: Throwable) {
            emptyList()
        }
    }

    /** One named bucket's day/week picture ("Play: 22 of 60 used"). */
    data class BucketView(
        val id: String,
        val label: String,
        val usedSeconds: Long,
        val limitSeconds: Long,
        val remainingSeconds: Long,
        /** -1 = no weekly cap set (or the whole set is paused). */
        val weekLimitSeconds: Long,
        val weekRemainingSeconds: Long,
        val spent: Boolean,
        val capped: Boolean,
    )

    /** One `askFirst` app still gated right now, labelled for the ward. */
    data class AskFirstApp(val pkg: String, val label: String)

    /** Every named bucket's picture plus the labelled `askFirst` list — the
     *  ward's own mirror surface (Task 9 UI consumes this). */
    fun bucketViews(nowUnix: Long): Pair<List<BucketView>, List<AskFirstApp>> {
        val raw = CharterNative.charterBucketViews(nowUnix)
        if (raw.isBlank()) return emptyList<BucketView>() to emptyList()
        return try {
            val o = JSONObject(raw)
            val buckets = o.optJSONArray("buckets") ?: JSONArray()
            val views = (0 until buckets.length()).map {
                val b = buckets.getJSONObject(it)
                BucketView(
                    id = b.optString("id", ""),
                    label = b.optString("label", ""),
                    usedSeconds = b.optLong("usedSeconds", 0),
                    limitSeconds = b.optLong("limitSeconds", 0),
                    remainingSeconds = b.optLong("remainingSeconds", 0),
                    weekLimitSeconds = b.optLong("weekLimitSeconds", -1),
                    weekRemainingSeconds = b.optLong("weekRemainingSeconds", -1),
                    spent = b.optBoolean("spent", false),
                    capped = b.optBoolean("capped", false),
                )
            }
            val askFirstArr = o.optJSONArray("askFirst") ?: JSONArray()
            val askFirst = (0 until askFirstArr.length()).map {
                val a = askFirstArr.getJSONObject(it)
                AskFirstApp(pkg = a.optString("pkg", ""), label = a.optString("label", ""))
            }
            views to askFirst
        } catch (_: Throwable) {
            emptyList<BucketView>() to emptyList()
        }
    }

    /** The ward's tethering posture right now — anything unexpected reads as
     *  "blocked" (fail-safe, matching the Rust side's every-direction default). */
    fun tetheringMode(nowUnix: Long): String =
        when (val m = CharterNative.charterTetheringMode(nowUnix)) {
            "raw", "filtered" -> m
            else -> "blocked"
        }

    /** One forced-resolution rewrite (host -> answer) for SafeSearch / YouTube. */
    data class DnsRewrite(val host: String, val answer: String)

    /**
     * The ward's effective web-content policy as a DNS-layer plan. `mode` ∈
     * {"allowlist","blocklist","unrestricted","locked"}. Rendered by the SHARED
     * charter-webpolicy renderer — Kotlin only enacts it, never re-decides.
     */
    data class DnsPlan(
        val revision: String,
        val mode: String,
        val allowDomains: List<String>,
        val blockDomains: List<String>,
        val blockCategories: List<String>,
        val allowExceptions: List<String>,
        val safeSearch: Boolean,
        val youtubeRestrict: String,
        val rewrites: List<DnsRewrite>,
    )

    /** Parse the current DNS plan; null when not paired (blank) or malformed. */
    fun dnsPlan(): DnsPlan? {
        val raw = CharterNative.charterDnsPlan()
        if (raw.isBlank()) return null
        return try {
            val root = JSONObject(raw)
            val p = root.getJSONObject("plan")
            fun list(key: String): List<String> {
                val a = p.optJSONArray(key) ?: return emptyList()
                return (0 until a.length()).map { a.getString(it) }
            }
            val rw = p.optJSONArray("rewrites")
            val rewrites = if (rw == null) emptyList() else (0 until rw.length()).map {
                val o = rw.getJSONObject(it)
                DnsRewrite(o.getString("host"), o.getString("answer"))
            }
            DnsPlan(
                revision = root.optString("revision", ""),
                mode = p.optString("mode", "locked"),
                allowDomains = list("allowDomains"),
                blockDomains = list("blockDomains"),
                blockCategories = list("blockCategories"),
                allowExceptions = list("allowExceptions"),
                safeSearch = p.optBoolean("safeSearch", true),
                youtubeRestrict = p.optString("youtubeRestrict", "off"),
                rewrites = rewrites,
            )
        } catch (_: Throwable) {
            // Fail-closed: an unparseable plan is treated as locked by the resolver.
            DnsPlan("", "locked", emptyList(), emptyList(), emptyList(), emptyList(), true, "off", emptyList())
        }
    }

    /** Report the device's installed launchable apps (JSON `[{pkg,label}]`). */
    fun setInstalledApps(appsJson: String) {
        CharterNative.charterSetInstalledApps(appsJson)
    }

    /** Report the boot this warden is running under, so it can count the ones
     *  it wasn't running for. See [CharterNative.charterNoteBoot]. */
    fun noteBoot(bootCount: Long, nowUnix: Long) {
        CharterNative.charterNoteBoot(bootCount, nowUnix)
    }

    fun ingestClause(eventJson: String, nowUnix: Long): ClauseResult {
        val o = JSONObject(CharterNative.charterIngestClause(eventJson, nowUnix))
        return ClauseResult(
            accepted = o.optBoolean("accepted", false),
            kind = o.optStringOrNull("kind"),
            reason = o.optString("reason", "error"),
        )
    }

    /**
     * [foregroundPkg] is the SAME foreground-package probe the caller already
     * made for the screen credit (never a second probe) — it also attributes
     * this tick to a named-times bucket when the package belongs to one.
     */
    fun tick(
        activeSubjectHex: String?,
        screenInteractive: Boolean,
        nowUnix: Long,
        foregroundPkg: String? = null,
    ): List<ChildDecision> {
        val arr = JSONArray(
            CharterNative.charterTick(activeSubjectHex, screenInteractive, nowUnix, foregroundPkg),
        )
        return (0 until arr.length()).map { parseDecision(arr.getJSONObject(it)) }
    }

    fun timeLeft(subjectHex: String?, nowUnix: Long): TimeLeft {
        val o = JSONObject(CharterNative.charterTimeLeft(subjectHex, nowUnix))
        return TimeLeft(
            known = o.optBoolean("known", false),
            locked = o.optBoolean("locked", true),
            reason = o.optString("reason", "unknown"),
            effectiveSecs = o.optLong("effectiveSecs", 0),
            nextOpenSecs = if (o.isNull("nextOpenSecs")) null else o.optLong("nextOpenSecs"),
        )
    }

    /** What may keep playing through the lock, and how long is left under a
     *  grace (null when the mode is `continue`, or when nothing is exempt). */
    data class ListeningView(val exempt: Set<String>, val secsLeft: Long?)

    fun listeningView(nowUnix: Long, locked: Boolean, audioPlaying: Boolean): ListeningView {
        val o = JSONObject(CharterNative.charterListeningView(nowUnix, locked, audioPlaying))
        val arr = o.optJSONArray("exempt")
        val pkgs = buildSet {
            for (i in 0 until (arr?.length() ?: 0)) arr!!.optString(i)?.let { add(it) }
        }
        return ListeningView(
            exempt = pkgs,
            secsLeft = if (o.isNull("secsLeft")) null else o.optLong("secsLeft"),
        )
    }

    /** Apps the family agreed are open at any hour, live right now. Empty
     *  whenever the clause is absent, unusable, expired, or the lock is one
     *  this clause may not outlive. */
    fun alwaysAvailable(nowUnix: Long, locked: Boolean, lockReason: String): Set<String> {
        val arr = JSONObject(CharterNative.charterAlwaysAvailable(nowUnix, locked, lockReason))
            .optJSONArray("open")
        return buildSet {
            for (i in 0 until (arr?.length() ?: 0)) arr!!.optString(i)?.let { add(it) }
        }
    }

    fun lockInfo(subjectHex: String?, nowUnix: Long): LockInfo {
        val o = JSONObject(CharterNative.charterLockInfo(subjectHex, nowUnix))
        return LockInfo(
            locked = o.optBoolean("locked", true),
            reason = o.optString("reason", "unknown"),
            title = o.optString("title", "Locked"),
            comeBack = o.optString("comeBack", ""),
            usedLine = o.optString("usedLine", ""),
            // Defaults false: an older core that doesn't send the field leaves
            // the shade exactly as it was, rather than mislabelling a normal
            // schedule lock as a device that is off.
            dormant = o.optBoolean("dormant", false),
        )
    }

    fun setEnforceMode(mode: String) = CharterNative.charterSetEnforceMode(mode)

    /** One lifeline entry the lock screen can dial (spec D9). */
    data class LifelineNumber(val label: String, val number: String)

    /** What the ward's break-glass unlock opens, and for how long. */
    data class BreakGlass(
        val enabled: Boolean,
        /** "calls" | "full" */
        val scope: String,
        val durationMinutes: Int,
    )

    /**
     * The ward's lifeline: guardian numbers, whether to show the platform's
     * own emergency number, and the break-glass config (all v2, 2026-07-24).
     * Fail-closed: anything unparseable yields no numbers and both features
     * off.
     */
    data class LifelineView(
        val numbers: List<LifelineNumber> = emptyList(),
        val emergencyServices: Boolean = false,
        /** Offer a torch on the shade — a locked phone is still a light. */
        val torch: Boolean = false,
        val breakGlass: BreakGlass? = null,
    )

    private fun parseNumbers(arr: org.json.JSONArray?): List<LifelineNumber> {
        if (arr == null) return emptyList()
        return (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val label = o.optString("label", "")
            val number = o.optString("number", "")
            if (label.isEmpty() || number.isEmpty()) null else LifelineNumber(label, number)
        }
    }

    /**
     * Reads the core's lifeline view. Accepts BOTH the v2 object and the
     * legacy bare array — an in-flight .so/APK mismatch must never blank the
     * lifeline, because "can't call a guardian" is the one failure this
     * feature exists to prevent.
     */
    fun lifelineView(): LifelineView {
        val raw = CharterNative.charterLifeline()
        if (raw.isEmpty()) return LifelineView()
        return runCatching {
            if (raw.trimStart().startsWith("[")) {
                LifelineView(numbers = parseNumbers(org.json.JSONArray(raw)))
            } else {
                val o = org.json.JSONObject(raw)
                val bg = o.optJSONObject("breakGlass")?.let {
                    BreakGlass(
                        enabled = it.optBoolean("enabled", false),
                        scope = it.optString("scope", "calls"),
                        durationMinutes = it.optInt("durationMinutes", 0),
                    )
                }
                LifelineView(
                    numbers = parseNumbers(o.optJSONArray("numbers")),
                    emergencyServices = o.optBoolean("emergencyServices", false),
                    torch = o.optBoolean("torch", false),
                    breakGlass = bg?.takeIf { it.enabled && it.durationMinutes > 0 },
                )
            }
        }.getOrDefault(LifelineView())
    }

    /** The ward's lifeline numbers; empty when none / malformed (fail-closed). */
    fun lifeline(): List<LifelineNumber> = lifelineView().numbers

    /** The outcome of breaking the glass. */
    data class BreakGlassResult(
        val allowed: Boolean,
        val scope: String = "",
        val secsLeft: Long = 0,
        val reason: String = "",
    )

    /**
     * The ward's emergency override: unlocks NOW (no approval, no network
     * needed) and journals it for the guardian. Refused only when the family
     * hasn't enabled it.
     */
    fun breakGlass(nowUnix: Long): BreakGlassResult {
        val raw = CharterNative.charterBreakGlass(nowUnix)
        if (raw.isEmpty()) return BreakGlassResult(false, reason = "unavailable")
        return runCatching {
            val o = org.json.JSONObject(raw)
            BreakGlassResult(
                allowed = o.optBoolean("allowed", false),
                scope = o.optString("scope", ""),
                secsLeft = o.optLong("secsLeft", 0),
                reason = o.optString("reason", ""),
            )
        }.getOrDefault(BreakGlassResult(false, reason = "unavailable"))
    }

    /** The ward-facing "your charter" mirror (spec D8). */
    data class ScheduleView(
        val locked: Boolean,
        /** `-1` = no WHOLE-DEVICE time wall at all (e.g. a buckets-only
         *  ward) — the "unset, never a clamped 0" sentinel this surface
         *  uses everywhere else too. Never feed it through [TimeText] as-is. */
        val minutesLeft: Long,
        val secondsLeft: Long,
        val detail: String?,
        val lines: List<String>,
        /** The week's out-of-hours use, straight off THIS device's own
         *  ledger (spec 2026-08-03) — raw numbers, not a pre-formatted
         *  sentence, so [GroupMirror.outOfHoursLine] can render the SAME
         *  wording the guardian sees (`outOfHoursLine` in
         *  `insights/usageHistory.ts`). `0` before the first tick / with no
         *  out-of-hours use this week — never a signal on their own, only
         *  meaningful together via [GroupMirror.outOfHoursLine]. */
        val outOfHoursNightsWeek: Int = 0,
        val outOfHoursWeekSecs: Long = 0,
    )

    /** Null when no charter is configured yet ("no charter yet", never a guess). */
    fun scheduleView(nowUnix: Long): ScheduleView? {
        val raw = CharterNative.charterScheduleView(nowUnix)
        if (raw.isEmpty()) return null
        return runCatching {
            val o = JSONObject(raw)
            val lines = o.optJSONArray("lines")?.let { arr ->
                (0 until arr.length()).map { arr.optString(it, "") }.filter { it.isNotEmpty() }
            } ?: emptyList()
            ScheduleView(
                locked = o.optBoolean("locked", true),
                minutesLeft = o.optLong("minutesLeft", 0),
                secondsLeft = o.optLong("secondsLeft", o.optLong("minutesLeft", 0) * 60),
                // NB optString turns JSON null into the STRING "null" — the
                // on-metal round rendered a literal "null" line. isNull first.
                detail = if (o.isNull("detail")) null
                    else o.optString("detail", "").takeIf { it.isNotEmpty() },
                lines = lines,
                outOfHoursNightsWeek = o.optInt("outOfHoursNightsWeek", 0),
                outOfHoursWeekSecs = o.optLong("outOfHoursWeekSecs", 0),
            )
        }.getOrNull()
    }

    data class SubmitResult(val reqId: String?, val error: String?)

    data class RequestRecord(
        val reqId: String,
        val op: String,
        val state: String,
        val createdAt: Long,
        val detail: String?,
    )

    /** Ask the guardian ("time.extend"). Blocking network IO — worker only. */
    fun submitRequest(op: String, paramsJson: String): SubmitResult {
        val o = JSONObject(CharterNative.charterSubmitRequest(op, paramsJson))
        return SubmitResult(
            reqId = o.optStringOrNull("reqId"),
            error = o.optStringOrNull("error"),
        )
    }

    /** Newest-first request records. */
    fun listRequests(limit: Int): List<RequestRecord> {
        val arr = JSONArray(CharterNative.charterListRequests(limit))
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            RequestRecord(
                reqId = o.optString("req_id"),
                op = o.optString("op"),
                state = o.optString("state"),
                createdAt = o.optLong("created_at"),
                detail = o.optStringOrNull("detail"),
            )
        }
    }

    /** A guardian-approved install the core has parked for the device to enact. */
    data class PendingInstall(
        val reqId: String,
        val packageName: String,
        /** The minimum version to accept, if the grant pinned one. */
        val versionCode: Long?,
        /** The signing-cert SHA-256 the archive MUST match before committing. */
        val signerCertSha256: String,
        val source: String,
        /** `source == "url"` only: where to fetch the archive. */
        val url: String? = null,
        /** `source == "url"` only: sha256 the fetched bytes MUST hash to. */
        val apkSha256: String? = null,
    )

    /** How an install attempt resolved — drives [installResult]. */
    enum class InstallOutcome(val wire: String) {
        /** Installed, or already present at the same-or-newer version. */
        OK("ok"),

        /** A permanent refusal — signing mismatch, corrupt/absent archive. */
        TERMINAL("terminal"),

        /** A retryable failure — try again next tick. */
        TRANSIENT("transient"),
    }

    /** Every parked install, oldest-first. */
    fun drainInstalls(nowUnix: Long): List<PendingInstall> {
        val arr = JSONArray(CharterNative.charterDrainInstalls(nowUnix))
        return (0 until arr.length()).map { i ->
            val o = arr.getJSONObject(i)
            PendingInstall(
                reqId = o.optString("reqId"),
                packageName = o.optString("packageName"),
                versionCode = if (o.isNull("versionCode") || !o.has("versionCode")) {
                    null
                } else {
                    o.optLong("versionCode")
                },
                signerCertSha256 = o.optString("signerCertSha256"),
                url = if (o.has("url")) o.optString("url") else null,
                apkSha256 = if (o.has("apkSha256")) o.optString("apkSha256") else null,
                source = o.optString("source"),
            )
        }
    }

    /** Report an install outcome so the core clears or retries the directive. */
    fun installResult(reqId: String, outcome: InstallOutcome, reason: String = "") {
        CharterNative.charterInstallResult(reqId, outcome.wire, reason)
    }

    private fun parseDecision(o: JSONObject): ChildDecision {
        val effs = o.optJSONArray("effects") ?: JSONArray()
        val list = (0 until effs.length()).mapNotNull { parseEffect(effs.getJSONObject(it)) }
        return ChildDecision(
            subject = o.optStringOrNull("subject"),
            locked = o.optBoolean("locked", false),
            reason = o.optString("reason", "none"),
            effects = list,
            remainingSecs = o.optLong("remainingSecs", -1),
            // Default configured=true if the field is somehow absent: fail SAFE
            // toward keeping restrictions, never toward tearing them down.
            configured = o.optBoolean("configured", true),
            enforceMode = Mode.parse(o.optString("enforceMode", "enforce")),
        )
    }

    private fun parseEffect(o: JSONObject): Effect? = when (o.optString("kind")) {
        "warn" -> Effect.Warn(o.optString("level", "ten"))
        "granted" -> Effect.Granted(o.optInt("minutes", 0))
        "audit" -> Effect.Audit(o.optString("outcome", ""))
        "denied" -> Effect.Denied
        else -> null
    }

    private fun JSONObject.optStringOrNull(key: String): String? =
        if (isNull(key) || !has(key)) null else optString(key)

    /**
     * A Kintrinsic update that keeps failing, as raw JSON for STATUS (or "" when
     * healthy). Surfaced so a guardian sees WHY an update isn't landing rather
     * than an Update button that silently reappears.
     */
    fun installHealth(nowUnix: Long): String =
        runCatching { CharterNative.charterInstallHealth(nowUnix) }.getOrDefault("")

    /** Is a guardian's signed maintenance window open? Fail-closed to false. */
    fun maintenanceOpen(nowUnix: Long): Boolean = runCatching {
        org.json.JSONObject(CharterNative.charterMaintenanceOpen(nowUnix)).optBoolean("open", false)
    }.getOrDefault(false)

    /** One install window as this device saw it. `null` when none has opened
     *  here — which is the steady state on almost every phone. */
    data class MaintenanceSpan(
        val startedAt: Long,
        val endedAt: Long?,
        /** The window's REAL expiry, from the clause. Null when shut, or when
         *  the clause could not be read — in which case the ward is told the
         *  window is open WITHOUT a duration, never a guessed one. */
        val untilUnix: Long?,
        val open: Boolean,
    )

    fun maintenanceSpan(nowUnix: Long): MaintenanceSpan? = runCatching {
        val o = org.json.JSONObject(CharterNative.charterMaintenanceSpan(nowUnix))
        val started = o.optLong("startedAt", 0L)
        if (started <= 0L) return@runCatching null
        MaintenanceSpan(
            startedAt = started,
            endedAt = if (o.isNull("endedAt")) null else o.optLong("endedAt"),
            untilUnix = if (o.isNull("untilUnix")) null else o.optLong("untilUnix"),
            open = o.optBoolean("open", false),
        )
    }.getOrNull()

    /** Report the guardian's account of what came through their window. */
    fun setInstallWindow(reportJson: String) {
        CharterNative.charterSetInstallWindow(reportJson)
    }
}
