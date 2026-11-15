package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.time.ZoneId

/**
 * "Activity on this phone": the line format, the per-kind retention, the file,
 * the merged feed and its words, the three edge-trigger decisions, and the
 * emergency control's best-effort logging.
 */
class ActivityLogTest {

    private val london: ZoneId = ZoneId.of("Europe/London")
    private val id1 = "aaaabbbbccccddddeeeeffff00009999"

    // 2026-11-14 19:42:00 Europe/London (GMT) = 19:42 UTC.
    private val at1942 = 1794685320L

    private fun tmpFile(): File = File(createTempDir(), "activity.jsonl")

    // ---- format ------------------------------------------------------------------

    @Test
    fun `every kind round-trips through its line`() {
        val all = listOf(
            ActivityEntry(at1942, ActivityKind.EMERGENCY),
            ActivityEntry(at1942, ActivityKind.DEBUGGING_ON, until = at1942 + 3600),
            ActivityEntry(at1942, ActivityKind.DEBUGGING_ON),
            ActivityEntry(at1942, ActivityKind.DEBUGGING_OFF),
            ActivityEntry(at1942, ActivityKind.PAIRED, guardian = "abcd1234"),
            ActivityEntry(at1942, ActivityKind.UNPAIRED, guardian = "abcd1234"),
            ActivityEntry(at1942, ActivityKind.GUARDIAN_CHANGED, guardian = "ef567890", was = "abcd1234"),
            ActivityEntry(at1942, ActivityKind.UPDATED, from = "0.8.1", to = "0.8.2"),
        )
        for (e in all) {
            val line = ActivityFormat.encode(e)
            assertFalse(line.contains('\n'))
            assertEquals(e, ActivityFormat.decode(line))
        }
    }

    @Test
    fun `a line has the version, the time and the kind first`() {
        assertEquals(
            """{"v":1,"at":$at1942,"kind":"emergency"}""",
            ActivityFormat.encode(ActivityEntry(at1942, ActivityKind.EMERGENCY)),
        )
        assertEquals(
            """{"v":1,"at":$at1942,"kind":"paired","guardian":"abcd1234"}""",
            ActivityFormat.encode(ActivityEntry(at1942, ActivityKind.PAIRED, guardian = "abcd1234")),
        )
    }

    @Test
    fun `malformed lines, unknown kinds and unknown versions are skipped, never fatal`() {
        assertNull(ActivityFormat.decode(""))
        assertNull(ActivityFormat.decode("   "))
        assertNull(ActivityFormat.decode("not json"))
        assertNull(ActivityFormat.decode("""{"v":1,"at":$at1942,"kind":"emerg"""))
        assertNull(ActivityFormat.decode("""{"v":1,"at":$at1942,"kind":"lockout"}"""))
        assertNull(ActivityFormat.decode("""{"v":1,"at":$at1942}"""))
        assertNull(ActivityFormat.decode("""{"v":2,"at":$at1942,"kind":"emergency"}"""))
        assertNull(ActivityFormat.decode("""{"at":$at1942,"kind":"emergency"}"""))
        assertNull(ActivityFormat.decode("""{"v":1,"kind":"emergency"}"""))
        assertNull(ActivityFormat.decode("""{"v":1,"at":"soon","kind":"emergency"}"""))
        // `updated` without its versions says nothing.
        assertNull(ActivityFormat.decode("""{"v":1,"at":$at1942,"kind":"updated"}"""))
        assertNull(ActivityFormat.decode("[1,2,3]"))
    }

    @Test
    fun `a long field is cut, not trusted`() {
        val e = ActivityFormat.decode("""{"v":1,"at":1,"kind":"paired","guardian":"${"x".repeat(500)}"}""")!!
        assertEquals(64, e.guardian!!.length)
    }

    // ---- retention ---------------------------------------------------------------

    @Test
    fun `a flood of one kind keeps every other kind and only the newest 200 of its own`() {
        val entries = buildList {
            add(ActivityEntry(1, ActivityKind.PAIRED, guardian = "abcd1234"))
            add(ActivityEntry(2, ActivityKind.EMERGENCY))
            for (i in 0 until 300) add(ActivityEntry(100L + i, ActivityKind.DEBUGGING_ON))
            add(ActivityEntry(1000, ActivityKind.PAIRED, guardian = "ef567890"))
            add(ActivityEntry(1001, ActivityKind.UPDATED, from = "0.8.1", to = "0.8.2"))
        }
        val kept = ActivityRetention.compact(entries)
        assertEquals(2, kept.count { it.kind == ActivityKind.PAIRED })
        assertEquals(1, kept.count { it.kind == ActivityKind.EMERGENCY })
        assertEquals(1, kept.count { it.kind == ActivityKind.UPDATED })
        val dbg = kept.filter { it.kind == ActivityKind.DEBUGGING_ON }
        assertEquals(200, dbg.size)
        assertEquals(200L, dbg.first().at) // the oldest 100 of 300 went
        assertEquals(399L, dbg.last().at)
        // Original order is kept.
        assertEquals(kept.sortedBy { it.at }, kept)
    }

    @Test
    fun `a kind under its cap loses nothing`() {
        val entries = (0 until 50).map { ActivityEntry(it.toLong(), ActivityKind.EMERGENCY) }
        assertEquals(entries, ActivityRetention.compact(entries))
    }

    // ---- the file ----------------------------------------------------------------

    @Test
    fun `entries append, fsync and read back, in a fresh instance too`() {
        val f = tmpFile()
        val log = FileActivityLog(f)
        assertEquals(emptyList<ActivityEntry>(), log.entries())
        val a = ActivityEntry(at1942, ActivityKind.EMERGENCY)
        val b = ActivityEntry(at1942 + 5, ActivityKind.DEBUGGING_ON, until = at1942 + 900)
        assertTrue(log.append(a))
        assertTrue(log.append(b))
        assertEquals(listOf(a, b), log.entries())
        assertEquals(listOf(a, b), FileActivityLog(f).entries())
        assertEquals("activity.jsonl", FileActivityLog.fileIn(f.parentFile).name)
    }

    @Test
    fun `a torn last line is skipped and does not swallow the next entry`() {
        val f = tmpFile()
        val a = ActivityEntry(at1942, ActivityKind.EMERGENCY)
        f.writeText(ActivityFormat.encode(a) + "\n" + """{"v":1,"at":$at1942,"kind":"debug""")
        val log = FileActivityLog(f)
        assertEquals(listOf(a), log.entries())
        val b = ActivityEntry(at1942 + 9, ActivityKind.DEBUGGING_OFF)
        assertTrue(log.append(b))
        assertEquals(listOf(a, b), FileActivityLog(f).entries())
    }

    @Test
    fun `a file that will not read reads as null, never as empty`() {
        val dir = createTempDir()
        val f = File(dir, "activity.jsonl").apply { mkdirs() } // a directory where the file should be
        val log = FileActivityLog(f)
        assertNull(log.entries())
        assertFalse(log.append(ActivityEntry(at1942, ActivityKind.EMERGENCY)))
        assertNull(log.entries())
    }

    @Test
    fun `compaction rewrites per kind, atomically, once the file passes its line count`() {
        val f = tmpFile()
        val log = FileActivityLog(f, compactAboveLines = 40, perKind = 5)
        val paired = ActivityEntry(1, ActivityKind.PAIRED, guardian = "abcd1234")
        assertTrue(log.append(paired))
        for (i in 0 until 60) assertTrue(log.append(ActivityEntry(100L + i, ActivityKind.DEBUGGING_ON)))
        val live = log.entries()!!
        assertTrue(live.contains(paired))
        assertTrue(live.count { it.kind == ActivityKind.DEBUGGING_ON } <= 40)
        assertEquals(159L, live.last().at)
        // What is on disk is what is in memory, and no temp file is left.
        assertEquals(live, FileActivityLog(f).entries())
        assertFalse(File(f.parentFile, "activity.jsonl.tmp").exists())
        assertTrue(f.readLines().size <= 41)
        // A later append still works after a compaction.
        val e = ActivityEntry(5000, ActivityKind.EMERGENCY)
        assertTrue(log.append(e))
        assertEquals(e, FileActivityLog(f).entries()!!.last())
    }

    @Test
    fun `compaction never lets one kind push out another`() {
        val f = tmpFile()
        val log = FileActivityLog(f, compactAboveLines = 30, perKind = 4)
        assertTrue(log.append(ActivityEntry(1, ActivityKind.PAIRED, guardian = "abcd1234")))
        assertTrue(log.append(ActivityEntry(2, ActivityKind.EMERGENCY)))
        for (i in 0 until 100) assertTrue(log.append(ActivityEntry(10L + i, ActivityKind.DEBUGGING_ON)))
        val read = FileActivityLog(f).entries()!!
        assertEquals(1, read.count { it.kind == ActivityKind.PAIRED })
        assertEquals(1, read.count { it.kind == ActivityKind.EMERGENCY })
    }

    // ---- the feed ----------------------------------------------------------------

    private fun reset(id: String, at: Long, outcome: String, ackAt: Long? = null, kind: String = ScreenLockChange.CHANGED) =
        ScreenLockRecord(id, at, kind, outcome, ackAt != null, ackAt)

    @Test
    fun `the feed merges both sources, newest first`() {
        val sl = listOf(
            reset("a", at1942, ScreenLockOutcome.APPLIED, at1942 + 60),
            reset("b", at1942 + 7200, ScreenLockOutcome.APPLIED),
        )
        val act = listOf(
            ActivityEntry(at1942 + 3600, ActivityKind.EMERGENCY),
            ActivityEntry(at1942 + 10_000, ActivityKind.UPDATED, from = "0.8.1", to = "0.8.2"),
        )
        val feed = ActivityFeed.merge(sl, act, london)
        assertFalse(feed.unreadable)
        assertEquals(listOf(at1942 + 10_000, at1942 + 7200, at1942 + 3600, at1942), feed.rows.map { it.at })
        assertTrue(feed.rows[0].text.contains("Kintrinsic updated from 0.8.1 to 0.8.2"))
        assertTrue(feed.rows[1].text.contains("Screen lock changed"))
        assertTrue(feed.rows[2].text.contains("Emergency button"))
    }

    @Test
    fun `rows with the same time keep the later written one first`() {
        val act = listOf(
            ActivityEntry(at1942, ActivityKind.DEBUGGING_ON),
            ActivityEntry(at1942, ActivityKind.DEBUGGING_OFF),
        )
        val rows = ActivityFeed.merge(emptyList(), act, london).rows
        assertTrue(rows[0].text.contains("turned off"))
        assertTrue(rows[1].text.contains("turned on"))
    }

    @Test
    fun `a source that will not read never hides the other, and the feed says so`() {
        val sl = listOf(reset("a", at1942, ScreenLockOutcome.APPLIED))
        val act = listOf(ActivityEntry(at1942, ActivityKind.EMERGENCY))
        val noActivity = ActivityFeed.merge(sl, null, london)
        assertTrue(noActivity.unreadable)
        assertEquals(1, noActivity.rows.size)
        assertTrue(noActivity.rows[0].text.contains("Screen lock"))
        val noScreenLock = ActivityFeed.merge(null, act, london)
        assertTrue(noScreenLock.unreadable)
        assertEquals(1, noScreenLock.rows.size)
        assertTrue(noScreenLock.rows[0].text.contains("Emergency"))
        assertTrue(ActivityFeed.merge(null, null, london).unreadable)
        assertEquals("Some activity couldn't be read.", ActivityFeed.UNREADABLE)
        assertFalse(ActivityFeed.merge(emptyList(), emptyList(), london).unreadable)
    }

    @Test
    fun `ten rows show, the rest wait for Show all`() {
        val rows = (0 until 25).map { ActivityFeed.Row(it.toLong(), "r$it") }
        assertTrue(ActivityFeed.needsToggle(rows))
        assertEquals(10, ActivityFeed.visible(rows, showAll = false).size)
        assertEquals(25, ActivityFeed.visible(rows, showAll = true).size)
        assertFalse(ActivityFeed.needsToggle(rows.take(10)))
        assertEquals(10, ActivityFeed.visible(rows.take(10), showAll = false).size)
    }

    // ---- the words ---------------------------------------------------------------

    @Test
    fun `screen lock rows say when the OK was tapped, never who tapped it`() {
        assertEquals(
            "19:42, 14 Nov · Screen lock changed by your guardian · done · OK tapped 19:43",
            ActivityCopy.screenLockRow(reset("a", at1942, ScreenLockOutcome.APPLIED, at1942 + 60), london),
        )
        assertEquals(
            "19:42, 14 Nov · Screen lock changed by your guardian · done · not seen yet",
            ActivityCopy.screenLockRow(reset("a", at1942, ScreenLockOutcome.APPLIED), london),
        )
        // Tapped on another day: the day is named.
        assertEquals(
            "19:42, 14 Nov · Screen lock changed by your guardian · done · OK tapped 09:12, 16 Nov",
            ActivityCopy.screenLockRow(reset("a", at1942, ScreenLockOutcome.APPLIED, at1942 + 135_000), london),
        )
        assertEquals(
            "19:42, 14 Nov · Screen lock removed by your guardian · may have happened · not seen yet",
            ActivityCopy.screenLockRow(reset("a", at1942, ScreenLockOutcome.PENDING, kind = ScreenLockChange.REMOVED), london),
        )
    }

    @Test
    fun `failed and not-ready resets have no OK part`() {
        val failed = ActivityCopy.screenLockRow(reset("a", at1942, ScreenLockOutcome.FAILED), london)
        val notReady = ActivityCopy.screenLockRow(reset("a", at1942, ScreenLockOutcome.NOT_READY), london)
        assertEquals("19:42, 14 Nov · Screen lock changed by your guardian · it didn't work, nothing changed", failed)
        assertEquals("19:42, 14 Nov · Screen lock changed by your guardian · not done, this phone wasn't ready yet", notReady)
        for (s in listOf(failed, notReady)) {
            assertFalse(s.contains("OK"))
            assertFalse(s.contains("not seen yet"))
        }
    }

    @Test
    fun `no row ever says you tapped or seen by you`() {
        val rows = buildList {
            for (o in listOf(ScreenLockOutcome.APPLIED, ScreenLockOutcome.PENDING, ScreenLockOutcome.FAILED, ScreenLockOutcome.NOT_READY)) {
                add(ActivityCopy.screenLockRow(reset("a", at1942, o), london))
                add(ActivityCopy.screenLockRow(reset("a", at1942, o, at1942 + 60), london))
            }
            for (k in ActivityKind.ALL) {
                add(ActivityCopy.row(ActivityEntry(at1942, k, guardian = "abcd1234", was = "ef567890", from = "0.8.1", to = "0.8.2"), london))
            }
        }
        for (r in rows) {
            val lower = r.lowercase()
            assertFalse(r, lower.contains("you tapped"))
            assertFalse(r, lower.contains("seen by you"))
            assertFalse(r, lower.contains("you "))
        }
    }

    @Test
    fun `each activity row reads as the spec says`() {
        fun row(e: ActivityEntry) = ActivityCopy.row(e, london)
        assertEquals("19:42, 14 Nov · Emergency button used on the lock screen", row(ActivityEntry(at1942, ActivityKind.EMERGENCY)))
        assertEquals(
            "19:42, 14 Nov · USB debugging turned on (a computer can control this phone)",
            row(ActivityEntry(at1942, ActivityKind.DEBUGGING_ON)),
        )
        assertEquals("19:42, 14 Nov · USB debugging turned off", row(ActivityEntry(at1942, ActivityKind.DEBUGGING_OFF)))
        assertEquals("19:42, 14 Nov · Paired with guardian abcd1234", row(ActivityEntry(at1942, ActivityKind.PAIRED, guardian = "abcd1234")))
        assertEquals("19:42, 14 Nov · Unpaired from guardian abcd1234", row(ActivityEntry(at1942, ActivityKind.UNPAIRED, guardian = "abcd1234")))
        assertEquals(
            "19:42, 14 Nov · Now paired with guardian ef567890 (was abcd1234)",
            row(ActivityEntry(at1942, ActivityKind.GUARDIAN_CHANGED, guardian = "ef567890", was = "abcd1234")),
        )
        assertEquals(
            "19:42, 14 Nov · Kintrinsic updated from 0.8.1 to 0.8.2",
            row(ActivityEntry(at1942, ActivityKind.UPDATED, from = "0.8.1", to = "0.8.2")),
        )
    }

    // ---- the edge triggers -------------------------------------------------------

    @Test
    fun `debugging records a baseline silently, then each edge`() {
        // First observation: baseline, no entry.
        assertEquals(Observation<Boolean>(null, false), ActivityTriggers.debugging(null, false, null, at1942))
        assertEquals(Observation<Boolean>(null, true), ActivityTriggers.debugging(null, true, null, at1942))
        // No change: nothing, remembered state untouched.
        assertEquals(Observation<Boolean>(null, null), ActivityTriggers.debugging(false, false, null, at1942))
        assertEquals(Observation<Boolean>(null, null), ActivityTriggers.debugging(true, true, 99L, at1942))
        // Edges.
        val on = ActivityTriggers.debugging(false, true, at1942 + 3600, at1942)
        assertEquals(ActivityEntry(at1942, ActivityKind.DEBUGGING_ON, until = at1942 + 3600), on.entry)
        assertEquals(true, on.state)
        val onNoEnd = ActivityTriggers.debugging(false, true, null, at1942)
        assertNull(onNoEnd.entry!!.until)
        assertNull(ActivityTriggers.debugging(false, true, 0L, at1942).entry!!.until)
        val off = ActivityTriggers.debugging(true, false, null, at1942)
        assertEquals(ActivityEntry(at1942, ActivityKind.DEBUGGING_OFF), off.entry)
        assertEquals(false, off.state)
    }

    @Test
    fun `a debugging read-back of null is not an observation`() {
        assertEquals(Observation<Boolean>(null, null), ActivityTriggers.debugging(null, null, null, at1942))
        assertEquals(Observation<Boolean>(null, null), ActivityTriggers.debugging(true, null, null, at1942))
        assertEquals(Observation<Boolean>(null, null), ActivityTriggers.debugging(false, null, null, at1942))
    }

    private fun pairing(paired: Boolean, guardian: String?, error: String? = null, unreadable: Boolean = false) =
        CharterCore.PairingState(paired, guardian, null, error = error, pairingUnreadable = unreadable)

    @Test
    fun `an already paired phone does not log Paired, and an unpaired one logs nothing at first`() {
        assertEquals(
            Observation(null, PairingMemory(true, "abcd1234")),
            ActivityTriggers.pairing(null, pairing(true, "abcd1234"), at1942),
        )
        assertEquals(
            Observation(null, PairingMemory(false, null)),
            ActivityTriggers.pairing(null, pairing(false, null), at1942),
        )
    }

    @Test
    fun `pairing logs paired, unpaired and a changed guardian`() {
        val paired = ActivityTriggers.pairing(PairingMemory(false, null), pairing(true, "abcd1234"), at1942)
        assertEquals(ActivityEntry(at1942, ActivityKind.PAIRED, guardian = "abcd1234"), paired.entry)
        assertEquals(PairingMemory(true, "abcd1234"), paired.state)

        val released = ActivityTriggers.pairing(PairingMemory(true, "abcd1234"), pairing(false, null), at1942)
        assertEquals(ActivityEntry(at1942, ActivityKind.UNPAIRED, guardian = "abcd1234"), released.entry)
        assertEquals(PairingMemory(false, null), released.state)

        val changed = ActivityTriggers.pairing(PairingMemory(true, "abcd1234"), pairing(true, "ef567890"), at1942)
        assertEquals(ActivityEntry(at1942, ActivityKind.GUARDIAN_CHANGED, guardian = "ef567890", was = "abcd1234"), changed.entry)
        assertEquals(PairingMemory(true, "ef567890"), changed.state)

        // Nothing changed.
        assertEquals(
            Observation<PairingMemory>(null, null),
            ActivityTriggers.pairing(PairingMemory(true, "abcd1234"), pairing(true, "abcd1234"), at1942),
        )
        assertEquals(
            Observation<PairingMemory>(null, null),
            ActivityTriggers.pairing(PairingMemory(false, null), pairing(false, null), at1942),
        )
    }

    @Test
    fun `an unreadable or errored pairing is not an observation and moves no baseline`() {
        val last = PairingMemory(true, "abcd1234")
        val none = Observation<PairingMemory>(null, null)
        assertEquals(none, ActivityTriggers.pairing(last, null, at1942))
        assertEquals(none, ActivityTriggers.pairing(last, pairing(false, null, error = "not initialized"), at1942))
        assertEquals(none, ActivityTriggers.pairing(last, pairing(true, null, unreadable = true), at1942))
        assertEquals(none, ActivityTriggers.pairing(last, pairing(false, null, unreadable = true), at1942))
        assertEquals(none, ActivityTriggers.pairing(null, pairing(true, "abcd1234", error = "x"), at1942))
        assertEquals(none, ActivityTriggers.pairing(null, null, at1942))
        // Paired with no key to name is not an observation either.
        assertEquals(none, ActivityTriggers.pairing(last, pairing(true, null), at1942))
    }

    @Test
    fun `an upgrade records a silent baseline, a later change logs updated`() {
        assertEquals(
            Observation(null, VersionMemory("0.8.1", 57)),
            ActivityTriggers.version(null, "0.8.1", 57, at1942),
        )
        assertEquals(Observation<VersionMemory>(null, null), ActivityTriggers.version(VersionMemory("0.8.1", 57), "0.8.1", 57, at1942))
        val up = ActivityTriggers.version(VersionMemory("0.8.1", 57), "0.8.2", 58, at1942)
        assertEquals(ActivityEntry(at1942, ActivityKind.UPDATED, from = "0.8.1", to = "0.8.2"), up.entry)
        assertEquals(VersionMemory("0.8.2", 58), up.state)
        // A new build number under the same name is still an update.
        assertNotNull(ActivityTriggers.version(VersionMemory("0.8.1", 57), "0.8.1", 58, at1942).entry)
        // No name to read: not an observation.
        assertEquals(Observation<VersionMemory>(null, null), ActivityTriggers.version(VersionMemory("0.8.1", 57), null, 58, at1942))
    }

    // ---- the watcher -------------------------------------------------------------

    private class FakeLog(var ok: Boolean = true, val throwing: Boolean = false) : ActivityLog {
        val written = mutableListOf<ActivityEntry>()
        override fun entries(): List<ActivityEntry> = written
        override fun append(entry: ActivityEntry): Boolean {
            if (throwing) error("disk on fire")
            if (ok) written += entry
            return ok
        }
    }

    private class FakeMemory : ActivityMemory {
        var dbg: Boolean? = null
        var pair: PairingMemory? = null
        var ver: VersionMemory? = null
        override fun debugging() = dbg
        override fun setDebugging(open: Boolean): Boolean { dbg = open; return true }
        override fun pairing() = pair
        override fun setPairing(p: PairingMemory): Boolean { pair = p; return true }
        override fun version() = ver
        override fun setVersion(v: VersionMemory): Boolean { ver = v; return true }
    }

    @Test
    fun `the watcher writes the entry, then remembers`() {
        val log = FakeLog()
        val mem = FakeMemory()
        val w = ActivityWatcher(log, mem) {}
        w.observeDebugging(false, null, at1942) // baseline
        assertTrue(log.written.isEmpty())
        assertEquals(false, mem.dbg)
        w.observeDebugging(true, at1942 + 60, at1942 + 1)
        assertEquals(listOf(ActivityEntry(at1942 + 1, ActivityKind.DEBUGGING_ON, until = at1942 + 60)), log.written)
        assertEquals(true, mem.dbg)
        w.observeDebugging(true, at1942 + 60, at1942 + 2) // no new edge
        assertEquals(1, log.written.size)
    }

    @Test
    fun `a failed write is tried again on the next observation, not lost`() {
        val log = FakeLog(ok = false)
        val mem = FakeMemory().apply { dbg = false }
        val lines = mutableListOf<String>()
        val w = ActivityWatcher(log, mem) { lines += it }
        w.observeDebugging(true, null, at1942)
        assertTrue(log.written.isEmpty())
        assertEquals(false, mem.dbg) // baseline not advanced
        assertTrue(lines.isNotEmpty())
        log.ok = true
        w.observeDebugging(true, null, at1942 + 2)
        assertEquals(1, log.written.size)
        assertEquals(true, mem.dbg)
    }

    @Test
    fun `a log that throws never reaches the tick`() {
        val w = ActivityWatcher(FakeLog(throwing = true), FakeMemory().apply { dbg = false; ver = VersionMemory("0.8.0", 56) }) {}
        w.observeDebugging(true, null, at1942)
        w.observeVersion("0.8.1", 57, at1942)
        w.observePairing(pairing(true, "abcd1234"), at1942)
    }

    @Test
    fun `an upgrade from 0_8_0 logs nothing, and the watcher logs pairing and version edges`() {
        val log = FakeLog()
        val mem = FakeMemory()
        val w = ActivityWatcher(log, mem) {}
        w.observeVersion("0.8.1", 57, at1942)
        w.observePairing(pairing(true, "abcd1234"), at1942)
        w.observeDebugging(true, null, at1942)
        assertTrue(log.written.isEmpty())
        w.observePairing(pairing(true, "ef567890"), at1942 + 5)
        w.observePairing(pairing(false, null), at1942 + 6)
        w.observeVersion("0.8.2", 58, at1942 + 7)
        assertEquals(
            listOf(ActivityKind.GUARDIAN_CHANGED, ActivityKind.UNPAIRED, ActivityKind.UPDATED),
            log.written.map { it.kind },
        )
    }

    // ---- the emergency control ---------------------------------------------------

    @Test
    fun `an emergency start is logged after it, and a failing log changes nothing`() {
        val order = mutableListOf<String>()
        val placed = EmergencyUse.placeAndRecord(
            start = { order += "start"; true },
            record = { order += "record" },
            runner = { it.run() },
            onFailure = { order += "failure" },
        )
        assertTrue(placed)
        assertEquals(listOf("start", "record"), order)

        // The write throws: the call still reports success.
        val failures = mutableListOf<Throwable>()
        assertTrue(
            EmergencyUse.placeAndRecord(
                start = { true },
                record = { error("disk full") },
                runner = { it.run() },
                onFailure = { failures += it },
            ),
        )
        assertEquals(1, failures.size)

        // The runner itself throws (no thread to hand it to): still success.
        assertTrue(
            EmergencyUse.placeAndRecord(
                start = { true },
                record = {},
                runner = { throw IllegalStateException("rejected") },
                onFailure = { failures += it },
            ),
        )
        assertEquals(2, failures.size)

        // Even an onFailure that throws cannot change the result.
        assertTrue(
            EmergencyUse.placeAndRecord(
                start = { true },
                record = { error("x") },
                runner = { it.run() },
                onFailure = { error("y") },
            ),
        )
    }

    @Test
    fun `a call that did not start, or was not an emergency, is not logged`() {
        var recorded = 0
        assertFalse(
            EmergencyUse.placeAndRecord(
                start = { error("no dialer") },
                record = { recorded++ },
                runner = { it.run() },
            ),
        )
        assertTrue(
            EmergencyUse.placeAndRecord(
                start = { false }, // the lifeline: a guardian's number
                record = { recorded++ },
                runner = { it.run() },
            ),
        )
        assertEquals(0, recorded)
    }

    @Test
    fun `the record is handed to the runner, not run on the calling path`() {
        var queued: Runnable? = null
        var recorded = false
        assertTrue(
            EmergencyUse.placeAndRecord(
                start = { true },
                record = { recorded = true },
                runner = { queued = it },
            ),
        )
        assertFalse(recorded)
        queued!!.run()
        assertTrue(recorded)
    }
}
