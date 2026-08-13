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
}
