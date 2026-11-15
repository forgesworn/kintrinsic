package org.forgesworn.mycharter.carrier

import java.io.File
import java.io.FileOutputStream

/**
 * [InboxSpool]'s file: its own file in the carrier's private storage (never the
 * preferences file that holds the guardian key), written atomically and
 * synchronously. Every operation takes ONE process-wide lock, so the foreground
 * service adding an arrival and the console bridge consuming one cannot
 * overwrite each other's update (each owns its own [CarrierStore]).
 *
 * Writes go to a temp file, are fsynced, then renamed over the spool, so a
 * crash or power cut leaves the old file or the new one, never half of one.
 * Callers are on worker or bridge threads, never the UI thread.
 */
class SpoolFile(private val file: File) {
    /** The spooled entries as JSON; `[]` when absent or unreadable. */
    fun read(): String = synchronized(LOCK) { readLocked() }

    /** Hold a wrap (see [InboxSpool.add]). Throws on a write failure. */
    fun add(id: String, machine: String, wrapJson: String, nowSecs: Long, known: Set<String>?) =
        synchronized(LOCK) {
            val before = readLocked()
            val after = InboxSpool.add(before, id, machine, wrapJson, nowSecs, known)
            if (after != before) write(after)
        }

    /** Forget these ids (a JSON array of strings). */
    fun remove(idsJson: String) = synchronized(LOCK) {
        val before = readLocked()
        val after = InboxSpool.remove(before, idsJson)
        if (after != before) write(after)
    }

    private fun readLocked(): String =
        try { if (file.isFile) file.readText(Charsets.UTF_8) else "[]" } catch (_: Exception) { "[]" }

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
            throw java.io.IOException("could not replace ${file.name}")
        }
    }

    private companion object {
        /** One lock for the process, whichever [CarrierStore] instance asks. */
        val LOCK = Any()
    }
}
