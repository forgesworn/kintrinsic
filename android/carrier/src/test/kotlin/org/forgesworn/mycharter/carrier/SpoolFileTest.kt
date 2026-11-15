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

    @Test fun absentOrCorruptReadsAsEmptyAndRecovers() {
        val f = File(tmp.root, "spool.json")
        assertEquals("[]", SpoolFile(f).read())
        f.writeText("not json{{")
        assertEquals(emptyList<String>(), ids(InboxSpool.merge("[]", SpoolFile(f).read())))
        SpoolFile(f).add("w1", "m", wrap("w1"), now, null)
        assertEquals(listOf("w1"), ids(SpoolFile(f).read()))
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

    @Test fun theServiceAndTheBridgeNeverDropEachOthersUpdate() {
        // Two instances over the same file, as the service and the bridge hold.
        val f = File(tmp.root, "spool.json")
        val service = SpoolFile(f)
        val bridge = SpoolFile(f)
        service.add("keep", "m", wrap("keep"), now, null)
        val go = CountDownLatch(1)
        val adder = thread {
            go.await()
            for (i in 1..40) service.add("new$i", "m", wrap("new$i"), now, null)
        }
        val consumer = thread {
            go.await()
            for (i in 1..40) bridge.remove("""["gone$i"]""")
        }
        go.countDown()
        adder.join()
        consumer.join()
        val held = ids(bridge.read())
        assertEquals(41, held.size)
        assertTrue("keep" in held && "new40" in held)
    }

    @Test fun theKnownRosterIsAppliedWhenAnEntryIsAdded() {
        val s = SpoolFile(File(tmp.root, "spool.json"))
        s.add("real", "mReal", wrap("real"), now, setOf("mReal"))
        for (i in 1..50) s.add("x$i", "stranger$i", wrap("x$i"), now + i, setOf("mReal"))
        val held = ids(s.read())
        assertTrue("real" in held)
        assertEquals(1 + InboxSpool.UNKNOWN_CAP, held.size)
    }
}
