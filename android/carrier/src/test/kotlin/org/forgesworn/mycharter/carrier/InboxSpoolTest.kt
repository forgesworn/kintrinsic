package org.forgesworn.mycharter.carrier

import org.json.JSONArray
import org.junit.Assert.assertEquals
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
        // The link's own copy wins.
        assertEquals(1L, JSONArray(InboxSpool.merge(link, s)).getJSONObject(0).getLong("receivedAt"))
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
