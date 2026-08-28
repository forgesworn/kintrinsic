package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Test

/** The ward's home-screen wording (2026-08-28): the child's question first. */
class HomeCopyTest {

    private fun view(locked: Boolean, secs: Long) =
        CharterCore.ScheduleView(locked = locked, minutesLeft = secs / 60, secondsLeft = secs, detail = null, lines = emptyList())

    @Test
    fun headlineLeadsWithTimeLeft() {
        assertEquals(HomeCopy.Headline("2h 15m", "left today"), HomeCopy.headline(view(false, 2 * 3600 + 15 * 60)))
        assertEquals(HomeCopy.Headline("48s", "left today"), HomeCopy.headline(view(false, 48)))
    }

    @Test
    fun lockedAndNoLimitAndNoCharterAreSaidPlainly() {
        assertEquals("Locked", HomeCopy.headline(view(true, 0)).big)
        // -1 is the "no whole-device wall" sentinel: never "0s left today".
        assertEquals(HomeCopy.Headline("No limit", "on the whole device today"), HomeCopy.headline(view(false, -1)))
        assertEquals("No charter set yet — ask your guardian", HomeCopy.headline(null).caption)
    }

    @Test
    fun versionAndAboutLinesNameTheVersion() {
        assertEquals("Kintrinsic 0.6.11 (42)", HomeCopy.versionLine("0.6.11", 42))
        assertEquals("Kintrinsic ? (0)", HomeCopy.versionLine(null, 0))
        assertEquals("About this device · Kintrinsic 0.6.11", HomeCopy.aboutLink("0.6.11"))
        assertEquals("About this device · Kintrinsic ?", HomeCopy.aboutLink(""))
    }

    @Test
    fun guardianLineSaysWhoAndWhere() {
        assertEquals("Not paired with a guardian yet.", HomeCopy.guardianLine(null))
        assertEquals(
            "Not paired with a guardian yet.",
            HomeCopy.guardianLine(CharterCore.PairingState(paired = false, guardianShort = null, subject = null)),
        )
        assertEquals(
            "Paired with guardian npub1ab…cd\nListening on wss://relay.example",
            HomeCopy.guardianLine(
                CharterCore.PairingState(paired = true, guardianShort = "npub1ab…cd", subject = "s", relays = listOf("wss://relay.example")),
            ),
        )
        assertEquals(
            "Paired with a guardian",
            HomeCopy.guardianLine(CharterCore.PairingState(paired = true, guardianShort = null, subject = "s")),
        )
    }
}
