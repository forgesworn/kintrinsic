package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore

/**
 * The ward's ask-for-more-time TYPE (§Times), as pure functions so the
 * lock shade and its test share one source.
 *
 * A child with any named time is offered a type: Free time, or one of theirs.
 * The default is the core's (inside a window now, that window's time; else the
 * time whose window most recently ended today; else the next one up today;
 * else Free). A child with no named time is offered nothing and the ask is the
 * untyped one it has always been.
 */
object AskType {

    /**
     * The type the ask will carry: the ward's [picked] one if the device still
     * offers it, else the core's [default], else Free. Null when no type is
     * offered at all (no named time), which is the untyped ask.
     */
    fun resolve(
        offered: List<CharterCore.AskTime>,
        picked: String?,
        default: String?,
    ): CharterCore.AskTime? {
        if (offered.isEmpty()) return null
        return offered.firstOrNull { it.id == picked }
            ?: offered.firstOrNull { it.id == default }
            ?: offered.firstOrNull { it.id == CharterCore.FREE_TIME_ID }
            ?: offered.first()
    }

    /**
     * The `time.extend` request params. `time` is added ONLY for a named time:
     * Free time (and an untyped ask) leave the field out so the request stays
     * byte-identical to a pre-times one, and the guardian's grant, which echoes
     * `time` verbatim, matches exactly.
     */
    fun params(minutes: Int, limitHit: String, timeId: String?): String {
        val base = """{"minutesRequested":$minutes,"reason":"","limitHit":"$limitHit""""
        val typed = timeId != null && timeId != CharterCore.FREE_TIME_ID
        // The id is a [a-z0-9-] slug on the wire, but it came from a clause: quote
        // it through org.json rather than trusting that.
        return if (typed) """$base,"time":${org.json.JSONObject.quote(timeId)}}""" else "$base}"
    }

    /** The chooser's label: "For: Learning time". */
    fun chooserLabel(chosen: CharterCore.AskTime): String = "For: ${chosen.label}"
}
