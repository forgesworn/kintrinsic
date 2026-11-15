package org.forgesworn.charter.ui

/**
 * A hold-to-call press in its 3-2-1 cancel window. [LockActivity] holds it,
 * not the button, so a render that rebuilds the buttons (a resume, a rotation,
 * a new lock reason) rebuilds this one still counting, and a tap on the new
 * button still cancels the press. Pure, apart from the tick the activity hands
 * it to keep.
 */
internal class CallCountdown(
    val label: String,
    val who: String,
    val number: String,
    val emergency: Boolean,
    /** The number on the button: 3, 2, 1. The call is placed when it reaches 0. */
    var left: Int,
) {
    /** Which button this is (see [key]). */
    val key: String = key(label, number, emergency)

    /** The tick the activity posted for this countdown, so it can be cancelled. */
    var posted: Runnable? = null

    /** The button's text while it counts. */
    fun text(): String = "Calling $who in $left… (tap to cancel)"

    /** One second has passed: true when the window is over and the call is placed. */
    fun tick(): Boolean {
        left--
        return left <= 0
    }

    companion object {
        /** One key per lifeline button. */
        fun key(label: String, number: String, emergency: Boolean): String = "$emergency|$label|$number"

        /** What a button shows when it is built: still counting, if its press is. */
        fun buttonText(label: String, countdown: CallCountdown?): String = countdown?.text() ?: label

        const val CANCELLED = "Call cancelled."

        /**
         * The status line once a countdown is cancelled. A sticky line already
         * showing (the advice an earlier emergency press left) stays: cancelling
         * a second press never wipes it. Otherwise, "Call cancelled.".
         */
        fun lineAfterCancel(stickyText: String): String = stickyText.ifEmpty { CANCELLED }
    }
}
