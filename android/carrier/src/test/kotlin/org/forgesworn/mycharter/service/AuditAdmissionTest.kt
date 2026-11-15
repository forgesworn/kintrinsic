package org.forgesworn.mycharter.service

import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.FakePrefs
import org.forgesworn.mycharter.carrier.RosterEntry
import org.forgesworn.mycharter.carrier.SpoolFile
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** Who gets told, how often, and on which row. */
class AuditAdmissionTest {
    @get:Rule val tmp = TemporaryFolder()
    private val ward = "w".repeat(64)
    private val released = "d".repeat(64)
    private val roster = listOf(
        RosterEntry(ward, "Mia", "Pixel 6"),
        RosterEntry(released, "Jo", "Pixel 4a", paired = false, released = true),
    )
    private val call = Notifier.AuditAction.EMERGENCY_CALL

    private var clockNow = 1_800_000_000L
    private fun store() = CarrierStore(FakePrefs(), SpoolFile(File(tmp.root, "inbox-spool.json")), clock = { clockNow })
    private fun stranger(i: Int) = "s$i".padEnd(64, '0')

    private fun decide(
        s: CarrierStore,
        machine: String,
        wrapId: String?,
        at: Long? = null,
        rosterKnown: Boolean = true,
        r: List<RosterEntry> = roster,
        action: Notifier.AuditAction = call,
        live: (String) -> Int = { 0 },
    ) = AuditAdmission.decide(s, action, machine, at, wrapId, r, rosterKnown, live)

    @Test fun aPairedWardIsToldOnceAndAReplayIsSkipped() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, "w1", at = 100))
        assertEquals(AuditAdmission.Skip, decide(s, ward, "w1", at = 100))
    }

    @Test fun aWrapRebuiltForTheSamePressIsNotToldAgain() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, "w1", at = 100))
        assertEquals(AuditAdmission.Skip, decide(s, ward, "w1-rebuilt", at = 100))
        // The next day's rebuild too, and a later wrap is not mistaken for it.
        assertEquals(AuditAdmission.Skip, decide(s, ward, "w1-rebuilt-2", at = 100))
        assertEquals(AuditAdmission.Alert, decide(s, ward, "w2", at = 200))
    }

    @Test fun twoPressesWithNoPressTimeEachAlertOnceByWrapId() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, "a", at = null))
        assertEquals(AuditAdmission.Alert, decide(s, ward, "b", at = null))
        assertEquals(AuditAdmission.Skip, decide(s, ward, "a", at = null))
    }

    @Test fun anUnreadableWrapIdStillAlerts() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, null))
        assertEquals(AuditAdmission.Alert, decide(s, ward, null))
    }

    @Test fun aKindOfPressIsToldSeparatelyFromAnotherAtTheSameTime() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, "a", at = 100, action = call))
        assertEquals(AuditAdmission.Alert, decide(s, ward, "b", at = 100, action = Notifier.AuditAction.UNLOCK))
    }

    @Test fun aReleasedDeviceIsToldAsOneOfTheirOwn() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, released, "r1", at = 50))
        assertEquals(AuditAdmission.Skip, decide(s, released, "r1", at = 50))
    }

    @Test fun aFloodOfUnpairedSendersIsOneRowAndNeverEvictsAPairedWardsEntry() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, "real-press", at = 100))
        var counts = 0
        for (i in 1..1000) {
            val a = decide(s, stranger(i), "flood$i", at = i.toLong())
            assertTrue("a stranger is never an alert of its own", a is AuditAdmission.Unpaired)
            counts = (a as AuditAdmission.Unpaired).count
        }
        // The count is bounded, so one row says "50 or more" and the state stays small.
        assertEquals(CarrierStore.UNPAIRED_CAP, counts)
        // The real press, replayed after the flood, is still known and is NOT re-alerted.
        assertEquals(AuditAdmission.Skip, decide(s, ward, "real-press", at = 100))
        assertEquals(AuditAdmission.Skip, decide(s, ward, "real-press-rebuilt", at = 100))
    }

    @Test fun anUnpairedReplayIsSkippedAndAnUnreadableOneStillSpeaks() {
        val s = store()
        assertEquals(AuditAdmission.Unpaired(1, sound = true), decide(s, stranger(1), "x1"))
        assertEquals(AuditAdmission.Skip, decide(s, stranger(1), "x1"))
        // Within the hour: counted, but silent.
        assertEquals(AuditAdmission.Unpaired(1, sound = false), decide(s, stranger(2), null))
    }

    @Test fun withNoUsableRosterNobodyIsUnpaired() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, stranger(1), "x1", rosterKnown = false, r = emptyList()))
    }

    // ---- G2: the stranger row sounds at most once an hour ------------------------------

    @Test fun theStrangerRowSoundsOnceAnHourAndOtherwiseUpdatesSilently() {
        val s = store()
        val first = decide(s, stranger(1), "x1") as AuditAdmission.Unpaired
        assertTrue(first.sound)
        clockNow += 60
        val second = decide(s, stranger(2), "x2") as AuditAdmission.Unpaired
        assertFalse("a minute later it is silent", second.sound)
        assertEquals(2, second.count)
        clockNow += CarrierStore.UNPAIRED_SOUND_SECS - 61
        assertFalse("one second short of the hour", (decide(s, stranger(3), "x3") as AuditAdmission.Unpaired).sound)
        clockNow += 1
        assertTrue("the hour is up", (decide(s, stranger(4), "x4") as AuditAdmission.Unpaired).sound)
        clockNow += 1
        assertFalse("and the next hour starts from that sound", (decide(s, stranger(5), "x5") as AuditAdmission.Unpaired).sound)
    }

    @Test fun aReplayDoesNotSpendTheSoundAndAPairedWardIsNeverRateLimited() {
        val s = store()
        decide(s, stranger(1), "x1")
        clockNow += CarrierStore.UNPAIRED_SOUND_SECS
        // A replay is skipped before the sound is claimed, so the next real report still sounds.
        assertEquals(AuditAdmission.Skip, decide(s, stranger(1), "x1"))
        assertTrue((decide(s, stranger(2), "x2") as AuditAdmission.Unpaired).sound)
        // Paired and released wards: always their own alert, whatever the stranger row did.
        assertEquals(AuditAdmission.Alert, decide(s, ward, "w1", at = 1))
        assertEquals(AuditAdmission.Alert, decide(s, released, "r1", at = 2))
    }

    @Test fun aClockThatWentBackDoesNotHoldTheSoundBack() {
        val s = store()
        decide(s, stranger(1), "x1")
        clockNow -= 10_000
        assertTrue((decide(s, stranger(2), "x2") as AuditAdmission.Unpaired).sound)
    }

    // ---- G3: the count restarts when the row is dismissed ---------------------------------

    @Test fun theCountCoversOnlyReportsSinceTheRowWasLastDismissed() {
        val s = store()
        for (i in 1..37) decide(s, stranger(i), "x$i")
        assertEquals(38, (decide(s, stranger(38), "x38") as AuditAdmission.Unpaired).count)
        s.resetUnpaired(asks = false)
        assertEquals(1, (decide(s, stranger(39), "x39") as AuditAdmission.Unpaired).count)
        assertEquals(2, (decide(s, stranger(40), "x40") as AuditAdmission.Unpaired).count)
        // The ids already seen are kept: a relay replay after the dismissal is no news and no count.
        assertEquals(AuditAdmission.Skip, decide(s, stranger(39), "x39"))
        s.resetUnpaired(asks = false)
        assertEquals(1, (decide(s, stranger(41), "x41") as AuditAdmission.Unpaired).count)
    }

    @Test fun dismissingTheEmergencyRowDoesNotResetTheAsksRowOrTheOtherWayRound() {
        val s = store()
        assertEquals(1, s.noteUnpaired("a"))
        assertEquals(2, s.noteUnpaired("b"))
        assertEquals(1, s.noteUnpairedAsk("q1"))
        assertEquals(2, s.noteUnpairedAsk("q2"))
        s.resetUnpaired(asks = false)
        assertEquals(1, s.noteUnpaired("c"))
        assertEquals(3, s.noteUnpairedAsk("q3"))
        s.resetUnpaired(asks = true)
        assertEquals(1, s.noteUnpairedAsk("q4"))
        assertEquals(2, s.noteUnpaired("d"))
    }

    // ---- G4: a released machine's live rows are capped ----------------------------------

    @Test fun aReleasedMachineBeyondFiveLiveRowsFoldsIntoTheSharedStrangerRow() {
        val s = store()
        var rows = 0
        val results = (1..8).map { i ->
            decide(s, released, "r$i", at = i.toLong(), live = { rows }).also { if (it == AuditAdmission.Alert) rows++ }
        }
        assertEquals(List(Notifier.RELEASED_ROW_CAP) { AuditAdmission.Alert }, results.take(5))
        assertTrue(results.drop(5).all { it is AuditAdmission.Unpaired })
        assertEquals(1, (results[5] as AuditAdmission.Unpaired).count)
        assertEquals(3, (results[7] as AuditAdmission.Unpaired).count)
        // A dismissed row frees a place.
        assertEquals(AuditAdmission.Alert, decide(s, released, "r9", at = 9, live = { 4 }))
    }

    @Test fun theCapIsPerMachineAndNeverAppliesToAPairedWard() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, ward, "w1", at = 1, live = { 99 }))
        val other = "e".repeat(64)
        val r = roster + RosterEntry(other, "Leo", "Pixel 3", paired = false, released = true)
        val live: (String) -> Int = { if (it == released) 5 else 0 }
        assertTrue(decide(s, released, "r1", at = 1, r = r, live = live) is AuditAdmission.Unpaired)
        assertEquals(AuditAdmission.Alert, decide(s, other, "o1", at = 1, r = r, live = live))
    }

    // ---- G1: asks from any machine that is not paired share one row ----------------------

    private fun ask(
        s: CarrierStore,
        machine: String,
        reqId: String,
        r: List<RosterEntry> = roster,
        known: Boolean = true,
    ) = AskAdmission.decide(s, reqId, machine, r, known)

    @Test fun aPairedWardsAskIsNamedOnceAndAReplayIsSkipped() {
        val s = store()
        assertEquals(AskAdmission.Notify, ask(s, ward, "q1"))
        assertEquals(AskAdmission.Skip, ask(s, ward, "q1"))
        assertEquals(AskAdmission.Skip, ask(s, ward, ""))
    }

    @Test fun asksFromStrangersAndReleasedMachinesFoldIntoOneRowAndAreNeverNamed() {
        val s = store()
        assertEquals(AskAdmission.Unpaired(1), ask(s, stranger(1), "q1"))
        assertEquals(AskAdmission.Unpaired(2), ask(s, released, "q2"))
        assertEquals(AskAdmission.Unpaired(3), ask(s, stranger(2), "q3"))
        // A replay folds away, whoever sent it.
        assertEquals(AskAdmission.Skip, ask(s, released, "q2"))
        assertEquals(AskAdmission.Skip, ask(s, stranger(1), "q1"))
    }

    @Test fun aFloodOfUnpairedAsksNeverEvictsAPairedWardsRequestIdsAndIsBounded() {
        val s = store()
        assertEquals(AskAdmission.Notify, ask(s, ward, "real"))
        var last = 0
        for (i in 1..1000) last = (ask(s, stranger(i), "flood$i") as AskAdmission.Unpaired).count
        assertEquals(CarrierStore.UNPAIRED_CAP, last)
        // The real ask, replayed after 1000 forged ones, is still remembered.
        assertEquals(AskAdmission.Skip, ask(s, ward, "real"))
    }

    @Test fun anAskCountRestartsOnDismissal() {
        val s = store()
        for (i in 1..5) ask(s, stranger(i), "q$i")
        s.resetUnpaired(asks = true)
        assertEquals(AskAdmission.Unpaired(1), ask(s, stranger(6), "q6"))
    }

    @Test fun withNoUsableRosterEveryAskIsJudgedTheOldWay() {
        val s = store()
        assertEquals(AskAdmission.Notify, ask(s, stranger(1), "q1", r = emptyList(), known = false))
        assertEquals(AskAdmission.Skip, ask(s, stranger(1), "q1", r = emptyList(), known = false))
    }
}
