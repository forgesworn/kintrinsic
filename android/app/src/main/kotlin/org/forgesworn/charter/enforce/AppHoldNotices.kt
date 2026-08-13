package org.forgesworn.charter.enforce

import android.content.Context
import org.forgesworn.charter.native.CharterCore

/**
 * Telling the ward when one app is held away from its usual rule.
 *
 * A guardian can now say "Vanadium, for an hour" instead of flipping the
 * standing block off and hoping to remember. From the ward's side that is an app
 * that starts working and later stops working, with nothing on the phone
 * explaining either moment — which is precisely the kind of silent change
 * Kintrinsic's transparency invariant exists to forbid. A child who cannot see what
 * is being done to their phone has no way to disagree with it.
 *
 * So both edges are announced: when a hold starts, and when it ends.
 *
 * The end wording is read from what the app ACTUALLY IS afterwards, never
 * assumed from the hold's direction — a guardian who changed the standing rule
 * part-way through an hour would otherwise be quoted saying the opposite of what
 * their phone is now doing.
 */

/** One notice to post. [pkg] is resolved to a label at delivery. */
data class HoldNotice(val pkg: String, val kind: Kind, val untilUnix: Long) {
    enum class Kind {
        /** A hold began: the app is open until `untilUnix`. */
        OPENED,

        /** A hold began: the app is paused until `untilUnix`. */
        PAUSED,

        /** A hold ended and the app is now usable. */
        BACK_OPEN,

        /** A hold ended and the app is now blocked. */
        BACK_CLOSED,

        /** An `appRules` app just went out of its own allowed hours. */
        RULE_CLOSED,

        /** An `appRules` app came back inside its own allowed hours. */
        RULE_OPEN,
    }
}

/**
 * What the last tick saw, as `pkg -> "state:untilUnix"`. A hold that merely
 * counts down is unchanged; a hold whose state or expiry moved is a NEW hold and
 * is announced again, because the guardian said something new about that app.
 */
typealias HoldMemory = Map<String, String>

fun holdKey(hold: CharterCore.AppHold): String = "${hold.state}:${hold.untilUnix}"

fun memoryOf(holds: List<CharterCore.AppHold>): HoldMemory =
    holds.associate { it.pkg to holdKey(it) }

/**
 * The notices owed, given what was seen last tick and what is live now.
 *
 * [blockedAfter] answers "is this package blocked, with the hold gone?" for the
 * apps whose holds ended — the caller reads it from the resolved policy, so this
 * function stays pure and JVM-testable.
 *
 * Returns an empty list when nothing changed, which is the overwhelmingly common
 * case: this runs on every tick, and a tick that announces nothing must also
 * WRITE nothing (the background-power pass named per-tick flash writes as one of
 * the five drains — see the caller).
 */
fun holdNotices(
    previous: HoldMemory,
    now: List<CharterCore.AppHold>,
    blockedAfter: (String) -> Boolean,
): List<HoldNotice> {
    val out = mutableListOf<HoldNotice>()
    val current = memoryOf(now)

    for (hold in now) {
        if (previous[hold.pkg] == holdKey(hold)) continue // still running, already said
        val kind =
            if (hold.state == "blocked") HoldNotice.Kind.PAUSED else HoldNotice.Kind.OPENED
        out.add(HoldNotice(hold.pkg, kind, hold.untilUnix))
    }

    for (pkg in previous.keys) {
        if (current.containsKey(pkg)) continue
        val kind =
            if (blockedAfter(pkg)) HoldNotice.Kind.BACK_CLOSED else HoldNotice.Kind.BACK_OPEN
        out.add(HoldNotice(pkg, kind, 0L))
    }

    return out
}

/**
 * The notices owed for the `appRules` clause — apps with their own allowed
 * hours, which turn on and off with the clock on every tick.
 *
 * These were announced NOWHERE until the 2026-08-02 audit: an app blinked in
 * and out of existence on a schedule and the phone never said why, in a feature
 * whose sibling (holds, above) opens by quoting the transparency invariant. A
 * child who cannot see what is being done to their phone has no way to disagree
 * with it, and "it just stopped working" is how a rule becomes a fault.
 *
 * Two silences are deliberate and both are about not crying wolf:
 *
 * - [firstRun] announces nothing and only records. Otherwise adopting Kintrinsic,
 *   or clearing app data, greets the ward with a burst of notices about rules
 *   that have been in force all along. Every LATER change is announced.
 * - An app the standing `apps` policy blocks anyway is skipped in both
 *   directions: "closed now" is not news when it was already closed, and
 *   "open again" would be a plain lie told by the wrong clause.
 */
fun ruleNotices(
    previous: Set<String>,
    now: Set<String>,
    firstRun: Boolean,
    blockedByPolicy: (String) -> Boolean,
): List<HoldNotice> {
    if (firstRun) return emptyList()
    val out = mutableListOf<HoldNotice>()
    for (pkg in now) {
        if (pkg in previous || blockedByPolicy(pkg)) continue
        out.add(HoldNotice(pkg, HoldNotice.Kind.RULE_CLOSED, 0L))
    }
    for (pkg in previous) {
        if (pkg in now || blockedByPolicy(pkg)) continue
        out.add(HoldNotice(pkg, HoldNotice.Kind.RULE_OPEN, 0L))
    }
    return out
}

/** Is [pkg] blocked under [policy]? The reading `appSuspendSet` already uses. */
fun blockedUnder(policy: CharterCore.AppPolicy?, pkg: String): Boolean = when {
    policy == null -> false
    policy.posture == "allowlist" -> pkg !in policy.allowed
    else -> pkg in policy.blocked
}

/** Where the notices actually go. Separated so the diff above stays testable. */
interface HoldNotifier {
    fun post(notice: HoldNotice)
}

/**
 * Remembers what was announced, across process restarts.
 *
 * Without persistence every reboot re-announces every live hold, which turns a
 * quiet feature into a phone that nags — and the ward's phone restarts more than
 * a guardian's does.
 */
class HoldMemoryStore(context: Context) {
    // Lazy so merely CONSTRUCTING a controller never touches storage — the
    // production wiring builds this eagerly, and a first tick is not guaranteed.
    private val prefs by lazy {
        context.getSharedPreferences("charter-app-holds", Context.MODE_PRIVATE)
    }

    fun read(): HoldMemory =
        prefs.all.entries
            .mapNotNull { (k, v) -> (v as? String)?.let { k to it } }
            .toMap()

    /**
     * Writes ONLY on a real change. A tick that found the same holds it found
     * last time must not touch flash — this runs on every enforcement tick.
     */
    fun write(next: HoldMemory, previous: HoldMemory) {
        if (next == previous) return
        prefs.edit().clear().apply {
            for ((k, v) in next) putString(k, v)
        }.apply()
    }
}

/**
 * What the last tick found SUSPENDED by `appRules`, across process restarts.
 *
 * Separate from [HoldMemoryStore] because the two answer different questions
 * (which holds are running vs which scheduled apps are shut) and share only a
 * shape. It records whether it has ever been written, so the first tick on a
 * new install seeds silently instead of announcing every standing rule at once
 * — see [ruleNotices].
 */
class RuleMemoryStore(context: Context) {
    private val prefs by lazy {
        context.getSharedPreferences("charter-app-rules", Context.MODE_PRIVATE)
    }

    /** True until the first [write] — nothing has ever been recorded here. */
    fun firstRun(): Boolean = !prefs.contains(SEEDED)

    fun read(): Set<String> =
        prefs.all.keys.filterNot { it == SEEDED }.toSet()

    /** Writes ONLY on a real change — a quiet tick must not touch flash. */
    fun write(next: Set<String>, previous: Set<String>) {
        if (next == previous && !firstRun()) return
        prefs.edit().clear().apply {
            putString(SEEDED, "1")
            for (pkg in next) putString(pkg, "1")
        }.apply()
    }

    private companion object {
        const val SEEDED = "__seeded"
    }
}

class NotificationHoldNotifier(private val context: Context) : HoldNotifier {

    override fun post(notice: HoldNotice) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
            as android.app.NotificationManager
        nm.createNotificationChannel(
            android.app.NotificationChannel(
                CHANNEL,
                "App changes",
                // Information, not an alarm: it should be READ, not obeyed.
                android.app.NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = "When an app is opened or paused for a while" },
        )
        val label = labelOf(notice.pkg)
        val until = timeOf(notice.untilUnix)
        val (title, text) = when (notice.kind) {
            HoldNotice.Kind.OPENED ->
                "$label is open" to "Until $until."
            HoldNotice.Kind.PAUSED ->
                "$label is paused" to "Until $until."
            HoldNotice.Kind.BACK_OPEN ->
                "$label is back to normal" to "You can use it as usual."
            HoldNotice.Kind.BACK_CLOSED ->
                "$label is closed again" to "That extra time is over."
            // No "until" to quote: an appRules window is the app's own agreed
            // hours, and naming a time the clause may not actually hold to
            // would be a promise the next tick could break.
            HoldNotice.Kind.RULE_CLOSED ->
                "$label is closed for now" to "It's outside the hours set for it."
            HoldNotice.Kind.RULE_OPEN ->
                "$label is open again" to "It's back inside the hours set for it."
        }
        nm.notify(
            // One slot per app, so a second word about the same app replaces the
            // first rather than stacking two contradictory notices.
            NOTIFICATION_BASE + (notice.pkg.hashCode() and 0xFFFF),
            android.app.Notification.Builder(context, CHANNEL)
                .setContentTitle(title)
                .setContentText(text)
                .setSmallIcon(android.R.drawable.ic_lock_idle_alarm)
                .setAutoCancel(true)
                .build(),
        )
    }

    /** The app's own name — a package id means nothing to a child. */
    private fun labelOf(pkg: String): String = runCatching {
        val pm = context.packageManager
        pm.getApplicationLabel(pm.getApplicationInfo(pkg, 0)).toString()
    }.getOrNull() ?: pkg

    /** The ward's own clock format and locale — never a hardcoded 12/24h guess. */
    private fun timeOf(untilUnix: Long): String = runCatching {
        android.text.format.DateFormat.getTimeFormat(context)
            .format(java.util.Date(untilUnix * 1000L))
    }.getOrNull() ?: "later"

    private companion object {
        const val CHANNEL = "charter-apps"
        const val NOTIFICATION_BASE = 2100
    }
}
