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
     */
    fun guardianLine(state: CharterCore.PairingState?): String {
        if (state?.paired != true) return "Not paired with a guardian yet."
        val who = state.guardianShort?.let { "Paired with guardian $it" } ?: "Paired with a guardian"
        val relay = state.relays.firstOrNull()?.let { "\nListening on $it" } ?: ""
        return who + relay
    }
}
