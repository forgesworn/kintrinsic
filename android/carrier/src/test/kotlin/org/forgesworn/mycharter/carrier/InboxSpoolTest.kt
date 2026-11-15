package org.forgesworn.mycharter.carrier

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Relay-delivered emergency audits held for the console until it consumes them. */
class InboxSpoolTest {
    private val now = 1_800_000_000L
    private fun wrap(id: String) = """{"id":"$id","kind":1059,"content":"x","tags":[]}"""
    private fun ids(json: String): List<String> =
        JSONArray(json).let { a -> (0 until a.length()).map { a.getJSONObject(it).getString("id") } }

    @Test fun aRelayWrapLandsInTheInboxExactlyOnce() {
        val once = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        val replayed = InboxSpool.add(once, "w1", "m1", wrap("w1"), now + 5)
        assertEquals(listOf("w1"), ids(replayed))
        // The console's shape: id, machine, the wrap object, receivedAt.
        val e = JSONArray(replayed).getJSONObject(0)
        assertEquals("m1", e.getString("machine"))
        assertEquals("w1", e.getJSONObject("wrap").getString("id"))
        assertEquals(now, e.getLong("receivedAt"))
    }

    @Test fun aRebuiltWrapIsAnotherEntryAndTheConsoleFoldsThem() {
        var s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        s = InboxSpool.add(s, "w2", "m1", wrap("w2"), now + 1)
        assertEquals(listOf("w1", "w2"), ids(s))
    }

    @Test fun consumingForgetsOnlyThoseIds() {
        var s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        s = InboxSpool.add(s, "w2", "m1", wrap("w2"), now)
        assertEquals(listOf("w2"), ids(InboxSpool.remove(s, """["w1","zzz"]""")))
        assertEquals(emptyList<String>(), ids(InboxSpool.remove(s, """["w1","w2"]""")))
    }

    @Test fun aBadIdListChangesNothing() {
        val s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        assertEquals(s, InboxSpool.remove(s, "not json"))
    }

    @Test fun mergeAppendsSpooledEntriesTheLinkDoesNotAlreadyHold() {
        val link = """[{"id":"w1","machine":"m1","wrap":{},"receivedAt":1}]"""
        var s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        s = InboxSpool.add(s, "w2", "m1", wrap("w2"), now)
        assertEquals(listOf("w1", "w2"), ids(InboxSpool.merge(link, s)))
    }

    @Test fun aSharedIdEmitsTheSpooledEntryWithItsVerifiedAt() {
        // The press also came over the home network: both stores hold the id.
        val link = """[{"id":"w1","machine":"m1","wrap":{},"receivedAt":1},{"id":"only","machine":"m1","wrap":{},"receivedAt":2}]"""
        val s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        val merged = JSONArray(InboxSpool.merge(link, s))
        assertEquals(listOf("w1", "only"), ids(merged.toString()))
        val shared = merged.getJSONObject(0)
        assertEquals(now, shared.getLong("verifiedAt"))
        assertEquals("w1", shared.getJSONObject("wrap").getString("id"))
        // The native-only entry is untouched and has no verifiedAt.
        assertFalse(merged.getJSONObject(1).has("verifiedAt"))
    }

    @Test fun aSharedIdIsNeverEmittedTwice() {
        val link = """[{"id":"w1","machine":"m1","wrap":{},"receivedAt":1}]"""
        val s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        assertEquals(1, JSONArray(InboxSpool.merge(link, s)).length())
    }

    @Test fun mergeWorksWithTheLinkOffAndNothingSpooled() {
        assertEquals("[]", InboxSpool.merge("[]", null))
        assertEquals("[]", InboxSpool.merge("[]", "garbage"))
        val s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        assertEquals(listOf("w1"), ids(InboxSpool.merge("[]", s)))
    }

    @Test fun theOldestKnownGoWhenFullAndStaleEntriesAreDropped() {
        var s: String? = null
        for (i in 1..InboxSpool.KNOWN_CAP + 5) s = InboxSpool.add(s, "w$i", "m", wrap("w$i"), now)
        val held = ids(s!!)
        assertEquals(InboxSpool.KNOWN_CAP, held.size)
        assertEquals("w6", held.first())
        val later = InboxSpool.add(s, "new", "m", wrap("new"), now + InboxSpool.MAX_AGE_SECS + 1)
        assertEquals(listOf("new"), ids(later))
    }

    @Test fun anEntryStaysHeldPastTheOldTwoDayLimit() {
        val s = InboxSpool.add(null, "w1", "m", wrap("w1"), now)
        val later = InboxSpool.add(s, "w2", "m", wrap("w2"), now + 5L * 24 * 60 * 60)
        assertEquals(listOf("w1", "w2"), ids(later))
    }

    // ---- V1: a stranger cannot push out a real press ---------------------------

    private val real = "r".repeat(64)
    private fun fake(i: Int) = "f$i".padEnd(64, '0')

    @Test fun unknownMachinesNeverEvictKnownOnes() {
        val known = setOf(real)
        var s = InboxSpool.add(null, "real1", real, wrap("real1"), now, known)
        s = InboxSpool.add(s, "real2", real, wrap("real2"), now + 1, known)
        for (i in 1..500) s = InboxSpool.add(s, "spoof$i", fake(i), wrap("spoof$i"), now + 2 + i, known)
        val held = ids(s)
        assertTrue("real1" in held && "real2" in held)
        assertEquals(2 + InboxSpool.UNKNOWN_CAP, held.size)
    }

    @Test fun unknownMachinesGoOldestFirstWithinTheirOwnCap() {
        val known = setOf(real)
        var s: String? = null
        for (i in 1..InboxSpool.UNKNOWN_CAP + 3) s = InboxSpool.add(s, "u$i", fake(i), wrap("u$i"), now + i, known)
        val held = ids(s!!)
        assertEquals(InboxSpool.UNKNOWN_CAP, held.size)
        assertEquals("u4", held.first())
        assertEquals("u${InboxSpool.UNKNOWN_CAP + 3}", held.last())
    }

    @Test fun knownEntriesHaveTheirOwnLargerCapAndOrderIsKept() {
        val known = setOf(real)
        var s = InboxSpool.add(null, "a", real, wrap("a"), now, known)
        s = InboxSpool.add(s, "u", fake(1), wrap("u"), now + 1, known)
        s = InboxSpool.add(s, "b", real, wrap("b"), now + 2, known)
        assertEquals(listOf("a", "u", "b"), ids(s))
        for (i in 1..InboxSpool.KNOWN_CAP + 2) s = InboxSpool.add(s, "k$i", real, wrap("k$i"), now + 10 + i, known)
        val held = ids(s)
        assertEquals(InboxSpool.KNOWN_CAP + 1, held.size) // the 200 known plus the one unknown
        assertTrue("u" in held)
        assertTrue("a" !in held && "b" !in held)
    }

    // ---- N2: decided at admission, never re-bucketed by a later roster ----------

    @Test fun anEmptyRosterLaterNeverEvictsOrDemotesHeldKnownPresses() {
        val known = setOf(real)
        var s: String? = null
        for (i in 1..15) s = InboxSpool.add(s, "real$i", real, wrap("real$i"), now + i, known)
        // The console boots without its household and pushes an empty roster;
        // strangers keep arriving. None of the 15 held presses may go.
        for (i in 1..40) s = InboxSpool.add(s, "spoof$i", fake(i), wrap("spoof$i"), now + 100 + i, emptySet())
        val held = ids(s!!)
        for (i in 1..15) assertTrue("real$i" in held)
        assertEquals(15 + InboxSpool.UNKNOWN_CAP, held.size)
    }

    @Test fun aRosterThatDropsAMachineLeavesItsHeldEntriesInTheKnownGroup() {
        var s = InboxSpool.add(null, "a", real, wrap("a"), now, setOf(real))
        // The roster now names someone else entirely.
        for (i in 1..30) s = InboxSpool.add(s, "u$i", fake(i), wrap("u$i"), now + i, setOf("someone-else"))
        assertTrue("a" in ids(s))
    }

    @Test fun anEntryAdmittedAsUnknownStaysUnknownEvenIfTheRosterLaterNamesIt() {
        var s: String? = null
        for (i in 1..15) s = InboxSpool.add(s, "x$i", real, wrap("x$i"), now + i, emptySet())
        assertEquals(InboxSpool.UNKNOWN_CAP, ids(s!!).size)
        val flagged = JSONArray(s).getJSONObject(0)
        assertEquals(false, flagged.getBoolean("known"))
        // A later admission against a roster that now names the machine is known,
        // and does not revive the old ones into the large group: they are still
        // capped at ten among themselves.
        s = InboxSpool.add(s, "late", real, wrap("late"), now + 99, setOf(real))
        s = InboxSpool.add(s, "x16", real, wrap("x16"), now + 100, emptySet())
        val held = ids(s)
        assertTrue("late" in held)
        assertTrue("x6" !in held)
        assertEquals(InboxSpool.UNKNOWN_CAP + 1, held.size)
    }

    @Test fun entriesAdmittedWithNoRosterCarryNoFlagAndFollowTheCurrentRoster() {
        val s = InboxSpool.add(null, "w1", real, wrap("w1"), now, null)
        assertFalse(JSONArray(s).getJSONObject(0).has("known"))
    }

    @Test fun beforeTheFirstRosterEveryMachineCountsAsKnown() {
        var s: String? = null
        for (i in 1..50) s = InboxSpool.add(s, "w$i", fake(i), wrap("w$i"), now + i, known = null)
        assertEquals(50, ids(s!!).size) // none squeezed into the unknown cap of 10
    }

    @Test fun anEmptyRosterMakesEveryoneUnknown() {
        var s: String? = null
        for (i in 1..30) s = InboxSpool.add(s, "w$i", fake(i), wrap("w$i"), now + i, known = emptySet())
        assertEquals(InboxSpool.UNKNOWN_CAP, ids(s!!).size)
    }

    @Test fun aWrapOverSixteenKilobytesIsDropped() {
        val big = """{"id":"big","content":"${"x".repeat(InboxSpool.MAX_ENTRY_BYTES)}"}"""
        val s = InboxSpool.add(null, "w1", "m", wrap("w1"), now)
        assertEquals(s, InboxSpool.add(s, "big", "m", big, now))
        val ok = """{"id":"ok","content":"${"x".repeat(InboxSpool.MAX_ENTRY_BYTES - 100)}"}"""
        assertEquals(listOf("w1", "ok"), ids(InboxSpool.add(s, "ok", "m", ok, now)))
    }

    @Test fun everyEntryCarriesWhenTheCarrierVerifiedIt() {
        val e = JSONArray(InboxSpool.add(null, "w1", "m", wrap("w1"), now)).getJSONObject(0)
        assertEquals(now, e.getLong("verifiedAt"))
        assertEquals(now, e.getLong("receivedAt"))
    }

    @Test fun aNonJsonWrapIsNotHeld() {
        val s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        assertEquals(s, InboxSpool.add(s, "bad", "m1", "not json", now))
        assertTrue(ids(InboxSpool.add(null, "bad", "m1", "not json", now)).isEmpty())
    }
}
