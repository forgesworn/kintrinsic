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

    @Test fun theOldestGoWhenFullAndStaleEntriesAreDropped() {
        var s: String? = null
        for (i in 1..InboxSpool.CAP + 5) s = InboxSpool.add(s, "w$i", "m", wrap("w$i"), now)
        val held = ids(s!!)
        assertEquals(InboxSpool.CAP, held.size)
        assertEquals("w6", held.first())
        val later = InboxSpool.add(s, "new", "m", wrap("new"), now + InboxSpool.MAX_AGE_SECS + 1)
        assertEquals(listOf("new"), ids(later))
    }

    @Test fun aNonJsonWrapIsNotHeld() {
        val s = InboxSpool.add(null, "w1", "m1", wrap("w1"), now)
        assertEquals(s, InboxSpool.add(s, "bad", "m1", "not json", now))
        assertTrue(ids(InboxSpool.add(null, "bad", "m1", "not json", now)).isEmpty())
    }
}
