package org.forgesworn.mycharter.carrier

import android.system.ErrnoException
import android.system.Os
import android.system.OsConstants
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import org.json.JSONArray

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
 * over: a read that fails is retried once and then throws, and it is never
 * taken for damage. Text that was read but is not a spool is kept aside for a
 * human and every entry that still parses is salvaged into the fresh spool (see
 * [loadLocked]). Callers are on worker or bridge threads, never the UI thread.
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
    fun add(
        id: String,
        machine: String,
        wrapJson: String,
        nowSecs: Long,
        known: Set<String>?,
        released: Set<String> = emptySet(),
    ) =
        synchronized(lock) {
            val before = loadLocked()
            betweenReadAndWrite()
            val consumed = InboxSpool.pruneTombstones(before.consumed, nowSecs)
            val entries = InboxSpool.add(before.entries, id, machine, wrapJson, nowSecs, known, consumed.keys, released)
            if (entries != before.entries || consumed != before.consumed) write(InboxSpool.Doc(entries, consumed))
        }

    /**
     * Forget these ids (a JSON array of strings) and remember as consumed those the
     * spool HELD. An id it never held (a LAN status, an ask, a duplicate) is not
     * tombstoned, so it cannot spend the tombstones a held audit's replay guard
     * needs; and when no held entry changes nothing is written (no fsync).
     */
    fun remove(idsJson: String) = synchronized(lock) {
        val before = loadLocked()
        betweenReadAndWrite()
        val held = InboxSpool.held(before.entries, idsJson)
        if (held.isEmpty()) return@synchronized
        val heldJson = JSONArray(held).toString()
        val entries = InboxSpool.remove(before.entries, heldJson)
        if (entries == before.entries) return@synchronized
        val consumed = InboxSpool.tombstone(before.consumed, heldJson, clock())
        write(InboxSpool.Doc(entries, consumed))
    }

    /**
     * Absent is an empty spool. Present and readable is its content. A read is
     * tried twice. If the LAST try fails to read (an I/O error), this THROWS: a
     * read error is never taken for damage, so a file whose bytes may be intact is
     * never set aside or written over, and the next call tries again. Only text
     * that is read but is not a spool is damage: the file is kept as
     * `<name>.damaged-<unix>` (the 3 newest are kept) and reported, and a fresh
     * spool starts holding every entry that could be salvaged from it.
     *
     * The damaged file is only replaced once the fresh spool has been written (the
     * rename is atomic): a set-aside COPY is made first, then the write. If the
     * copy or the write fails this THROWS with the damaged file still in place, so
     * the next call finds the damage again and retries, and nothing is lost. When
     * nothing can be salvaged there is nothing to write, and the file is moved aside.
     */
    private fun loadLocked(): InboxSpool.Doc {
        if (!file.exists()) return InboxSpool.Doc("[]", emptyMap())
        var bad: String? = null
        var failure: Exception? = null
        repeat(2) {
            val t = try { readText(file) } catch (e: Exception) { failure = e; return@repeat }
            failure = null
            InboxSpool.decode(t)?.let { return it }
            bad = t
        }
        failure?.let { throw it }
        val salvaged = InboxSpool.salvage(bad ?: "")
        if (salvaged == "[]") {
            onDamaged(moveAside())
            return InboxSpool.Doc("[]", emptyMap())
        }
        onDamaged(copyAside())
        val doc = InboxSpool.Doc(salvaged, emptyMap())
        write(doc)
        return doc
    }

    /** A name for a set-aside copy that does not exist yet. */
    private fun asideTarget(): File {
        val dir = file.absoluteFile.parentFile
            ?: throw IOException("no directory for ${file.name}")
        var stamp = clock()
        var target = File(dir, "${file.name}$DAMAGED$stamp")
        while (target.exists()) target = File(dir, "${file.name}$DAMAGED${++stamp}")
        return target
    }

    private fun moveAside(): File {
        val target = asideTarget()
        if (!file.renameTo(target)) throw IOException("could not set aside a damaged ${file.name}")
        pruneAside()
        return target
    }

    private fun copyAside(): File {
        val target = asideTarget()
        try {
            file.copyTo(target)
        } catch (e: Exception) {
            target.delete()
            throw IOException("could not set aside a copy of a damaged ${file.name}", e)
        }
        pruneAside()
        return target
    }

    /** Keep only the newest copies: the damaged files are evidence, not a log. */
    private fun pruneAside() {
        val dir = file.absoluteFile.parentFile ?: return
        val prefix = "${file.name}$DAMAGED"
        dir.listFiles { f -> f.name.startsWith(prefix) }
            ?.sortedByDescending { it.name.removePrefix(prefix).toLongOrNull() ?: 0L }
            ?.drop(KEEP_DAMAGED)
            ?.forEach { it.delete() }
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
