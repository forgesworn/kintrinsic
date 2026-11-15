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

    @Test fun rejectsAnEntryMissingAField() {
        assertNull(RosterPayload.parse(json(entries = """[{"machine":"$machine","childName":"Mia"}]""")))
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
}
