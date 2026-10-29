package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore

/**
 * What the ward says about its guardians, as pure functions: the guardian
 * list under About, and the Guardian presence screen.
 *
 * Contract rule 5: a guardian is always named by a label AND the short npub
 * of their root identity, because a label alone can be spoofed.
 */
object GuardianCopy {

    /** `npub1abcdefg…uvwxyz` for a 64-hex root, or a short hex fallback. */
    fun shortNpub(rootHex: String): String {
        val npub = PairingSas.npub(rootHex) ?: return rootHex.take(8)
        return if (npub.length > 20) "${npub.take(12)}…${npub.takeLast(6)}" else npub
    }

    /** "Mum (npub1…)" or "A guardian (npub1…)". */
    fun guardianName(label: String?, rootHex: String): String {
        val name = label?.trim()?.takeIf { it.isNotEmpty() } ?: "A guardian"
        return "$name (${shortNpub(rootHex)})"
    }

    /** A rule kind as the ward reads it. */
    fun ruleName(kind: String): String = when (kind) {
        "budget" -> "daily time"
        "schedule" -> "schedule"
        "apps" -> "apps"
        "appRules", "apprules" -> "app rules"
        "buckets" -> "named times"
        "content" -> "web filter"
        "lifeline" -> "emergency contacts"
        "standDown", "standdown" -> "stand-down"
        "alwaysAvailable", "alwaysavailable" -> "always-available apps"
        else -> kind
    }

    /** One guardian's lines for About: who, how many installs, what they set. */
    fun rootLines(r: CharterCore.GuardianRoot): String {
        val apps = when (r.keys) {
            0 -> "No Kintrinsic app linked yet"
            1 -> "1 Kintrinsic app linked"
            else -> "${r.keys} Kintrinsic apps linked"
        }
        val rules = if (r.rules.isEmpty()) "" else "\nSet: " + r.rules.joinToString(", ") { ruleName(it) }
        return guardianName(r.label, r.root) + "\n" + apps + rules
    }

    /** The guardian list's footer: anything waiting for a guardian in person. */
    fun pendingLine(pending: Int): String = when (pending) {
        0 -> "Nothing is waiting for a guardian to confirm here."
        1 -> "1 change is waiting for a guardian to confirm it at this device."
        else -> "$pending changes are waiting for a guardian to confirm them at this device."
    }

    /** What a pending statement asks for, in a sentence. */
    fun pendingTitle(p: CharterCore.PendingStatement): String = when (p.kind) {
        "coguardian" -> "Add ${guardianName(p.label, p.key)} as a guardian"
        "removal" -> "Remove ${shortNpub(p.key)} as a guardian"
        "move" -> "Move a guardian to ${shortNpub(p.key)}"
        else -> "A guardian change"
    }

    /** Who asked, and the clock on it. */
    fun pendingDetail(p: CharterCore.PendingStatement, nowUnix: Long): String {
        val by = "Asked by " + guardianName(null, p.signer)
        val effect = p.takesEffectAt?.let { "\nTakes effect " + whenText(it, nowUnix) } ?: ""
        val expires = "\nExpires " + whenText(p.expiresAt, nowUnix)
        return by + effect + expires
    }

    /** "in 3 h" / "now". */
    fun whenText(atUnix: Long, nowUnix: Long): String {
        val d = atUnix - nowUnix
        return when {
            d <= 0 -> "now"
            d < 3600 -> "in ${(d + 59) / 60} min"
            d < 48 * 3600 -> "in ${(d + 3599) / 3600} h"
            else -> "in ${(d + 86399) / 86400} days"
        }
    }

    /** The backoff line, or null when a try is allowed now. */
    fun waitLine(waitSeconds: Long): String? = when {
        waitSeconds <= 0 -> null
        waitSeconds < 60 -> "Too many wrong PINs. Try again in $waitSeconds s."
        else -> "Too many wrong PINs. Try again in ${(waitSeconds + 59) / 60} min."
    }

    /** The PIN error after a refused mint. */
    fun mintError(r: CharterCore.PresenceResult): String = when (r.error) {
        "noPin" -> NO_PIN
        "wrongPin" -> "That PIN is not right." +
            (waitLine(r.waitSeconds)?.let { " $it" } ?: "")
        "wait" -> waitLine(r.waitSeconds) ?: "Try again."
        "notPending" -> "That change is no longer waiting. Go back and look again."
        "unavailable" -> "This device can't check guardian changes right now."
        else -> "Could not make a code. Try again."
    }

    /** "mm:ss". */
    fun countdown(seconds: Long): String {
        val s = seconds.coerceAtLeast(0)
        return "%d:%02d".format(s / 60, s % 60)
    }

    const val NO_PIN =
        "No guardian PIN is set on this device yet. A guardian sets the PIN from the " +
            "Kintrinsic app; until then nothing here can be confirmed in person."

    const val EXPLAIN =
        "Some changes to who looks after this device need a guardian to be here in " +
            "person. A guardian enters the PIN, then scans the code with their " +
            "Kintrinsic app. Nothing on this screen confirms a change by itself."
}
