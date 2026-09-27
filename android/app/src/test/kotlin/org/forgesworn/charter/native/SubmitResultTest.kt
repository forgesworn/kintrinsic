package org.forgesworn.charter.native

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * G-7: an ask the core refuses for being over the ward's ask limit is shown
 * in the core's own words, which are written for the ward. Every other
 * failure is a fault the ward cannot act on, and keeps the app's own copy.
 */
class SubmitResultTest {

    private val fallback = "Couldn't reach your guardian — try again."

    @Test
    fun aRateLimitRefusalIsShownVerbatim() {
        val r = CharterCore.SubmitResult(
            reqId = null,
            error = "you've asked a lot recently — try again in a while",
            code = "rate-limited",
        )
        assertEquals("you've asked a lot recently — try again in a while", r.wardMessage(fallback))
    }

    @Test
    fun anyOtherFailureKeepsTheAppsOwnCopy() {
        for (code in listOf("not-paired", "invalid", "failed", null)) {
            val r = CharterCore.SubmitResult(reqId = null, error = "store error: Io(\"EIO\")", code = code)
            assertEquals(fallback, r.wardMessage(fallback))
        }
    }

    @Test
    fun aBlankRateLimitMessageFallsBack() {
        val r = CharterCore.SubmitResult(reqId = null, error = " ", code = "rate-limited")
        assertEquals(fallback, r.wardMessage(fallback))
    }

    @Test
    fun noResultAtAllIsAFault() {
        val none: CharterCore.SubmitResult? = null
        assertEquals(fallback, none.wardMessageOr(fallback))
    }
}
