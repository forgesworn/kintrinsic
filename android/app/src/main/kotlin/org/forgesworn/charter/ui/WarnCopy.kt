package org.forgesworn.charter.ui

/**
 * The 10-/1-minute warning wording as a pure function, so the notification and
 * its test share one source.
 *
 * A ward that uses no times gets exactly the words it always had. Once a named
 * time is involved the warning names it and says what follows (§Times): "10
 * minutes of Learning time left — then Free time", "…, then locked".
 */
object WarnCopy {

    /**
     * [time] is the display label of the named time the ward is in (null in
     * Free time or with no times); [then] is what follows ("Free time",
     * "Learning time", "locked"; null when unknown or not applicable).
     */
    fun text(level: String, time: String?, then: String?): String {
        val one = level == "one"
        if (time == null && then == null) {
            return if (one) "1 minute left — save your game" else "10 minutes left"
        }
        val amount = if (one) "1 minute" else "10 minutes"
        val of = if (time != null) "$amount of $time left" else "$amount left"
        val follows = when {
            then == null -> ""
            then == "locked" -> " — then locked"
            else -> " — then $then"
        }
        val tail = if (one) ". Save your game." else ""
        return of + follows + tail
    }
}
