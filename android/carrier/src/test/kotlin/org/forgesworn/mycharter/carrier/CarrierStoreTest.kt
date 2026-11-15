package org.forgesworn.mycharter.carrier

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File

/** The store's own rules, over an in-memory preferences file and a real temp spool. */
class CarrierStoreTest {
    @get:Rule val tmp = TemporaryFolder()
    private val now = 1_800_000_000L
    private val real = "r".repeat(64)
    private fun wrap(id: String) = """{"id":"$id","kind":1059}"""
    private fun fake(i: Int) = "f$i".padEnd(64, '0')
    private fun ids(json: String): List<String> =
        JSONArray(json).let { a -> (0 until a.length()).map { a.getJSONObject(it).getString("id") } }

    private fun store(prefs: FakePrefs = FakePrefs()) =
        CarrierStore(prefs, SpoolFile(File(tmp.root, "inbox-spool.json"), clock = { now }))

    private fun roster(vararg e: RosterEntry) = RosterPayload(e.toList())

    // ---- C1: an empty roster is not a roster -----------------------------------------

    @Test fun anEmptyRosterIsNoUsableRoster() {
        val s = store()
        assertNull(s.knownRoster())
        s.saveRoster(roster())
        assertEquals(emptySet<String>(), s.knownMachines())
        assertNull(s.knownRoster())
        s.saveRoster(roster(RosterEntry(real, "Mia", "Pixel 6")))
        assertEquals(setOf(real), s.knownRoster())
    }

    @Test fun aFloodAfterAnEmptyRosterNeverEvictsTheFirstPress() {
        val s = store()
        s.saveRoster(roster()) // the console could not read its household
        s.spoolInboxWrap("press", real, wrap("press"), now)
        for (i in 1..30) s.spoolInboxWrap("spoof$i", fake(i), wrap("spoof$i"), now + i)
        assertTrue("press" in ids(s.inboxSpool()))
        // Nothing was stamped known (or unknown) under the empty roster.
        val held = JSONArray(s.inboxSpool())
        for (i in 0 until held.length()) assertFalse(held.getJSONObject(i).has("known"))
    }

    @Test fun anEntryAdmittedUnderAnEmptyRosterFollowsTheNextRoster() {
        val s = store()
        s.saveRoster(roster())
        for (i in 1..30) s.spoolInboxWrap("w$i", fake(i), wrap("w$i"), now + i)
        s.saveRoster(roster(RosterEntry(real, "Mia", "Pixel 6")))
        s.spoolInboxWrap("real1", real, wrap("real1"), now + 99)
        val held = ids(s.inboxSpool())
        assertTrue("real1" in held)
        // The 30 earlier ones name nobody on the roster: now judged unknown and capped.
        assertEquals(1 + InboxSpool.UNKNOWN_CAP, held.size)
    }

    // ---- C2: audits have their own dedup list -----------------------------------------

    @Test fun aFloodOfAskIdsNeverEvictsAnAuditKey() {
        val s = store()
        assertTrue(s.markAuditSeen("audit:w1"))
        for (i in 1..500) s.markSeen("req$i")
        assertFalse("a replayed wrap must not re-alert", s.markAuditSeen("audit:w1"))
    }

    @Test fun aFloodOfAuditKeysNeverEvictsAnAsksReqId() {
        val s = store()
        assertTrue(s.markSeen("req1"))
        for (i in 1..500) s.markAuditSeen("audit:w$i")
        assertFalse(s.markSeen("req1"))
    }

    @Test fun anAuditAlreadyToldBeforeTheSplitDoesNotReAlertOnceAfterAnUpdate() {
        val prefs = FakePrefs()
        val old = store(prefs)
        assertTrue(old.markSeen("audit:w1")) // how it was recorded in the shared list
        assertFalse(store(prefs).markAuditSeen("audit:w1"))
        assertTrue(store(prefs).markAuditSeen("audit:w2"))
    }

    @Test fun theAuditListIsBoundedAndForgetsTheOldest() {
        val s = store()
        for (i in 1..250) s.markAuditSeen("audit:w$i")
        assertTrue(s.markAuditSeen("audit:w1")) // long gone
        assertFalse(s.markAuditSeen("audit:w250"))
    }

    @Test fun unpairedReportsAreCountedOnceEachInASmallBoundedSet() {
        val s = store()
        assertEquals(1, s.noteUnpaired("a"))
        assertEquals(2, s.noteUnpaired("b"))
        assertNull(s.noteUnpaired("a")) // a replay: no re-alert
        for (i in 1..200) s.noteUnpaired("x$i")
        assertEquals(CarrierStore.UNPAIRED_CAP, s.noteUnpaired("one-more"))
        // They never entered the audit list a paired ward's wraps live in.
        assertTrue(s.markAuditSeen("audit:a"))
    }

    @Test fun anUnreadableWrapIdIsStillCountedAsAtLeastOneReport() {
        assertEquals(1, store().noteUnpaired(null))
    }

    // ---- C4: the roster keeps released entries -----------------------------------------

    @Test fun theRosterPersistsReleasedEntriesAndNeverReadsThemAsPaired() {
        val prefs = FakePrefs()
        store(prefs).saveRoster(
            roster(
                RosterEntry(real, "Mia", "Pixel 6"),
                RosterEntry(fake(1), "Jo", "Pixel 4a", paired = false, released = true),
            ),
        )
        val back = store(prefs).roster()
        assertEquals(listOf(false, true), back.map { it.released })
        assertEquals(listOf(true, false), back.map { it.paired })
        assertEquals(setOf(real, fake(1)), store(prefs).knownMachines())
    }

    // ---- C7 through the store ----------------------------------------------------------

    @Test fun aConsumedPressIsNotReadmittedByAReplay() {
        val s = store()
        s.spoolInboxWrap("w1", real, wrap("w1"), now)
        s.consumeInboxSpool("""["w1"]""")
        s.spoolInboxWrap("w1", real, wrap("w1"), now + 30)
        assertEquals("[]", s.inboxSpool())
    }
}
