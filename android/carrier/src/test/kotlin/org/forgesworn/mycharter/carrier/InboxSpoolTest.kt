package org.forgesworn.mycharter.carrier

import org.json.JSONArray
import org.json.JSONObject
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
        // An empty roster is "no roster yet": nobody is demoted or evicted.
        assertEquals(15 + 40, held.size)
        // And a later NON-empty roster that does not name the strangers files them as unknown.
        s = InboxSpool.add(s, "spoof41", fake(41), wrap("spoof41"), now + 200, setOf(real))
        assertEquals(15 + InboxSpool.UNKNOWN_CAP, ids(s).size)
        for (i in 1..15) assertTrue("real$i" in ids(s))
    }

    @Test fun aRosterThatDropsAMachineLeavesItsHeldEntriesInTheKnownGroup() {
        var s = InboxSpool.add(null, "a", real, wrap("a"), now, setOf(real))
        // The roster now names someone else entirely.
        for (i in 1..30) s = InboxSpool.add(s, "u$i", fake(i), wrap("u$i"), now + i, setOf("someone-else"))
        assertTrue("a" in ids(s))
    }

    @Test fun anEntryAdmittedAsUnknownStaysUnknownEvenIfTheRosterLaterNamesIt() {
        var s: String? = null
        for (i in 1..15) s = InboxSpool.add(s, "x$i", real, wrap("x$i"), now + i, setOf("someone-else"))
        assertEquals(InboxSpool.UNKNOWN_CAP, ids(s!!).size)
        val flagged = JSONArray(s).getJSONObject(0)
        assertEquals(false, flagged.getBoolean("known"))
        // A later admission against a roster that now names the machine is known,
        // and does not revive the old ones into the large group: they are still
        // capped at ten among themselves.
        s = InboxSpool.add(s, "late", real, wrap("late"), now + 99, setOf(real))
        s = InboxSpool.add(s, "x16", real, wrap("x16"), now + 100, setOf("someone-else"))
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

    @Test fun anEmptyRosterMakesNobodyUnknown() {
        // An empty roster (a console that booted without its household) says nothing
        // about who is a stranger: the first presses must not land in the 10-slot
        // bucket where ten forged audits would evict them.
        var s: String? = null
        for (i in 1..30) s = InboxSpool.add(s, "w$i", fake(i), wrap("w$i"), now + i, known = emptySet())
        assertEquals(30, ids(s!!).size)
        val all = JSONArray(s)
        for (i in 0 until all.length()) assertFalse("no entry is stamped under an empty roster", all.getJSONObject(i).has("known"))
    }

    @Test fun entriesAdmittedUnderAnEmptyRosterFollowTheNextNonEmptyOne() {
        var s: String? = null
        // (fake(1) and fake(10) are the same string once padded, so use distinct machines here.)
        fun machine(i: Int) = "%064x".format(i)
        for (i in 1..30) s = InboxSpool.add(s, "w$i", machine(i), wrap("w$i"), now + i, known = emptySet())
        // The roster arrives and names only one of them: the other 29 are strangers, capped at 10.
        s = InboxSpool.add(s, "w31", machine(1), wrap("w31"), now + 99, known = setOf(machine(1)))
        val held = ids(s)
        assertTrue("w1" in held && "w31" in held)
        assertEquals(1 + 1 + InboxSpool.UNKNOWN_CAP, held.size)
    }

    // ---- C7: tombstones ------------------------------------------------------------

    @Test fun aConsumedIdIsNotReadmitted() {
        val s = InboxSpool.add(null, "w2", "m", wrap("w2"), now)
        val again = InboxSpool.add(s, "w1", "m", wrap("w1"), now + 1, consumed = setOf("w1"))
        assertEquals(listOf("w2"), ids(again))
    }

    @Test fun tombstonesAreStampedAndPrunedAfterFortyEightHours() {
        val t = InboxSpool.tombstone(emptyMap(), """["a","b",""]""", now)
        assertEquals(mapOf("a" to now, "b" to now), t)
        val later = InboxSpool.tombstone(t, """["c"]""", now + InboxSpool.TOMBSTONE_SECS)
        assertEquals(setOf("a", "b", "c"), later.keys) // exactly 48 h old is still held
        val pruned = InboxSpool.tombstone(t, """["c"]""", now + InboxSpool.TOMBSTONE_SECS + 1)
        assertEquals(setOf("c"), pruned.keys)
    }

    @Test fun theOldestTombstonesGoByTimestampNotByMapOrder() {
        // A decoded document may not keep its key order: the map below lists the NEWEST first.
        val consumed = LinkedHashMap<String, Long>()
        for (i in InboxSpool.MAX_TOMBSTONES + 5 downTo 1) consumed["t$i"] = now + i
        val kept = InboxSpool.pruneTombstones(consumed, now + InboxSpool.MAX_TOMBSTONES + 5)
        assertEquals(InboxSpool.MAX_TOMBSTONES, kept.size)
        for (i in 1..5) assertFalse("t$i is the oldest and goes", "t$i" in kept)
        assertTrue("t6" in kept)
        assertTrue("t${InboxSpool.MAX_TOMBSTONES + 5}" in kept)
    }

    @Test fun heldIdsAreOnlyTheOnesTheSpoolHolds() {
        var s = InboxSpool.add(null, "w1", "m", wrap("w1"), now)
        s = InboxSpool.add(s, "w2", "m", wrap("w2"), now)
        assertEquals(listOf("w1"), InboxSpool.held(s, """["w1","ghost","","w1"]"""))
        assertEquals(emptyList<String>(), InboxSpool.held(s, """["ghost"]"""))
        assertEquals(emptyList<String>(), InboxSpool.held(s, "not json"))
        assertEquals(emptyList<String>(), InboxSpool.held(null, """["w1"]"""))
    }

    @Test fun tombstonesAreBounded() {
        val ids = (1..InboxSpool.MAX_TOMBSTONES + 10).joinToString(",", "[", "]") { "\"t$it\"" }
        val t = InboxSpool.tombstone(emptyMap(), ids, now)
        assertEquals(InboxSpool.MAX_TOMBSTONES, t.size)
        assertFalse("t1" in t)
        assertTrue("t${InboxSpool.MAX_TOMBSTONES + 10}" in t)
    }

    @Test fun aBadIdListTombstonesNothing() {
        assertEquals(mapOf("a" to now), InboxSpool.tombstone(mapOf("a" to now), "not json", now))
    }

    // ---- the file's document -------------------------------------------------------

    @Test fun theDocumentRoundTripsAndABareArrayStillReads() {
        val entries = InboxSpool.add(null, "w1", "m", wrap("w1"), now)
        val doc = InboxSpool.decode(InboxSpool.encode(InboxSpool.Doc(entries, mapOf("old" to now))))!!
        assertEquals(listOf("w1"), ids(doc.entries))
        assertEquals(mapOf("old" to now), doc.consumed)
        val legacy = InboxSpool.decode(entries)!!
        assertEquals(listOf("w1"), ids(legacy.entries))
        assertTrue(legacy.consumed.isEmpty())
    }

    @Test fun textThatIsNotASpoolDocumentIsDamage() {
        for (junk in listOf("", "not json", """{"a":1}""", """{"entries":"x"}""", """[{"id":"x""")) {
            assertEquals(junk, null, InboxSpool.decode(junk))
        }
    }

    // ---- C9: salvage ---------------------------------------------------------------

    @Test fun salvageKeepsEveryCompleteEntryOfATruncatedFile() {
        var s: String? = null
        for (i in 1..3) s = InboxSpool.add(s, "w$i", real, wrap("w$i"), now + i)
        val cut = s!!.substring(0, s.length - 25) // the last entry is torn
        assertEquals(listOf("w1", "w2"), ids(InboxSpool.salvage(cut)))
    }

    @Test fun salvageReadsEntriesOutOfATornDocumentAndSkipsGarbageBetweenThem() {
        var s: String? = null
        for (i in 1..3) s = InboxSpool.add(s, "w$i", real, wrap("w$i"), now + i)
        val parts = JSONArray(s)
        val torn = """{"v":2,"entries":[${parts.getJSONObject(0)},@@@garbage@@@,${parts.getJSONObject(1)},${parts.getJSONObject(2).toString().take(40)}"""
        assertEquals(listOf("w1", "w2"), ids(InboxSpool.salvage(torn)))
    }

    @Test fun salvageIgnoresObjectsThatAreNotEntriesAndKeepsStringsWithBraces() {
        val tricky = JSONObject()
            .put("id", "t").put("machine", "m").put("receivedAt", now)
            .put("wrap", JSONObject().put("id", "t").put("content", "a } { \\\" b"))
        val text = """[{"id":"nope","machine":"m"},$tricky,{"id":"t","machine":"m","wrap":{"id":"dup"}}"""
        assertEquals(listOf("t"), ids(InboxSpool.salvage(text)))
    }

    @Test fun salvageOfNothingReadableIsEmpty() {
        assertEquals("[]", InboxSpool.salvage(""))
        assertEquals("[]", InboxSpool.salvage("\u0000\u0000 not json"))
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

    // ---- G4: a released machine's own bucket -----------------------------------------------

    @Test fun aReleasedMachineHasTwentyEntriesAndNeverEvictsAPairedOnes() {
        val roster = setOf(real, "gone")
        var s = InboxSpool.add(null, "paired", real, wrap("paired"), now, roster, released = setOf("gone"))
        for (i in 1..60) s = InboxSpool.add(s, "g$i", "gone", wrap("g$i"), now + i, roster, released = setOf("gone"))
        val held = ids(s)
        assertEquals(1 + InboxSpool.RELEASED_CAP, held.size)
        assertTrue("paired" in held)
        assertTrue("g60" in held && "g41" in held)
        assertFalse("g40" in held)
        // Flagged at admission: still known (as before) and released.
        val e = JSONArray(s).getJSONObject(1)
        assertTrue(e.getBoolean("known"))
        assertTrue(e.getBoolean("released"))
    }

    @Test fun releasedEntriesDoNotCountAgainstThePairedCapNorTheUnknownOne() {
        val roster = setOf(real, "gone")
        var s: String? = null
        for (i in 1..InboxSpool.KNOWN_CAP) s = InboxSpool.add(s, "p$i", real, wrap("p$i"), now + i, roster, released = setOf("gone"))
        for (i in 1..InboxSpool.UNKNOWN_CAP) s = InboxSpool.add(s, "u$i", fake(i), wrap("u$i"), now + 500 + i, roster, released = setOf("gone"))
        for (i in 1..InboxSpool.RELEASED_CAP) s = InboxSpool.add(s, "g$i", "gone", wrap("g$i"), now + 900 + i, roster, released = setOf("gone"))
        assertEquals(InboxSpool.KNOWN_CAP + InboxSpool.UNKNOWN_CAP + InboxSpool.RELEASED_CAP, ids(s!!).size)
    }

    @Test fun anUnflaggedEntryFollowsTheCurrentRostersReleasedSet() {
        // Admitted with no roster (so no flag); the roster later names the machine as released.
        var s: String? = null
        for (i in 1..30) s = InboxSpool.add(s, "g$i", "gone", wrap("g$i"), now + i, known = null)
        val e = JSONArray(s).getJSONObject(0)
        assertFalse(e.has("known") || e.has("released"))
        s = InboxSpool.add(s, "g31", "gone", wrap("g31"), now + 31, setOf(real, "gone"), released = setOf("gone"))
        assertEquals(InboxSpool.RELEASED_CAP, ids(s).size)
    }

    @Test fun anEntryHeldBeforeTheFlagExistedStaysWithThePairedOnes() {
        // `known:true` and no `released` is how a released machine's entry was stored until now.
        val old = JSONObject().put("id", "old").put("machine", "gone").put("wrap", JSONObject()).put("receivedAt", now).put("known", true)
        var s = JSONArray().put(old).toString()
        for (i in 1..40) s = InboxSpool.add(s, "g$i", "gone", wrap("g$i"), now + i, setOf(real, "gone"), released = setOf("gone"))
        assertTrue("old" in ids(s))
        assertEquals(1 + InboxSpool.RELEASED_CAP, ids(s).size)
    }
}
