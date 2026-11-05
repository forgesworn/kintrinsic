package org.forgesworn.charter.ui

/**
 * How much more time the ward's lock-shade ask is for, as pure functions so
 * the shade and its test share one source.
 *
 * The ask used to be a fixed 30 minutes, which also fixed what the guardian
 * could answer with ("Approve 30 min" and a stepper that only went DOWN from
 * it): a child who needed an hour to finish had to ask twice. Now the shade
 * offers the contract's guidance amounts — 10 min, 20 min, 1 hour — plus a
 * typed amount, and the guardian's own sheet steps freely above or below.
 */
object AskMinutes {

    /** The wire's `MAX_EXTEND_MINUTES` (§Device brokering): `u16`, 1..=1440. */
    const val MAX = 1440
    const val MIN = 1

    /** The preset amounts the chooser offers, in the order shown. */
    val PRESETS: List<Int> = listOf(10, 20, 60)

    /** Which preset the chooser starts on when the ward has not picked yet. */
    const val DEFAULT = 20

    /** The last chooser row: a typed amount. */
    const val CUSTOM_LABEL = "Custom…"

    /** "10 min", "1 hour", "1 h 30 min" — the way a child reads a clock. */
    fun label(minutes: Int): String {
        val h = minutes / 60
        val m = minutes % 60
        return when {
            h == 0 -> "$m min"
            m == 0 -> if (h == 1) "1 hour" else "$h hours"
            else -> "$h h $m min"
        }
    }

    /** The chooser's rows: each preset's label, then [CUSTOM_LABEL]. */
    fun chooserLabels(): List<String> = PRESETS.map { label(it) } + CUSTOM_LABEL

    /** The index of [CUSTOM_LABEL] in [chooserLabels]. */
    val CUSTOM_INDEX: Int get() = PRESETS.size

    /**
     * The row the chooser opens on: the ward's last pick if it is a preset,
     * else the custom row (a typed amount was their choice), else [DEFAULT].
     */
    fun checkedIndex(picked: Int?): Int {
        if (picked == null) return PRESETS.indexOf(DEFAULT)
        val i = PRESETS.indexOf(picked)
        return if (i >= 0) i else CUSTOM_INDEX
    }

    /**
     * A typed amount, or null when it is not an ask the wire can carry: not a
     * whole number of minutes, zero, negative, or over [MAX]. The device
     * refuses an out-of-range `minutesRequested` before it ever leaves, so the
     * shade must say "between 1 and 1440" rather than report a failed send.
     */
    fun parseCustom(input: String?): Int? {
        val n = input?.trim()?.toIntOrNull() ?: return null
        return if (n in MIN..MAX) n else null
    }

    /** What the custom dialog says when [parseCustom] returns null. */
    const val CUSTOM_RANGE_HINT = "Pick a whole number of minutes, from 1 to 1440."
}
