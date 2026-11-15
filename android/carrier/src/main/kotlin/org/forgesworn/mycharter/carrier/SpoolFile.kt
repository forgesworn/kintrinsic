package org.forgesworn.mycharter.carrier

import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

/**
 * [InboxSpool]'s file: its own file in the carrier's private storage (never the
 * preferences file that holds the guardian key), written atomically and
 * synchronously. Every operation takes ONE process-wide lock, so the foreground
 * service adding an arrival and the console bridge consuming one cannot
 * overwrite each other's update (each owns its own [CarrierStore]).
 *
 * The file holds the held entries and the tombstones of consumed ids (see
 * [InboxSpool.encode]); a bare array from an older release still reads.
 *
 * Writes go to a temp file, are fsynced, renamed over the spool, and then the
 * directory is fsynced, so a crash or power cut leaves the old file or the new
 * one, never half of one, and a rename that was reported is not lost.
 * A file that cannot be read is NEVER treated as an empty spool and written
 * over: a read that fails is retried once, and if the file is still unreadable
 * it is moved aside (see [loadLocked]) so held presses are kept for a human.
 * Every entry that still parses is salvaged into the fresh spool. Callers are
 * on worker or bridge threads, never the UI thread.
 *
 * [lock], [betweenReadAndWrite], [readText] and [syncDirectory] are test seams: the
 * lock is the shared one in production, the hook runs inside an add or remove
 * after the read and before the write (exactly where a race would bite), and the
 * last two stand in for the file read and the directory fsync.
 */
class SpoolFile(
    private val file: File,
    private val lock: Any = LOCK,
    private val clock: () -> Long = { System.currentTimeMillis() / 1000 },
    private val onDamaged: (File) -> Unit = {},
    private val betweenReadAndWrite: () -> Unit = {},
    private val readText: (File) -> String = { it.readText(Charsets.UTF_8) },
    private val syncDirectory: (File) -> Unit = { fsyncDirectory(it) },
) {
    /**
     * The held entries as JSON; `[]` when absent. A damaged file is set aside and
     * its readable entries salvaged. Throws only if a damaged file could not be set aside.
     */
    fun read(): String = synchronized(lock) { loadLocked().entries }

    /** Hold a wrap (see [InboxSpool.add]). Throws on a write failure, or if a damaged file could not be set aside. */
    fun add(id: String, machine: String, wrapJson: String, nowSecs: Long, known: Set<String>?) =
        synchronized(lock) {
            val before = loadLocked()
            betweenReadAndWrite()
            val consumed = InboxSpool.pruneTombstones(before.consumed, nowSecs)
            val entries = InboxSpool.add(before.entries, id, machine, wrapJson, nowSecs, known, consumed.keys)
            if (entries != before.entries || consumed != before.consumed) write(InboxSpool.Doc(entries, consumed))
        }

    /** Forget these ids (a JSON array of strings) and remember them as consumed. */
    fun remove(idsJson: String) = synchronized(lock) {
        val before = loadLocked()
        betweenReadAndWrite()
        val entries = InboxSpool.remove(before.entries, idsJson)
        val consumed = InboxSpool.tombstone(before.consumed, idsJson, clock())
        if (entries != before.entries || consumed != before.consumed) write(InboxSpool.Doc(entries, consumed))
    }

    /**
     * Absent is an empty spool. Present and readable is its content. A read that
     * fails, or text that is not a spool, is tried once more. Still unreadable,
     * the file is moved aside to `<name>.damaged-<unix>` (the 3 newest are kept)
     * untouched, reported, and a fresh spool starts holding every entry that
     * could be salvaged from it. If it cannot be moved aside this THROWS, so
     * nothing is ever written over it.
     */
    private fun loadLocked(): InboxSpool.Doc {
        if (!file.exists()) return InboxSpool.Doc("[]", emptyMap())
        var text: String? = null
        repeat(2) {
            if (text == null) {
                val t = try { readText(file) } catch (_: Exception) { null }
                if (t != null && InboxSpool.decode(t) != null) text = t
            }
        }
        text?.let { return InboxSpool.decode(it)!! }
        // Whatever can be read from the damaged bytes (nothing, if they cannot be read at all).
        val salvaged = try { InboxSpool.salvage(readText(file)) } catch (_: Exception) { "[]" }
        val aside = moveAside()
        onDamaged(aside)
        val doc = InboxSpool.Doc(salvaged, emptyMap())
        // A fresh spool with the salvage in it. If this write fails the entries are
        // still returned, and the next successful write carries them; the set-aside
        // copy keeps everything either way.
        if (salvaged != "[]") runCatching { write(doc) }
        return doc
    }

    private fun moveAside(): File {
        val dir = file.absoluteFile.parentFile
            ?: throw IOException("no directory for ${file.name}")
        val prefix = "${file.name}$DAMAGED"
        var stamp = clock()
        var target = File(dir, "$prefix$stamp")
        while (target.exists()) target = File(dir, "$prefix${++stamp}")
        if (!file.renameTo(target)) throw IOException("could not set aside a damaged ${file.name}")
        // Keep only the newest copies: the damaged files are evidence, not a log.
        dir.listFiles { f -> f.name.startsWith(prefix) }
            ?.sortedByDescending { it.name.removePrefix(prefix).toLongOrNull() ?: 0L }
            ?.drop(KEEP_DAMAGED)
            ?.forEach { it.delete() }
        return target
    }

    private fun write(doc: InboxSpool.Doc) {
        val dir = file.absoluteFile.parentFile
        dir?.mkdirs()
        val tmp = File(file.path + ".tmp")
        FileOutputStream(tmp).use { out ->
            out.write(InboxSpool.encode(doc).toByteArray(Charsets.UTF_8))
            out.flush()
            out.fd.sync()
        }
        if (!tmp.renameTo(file)) {
            tmp.delete()
            throw IOException("could not replace ${file.name}")
        }
        // The rename is only durable once its directory entry is.
        if (dir != null) syncDirectory(dir)
    }

    companion object {
        /** One lock for the process, whichever [CarrierStore] instance asks. */
        internal val LOCK = Any()

        /** The suffix before the unix time on a file that was set aside. */
        const val DAMAGED = ".damaged-"

        /** How many set-aside copies are kept. */
        const val KEEP_DAMAGED = 3

        /**
         * fsync a directory so a rename inside it survives a power cut. A
         * filesystem that cannot fsync a directory (EINVAL) has nothing to sync
         * and is ignored; any other failure is an [IOException].
         */
        fun fsyncDirectory(dir: File) {
            try {
                val fd = Os.open(dir.path, OsConstants.O_RDONLY, 0)
                try {
                    Os.fsync(fd)
                } finally {
                    Os.close(fd)
                }
            } catch (e: ErrnoException) {
                if (e.errno != OsConstants.EINVAL) throw IOException("could not sync ${dir.name}: ${e.message}", e)
            }
        }
    }
}
