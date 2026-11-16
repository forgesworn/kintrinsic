package org.forgesworn.mycharter.carrier

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class RosterTest {
    private val machine = "a".repeat(64)

    private fun json(entries: String = """[{"machine":"$machine","childName":"Mia","deviceLabel":"Pixel 6"}]""") =
        """{"v":1,"entries":$entries}"""

    // ---- RosterPayload.parse ----------------------------------------------

    @Test fun parsesAValidRoster() {
        val r = RosterPayload.parse(json())!!
        assertEquals(1, r.entries.size)
        assertEquals(RosterEntry(machine, "Mia", "Pixel 6"), r.entries[0])
    }

    @Test fun parsesAnEmptyRoster() {
        val r = RosterPayload.parse(json(entries = "[]"))!!
        assertTrue(r.entries.isEmpty())
    }

    @Test fun rejectsBadVersion() {
        assertNull(RosterPayload.parse("""{"v":2,"entries":[]}"""))
    }

    // ---- G9: one bad entry never rejects the push ---------------------------------

    @Test fun anEntryMissingOrEmptyInAFieldIsSkippedAndTheRestKept() {
        val other = "b".repeat(64)
        val good = """{"machine":"$other","childName":"Leo","deviceLabel":"Pixel 4a"}"""
        val bad = listOf(
            """{"machine":"$machine","childName":"Mia"}""",
            """{"machine":"$machine","childName":"","deviceLabel":"Pixel 6"}""",
            """{"machine":"$machine","childName":"Mia","deviceLabel":""}""",
            """{"childName":"Mia","deviceLabel":"Pixel 6"}""",
            """{"machine":"","childName":"Mia","deviceLabel":"Pixel 6"}""",
            "\"not an object\"",
            "null",
            "7",
        )
        for (b in bad) {
            val r = RosterPayload.parse(json(entries = "[$b,$good]"))
            assertEquals(b, listOf(RosterEntry(other, "Leo", "Pixel 4a")), r!!.entries)
        }
    }

    @Test fun aPushOfOnlyBadEntriesIsAnEmptyRosterNotARejection() {
        val r = RosterPayload.parse(json(entries = """[{"machine":"$machine","childName":""}]"""))
        assertTrue(r!!.entries.isEmpty())
    }

    @Test fun aBadEnvelopeIsStillRejectedWhole() {
        assertNull(RosterPayload.parse("""{"v":1,"entries":"x"}"""))
        assertNull(RosterPayload.parse("""[]"""))
    }

    @Test fun rejectsGarbageJson() {
        assertNull(RosterPayload.parse("not json at all"))
    }

    @Test fun rejectsMissingEntriesArray() {
        assertNull(RosterPayload.parse("""{"v":1}"""))
    }

    // ---- paired (local link admission) ---------------------------------------

    private fun entry(paired: String?) =
        """{"machine":"$machine","childName":"Mia","deviceLabel":"Pixel 6"${paired?.let { ",\"paired\":$it" } ?: ""}}"""

    @Test fun absentPairedMeansPaired() {
        assertTrue(RosterPayload.parse(json(entries = "[${entry(null)}]"))!!.entries[0].paired)
    }

    @Test fun pairedTrueAndFalseAreRead() {
        assertTrue(RosterPayload.parse(json(entries = "[${entry("true")}]"))!!.entries[0].paired)
        assertEquals(false, RosterPayload.parse(json(entries = "[${entry("false")}]"))!!.entries[0].paired)
    }

    @Test fun aMalformedPairedDeniesRatherThanAdmits() {
        for (bad in listOf("\"true\"", "1", "null", "{}")) {
            assertEquals(bad, false, RosterPayload.parse(json(entries = "[${entry(bad)}]"))!!.entries[0].paired)
        }
    }

    // ---- describeWard -------------------------------------------------------

    @Test fun aKnownMachineResolves() {
        val roster = listOf(RosterEntry(machine, "Mia", "Pixel 6"))
        assertEquals(WardName("Mia", "Pixel 6"), describeWard(machine, roster))
    }

    @Test fun anUnknownMachineReturnsNull() {
        val roster = listOf(RosterEntry(machine, "Mia", "Pixel 6"))
        assertNull(describeWard("b".repeat(64), roster))
    }

    @Test fun anEmptyRosterReturnsNullAndDoesNotThrow() {
        assertNull(describeWard(machine, emptyList()))
    }

    @Test fun anEmptyMachineReturnsNull() {
        val roster = listOf(RosterEntry(machine, "Mia", "Pixel 6"))
        assertNull(describeWard("", roster))
    }

    // ---- released ---------------------------------------------------------------

    private fun releasedEntry(extra: String) =
        """{"machine":"$machine","childName":"Mia","deviceLabel":"Pixel 6"$extra}"""

    @Test fun absentReleasedMeansNotReleased() {
        assertEquals(false, RosterPayload.parse(json())!!.entries[0].released)
    }

    @Test fun aReleasedEntryIsNeverPairedWhateverItSays() {
        for (paired in listOf("", ",\"paired\":true", ",\"paired\":false")) {
            val e = RosterPayload.parse(json(entries = "[${releasedEntry("$paired,\"released\":true")}]"))!!.entries[0]
            assertTrue(paired, e.released)
            assertEquals(paired, false, e.paired)
        }
    }

    @Test fun aMalformedReleasedIsNotReleased() {
        for (bad in listOf("\"true\"", "1", "null", "{}")) {
            val e = RosterPayload.parse(json(entries = "[${releasedEntry(",\"released\":$bad")}]"))!!.entries[0]
            assertEquals(bad, false, e.released)
            assertTrue(bad, e.paired)
        }
    }

    @Test fun aReleasedMachineIsNamedAndReportedAsReleased() {
        val roster = listOf(RosterEntry(machine, "Mia", "Pixel 6", paired = false, released = true))
        assertEquals(WardName("Mia", "Pixel 6"), describeWard(machine, roster))
        assertTrue(isReleased(machine, roster))
    }

    @Test fun aPairedMachineIsNotReleasedEvenIfAnOldReleasedEntrySharesItsKey() {
        val roster = listOf(
            RosterEntry(machine, "Old name", "Old phone", paired = false, released = true),
            RosterEntry(machine, "Mia", "Pixel 6"),
        )
        assertEquals(WardName("Mia", "Pixel 6"), describeWard(machine, roster))
        assertEquals(false, isReleased(machine, roster))
    }

    @Test fun anUnknownOrEmptyMachineIsNotReleased() {
        val roster = listOf(RosterEntry(machine, "Mia", "Pixel 6", paired = false, released = true))
        assertEquals(false, isReleased("b".repeat(64), roster))
        assertEquals(false, isReleased("", roster))
    }

    // ---- G5: releasedAt and the 14-day grace period -------------------------------

    private val now = 1_800_000_000L
    private fun released(at: String?) =
        """{"machine":"$machine","childName":"Mia","deviceLabel":"Pixel 6","released":true${at?.let { ",\"releasedAt\":$it" } ?: ""}}"""

    @Test fun releasedAtIsReadFromAReleasedEntry() {
        val e = RosterPayload.parse(json(entries = "[${released("${now - 5}")}]"), now)!!.entries[0]
        assertEquals(now - 5, e.releasedAt)
        assertTrue(e.released)
    }

    @Test fun aReleasedEntryWithNoUsableReleasedAtIsKeptWithNone() {
        for (bad in listOf(null, "\"soon\"", "true", "-4", "null", "{}")) {
            val e = RosterPayload.parse(json(entries = "[${released(bad)}]"), now)!!.entries
            assertEquals(bad, 1, e.size)
            assertNull(bad, e[0].releasedAt)
        }
    }

    @Test fun releasedAtOnAnEntryThatIsNotReleasedIsIgnored() {
        val e = RosterPayload.parse(json(entries = """[{"machine":"$machine","childName":"Mia","deviceLabel":"P","releasedAt":$now}]"""), now)!!.entries[0]
        assertNull(e.releasedAt)
    }

    @Test fun aReleasedEntryIsDroppedOnceItsFourteenDaysAreOver() {
        val edge = now - RELEASED_GRACE_SECS
        // Exactly 14 days old is still kept; one second more is gone.
        assertEquals(1, RosterPayload.parse(json(entries = "[${released("$edge")}]"), now)!!.entries.size)
        assertEquals(0, RosterPayload.parse(json(entries = "[${released("${edge - 1}")}]"), now)!!.entries.size)
    }

    @Test fun expiryNeverTouchesAPairedEntryAndKeepsTheOthers() {
        val old = now - RELEASED_GRACE_SECS - 1000
        val r = RosterPayload.parse(
            json(entries = """[{"machine":"p","childName":"Leo","deviceLabel":"X","releasedAt":$old},${released("$old")}]"""),
            now,
        )!!
        assertEquals(listOf("p"), r.entries.map { it.machine })
    }

    @Test fun releasedAtOfIsTheLatestForAMachineOnlyNamedAsReleased() {
        val roster = listOf(
            RosterEntry(machine, "Mia", "Pixel 6", paired = false, released = true, releasedAt = 100),
            RosterEntry(machine, "Mia", "Pixel 6", paired = false, released = true, releasedAt = 300),
        )
        assertEquals(300L, releasedAtOf(machine, roster))
        assertNull(releasedAtOf(machine, roster + RosterEntry(machine, "Mia", "Pixel 6")))
        assertNull(releasedAtOf("b".repeat(64), roster))
        assertNull(releasedAtOf(machine, listOf(RosterEntry(machine, "Mia", "P", paired = false, released = true))))
    }
}
