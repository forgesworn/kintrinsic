package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore

/**
 * The ward's named-times mirror (spec D8, Task 9): pure JSON→row mapping over
 * [CharterCore.bucketViews] — Kotlin does no policy math here, only warm
 * formatting of numbers the Rust core already enforced. Everything below is a
 * plain function of its inputs so it is JVM-testable without a device
 * (same idiom as [LifelineOffer.decide]/[TimeText.timeLeft]).
 */
object GroupMirror {

    /** One named-times group's mirror row ("Play — 45m of 1h left today · 2h
     *  of 5h this week"). */
    data class GroupRow(
        val id: String,
        val label: String,
        val detail: String,
        /** Offer "Ask for more X time": spent AND still capped — a lifted
         *  (paused) set has nothing to ask for, and a group still running has
         *  nothing to ask for YET. */
        val canAskForMore: Boolean,
    )

    /** One `askFirst` app still gated right now — label only; the wire `pkg`
     *  drives the ask. */
    data class AskToOpenRow(val pkg: String, val label: String)

    /** Build every group's mirror row, in the order the core reported them. */
    fun groupRows(views: List<CharterCore.BucketView>): List<GroupRow> =
        views.map { b ->
            GroupRow(
                id = b.id,
                label = b.label,
                detail = detailLine(b),
                canAskForMore = b.capped && b.spent,
            )
        }

    /** Build the "Ask to open" rows from the core's labelled `askFirst` list. */
    fun askToOpenRows(apps: List<CharterCore.AskFirstApp>): List<AskToOpenRow> =
        apps.map { AskToOpenRow(pkg = it.pkg, label = it.label) }

    /**
     * Whether [pkg] is CURRENTLY held open by a live [CharterCore.AppHold] at
     * [nowUnix] — feeds [AskLifecycle.status]'s `stillOpenable`, so the
     * "✓ You can open it now!" copy does not linger once the hold that made
     * it true has actually lapsed (round-2 review minor, 2026-08-03): the
     * REQUEST record stays `enacted` forever (it's a one-time answer), but
     * whether the app can actually be opened right now is a different,
     * time-boxed fact that lives in the `apps` clause's holds, not the ask.
     */
    fun isCurrentlyHeldOpen(pkg: String, holds: List<CharterCore.AppHold>, nowUnix: Long): Boolean =
        holds.any { it.pkg == pkg && it.state == "allowed" && it.untilUnix > nowUnix }

    /**
     * The warm one- or two-clause detail line for a single group.
     *
     * `capped == false` means the whole bucket set is lifted (paused) —
     * [CharterCore.BucketView.usedSeconds] still carries the real meter even
     * then (the core deliberately keeps reporting it, per `bucket_view`'s own
     * doc comment: "the family sees what was spent even while the cap is
     * lifted"), so a paused row says what ran today rather than a bare "0m of
     * 0m" that would read as a bug.
     */
    fun detailLine(b: CharterCore.BucketView): String {
        if (!b.capped) {
            return if (b.usedSeconds > 0) {
                "Paused — ${TimeText.timeLeft(b.usedSeconds)} used today, nothing capped right now."
            } else {
                "Paused — nothing capped right now."
            }
        }
        val todayAmount = amount(b.remainingSeconds, b.limitSeconds)
        if (b.weekLimitSeconds < 0) return "$todayAmount left today"
        val weekAmount = amount(b.weekRemainingSeconds.coerceAtLeast(0), b.weekLimitSeconds)
        // A weekly-only bucket has no day cap of its own — its day fields
        // mirror the week's own binding wall (the core's fallback so a
        // weekly-only group never shows a phantom "0m"), so day and week
        // read identically here. Showing both would stutter ("45m of 1h left
        // today · 45m of 1h this week") — collapse to the one true sentence,
        // same dedupe the Linux tray's `bucket_time_detail` uses.
        if (b.weekRemainingSeconds == b.remainingSeconds) {
            return "$weekAmount left this week"
        }
        return "$todayAmount left today · $weekAmount this week"
    }

    private fun amount(remainingSecs: Long, limitSecs: Long): String =
        "${TimeText.timeLeft(remainingSecs.coerceAtLeast(0))} of ${TimeText.timeLeft(limitSecs)}"

    /** "40m" / "2h 15m" / "1h" — rounds to the nearest minute, never shows a
     *  trailing "0m" on a whole hour. Deliberately NOT [TimeText.timeLeft]:
     *  that formatter shows seconds under five minutes and always pads a
     *  whole hour to "1h 0m", which would silently drift this sentence away
     *  from the guardian's own wording (`humanDuration` in
     *  `insights/usageHistory.ts`, TypeScript) for exactly those values. This
     *  mirrors `humanDuration` field-for-field instead, so [outOfHoursLine]
     *  stays byte-identical to its TypeScript twin for every input, not just
     *  the common ones.
     */
    private fun humanDuration(secs: Long): String {
        val m = Math.round(secs / 60.0)
        if (m < 60) return "${m}m"
        val h = m / 60
        val rem = m % 60
        return if (rem != 0L) "${h}h ${rem}m" else "${h}h"
    }

    /**
     * The week's out-of-hours use, in the same calm register as every other
     * line here — a fact about the week, never a warning. `null` when there
     * is nothing to say, so a ward whose family never set the clause sees no
     * line at all rather than a zero. Identical FORMATTER to the guardian's
     * own line (`outOfHoursLine` in `insights/usageHistory.ts`, TypeScript,
     * pinned byte-for-byte by the shared vector fixture) — but each side
     * feeds it its OWN device's numbers, so this is a guarantee about the
     * WORDING, not about the two sides always agreeing on a figure. There is
     * deliberately no guardian-only VARIANT of the formatter itself.
     *
     * "so far this week" (review fix I2, 2026-08-04): this resets on the
     * device's own CALENDAR week while the guardian's adjacent weekly-total
     * line is a rolling 7 days — without the scope word both would read as
     * plain "this week" and the reset would look like the night use simply
     * stopped.
     */
    fun outOfHoursLine(nights: Int, secs: Long): String? {
        if (nights == 0 || secs == 0L) return null
        return "$nights night${if (nights > 1) "s" else ""} so far this week · ${humanDuration(secs)}"
    }
}

/**
 * The brokered-ask lifecycle → status copy, shared by every ask surface on
 * this mirror (a group's "Ask for more" and an `askFirst` app's "Ask to
 * open"). Pure: given the request's current state (as [CharterCore.listRequests]
 * reports it, or `null` for "never asked"), decide what to say and whether the
 * button should be live.
 *
 * CRITICAL lesson carried from the gift-time outcome-staleness bug (a denied
 * ask that read "Asked!" forever because the answer lived only in a captured
 * view): callers must re-derive this from the CURRENT state on every render,
 * never cache the resulting [AskStatus] itself across a rebuild.
 *
 * A denial is NEVER final: Kintrinsic is companion tech, not a control panel — a
 * "no" is an answer for right now, not a lock on the button (round-1 review,
 * 2026-08-03). `canAsk` is `true` on `denied`, same as every other terminal
 * non-happy state, matching [org.forgesworn.charter.ui.RequestAppsActivity]'s
 * own "Ask again" precedent and the identical copy already shipped for these
 * same ops on Linux (`charter-tray::model`). No cool-down timer is applied
 * here — that would be client-invented policy; the guardian can simply deny
 * again. The ANSWERED copy stays on screen until the ward acts: submitting a
 * new ask overwrites the persisted reqId (see `MainActivity.submitGroupAsk`/
 * `submitAppOpenAsk`), so the new lifecycle takes over cleanly — no
 * staleness regression, the outcome is still re-read fresh every render.
 *
 * The SAME is true of a GRANT (round-1 review, second pass — found while
 * verifying the app.open grant fix): `enacted` is a TERMINAL state, and for a
 * time-boxed answer (an app.open hold, a group's extension) it does not mean
 * "settled forever" — the hold expires, the extension gets used up, and the
 * ward is owed a live button to ask again rather than one permanently
 * disabled by the happy path. `enacting` (the brief in-flight window between
 * a verified grant and the enactor actually running) stays `canAsk = false`
 * — that mirrors "Sending…", not "Answered" — but `enacted` now matches
 * `denied`'s precedent exactly: the happy-path copy stays on screen (the
 * ward still sees they were said yes to) AND the button comes back.
 *
 * That copy is only honest while the thing it describes is still true,
 * though (round-2 review minor, 2026-08-03): a granted app.open hold EXPIRES
 * — the REQUEST record stays `enacted` forever (it answers one ask, once),
 * but "you can open it now!" stops being true the moment the hold's
 * `untilUnix` passes, and nothing here re-derives that on its own. Callers
 * with a time-boxed happy path (app.open; a group's own ask has no
 * equivalent — a bucket's cap is read fresh every render some other way)
 * pass `stillOpenable` computed from [GroupMirror.isCurrentlyHeldOpen]. An
 * `enacted` row with `stillOpenable = false` collapses to [NONE] — exactly
 * how a row that was never asked about reads, since "you can open it" is no
 * longer true and `canAsk` was already `true` regardless.
 */
object AskLifecycle {

    data class AskStatus(val text: String, val canAsk: Boolean)

    /** No ask has ever been made for this row. */
    val NONE = AskStatus(text = "", canAsk = true)

    /** Same wording already shipped for these ops on Linux
     *  (`charter-tray::model`) — kept identical across platforms. */
    private const val DENIED_TEXT =
        "Your guardian said not right now. You can always ask again — or better, ask in person."

    /**
     * [grantedText] is the op-specific happy-path line ("✓ Your guardian
     * added more Play time." / "✓ You can open it now!"); everything else is
     * shared copy across every brokered ask on this screen. [stillOpenable]
     * only matters on `enacted` — see the class doc's round-2 note; every
     * caller without a time-boxed happy path leaves it at its default (no
     * behaviour change for group "ask for more" rows).
     */
    fun status(state: String?, grantedText: String, stillOpenable: Boolean = true): AskStatus = when (state) {
        // In flight only: a verified grant landing, not yet acted on.
        "enacting" -> AskStatus(grantedText, canAsk = false)
        // Terminal — but a hold/extension expires, so a "yes" is not forever
        // either (see the class doc's second-pass note). Once it's no longer
        // true (the hold lapsed) the row reads exactly like "never asked".
        "enacted" -> if (stillOpenable) AskStatus(grantedText, canAsk = true) else NONE
        "denied" -> AskStatus(DENIED_TEXT, canAsk = true)
        "failed", "expired", "rejected", "cancelled" ->
            AskStatus("That didn't go through — you can ask again.", canAsk = true)
        "pending" -> AskStatus("Asked! Your guardian will see it shortly.", canAsk = false)
        else -> NONE
    }
}
