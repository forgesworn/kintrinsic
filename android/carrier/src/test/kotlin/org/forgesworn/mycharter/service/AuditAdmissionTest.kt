package org.forgesworn.mycharter.service

import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.FakePrefs
import org.forgesworn.mycharter.carrier.RosterEntry
import org.forgesworn.mycharter.carrier.SpoolFile
import org.junit.Assert.assertEquals
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

    private fun store() = CarrierStore(FakePrefs(), SpoolFile(File(tmp.root, "inbox-spool.json")))
    private fun stranger(i: Int) = "s$i".padEnd(64, '0')

    private fun decide(
        s: CarrierStore,
        machine: String,
        wrapId: String?,
        at: Long? = null,
        rosterKnown: Boolean = true,
        r: List<RosterEntry> = roster,
        action: Notifier.AuditAction = call,
    ) = AuditAdmission.decide(s, action, machine, at, wrapId, r, rosterKnown)

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
        assertEquals(AuditAdmission.Unpaired(1), decide(s, stranger(1), "x1"))
        assertEquals(AuditAdmission.Skip, decide(s, stranger(1), "x1"))
        assertEquals(AuditAdmission.Unpaired(1), decide(s, stranger(2), null))
    }

    @Test fun withNoUsableRosterNobodyIsUnpaired() {
        val s = store()
        assertEquals(AuditAdmission.Alert, decide(s, stranger(1), "x1", rosterKnown = false, r = emptyList()))
    }
}
