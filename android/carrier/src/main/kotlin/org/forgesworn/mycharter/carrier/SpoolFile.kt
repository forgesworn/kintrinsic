package org.forgesworn.mycharter.carrier

import org.json.JSONArray
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
 * Writes go to a temp file, are fsynced, then renamed over the spool, so a
 * crash or power cut leaves the old file or the new one, never half of one.
 * A file that cannot be read is NEVER treated as an empty spool and written
 * over: it is moved aside (see [readLocked]) so held presses are kept for a
 * human, and only then is a fresh spool started. Callers are on worker or
 * bridge threads, never the UI thread.
 *
 * [lock] and [betweenReadAndWrite] are test seams: the lock is the shared one in
 * production, and the hook runs inside an add or remove after the read and
 * before the write, which is exactly where a race would bite.
 */
class SpoolFile(
    private val file: File,
    private val lock: Any = LOCK,
    private val clock: () -> Long = { System.currentTimeMillis() / 1000 },
    private val onDamaged: (File) -> Unit = {},
    private val betweenReadAndWrite: () -> Unit = {},
) {
    /** The spooled entries as JSON; `[]` when absent. Never throws for a damaged file (it is set aside). */
    fun read(): String = synchronized(lock) { readLocked() }

    /** Hold a wrap (see [InboxSpool.add]). Throws on a write failure, or if a damaged file could not be set aside. */
    fun add(id: String, machine: String, wrapJson: String, nowSecs: Long, known: Set<String>?) =
        synchronized(lock) {
            val before = readLocked()
            betweenReadAndWrite()
            val after = InboxSpool.add(before, id, machine, wrapJson, nowSecs, known)
            if (after != before) write(after)
        }

    /** Forget these ids (a JSON array of strings). */
    fun remove(idsJson: String) = synchronized(lock) {
        val before = readLocked()
        betweenReadAndWrite()
        val after = InboxSpool.remove(before, idsJson)
        if (after != before) write(after)
    }

    /**
     * Absent is an empty spool. Present and readable is its content. Present but
     * unreadable (an IO error, or not a JSON array) is moved aside to
     * `<name>.damaged-<unix>` (the 3 newest are kept), reported, and then a fresh
     * spool starts. If it cannot be moved aside this THROWS, so nothing is ever
     * written over it.
     */
    private fun readLocked(): String {
        if (!file.exists()) return "[]"
        val text = try { file.readText(Charsets.UTF_8) } catch (_: Exception) { null }
        if (text != null && isJsonArray(text)) return text
        val aside = moveAside()
        onDamaged(aside)
        return "[]"
    }

    private fun isJsonArray(text: String): Boolean =
        try { JSONArray(text); true } catch (_: Exception) { false }

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

    private fun write(json: String) {
        file.parentFile?.mkdirs()
        val tmp = File(file.path + ".tmp")
        FileOutputStream(tmp).use { out ->
            out.write(json.toByteArray(Charsets.UTF_8))
            out.flush()
            out.fd.sync()
        }
        if (!tmp.renameTo(file)) {
            tmp.delete()
            throw IOException("could not replace ${file.name}")
        }
    }

    companion object {
        /** One lock for the process, whichever [CarrierStore] instance asks. */
        internal val LOCK = Any()

        /** The suffix before the unix time on a file that was set aside. */
        const val DAMAGED = ".damaged-"

        /** How many set-aside copies are kept. */
        const val KEEP_DAMAGED = 3
    }
}
