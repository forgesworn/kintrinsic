package org.forgesworn.charter.ui

import org.forgesworn.charter.ui.LockCall.Route
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * A call from the lock must not depend on the default dialer, and no single
 * failure may strand the ward.
 */
class LockCallTest {

    @Test
    fun `an ordinary call goes straight to Telecom, with no activity to resolve`() {
        assertEquals(Route.TELECOM, LockCall.routes(false, "com.android.dialer").first())
    }

    @Test
    fun `an emergency call goes straight to the system dial pad`() {
        assertEquals(Route.SYSTEM_DIAL_PAD, LockCall.routes(true, "com.android.dialer").first())
    }

    @Test
    fun `the intent any app can answer is always the last resort`() {
        for (emergency in listOf(true, false)) {
            for (dialer in listOf("com.android.dialer", null)) {
                assertEquals(Route.ANY_CALL_HANDLER, LockCall.routes(emergency, dialer).last())
            }
        }
    }

    @Test
    fun `no system dialer means no dial pad route, and still a way to call`() {
        assertEquals(
            listOf(Route.TELECOM, Route.ANY_CALL_HANDLER),
            LockCall.routes(false, null),
        )
        assertEquals(
            listOf(Route.TELECOM, Route.ANY_CALL_HANDLER),
            LockCall.routes(true, ""),
        )
    }

    @Test
    fun `a route that throws falls through to the next`() {
        val tried = mutableListOf<Route>()
        val used = LockCall.placeFirst(LockCall.routes(false, "d")) { route ->
            tried += route
            if (route == Route.TELECOM) throw SecurityException("no CALL_PHONE")
        }
        assertEquals(Route.SYSTEM_DIAL_PAD, used)
        assertEquals(listOf(Route.TELECOM, Route.SYSTEM_DIAL_PAD), tried)
    }

    @Test
    fun `the first route that starts ends the search`() {
        val tried = mutableListOf<Route>()
        LockCall.placeFirst(LockCall.routes(true, "d")) { tried += it }
        assertEquals(listOf(Route.SYSTEM_DIAL_PAD), tried)
    }

    @Test
    fun `when nothing can start the failure is reported, not swallowed`() {
        assertNull(LockCall.placeFirst(LockCall.routes(true, "d")) { throw IllegalStateException() })
    }
}
