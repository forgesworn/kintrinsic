package org.forgesworn.mycharter.link

import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.FakePrefs
import org.forgesworn.mycharter.carrier.RosterEntry
import org.forgesworn.mycharter.carrier.RosterPayload
import org.forgesworn.mycharter.carrier.SpoolFile
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** The disconnect grace lives on the carrier: a released phone is admitted to the link until `linkUntil`. */
class LinkGraceTest {
    @get:Rule val tmp = TemporaryFolder()
    private val paired = "a".repeat(64)
    private val gone = "b".repeat(64)
    private val nowMs = 1_800_000_000_000L

    private fun released(machine: String, until: Long?, key: String = machine) =
        RosterEntry(key, "Jo", "Pixel 4a", paired = false, released = true, releasedAt = nowMs / 1000, linkUntil = until)

    private fun json(until: String?) =
        """{"v":1,"entries":[{"machine":"$gone","childName":"Jo","deviceLabel":"Pixel 4a","paired":false,"released":true,"releasedAt":${nowMs / 1000}${until?.let { ",\"linkUntil\":$it" } ?: ""}}]}"""

    // ---- parsing and persistence ---------------------------------------------------

    @Test fun linkUntilIsReadFromAReleasedEntryInMilliseconds() {
        val e = RosterPayload.parse(json("${nowMs + 120_000}"), nowMs / 1000)!!.entries[0]
        assertEquals(nowMs + 120_000, e.linkUntil)
        assertTrue(e.released)
        assertFalse(e.paired)
    }

    @Test fun anUnusableOrAbsentLinkUntilMeansNoGrace() {
        for (bad in listOf(null, "\"soon\"", "true", "-1", "null", "{}")) {
            assertNull(bad, RosterPayload.parse(json(bad), nowMs / 1000)!!.entries[0].linkUntil)
        }
    }

    @Test fun linkUntilOnAnEntryThatIsNotReleasedIsIgnored() {
        val j = """{"v":1,"entries":[{"machine":"$gone","childName":"Jo","deviceLabel":"P","linkUntil":${nowMs + 5}}]}"""
        assertNull(RosterPayload.parse(j, nowMs / 1000)!!.entries[0].linkUntil)
    }

    @Test fun theStoredRosterKeepsLinkUntilAcrossARestart() {
        val prefs = FakePrefs()
        fun store(clock: () -> Long) = CarrierStore(prefs, SpoolFile(File(tmp.root, "s.json")), clock)
        store { nowMs / 1000 }.saveRoster(RosterPayload.parse(json("${nowMs + 60_000}"), nowMs / 1000)!!)
        val stored = store { nowMs / 1000 }.roster().single()
        assertEquals(nowMs + 60_000, stored.linkUntil)
    }

    // ---- admission --------------------------------------------------------------------

    @Test fun aReleasedPhoneIsAdmittedOnlyWhileItsGraceRuns() {
        val roster = listOf(RosterEntry(paired, "Mia", "Pixel 6"), released(gone, nowMs + 1000))
        assertEquals(listOf(paired, gone), LinkJson.allowedMachines(roster, nowMs))
        assertEquals(listOf(paired, gone), LinkJson.allowedMachines(roster, nowMs + 999))
        // now < linkUntil is strict: at the instant it ends, it is out.
        assertEquals(listOf(paired), LinkJson.allowedMachines(roster, nowMs + 1000))
        assertEquals("""["$paired"]""", LinkJson.allowedJson(roster, nowMs + 5000))
    }

    @Test fun aReleasedPhoneWithNoGraceIsNeverAdmitted() {
        assertEquals(emptyList<String>(), LinkJson.allowedMachines(listOf(released(gone, null)), nowMs))
    }

    @Test fun anExpiredGraceIsNotAdmittedAfterARestartFromTheStoredRoster() {
        val prefs = FakePrefs()
        CarrierStore(prefs, SpoolFile(File(tmp.root, "s.json"))) { nowMs / 1000 }
            .saveRoster(RosterPayload.parse(json("${nowMs + 10_000}"), nowMs / 1000)!!)
        // The carrier restarts after the grace: the persisted entry is no longer admitted.
        val restarted = CarrierStore(prefs, SpoolFile(File(tmp.root, "s.json"))) { nowMs / 1000 + 60 }
        assertEquals(listOf(gone), LinkJson.allowedMachines(restarted.roster(), nowMs))
        assertEquals(emptyList<String>(), LinkJson.allowedMachines(restarted.roster(), nowMs + 60_000))
    }

    @Test fun aPairedMachineStaysAdmittedWhateverItsLinkUntilSays() {
        val e = RosterEntry(paired, "Mia", "Pixel 6", linkUntil = 1)
        assertEquals(listOf(paired), LinkJson.allowedMachines(listOf(e), nowMs))
    }

    @Test fun aMalformedKeyIsNeverAdmittedEvenInsideItsGrace() {
        assertEquals(emptyList<String>(), LinkJson.allowedMachines(listOf(released("short", nowMs + 1000)), nowMs))
    }

    // ---- the timer's target -----------------------------------------------------------

    @Test fun theNextGraceEndIsTheEarliestRunningOne() {
        val other = "c".repeat(64)
        val roster = listOf(
            RosterEntry(paired, "Mia", "Pixel 6"),
            released(gone, nowMs + 9000),
            released(other, nowMs + 4000),
            released("d".repeat(64), nowMs - 1), // already over
            released("e".repeat(64), null),
        )
        assertEquals(nowMs + 4000, LinkJson.nextGraceEndMs(roster, nowMs))
        assertEquals(nowMs + 9000, LinkJson.nextGraceEndMs(roster, nowMs + 4000))
        assertNull(LinkJson.nextGraceEndMs(roster, nowMs + 9000))
        assertNull(LinkJson.nextGraceEndMs(listOf(RosterEntry(paired, "Mia", "P")), nowMs))
    }
}
