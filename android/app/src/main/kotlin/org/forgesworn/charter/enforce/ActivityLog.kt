package org.forgesworn.charter.enforce

import android.content.Context
import android.util.Log
import org.forgesworn.charter.native.CharterCore
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.RandomAccessFile
import java.nio.file.Files
import java.nio.file.StandardCopyOption
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import java.util.concurrent.Executors

/*
 * "Activity on this phone": a permanent, child-visible log of the things that
 * are out of the ordinary, beside the screen-lock history.
 *
 * Everything here is local. Nothing in this log is sent to the guardian, and
 * nothing a user typed (a PIN, a phone number) ever reaches it. The events are
 * few and fixed: the emergency control on the lock shade, USB debugging
 * switching on or off, the pairing changing, and the app being updated.
 *
 * The file is append-only with no delete and no command, in device-protected
 * storage OUTSIDE the warden's base dir, so a guardian's release (which wipes
 * the clauses and the pairing) never erases it.
 */

/** What happened. A small closed set; unknown kinds are skipped on read. */
object ActivityKind {
    const val EMERGENCY = "emergency"
    const val DEBUGGING_ON = "debugging_on"
    const val DEBUGGING_OFF = "debugging_off"
    const val PAIRED = "paired"
    const val UNPAIRED = "unpaired"
    const val GUARDIAN_CHANGED = "guardian_changed"
    const val UPDATED = "updated"

    val ALL: Set<String> = setOf(
        EMERGENCY, DEBUGGING_ON, DEBUGGING_OFF, PAIRED, UNPAIRED, GUARDIAN_CHANGED, UPDATED,
    )
}

/**
 * One entry. [at] is unix secs. Per kind: `debugging_on` may carry [until]
 * (when known); the pairing kinds carry the short guardian key in [guardian]
 * (and the previous one in [was] for `guardian_changed`); `updated` carries
 * the version names in [from] and [to]. There is deliberately no field for
 * anything a user typed.
 */
data class ActivityEntry(
    val at: Long,
    val kind: String,
    val until: Long? = null,
    val guardian: String? = null,
    val was: String? = null,
    val from: String? = null,
    val to: String? = null,
)

/** The file's line format. Pure. */
object ActivityFormat {
    const val VERSION = 1
    private const val MAX_FIELD = 64

    /** One compact JSON line, fields in a fixed order. */
    fun encode(e: ActivityEntry): String = buildString {
        append("{\"v\":").append(VERSION)
        append(",\"at\":").append(e.at)
        append(",\"kind\":").append(JSONObject.quote(e.kind))
        e.until?.let { append(",\"until\":").append(it) }
        e.guardian?.let { append(",\"guardian\":").append(JSONObject.quote(it)) }
        e.was?.let { append(",\"was\":").append(JSONObject.quote(it)) }
        e.from?.let { append(",\"from\":").append(JSONObject.quote(it)) }
        e.to?.let { append(",\"to\":").append(JSONObject.quote(it)) }
        append('}')
    }

    /** Null for anything unusable: a torn line after a crash, a kind this
     *  build does not know, a version it does not know. Never throws. */
    fun decode(raw: String): ActivityEntry? = runCatching {
        if (raw.isBlank()) return null
        val o = JSONObject(raw)
        if (o.optInt("v", -1) != VERSION || !o.has("at")) return null
        val kind = o.optString("kind", "")
        if (kind !in ActivityKind.ALL) return null
        fun text(key: String): String? =
            if (o.has(key) && !o.isNull(key)) o.optString(key, "").take(MAX_FIELD).ifEmpty { null } else null
        val until = if (o.has("until") && !o.isNull("until")) o.getLong("until") else null
        val entry = ActivityEntry(
            at = o.getLong("at"),
            kind = kind,
            until = until,
            guardian = text("guardian"),
            was = text("was"),
            from = text("from"),
            to = text("to"),
        )
        if (kind == ActivityKind.UPDATED && (entry.from == null || entry.to == null)) return null
        entry
    }.getOrNull()
}

/** What is kept. Pure. */
object ActivityRetention {
    /** At most this many of the NEWEST entries of each kind. */
    const val PER_KIND = 200

    /** The file is rewritten only once it passes this many lines. */
    const val COMPACT_ABOVE_LINES = 2_000

    /**
     * The newest [perKind] entries of EACH kind, in their original order. The
     * cap is per kind and never across kinds, so a flood of one kind (say USB
     * debugging switched on and off a few hundred times) can never push out
     * another kind's entries.
     */
    fun compact(entries: List<ActivityEntry>, perKind: Int = PER_KIND): List<ActivityEntry> {
        val seen = HashMap<String, Int>()
        val keptNewestFirst = ArrayList<ActivityEntry>()
        for (e in entries.asReversed()) {
            val n = (seen[e.kind] ?: 0) + 1
            seen[e.kind] = n
            if (n <= perKind) keptNewestFirst += e
        }
        return keptNewestFirst.asReversed()
    }
}

/** Where the activity lives. Seam for tests. */
interface ActivityLog {
    /** Null when the log will not read (never read as empty). */
    fun entries(): List<ActivityEntry>?

    /** True only once the entry is durably written. */
    fun append(entry: ActivityEntry): Boolean
}

/**
 * The activity file. One instance per process (the service and the lock shade
 * append, the home screen reads), so every read and append goes through one
 * lock and one in-memory copy, loaded from disk once. Appends are
 * append + flush + fsync; compaction rewrites to a temp file, fsyncs, then
 * renames it over the old one atomically.
 */
class FileActivityLog(
    private val file: File,
    private val compactAboveLines: Int = ActivityRetention.COMPACT_ABOVE_LINES,
    private val perKind: Int = ActivityRetention.PER_KIND,
) : ActivityLog {
    private var entries: MutableList<ActivityEntry>? = null

    /** Non-blank lines in the file (decodable or not), for the compaction trigger. */
    private var lineCount = 0

    @Synchronized
    override fun entries(): List<ActivityEntry>? = loaded()?.toList()

    @Synchronized
    override fun append(entry: ActivityEntry): Boolean {
        val current = loaded()
        val ok = try {
            file.parentFile?.mkdirs()
            // A torn last line (a crash mid-write) must not swallow this one.
            val lead = if (endsMidLine()) "\n" else ""
            FileOutputStream(file, true).use { out ->
                out.write((lead + ActivityFormat.encode(entry) + "\n").toByteArray(Charsets.UTF_8))
                out.flush()
                out.fd.sync()
            }
            true
        } catch (_: Throwable) {
            false
        }
        if (ok && current != null) {
            current += entry
            lineCount++
            if (lineCount > compactAboveLines) compact(current)
        } else {
            entries = null // re-read: the file is the truth
        }
        return ok
    }

    /** Best effort: a failure leaves the old file whole and is tried again on
     *  a later append. */
    private fun compact(current: MutableList<ActivityEntry>) {
        val kept = ActivityRetention.compact(current, perKind)
        val tmp = File(file.parentFile, file.name + ".tmp")
        try {
            FileOutputStream(tmp, false).use { out ->
                out.write(kept.joinToString("") { ActivityFormat.encode(it) + "\n" }.toByteArray(Charsets.UTF_8))
                out.flush()
                out.fd.sync()
            }
            Files.move(tmp.toPath(), file.toPath(), StandardCopyOption.ATOMIC_MOVE)
            current.clear()
            current += kept
            lineCount = kept.size
        } catch (t: Throwable) {
            runCatching { tmp.delete() }
            Log.w("ActivityLog", "compaction failed, kept the full file: ${t.javaClass.simpleName}")
        }
    }

    private fun endsMidLine(): Boolean {
        if (!file.exists() || file.length() == 0L) return false
        RandomAccessFile(file, "r").use { f ->
            f.seek(f.length() - 1)
            return f.read() != '\n'.code
        }
    }

    /** Null when the file will not read. That is not cached, and never read
     *  as empty: the next call tries again. */
    private fun loaded(): MutableList<ActivityEntry>? {
        entries?.let { return it }
        val read = runCatching {
            if (file.exists()) file.readLines(Charsets.UTF_8) else emptyList()
        }.getOrNull() ?: return null
        lineCount = read.count { it.isNotBlank() }
        return read.mapNotNull { ActivityFormat.decode(it) }.toMutableList().also { entries = it }
    }

    companion object {
        /** In device-protected storage (readable before the first unlock),
         *  next to the screen-lock history and outside the warden's base dir. */
        fun fileIn(deviceProtectedFilesDir: File): File =
            File(deviceProtectedFilesDir, "activity.jsonl")

        @Volatile private var shared: FileActivityLog? = null

        fun get(context: Context): FileActivityLog =
            shared ?: synchronized(this) {
                shared ?: FileActivityLog(
                    fileIn(context.createDeviceProtectedStorageContext().filesDir),
                ).also { shared = it }
            }
    }
}

/** The child-facing words for the activity card. Pure; British English. */
object ActivityCopy {
    private val TIME = DateTimeFormatter.ofPattern("HH:mm", Locale.UK)
    private val DAY = DateTimeFormatter.ofPattern("d MMM", Locale.UK)

    /** "14:02, 15 Nov", in the phone's own time zone. */
    fun stamp(at: Long, zone: ZoneId): String {
        val t = Instant.ofEpochSecond(at).atZone(zone)
        return "${TIME.format(t)}, ${DAY.format(t)}"
    }

    /** What happened, without when. */
    fun what(e: ActivityEntry): String = when (e.kind) {
        ActivityKind.EMERGENCY -> "Emergency button used on the lock screen"
        ActivityKind.DEBUGGING_ON -> "USB debugging turned on (a computer can control this phone)"
        ActivityKind.DEBUGGING_OFF -> "USB debugging turned off"
        ActivityKind.PAIRED -> e.guardian?.let { "Paired with guardian $it" } ?: "Paired with a guardian"
        ActivityKind.UNPAIRED -> e.guardian?.let { "Unpaired from guardian $it" } ?: "Unpaired from your guardian"
        ActivityKind.GUARDIAN_CHANGED -> when {
            e.guardian != null && e.was != null -> "Now paired with guardian ${e.guardian} (was ${e.was})"
            e.guardian != null -> "Now paired with guardian ${e.guardian}"
            else -> "Now paired with a different guardian"
        }
        ActivityKind.UPDATED ->
            if (e.from == e.to) "Kintrinsic updated (version ${e.to})" else "Kintrinsic updated from ${e.from} to ${e.to}"
        else -> e.kind
    }

    fun row(e: ActivityEntry, zone: ZoneId): String = "${stamp(e.at, zone)} · ${what(e)}"

    /**
     * One reset as a row. The device cannot know who tapped OK, so this never
     * says "you": "OK tapped 14:03" (same day as the reset) or "OK tapped
     * 09:12, 16 Nov". A reset still pending or applied and not acknowledged
     * says "not seen yet". A failed or not-ready reset had no notice, so it
     * has no OK part.
     */
    fun screenLockRow(r: ScreenLockRecord, zone: ZoneId): String {
        val what = if (r.kind == ScreenLockChange.REMOVED) "removed" else "changed"
        val how = when (r.outcome) {
            ScreenLockOutcome.APPLIED -> "done"
            ScreenLockOutcome.FAILED -> "it didn't work, nothing changed"
            ScreenLockOutcome.NOT_READY -> "not done, this phone wasn't ready yet"
            ScreenLockOutcome.PENDING -> "may have happened"
            else -> r.outcome
        }
        val hadNotice = r.outcome == ScreenLockOutcome.PENDING || r.outcome == ScreenLockOutcome.APPLIED
        val ok = when {
            !hadNotice -> null
            r.acknowledgedAt != null -> "OK tapped ${okStamp(r.at, r.acknowledgedAt, zone)}"
            r.acknowledged -> "OK tapped"
            else -> "not seen yet"
        }
        return listOfNotNull("${stamp(r.at, zone)} · Screen lock $what by your guardian", how, ok).joinToString(" · ")
    }

    private fun okStamp(recordAt: Long, ackAt: Long, zone: ZoneId): String {
        val rec = Instant.ofEpochSecond(recordAt).atZone(zone).toLocalDate()
        val ack = Instant.ofEpochSecond(ackAt).atZone(zone)
        return if (ack.toLocalDate() == rec) TIME.format(ack) else "${TIME.format(ack)}, ${DAY.format(ack)}"
    }
}

/** Both sources merged for the home screen. Pure. */
object ActivityFeed {
    /** How many rows show before "Show all". */
    const val SHORT = 10
    const val UNREADABLE = "Some activity couldn't be read."
    const val EMPTY = "Nothing yet. Screen lock changes, emergency calls, USB debugging and pairing changes will show here."
    const val SUB = "Things out of the ordinary. Kept on this phone for as long as Kintrinsic is on it, and nobody can delete them."

    /** [at] is the time the row sorts by (unix secs). */
    data class Row(val at: Long, val text: String)

    /** [unreadable] is true when either source would not read; the other's
     *  rows are still all here. */
    data class Feed(val rows: List<Row>, val unreadable: Boolean)

    /**
     * Newest first. A null source did not read: its rows are missing but the
     * other's still show, and [Feed.unreadable] says so. Rows with the same
     * time keep the later-written one first.
     */
    fun merge(screenLock: List<ScreenLockRecord>?, activity: List<ActivityEntry>?, zone: ZoneId): Feed {
        val rows = ArrayList<Row>()
        screenLock?.asReversed()?.forEach { rows += Row(it.at, ActivityCopy.screenLockRow(it, zone)) }
        activity?.asReversed()?.forEach { rows += Row(it.at, ActivityCopy.row(it, zone)) }
        return Feed(rows.sortedByDescending { it.at }, unreadable = screenLock == null || activity == null)
    }

    /** The rows to show, and whether a "Show all" / "Show fewer" button is needed. */
    fun visible(rows: List<Row>, showAll: Boolean): List<Row> = if (showAll) rows else rows.take(SHORT)

    fun needsToggle(rows: List<Row>): Boolean = rows.size > SHORT
}

/** What an edge-trigger decision says: an entry to write (or none), and the
 *  state to remember (null = leave the remembered state alone). */
data class Observation<S>(val entry: ActivityEntry?, val state: S?)

/** The pairing as last seen, as the edge trigger remembers it. */
data class PairingMemory(val paired: Boolean, val guardian: String?)

/** The app version as last seen. */
data class VersionMemory(val name: String, val code: Long)

/**
 * The three edge-triggered decisions. Pure. Each compares the last remembered
 * state with what is observed now. The first observation after install or
 * upgrade (nothing remembered) records a silent baseline and writes no entry,
 * so an upgrade logs nothing and an already-paired phone does not log
 * "Paired". An observation that is not one (a null read-back, an unreadable
 * pairing) writes nothing and leaves the remembered state alone.
 */
object ActivityTriggers {

    /** [open] is the read-back that `syncDebugging` reports (true = USB
     *  debugging allowed); [until] is when an open window ends, if known. */
    fun debugging(last: Boolean?, open: Boolean?, until: Long?, nowUnix: Long): Observation<Boolean> = when {
        open == null -> Observation(null, null)
        last == null -> Observation(null, open)
        last == open -> Observation(null, null)
        open -> Observation(
            ActivityEntry(nowUnix, ActivityKind.DEBUGGING_ON, until = until?.takeIf { it > 0 }),
            true,
        )
        else -> Observation(ActivityEntry(nowUnix, ActivityKind.DEBUGGING_OFF), false)
    }

    fun pairing(last: PairingMemory?, state: CharterCore.PairingState?, nowUnix: Long): Observation<PairingMemory> {
        // Not observations: no read, a refused read, a record that will not read.
        if (state == null || state.error != null || state.pairingUnreadable) return Observation(null, null)
        val guardian = state.guardianShort?.takeIf { it.isNotEmpty() }
        // Paired but no key to name: nothing to say yet.
        if (state.paired && guardian == null) return Observation(null, null)
        val now = PairingMemory(state.paired, if (state.paired) guardian else null)
        if (last == null) return Observation(null, now)
        return when {
            !last.paired && now.paired ->
                Observation(ActivityEntry(nowUnix, ActivityKind.PAIRED, guardian = now.guardian), now)
            last.paired && !now.paired ->
                Observation(ActivityEntry(nowUnix, ActivityKind.UNPAIRED, guardian = last.guardian), now)
            last.paired && now.paired && last.guardian != now.guardian ->
                Observation(
                    ActivityEntry(nowUnix, ActivityKind.GUARDIAN_CHANGED, guardian = now.guardian, was = last.guardian),
                    now,
                )
            else -> Observation(null, null)
        }
    }

    /** [name] and [code] are this build's; a null name is no observation. */
    fun version(last: VersionMemory?, name: String?, code: Long, nowUnix: Long): Observation<VersionMemory> {
        if (name.isNullOrEmpty()) return Observation(null, null)
        val now = VersionMemory(name, code)
        return when {
            last == null -> Observation(null, now)
            last == now -> Observation(null, null)
            else -> Observation(ActivityEntry(nowUnix, ActivityKind.UPDATED, from = last.name, to = name), now)
        }
    }
}

/** The remembered last state of the three edge triggers. Seam for tests. */
interface ActivityMemory {
    fun debugging(): Boolean?
    fun setDebugging(open: Boolean): Boolean
    fun pairing(): PairingMemory?
    fun setPairing(p: PairingMemory): Boolean
    fun version(): VersionMemory?
    fun setVersion(v: VersionMemory): Boolean
}

/** Device-protected SharedPreferences, file `activity_log`. Outside the
 *  warden's base dir, so a release never clears it. */
class PrefsActivityMemory(context: Context) : ActivityMemory {
    private val prefs = context.createDeviceProtectedStorageContext()
        .getSharedPreferences("activity_log", Context.MODE_PRIVATE)

    override fun debugging(): Boolean? = if (prefs.contains(DEBUGGING)) prefs.getBoolean(DEBUGGING, false) else null
    override fun setDebugging(open: Boolean) = prefs.edit().putBoolean(DEBUGGING, open).commit()

    override fun pairing(): PairingMemory? =
        if (prefs.contains(PAIRED)) PairingMemory(prefs.getBoolean(PAIRED, false), prefs.getString(GUARDIAN, null)) else null

    override fun setPairing(p: PairingMemory) =
        prefs.edit().putBoolean(PAIRED, p.paired).putString(GUARDIAN, p.guardian).commit()

    override fun version(): VersionMemory? {
        val name = prefs.getString(VERSION_NAME, null) ?: return null
        return if (prefs.contains(VERSION_CODE)) VersionMemory(name, prefs.getLong(VERSION_CODE, 0L)) else null
    }

    override fun setVersion(v: VersionMemory) =
        prefs.edit().putString(VERSION_NAME, v.name).putLong(VERSION_CODE, v.code).commit()

    private companion object {
        const val DEBUGGING = "debuggingOpen"
        const val PAIRED = "paired"
        const val GUARDIAN = "guardian"
        const val VERSION_NAME = "versionName"
        const val VERSION_CODE = "versionCode"
    }
}

/**
 * Applies the edge triggers: decide, write the entry, then remember the new
 * state. The state is remembered only once the entry is written, so a failed
 * write is tried again on the next observation instead of being lost. Never
 * throws: the warden's tick must not be broken by the log.
 */
class ActivityWatcher(
    private val log: ActivityLog,
    private val memory: ActivityMemory,
    private val logLine: (String) -> Unit = { Log.w("ActivityLog", it) },
) {
    @Synchronized
    fun observeDebugging(open: Boolean?, until: Long?, nowUnix: Long) = guarded("debugging") {
        settle(ActivityTriggers.debugging(memory.debugging(), open, until, nowUnix)) { memory.setDebugging(it) }
    }

    @Synchronized
    fun observePairing(state: CharterCore.PairingState?, nowUnix: Long) = guarded("pairing") {
        settle(ActivityTriggers.pairing(memory.pairing(), state, nowUnix)) { memory.setPairing(it) }
    }

    @Synchronized
    fun observeVersion(name: String?, code: Long, nowUnix: Long) = guarded("version") {
        settle(ActivityTriggers.version(memory.version(), name, code, nowUnix)) { memory.setVersion(it) }
    }

    private fun <S> settle(obs: Observation<S>, remember: (S) -> Boolean) {
        obs.entry?.let {
            if (!log.append(it)) {
                logLine("activity entry '${it.kind}' could not be written; will try again")
                return
            }
        }
        obs.state?.let { if (!remember(it)) logLine("activity state could not be remembered") }
    }

    private inline fun guarded(what: String, block: () -> Unit) {
        try {
            block()
        } catch (t: Throwable) {
            logLine("activity $what observation failed: ${t.javaClass.simpleName}")
        }
    }
}

/**
 * The lock shade's emergency control. The log write is best-effort and after
 * the fact: it happens only once the emergency dialer or call has started, off
 * the main thread, wrapped so it can never throw, and it can never block,
 * delay or change the call path. A failed write is logged and dropped.
 */
object EmergencyUse {

    /** One daemon thread for the odd log write, so the call path never waits on disk. */
    private val io by lazy {
        Executors.newSingleThreadExecutor { r -> Thread(r, "activity-log").apply { isDaemon = true } }
    }

    /**
     * Runs [start], which starts the call (throwing if it could not) and
     * returns whether it was an emergency one. Returns whether the call
     * started, exactly as without any logging. When it started and was an
     * emergency, [record] is handed to [runner]; anything either of them
     * throws is passed to [onFailure] and dropped.
     */
    fun placeAndRecord(
        start: () -> Boolean,
        record: () -> Unit,
        runner: (Runnable) -> Unit = { io.execute(it) },
        onFailure: (Throwable) -> Unit = { Log.w("ActivityLog", "emergency use not logged: ${it.javaClass.simpleName}") },
    ): Boolean {
        val emergency = try {
            start()
        } catch (_: Throwable) {
            return false
        }
        if (emergency) {
            try {
                runner(Runnable { runCatching(record).onFailure(onFailure) })
            } catch (t: Throwable) {
                runCatching { onFailure(t) }
            }
        }
        return true
    }

    /** Appends the `emergency` entry (called by [placeAndRecord]'s runner). */
    fun record(context: Context, nowUnix: Long = System.currentTimeMillis() / 1000) {
        val ok = FileActivityLog.get(context).append(ActivityEntry(nowUnix, ActivityKind.EMERGENCY))
        if (!ok) Log.w("ActivityLog", "emergency use could not be written")
    }
}
