package org.forgesworn.mycharter.carrier

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.rules.TemporaryFolder
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread

/** The spool's own file: atomic, synchronous, and safe across instances. */
class SpoolFileTest {
    @get:Rule val tmp = TemporaryFolder()
    private val now = 1_800_000_000L
    private fun wrap(id: String) = """{"id":"$id","kind":1059}"""
    private fun ids(json: String): List<String> =
        JSONArray(json).let { a -> (0 until a.length()).map { a.getJSONObject(it).getString("id") } }

    @Test fun anAddIsOnDiskWhenItReturnsAndLeavesNoTempFile() {
        val f = File(tmp.root, "inbox-spool.json")
        SpoolFile(f).add("w1", "m", wrap("w1"), now, null)
        assertTrue(f.isFile)
        assertFalse(File(f.path + ".tmp").exists())
        // A fresh instance (a restarted process) reads it back.
        assertEquals(listOf("w1"), ids(SpoolFile(f).read()))
    }

    @Test fun absentReadsAsEmpty() {
        assertEquals("[]", SpoolFile(File(tmp.root, "spool.json")).read())
    }

    @Test fun aLeftoverTempFileFromACrashIsIgnoredAndReplaced() {
        val f = File(tmp.root, "spool.json")
        File(f.path + ".tmp").writeText("half writ")
        val s = SpoolFile(f)
        s.add("w1", "m", wrap("w1"), now, null)
        assertEquals(listOf("w1"), ids(s.read()))
        assertFalse(File(f.path + ".tmp").exists())
    }

    @Test fun removeForgetsOnlyThoseIds() {
        val s = SpoolFile(File(tmp.root, "spool.json"))
        s.add("w1", "m", wrap("w1"), now, null)
        s.add("w2", "m", wrap("w2"), now, null)
        s.remove("""["w1"]""")
        assertEquals(listOf("w2"), ids(s.read()))
    }

    // ---- a damaged spool is never written over ------------------------------

    @Test fun aDamagedFileIsSetAsideNotOverwritten() {
        val f = File(tmp.root, "inbox-spool.json")
        f.writeText("""[{"id":"precious""")
        val reported = mutableListOf<String>()
        val s = SpoolFile(f, clock = { 1_800_000_000L }, onDamaged = { reported += it.name })
        s.add("w1", "m", wrap("w1"), now, null)
        // The damaged bytes survive, byte for byte, beside the fresh spool.
        val aside = File(tmp.root, "inbox-spool.json.damaged-1800000000")
        assertTrue(aside.isFile)
        assertEquals("""[{"id":"precious""", aside.readText())
        assertEquals(listOf(aside.name), reported)
        assertEquals(listOf("w1"), ids(s.read()))
    }

    @Test fun notJsonAtAllAndNotAnArrayAreBothDamage() {
        for ((i, junk) in listOf("not json{{", "{\"a\":1}", "").withIndex()) {
            val dir = tmp.newFolder("case$i")
            val f = File(dir, "inbox-spool.json")
            f.writeText(junk)
            assertEquals("[]", SpoolFile(f, clock = { 5L }).read())
            assertEquals(junk, File(dir, "inbox-spool.json.damaged-5").readText())
            assertFalse(f.exists())
        }
    }

    @Test fun aDamagedFileIsNeverOverwrittenEvenWhenTheAsideNameIsTaken() {
        // The spool path is a directory (unreadable as a file) and the first aside
        // name is already taken by another directory: the next free name is used
        // and neither the damaged spool nor the earlier copy is lost.
        val dir = tmp.newFolder("blocked")
        val f = File(dir, "inbox-spool.json")
        f.mkdir()
        File(f, "child").writeText("x")
        val blocker = File(dir, "inbox-spool.json.damaged-7")
        blocker.mkdir()
        File(blocker, "child").writeText("y")
        SpoolFile(f, clock = { 7L }).read()
        assertTrue(File(dir, "inbox-spool.json.damaged-8/child").isFile)
        assertTrue(File(blocker, "child").isFile)
    }

    @Test fun onlyTheThreeNewestDamagedCopiesAreKept() {
        val f = File(tmp.root, "inbox-spool.json")
        for (t in 100L..105L) {
            f.writeText("bad$t{")
            SpoolFile(f, clock = { t }).read()
        }
        val kept = tmp.root.listFiles()!!.map { it.name }.filter { it.contains(".damaged-") }.sorted()
        assertEquals(
            listOf("inbox-spool.json.damaged-103", "inbox-spool.json.damaged-104", "inbox-spool.json.damaged-105"),
            kept,
        )
        assertEquals("bad105{", File(tmp.root, "inbox-spool.json.damaged-105").readText())
    }

    @Test fun twoDamageEventsInTheSameSecondDoNotClobberEachOther() {
        val f = File(tmp.root, "inbox-spool.json")
        f.writeText("first{")
        SpoolFile(f, clock = { 9L }).read()
        f.writeText("second{")
        SpoolFile(f, clock = { 9L }).read()
        assertEquals("first{", File(tmp.root, "inbox-spool.json.damaged-9").readText())
        assertEquals("second{", File(tmp.root, "inbox-spool.json.damaged-10").readText())
    }

    // ---- the race, made deterministic ------------------------------------------

    /**
     * The service adds B while the bridge consumes A, over the same file. The
     * add's hook pauses it AFTER it has read the spool and BEFORE it writes: the
     * exact window in which, without a shared lock, the consume reads the same
     * stale spool, writes it without A, and then the add writes [A, B] over it,
     * bringing A back. Returns the final ids.
     */
    private fun raceAddAgainstConsume(addLock: Any, consumeLock: Any): List<String> {
        val f = File(tmp.newFolder(), "spool.json")
        SpoolFile(f).add("A", "m", wrap("A"), now, null)
        val addPaused = CountDownLatch(1)
        val consumeDone = CountDownLatch(1)
        val service = SpoolFile(f, lock = addLock, betweenReadAndWrite = {
            addPaused.countDown()
            // With a shared lock the consume is blocked and this simply times
            // out; without one the consume finishes inside this wait.
            consumeDone.await(500, TimeUnit.MILLISECONDS)
        })
        val bridge = SpoolFile(f, lock = consumeLock)
        val adder = thread { service.add("B", "m", wrap("B"), now, null) }
        assertTrue(addPaused.await(10, TimeUnit.SECONDS))
        val consumer = thread {
            bridge.remove("""["A"]""")
            consumeDone.countDown()
        }
        adder.join(10_000)
        consumer.join(10_000)
        return ids(SpoolFile(f).read())
    }

    @Test fun withTheSharedLockAConsumeAndAnAddNeverLoseEachOthersUpdate() {
        val shared = Any()
        assertEquals(listOf("B"), raceAddAgainstConsume(shared, shared))
    }

    @Test fun theProductionDefaultIsOneLockForEveryInstance() {
        // Both instances take the companion's lock when none is given.
        assertEquals(listOf("B"), raceAddAgainstConsume(SpoolFile.LOCK, SpoolFile.LOCK))
    }

    @Test fun theRaceTestHasTeethWithoutTheSharedLock() {
        // Two different locks (what two CarrierStore instances had before): the
        // consume's removal of A is overwritten and A comes back. If this ever
        // returned just [B], the tests above would prove nothing.
        assertEquals(listOf("A", "B"), raceAddAgainstConsume(Any(), Any()))
    }

    @Test fun theKnownRosterIsAppliedWhenAnEntryIsAdded() {
        val s = SpoolFile(File(tmp.root, "spool.json"))
        s.add("real", "mReal", wrap("real"), now, setOf("mReal"))
        for (i in 1..50) s.add("x$i", "stranger$i", wrap("x$i"), now + i, setOf("mReal"))
        val held = ids(s.read())
        assertTrue("real" in held)
        assertEquals(1 + InboxSpool.UNKNOWN_CAP, held.size)
    }

    // ---- C7: consumed ids are not re-admitted -----------------------------------

    @Test fun aConsumedWrapIsNotReadmittedByARelayReplay() {
        val s = SpoolFile(File(tmp.root, "spool.json"), clock = { now })
        s.add("w1", "m", wrap("w1"), now, null)
        s.remove("""["w1"]""")
        s.add("w1", "m", wrap("w1"), now + 60, null) // the relay replays it on reconnect
        assertEquals(emptyList<String>(), ids(s.read()))
        s.add("w2", "m", wrap("w2"), now + 61, null)
        assertEquals(listOf("w2"), ids(s.read()))
    }

    @Test fun theTombstonesSurviveARestartAndAreForgottenAfterFortyEightHours() {
        val f = File(tmp.root, "spool.json")
        SpoolFile(f, clock = { now }).apply {
            add("w1", "m", wrap("w1"), now, null)
            remove("""["w1"]""")
        }
        val restarted = SpoolFile(f, clock = { now })
        restarted.add("w1", "m", wrap("w1"), now + InboxSpool.TOMBSTONE_SECS, null)
        assertEquals(emptyList<String>(), ids(restarted.read()))
        restarted.add("w1", "m", wrap("w1"), now + InboxSpool.TOMBSTONE_SECS + 1, null)
        assertEquals(listOf("w1"), ids(restarted.read()))
    }

    @Test fun anOlderBareArraySpoolStillReadsAndIsRewrittenAsADocument() {
        val f = File(tmp.root, "spool.json")
        f.writeText(InboxSpool.add(null, "old", "m", wrap("old"), now))
        val s = SpoolFile(f, clock = { now })
        assertEquals(listOf("old"), ids(s.read()))
        s.add("w1", "m", wrap("w1"), now, null)
        assertEquals(listOf("old", "w1"), ids(s.read()))
        assertTrue(f.readText().startsWith("{"))
    }

    // ---- C8: the directory is synced after the rename ---------------------------

    @Test fun theDirectoryIsSyncedAfterTheRenameOnEveryWrite() {
        val f = File(tmp.root, "spool.json")
        val events = mutableListOf<String>()
        val s = SpoolFile(f, syncDirectory = { dir ->
            // By now the new file is already in place under its final name.
            events += "sync:${dir.name}:${f.isFile}:${File(f.path + ".tmp").exists()}"
        })
        s.add("w1", "m", wrap("w1"), now, null)
        s.remove("""["w1"]""")
        assertEquals(List(2) { "sync:${tmp.root.name}:true:false" }, events)
    }

    @Test fun aFailedDirectorySyncIsReportedToTheCaller() {
        val s = SpoolFile(File(tmp.root, "spool.json"), syncDirectory = { throw java.io.IOException("EIO") })
        val failure = runCatching { s.add("w1", "m", wrap("w1"), now, null) }.exceptionOrNull()
        assertTrue(failure is java.io.IOException)
    }

    // ---- C9: retry once, then salvage -------------------------------------------

    @Test fun aReadThatFailsOnceIsRetriedAndIsNotDamage() {
        val f = File(tmp.root, "spool.json")
        SpoolFile(f).add("w1", "m", wrap("w1"), now, null)
        var calls = 0
        val reported = mutableListOf<File>()
        val s = SpoolFile(f, onDamaged = { reported += it }, readText = {
            if (calls++ == 0) throw java.io.IOException("transient") else it.readText()
        })
        assertEquals(listOf("w1"), ids(s.read()))
        assertEquals(2, calls)
        assertTrue(reported.isEmpty())
        assertTrue(f.isFile)
    }

    @Test fun aReadThatKeepsFailingIsDamageAfterExactlyOneRetry() {
        val f = File(tmp.root, "spool.json")
        SpoolFile(f).add("w1", "m", wrap("w1"), now, null)
        var calls = 0
        val reported = mutableListOf<File>()
        val s = SpoolFile(f, clock = { 3L }, onDamaged = { reported += it }, readText = {
            calls++
            throw java.io.IOException("gone")
        })
        // Two reads to decide, one more to try to salvage; nothing readable, so empty.
        assertEquals("[]", s.read())
        assertEquals(3, calls)
        assertEquals(1, reported.size)
        assertTrue(File(tmp.root, "spool.json.damaged-3").isFile)
    }

    @Test fun aTruncatedFileKeepsItsBytesAsideAndItsReadableEntriesInTheFreshSpool() {
        val f = File(tmp.root, "spool.json")
        var arr: String? = null
        for (i in 1..3) arr = InboxSpool.add(arr, "w$i", "m", wrap("w$i"), now + i)
        val torn = arr!!.substring(0, arr.length - 20)
        f.writeText(torn)
        val reported = mutableListOf<String>()
        val s = SpoolFile(f, clock = { 11L }, onDamaged = { reported += it.name })
        assertEquals(listOf("w1", "w2"), ids(s.read()))
        // The damaged file is exactly as it was found.
        assertEquals(torn, File(tmp.root, "spool.json.damaged-11").readText())
        assertEquals(listOf("spool.json.damaged-11"), reported)
        // The salvage is in the fresh file on disk, so a restart sees it too.
        assertEquals(listOf("w1", "w2"), ids(SpoolFile(f).read()))
        // And an add lands beside it.
        s.add("w4", "m", wrap("w4"), now + 9, null)
        assertEquals(listOf("w1", "w2", "w4"), ids(s.read()))
    }

    @Test fun aDamagedFileWithNothingReadableLeavesNoFreshFileUntilTheNextAdd() {
        val f = File(tmp.root, "spool.json")
        f.writeText("\u0000\u0000garbage")
        assertEquals("[]", SpoolFile(f, clock = { 2L }).read())
        assertFalse(f.exists())
    }
}
