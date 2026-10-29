package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore

/**
 * What the ward's home screen SAYS, as pure functions — the one place the
 * headline, its caption and the About screen's lines are worded, shared by
 * [org.forgesworn.charter.MainActivity] and [TimeLeftWidget] so the widget on
 * the launcher and the screen behind it can never disagree.
 *
 * Born from a founder's look at the tablet (2026-08-28): the app opened on
 * "Kintrinsic is set up on this phone", a 64-character device code, the
 * guardian's key and the relay — the grown-up's set-up detail — and the one
 * thing a child actually opens it for, how much time they have left, was a
 * paragraph further down. A child's screen leads with the child's question.
 * The set-up detail is still all there, one tap away under "About this
 * device", together with the version number nobody could find.
 */
object HomeCopy {

    /** The hero: the big figure and the small line beneath it. */
    data class Headline(val big: String, val caption: String)

    /**
     * Same mapping the launcher widget has always used, now the single copy.
     * `secondsLeft < 0` is the core's "no WHOLE-DEVICE time wall at all"
     * sentinel (a buckets-only ward, most commonly) and must never be fed
     * through [TimeText.timeLeft], which would print "0s" and read as "about
     * to lock any second" on a device that is not.
     */
    fun headline(view: CharterCore.ScheduleView?): Headline = when {
        view == null -> Headline("—", "No charter set yet — ask your guardian")
        view.locked -> Headline("Locked", "right now")
        view.secondsLeft < 0 -> Headline("No limit", "on the whole device today")
        else -> Headline(TimeText.timeLeft(view.secondsLeft), "left today")
    }

    /**
     * The ward's own line when the guardian has opened USB debugging (the
     * `debugging` clause): a ward can always see what is being enforced.
     * [until] is the core's `debuggingUntil`: negative (blocked, or held on a
     * store read error) is null, no line; 0 no end, >0 the unix second it ends. Local time; the day is named only when
     * the window ends on another day than [now].
     */
    fun debuggingLine(until: Long, now: Long, zone: java.time.ZoneId = java.time.ZoneId.systemDefault()): String? = when {
        until < 0 -> null
        until == 0L -> "USB debugging is on until your guardian turns it off"
        else -> {
            val end = java.time.Instant.ofEpochSecond(until).atZone(zone)
            val sameDay = end.toLocalDate() == java.time.Instant.ofEpochSecond(now).atZone(zone).toLocalDate()
            val clock = "%02d:%02d".format(end.hour, end.minute)
            val day = if (sameDay) "" else
                end.dayOfWeek.getDisplayName(java.time.format.TextStyle.SHORT, java.util.Locale.UK) + " "
            "USB debugging is on until $day$clock"
        }
    }

    /** "Kintrinsic 0.6.11 (42)" — the line a tester wants without a long-press. */
    fun versionLine(versionName: String?, versionCode: Long): String {
        val name = versionName?.takeIf { it.isNotBlank() } ?: "?"
        return "Kintrinsic $name ($versionCode)"
    }

    /** The home screen's quiet footer link. */
    fun aboutLink(versionName: String?): String =
        "About this device · Kintrinsic ${versionName?.takeIf { it.isNotBlank() } ?: "?"}"

    /**
     * The guardian line for About. Paired: who, and where this device
     * listens. Unpaired: say so plainly — the pairing itself lives on the home
     * screen, which is the onboarding surface until a guardian is pinned.
     *
     * Who comes from the guardian summary (the roots the ward's guardian-keys
     * state holds), not the pairing record: after a root move the record's
     * guardian is legacy only. One root is named (label and short npub);
     * several are counted; an unreadable summary says only "a guardian".
     */
    fun guardianLine(state: CharterCore.PairingState?, guardians: CharterCore.GuardiansSummary? = null): String {
        if (state?.paired != true) return "Not paired with a guardian yet."
        val roots = guardians?.takeIf { it.available }?.roots.orEmpty()
        val who = when (roots.size) {
            0 -> "Paired with a guardian"
            1 -> "Paired with guardian ${GuardianCopy.guardianName(roots[0].label, roots[0].root)}"
            else -> "Paired with ${roots.size} guardians"
        }
        val relay = state.relays.firstOrNull()?.let { "\nListening on $it" } ?: ""
        return who + relay
    }
}
