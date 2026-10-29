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
    fun debuggingLineOnlyWhenAllowed() {
        val utc = java.time.ZoneOffset.UTC
        val noon = java.time.LocalDateTime.of(2026, 10, 27, 12, 0).toEpochSecond(utc)
        val six = java.time.LocalDateTime.of(2026, 10, 27, 18, 0).toEpochSecond(utc)
        val tomorrow = java.time.LocalDateTime.of(2026, 10, 28, 9, 5).toEpochSecond(utc)
        assertEquals(null, HomeCopy.debuggingLine(-1, noon, utc))
        assertEquals("USB debugging is on until 18:00", HomeCopy.debuggingLine(six, noon, utc))
        assertEquals("USB debugging is on until Wed 09:05", HomeCopy.debuggingLine(tomorrow, noon, utc))
        assertEquals("USB debugging is on until your guardian turns it off", HomeCopy.debuggingLine(0, noon, utc))
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
        val root = "a".repeat(64)
        val other = "b".repeat(64)
        fun summary(vararg roots: CharterCore.GuardianRoot, available: Boolean = true) = CharterCore.GuardiansSummary(
            available = available, paired = true, roots = roots.toList(),
            pending = 0, pinSet = false, pinFailures = 0, pinWaitSeconds = 0,
        )
        fun guardian(hex: String, label: String?) =
            CharterCore.GuardianRoot(root = hex, label = label, keys = 1, keyLabels = emptyList(), rules = emptyList())
        val paired = CharterCore.PairingState(paired = true, guardianShort = "legacy", subject = "s", relays = listOf("wss://relay.example"))
        // One root: named by its label and short npub, not the legacy pin.
        assertEquals(
            "Paired with guardian Mum (${GuardianCopy.shortNpub(root)})\nListening on wss://relay.example",
            HomeCopy.guardianLine(paired, summary(guardian(root, "Mum"))),
        )
        assertEquals(
            "Paired with guardian A guardian (${GuardianCopy.shortNpub(root)})",
            HomeCopy.guardianLine(paired.copy(relays = emptyList()), summary(guardian(root, null))),
        )
        // Several roots: counted.
        assertEquals(
            "Paired with 2 guardians\nListening on wss://relay.example",
            HomeCopy.guardianLine(paired, summary(guardian(root, "Mum"), guardian(other, "Dad"))),
        )
        // No usable summary: the plain fallback, never the legacy pin.
        assertEquals("Paired with a guardian\nListening on wss://relay.example", HomeCopy.guardianLine(paired, null))
        assertEquals("Paired with a guardian", HomeCopy.guardianLine(paired.copy(relays = emptyList()), summary(available = false)))
        assertEquals("Paired with a guardian", HomeCopy.guardianLine(paired.copy(relays = emptyList()), summary()))
    }
}
