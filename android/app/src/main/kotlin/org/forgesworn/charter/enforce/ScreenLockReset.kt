package org.forgesworn.charter.enforce

import android.content.Context
import android.util.Log
import org.forgesworn.charter.native.CharterCore
import org.json.JSONObject
import java.io.File
import java.io.FileOutputStream
import java.io.RandomAccessFile
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale

/*
 * The guardian's screen-lock reset, ward side (spec "Ward app"; amendments
 * A2, A3, A4, A5, A7).
 *
 * A guardian can always get into a ward phone by resetting its own screen lock
 * to a PIN they choose, or removing it. The child is shown a notice and keeps
 * a record of it that no command can delete (whoever holds the phone can
 * dismiss the notice; the record stays). The core verifies the signed reset
 * and parks it; this file applies it through
 * the Device Owner's reset-password token and keeps the child's record of it.
 *
 * The record is an append-only JSON-lines file in device-protected storage,
 * OUTSIDE the warden's base dir, so a guardian's release (which wipes the
 * clauses and the pairing) never erases it. It never holds the PIN. Nothing
 * here ever logs the PIN, and nothing that could carry it reaches an
 * exception message.
 */

/** What became of a reset, as the child's history records it. */
object ScreenLockOutcome {
    const val PENDING = "pending"
    const val APPLIED = "applied"
    const val FAILED = "failed"
    const val NOT_READY = "not_ready"

    /** Reported to the core only: no history line could be written, so no
     *  reset was attempted. Never appears in the history itself. */
    const val NOTICE_FAILED = "notice_failed"
}

/** What a reset did to the lock: set a new PIN, or remove the lock. */
object ScreenLockChange {
    const val CHANGED = "changed"
    const val REMOVED = "removed"
}

/**
 * One line of the history. Three shapes, all keyed by [id]:
 * - the record, written before the reset is attempted: [kind] and
 *   [outcome] = `pending`;
 * - an outcome update: [outcome] only;
 * - the child's acknowledgement: [acknowledged] only.
 * [at] is when the line was written (unix secs). There is deliberately no
 * field for the PIN.
 */
data class ScreenLockLine(
    val id: String,
    val at: Long,
    val kind: String? = null,
    val outcome: String? = null,
    val acknowledged: Boolean = false,
)

/** One reset as the history tells it, folded from its lines. [at] is when
 *  it arrived (its first line). [acknowledgedAt] is the `at` of the earliest
 *  acknowledgement line that belongs to this record, null while it has none. */
data class ScreenLockRecord(
    val id: String,
    val at: Long,
    val kind: String,
    val outcome: String,
    val acknowledged: Boolean,
    val acknowledgedAt: Long? = null,
)

/** The history's line format and its fold. Pure. */
object ScreenLockHistoryFormat {

    /** One compact JSON line, fields in a fixed order. */
    fun encode(line: ScreenLockLine): String = buildString {
        append("{\"id\":").append(JSONObject.quote(line.id))
        append(",\"at\":").append(line.at)
        line.kind?.let { append(",\"kind\":").append(JSONObject.quote(it)) }
        line.outcome?.let { append(",\"outcome\":").append(JSONObject.quote(it)) }
        if (line.acknowledged) append(",\"acknowledged\":true")
        append('}')
    }

    /** Null for anything unreadable: a torn last line after a crash, say. */
    fun decode(raw: String): ScreenLockLine? = runCatching {
        if (raw.isBlank()) return null
        val o = JSONObject(raw)
        val id = o.optString("id", "")
        if (id.isEmpty() || !o.has("at")) return null
        ScreenLockLine(
            id = id,
            at = o.getLong("at"),
            kind = o.optString("kind", "").ifEmpty { null },
            outcome = o.optString("outcome", "").ifEmpty { null },
            acknowledged = o.optBoolean("acknowledged", false),
        )
    }.getOrNull()

    /**
     * Fold lines into records, oldest first. Every record line (one with a
     * kind) starts a NEW record, even for an id seen before, so a reset can
     * never inherit an earlier record's acknowledgement and go untold. An
     * outcome or acknowledgement line belongs to the latest record for its
     * id. Field by field, never "last line wins": [ScreenLockRecord.at] and
     * kind come from the record line, the outcome from the latest line that
     * has one, and an acknowledgement, once made, stays made (with the `at` of
     * the earliest one, in [ScreenLockRecord.acknowledgedAt]). An id whose
     * record line was lost still shows (as a change, `pending` until an
     * outcome says otherwise) rather than vanishing.
     */
    fun fold(lines: List<ScreenLockLine>): List<ScreenLockRecord> {
        val records = mutableListOf<ScreenLockRecord>()
        val latest = mutableMapOf<String, Int>()
        for (l in lines) {
            val at = latest[l.id]
            if (at == null || l.kind != null) {
                latest[l.id] = records.size
                records += ScreenLockRecord(
                    id = l.id,
                    at = l.at,
                    kind = l.kind ?: ScreenLockChange.CHANGED,
                    outcome = l.outcome ?: ScreenLockOutcome.PENDING,
                    acknowledged = l.acknowledged,
                    acknowledgedAt = if (l.acknowledged) l.at else null,
                )
            } else {
                val prev = records[at]
                records[at] = prev.copy(
                    outcome = l.outcome ?: prev.outcome,
                    acknowledged = prev.acknowledged || l.acknowledged,
                    // The earliest acknowledgement of THIS record: once set it
                    // is never moved, and an earlier record's never carries over.
                    acknowledgedAt = prev.acknowledgedAt ?: if (l.acknowledged) l.at else null,
                )
            }
        }
        return records
    }

    /** The records the child must still be told about, newest first (A2):
     *  `pending` (it may have happened) or `applied`, and not acknowledged. */
    fun unacknowledged(records: List<ScreenLockRecord>): List<ScreenLockRecord> =
        records
            .filter { !it.acknowledged && (it.outcome == ScreenLockOutcome.PENDING || it.outcome == ScreenLockOutcome.APPLIED) }
            .sortedByDescending { it.at }
}

/** The child-facing words. Pure; British English, wardship terms. */
object ScreenLockCopy {

    private val TIME = DateTimeFormatter.ofPattern("HH:mm", Locale.UK)
    private val DAY = DateTimeFormatter.ofPattern("d MMM", Locale.UK)

    /** "19:42 on 14 Nov", in the phone's own time zone. */
    fun whenText(at: Long, zone: ZoneId): String {
        val t = Instant.ofEpochSecond(at).atZone(zone)
        return "${TIME.format(t)} on ${DAY.format(t)}"
    }

    /** The notice for one reset. A record stuck at `pending` (the phone
     *  stopped between the record and its outcome) says "may have". */
    fun notice(r: ScreenLockRecord, zone: ZoneId): String {
        val time = whenText(r.at, zone)
        val removed = r.kind == ScreenLockChange.REMOVED
        return if (r.outcome == ScreenLockOutcome.PENDING) {
            if (removed) {
                "Your guardian may have removed your screen lock at $time."
            } else {
                "Your guardian may have changed your screen lock at $time. If your PIN doesn't work, ask them for the new one."
            }
        } else if (removed) {
            "Your guardian removed your screen lock at $time."
        } else {
            "Your guardian changed your screen lock at $time. Ask them for the new PIN."
        }
    }

    /** The notice to show now (the newest unacknowledged reset), or null. */
    fun currentNotice(records: List<ScreenLockRecord>, zone: ZoneId): String? =
        ScreenLockHistoryFormat.unacknowledged(records).firstOrNull()?.let { notice(it, zone) }

    /** One row of the old "Screen lock history" card. The home screen now
     *  shows [ActivityCopy.screenLockRow] in "Activity on this phone". */
    fun historyRow(r: ScreenLockRecord, zone: ZoneId): String {
        val time = whenText(r.at, zone)
        val what = if (r.kind == ScreenLockChange.REMOVED) "Removed" else "Changed"
        val how = when (r.outcome) {
            ScreenLockOutcome.APPLIED -> "done"
            ScreenLockOutcome.FAILED -> "it didn't work, nothing changed"
            ScreenLockOutcome.NOT_READY -> "not done, this phone wasn't ready yet"
            ScreenLockOutcome.PENDING -> "may have happened"
            else -> r.outcome
        }
        return "$time · $what by your guardian · $how"
    }
}

/** What the reset-password token needs this tick (spec "Reset token"; A3). */
enum class ScreenLockTokenStep {
    /** Nothing to do. */
    NONE,
    /** No token of ours is stored: make one, set it, keep it only if the
     *  platform took it. */
    PROVISION,
    /** Ours is stored but the platform does not hold it active: set the same
     *  token again. */
    RESTORE,
}

/**
 * The token decision. Pure.
 *
 * - No token stored: provision one. A refusal is retried after
 *   [RETRY_SECS], not every tick.
 * - Stored and active: nothing. Never set it again: on a phone with a PIN
 *   that would turn an active token back into a pending one.
 * - Stored but not active: set the same token again once, whether or not the
 *   phone has a screen lock (A3: a token set while a PIN exists is pending in
 *   system memory only, and a reboot before the next unlock drops it). Once
 *   means once until the token is next seen active ([restored] is cleared
 *   then), so a platform that drops it again is covered without setting it
 *   every tick.
 */
object ScreenLockTokenPolicy {
    const val RETRY_SECS = 60L

    fun step(
        stored: Boolean,
        active: Boolean,
        restored: Boolean,
        lastRefusedAt: Long?,
        nowUnix: Long,
    ): ScreenLockTokenStep {
        val refusedRecently = lastRefusedAt != null && nowUnix >= lastRefusedAt && nowUnix - lastRefusedAt < RETRY_SECS
        return when {
            !stored -> if (refusedRecently) ScreenLockTokenStep.NONE else ScreenLockTokenStep.PROVISION
            active -> ScreenLockTokenStep.NONE
            restored -> ScreenLockTokenStep.NONE
            refusedRecently -> ScreenLockTokenStep.NONE
            else -> ScreenLockTokenStep.RESTORE
        }
    }
}

/** The platform the reset needs (Device Owner calls). Seam for tests. */
interface ScreenLockPlatform {
    fun isDeviceSecure(): Boolean
    fun isTokenActive(): Boolean
    fun setToken(token: ByteArray): Boolean
    /** [newPin] null removes the screen lock. */
    fun resetWithToken(newPin: String?, token: ByteArray): Boolean
}

/** Where our token is kept. Seam for tests. */
interface ScreenLockTokenStore {
    fun load(): ByteArray?
    fun save(token: ByteArray): Boolean
}

/** The append-only history. Seam for tests. */
interface ScreenLockHistory {
    /** Durable when it returns true. */
    fun append(line: ScreenLockLine): Boolean
    /** Null when the history will not read: never the same as empty, so an
     *  unreadable history can never hide a notice. */
    fun records(): List<ScreenLockRecord>?
}

/** The core's half (JNI). Seam for tests. */
interface ScreenLockCore {
    fun take(): CharterCore.ScreenLockDirective?
    /** True once the core has taken the outcome (recorded it, or already
     *  had one for that id); false when it could not, so it is sent again. */
    fun report(id: String, result: String, atUnix: Long): Boolean
    /** [resetSupported] false: the platform refuses a reset token on this
     *  device for good, so no unlock will make [resetReady] true. */
    fun setState(secure: Boolean, resetReady: Boolean, resetSupported: Boolean)
}

/** The child's notification. Seam for tests. */
interface ScreenLockNoticeSurface {
    fun show(text: String)
    fun hide()
}

/**
 * The per-tick screen-lock step. [platformTick] is the Device Owner part
 * (token, state, applying a parked reset); [noticeTick] derives the child's
 * notice from the history and runs on every tick, paired or not, so the notice
 * outlives a release (A5) and a crash between a reset and its outcome line
 * still tells the child (A2).
 */
class ScreenLockResetStep(
    private val platform: ScreenLockPlatform,
    private val tokens: ScreenLockTokenStore,
    private val history: ScreenLockHistory,
    private val core: ScreenLockCore,
    private val notice: ScreenLockNoticeSurface,
    private val zone: () -> ZoneId = { ZoneId.systemDefault() },
    private val newToken: () -> ByteArray = {
        ByteArray(32).also { java.security.SecureRandom().nextBytes(it) }
    },
    private val log: (String) -> Unit = {},
) {
    private var restored = false
    private var lastRefusedAt: Long? = null
    /** The refusals logged since the token last went in: a phone whose escrow
     *  support the platform has switched off (one used with a screen lock
     *  before it had a Device Owner) refuses every retry for good, so each is
     *  said once, not every minute. */
    private val refusalsLogged = mutableSetOf<String>()
    /**
     * Whether the platform will hold a reset token here at all (STATUS
     * `screenLockResetSupported`). Latched: false once setting the token
     * throws `SecurityException`, which Android does for good on a phone that
     * had a screen lock and was unlocked before Kintrinsic was its Device
     * Owner ("Permanently disabling support for escrow tokens"). It stays
     * false through the ticks between retries, so it never flaps, and turns
     * true again only if the platform takes a token without throwing or holds
     * one active.
     */
    private var supported = true
    /** Outcomes the core could not take yet, by id, re-sent every tick until
     *  it does. Its settle is once per id, so sending one again is harmless. */
    private val unreported = LinkedHashMap<String, Pair<String, Long>>()

    private fun logRefusal(message: String) {
        if (refusalsLogged.add(message)) log(message)
    }

    /** Device Owner only, with the core up. */
    fun platformTick(nowUnix: Long) {
        syncToken(nowUnix)
        val secure = runCatching { platform.isDeviceSecure() }.getOrDefault(false)
        val active = tokenActive()
        if (active) supported = true
        // Ready only when the token the platform holds active is one this app
        // has: an active token it lacks would answer every reset `not_ready`.
        val held = runCatching { tokens.load() }.getOrNull() != null
        core.setState(secure, active && held, supported)
        resendReports()
        drain(nowUnix)
    }

    /** Every tick. A history that will not read keeps whatever is showing:
     *  only a good read with nothing to tell takes the notice down. */
    fun noticeTick() {
        val records = runCatching { history.records() }.getOrNull() ?: return
        // A damaged record that will not format keeps whatever is showing too.
        val text = runCatching { ScreenLockCopy.currentNotice(records, zone()) }.getOrElse { return }
        if (text == null) notice.hide() else notice.show(text)
    }

    private fun tokenActive(): Boolean = runCatching { platform.isTokenActive() }.getOrDefault(false)

    private fun syncToken(nowUnix: Long) {
        // A read that throws is not "no token": provisioning over it could
        // replace a token the platform holds active. Try again next tick.
        val stored = try {
            tokens.load()
        } catch (_: Throwable) {
            return
        }
        val active = tokenActive()
        if (active) restored = false
        when (ScreenLockTokenPolicy.step(stored != null, active, restored, lastRefusedAt, nowUnix)) {
            ScreenLockTokenStep.NONE -> Unit
            ScreenLockTokenStep.PROVISION -> {
                if (active) {
                    // Never silently: it is useless to us without its bytes,
                    // and on a phone with a PIN the new one waits for an unlock.
                    logRefusal("the platform holds an active reset token this app does not have: replacing it")
                }
                val token = newToken()
                // Kept first, then set: the platform never holds a token this
                // app lacks. A set it refuses leaves the kept token inactive,
                // which RESTORE retries.
                if (!runCatching { tokens.save(token) }.getOrDefault(false)) {
                    lastRefusedAt = nowUnix
                    logRefusal("screen-lock reset token could not be kept, so it was not set")
                    return
                }
                if (setToken(token)) {
                    // Just set: on a phone with a PIN it is pending until the
                    // next unlock, and setting it again would change nothing.
                    restored = true
                    lastRefusedAt = null
                    refusalsLogged.clear()
                    log("screen-lock reset token provisioned")
                } else {
                    lastRefusedAt = nowUnix
                    logRefusal("screen-lock reset token not provisioned (the platform refused it)")
                }
            }
            ScreenLockTokenStep.RESTORE -> {
                if (setToken(stored!!)) {
                    restored = true
                    lastRefusedAt = null
                    refusalsLogged.clear()
                    log("screen-lock reset token set again (it was not active)")
                } else {
                    lastRefusedAt = nowUnix
                    logRefusal("screen-lock reset token refused when set again")
                }
            }
        }
    }

    private fun setToken(token: ByteArray): Boolean = try {
        platform.setToken(token).also { supported = true }
    } catch (e: SecurityException) {
        // The platform refuses reset tokens on this device outright.
        supported = false
        logRefusal("setting the screen-lock reset token threw ${e.javaClass.simpleName}")
        false
    } catch (t: Throwable) {
        logRefusal("setting the screen-lock reset token threw ${t.javaClass.simpleName}")
        false
    }

    /**
     * Apply a parked reset, if there is one. No reset without a notice: the
     * history line is written (durably) first, and if it cannot be, nothing
     * is changed and the guardian is told `notice_failed`.
     */
    private fun drain(nowUnix: Long) {
        val d = core.take() ?: return
        val tag = d.id.take(8)
        val kind = if (d.removes) ScreenLockChange.REMOVED else ScreenLockChange.CHANGED
        val known = runCatching { history.records() }.getOrNull()
        if (known == null) {
            // Its id cannot be checked, and the child may not be told.
            log("screen-lock reset $tag…: the history will not read, so nothing was changed")
            report(d.id, ScreenLockOutcome.NOTICE_FAILED, nowUnix)
            return
        }
        if (known.any { it.id == d.id }) {
            // The core forgets ids in time; the child's history never does. A
            // reused id must never apply: its record could carry an earlier
            // acknowledgement, and the child would not be told.
            log("screen-lock reset $tag…: its id is already in the history, so nothing was changed")
            report(d.id, ScreenLockOutcome.FAILED, nowUnix)
            return
        }
        val recorded = runCatching {
            history.append(ScreenLockLine(d.id, nowUnix, kind = kind, outcome = ScreenLockOutcome.PENDING))
        }.getOrDefault(false)
        if (!recorded) {
            log("screen-lock reset $tag…: no notice could be recorded, so nothing was changed")
            report(d.id, ScreenLockOutcome.NOTICE_FAILED, nowUnix)
            return
        }
        val token = runCatching { tokens.load() }.getOrNull()
        if (token == null || !tokenActive()) {
            finish(d.id, ScreenLockOutcome.NOT_READY, nowUnix, tag)
            return
        }
        // A7: a throw is `failed`. Logged by class only: a platform message
        // must never get the chance to quote what it was given.
        val applied = try {
            platform.resetWithToken(d.newPin.ifEmpty { null }, token)
        } catch (t: Throwable) {
            log("screen-lock reset $tag… threw ${t.javaClass.simpleName}")
            false
        }
        finish(d.id, if (applied) ScreenLockOutcome.APPLIED else ScreenLockOutcome.FAILED, nowUnix, tag)
    }

    private fun finish(id: String, outcome: String, nowUnix: Long, tag: String) {
        // If this line cannot be written the record stays `pending`, which
        // still tells the child ("may have changed"); the guardian hears the
        // real outcome either way.
        runCatching { history.append(ScreenLockLine(id, nowUnix, outcome = outcome)) }
        report(id, outcome, nowUnix)
        log("screen-lock reset $tag…: $outcome")
    }

    /** Tell the core an outcome; one it cannot take yet is sent again on
     *  later ticks, so the guardian is not left at `accepted`. */
    private fun report(id: String, outcome: String, nowUnix: Long) {
        unreported.remove(id)
        if (runCatching { core.report(id, outcome, nowUnix) }.getOrDefault(false)) return
        unreported[id] = outcome to nowUnix
        while (unreported.size > MAX_UNREPORTED) unreported.remove(unreported.keys.first())
    }

    private fun resendReports() {
        for ((id, sent) in unreported.entries.toList()) {
            if (runCatching { core.report(id, sent.first, sent.second) }.getOrDefault(false)) unreported.remove(id)
        }
    }

    private companion object {
        /** As many as the core's own ring of outcomes. */
        const val MAX_UNREPORTED = 16
    }
}

// ---- production implementations ---------------------------------------------

/** The Device Owner's DPM and the keyguard. */
class DpmScreenLockPlatform(
    private val context: Context,
    private val dpm: android.app.admin.DevicePolicyManager,
    private val admin: android.content.ComponentName,
) : ScreenLockPlatform {
    private val keyguard by lazy {
        context.getSystemService(Context.KEYGUARD_SERVICE) as android.app.KeyguardManager
    }
    override fun isDeviceSecure(): Boolean = keyguard.isDeviceSecure
    override fun isTokenActive(): Boolean = dpm.isResetPasswordTokenActive(admin)
    override fun setToken(token: ByteArray): Boolean = dpm.setResetPasswordToken(admin, token)
    // A7: no flag picks PIN or password; the platform makes a digits-only
    // string a PIN.
    override fun resetWithToken(newPin: String?, token: ByteArray): Boolean =
        dpm.resetPasswordWithToken(admin, newPin, token, 0)
}

/** Device-protected SharedPreferences, file `screen_lock`. */
class PrefsScreenLockTokenStore(context: Context) : ScreenLockTokenStore {
    private val prefs = context.createDeviceProtectedStorageContext()
        .getSharedPreferences("screen_lock", Context.MODE_PRIVATE)

    override fun load(): ByteArray? = prefs.getString(KEY, null)?.let {
        runCatching { android.util.Base64.decode(it, android.util.Base64.NO_WRAP) }.getOrNull()
    }?.takeIf { it.size == 32 }

    override fun save(token: ByteArray): Boolean =
        prefs.edit().putString(KEY, android.util.Base64.encodeToString(token, android.util.Base64.NO_WRAP)).commit()

    private companion object {
        const val KEY = "resetToken"
    }
}

/**
 * The history file. One per process (the service writes outcomes, the home
 * screen writes acknowledgements), so every read and append goes through one
 * lock and one in-memory copy, loaded from disk once.
 */
class FileScreenLockHistory(private val file: File) : ScreenLockHistory {
    private var lines: MutableList<ScreenLockLine>? = null

    @Synchronized
    override fun records(): List<ScreenLockRecord>? = loaded()?.let { ScreenLockHistoryFormat.fold(it) }

    @Synchronized
    override fun append(line: ScreenLockLine): Boolean {
        val current = loaded()
        val ok = try {
            file.parentFile?.mkdirs()
            // A torn last line (a crash mid-write) must not swallow this one.
            val lead = if (endsMidLine()) "\n" else ""
            FileOutputStream(file, true).use { out ->
                out.write((lead + ScreenLockHistoryFormat.encode(line) + "\n").toByteArray(Charsets.UTF_8))
                out.fd.sync()
            }
            true
        } catch (_: Throwable) {
            false
        }
        if (ok && current != null) current += line else lines = null // re-read: the file is the truth
        return ok
    }

    private fun endsMidLine(): Boolean {
        if (!file.exists() || file.length() == 0L) return false
        RandomAccessFile(file, "r").use { f ->
            f.seek(f.length() - 1)
            return f.read() != '\n'.code
        }
    }

    /** Null when the file will not read. That is not cached, and never
     *  read as empty: the next call tries again. */
    private fun loaded(): MutableList<ScreenLockLine>? {
        lines?.let { return it }
        val read = runCatching {
            if (file.exists()) file.readLines(Charsets.UTF_8).mapNotNull { ScreenLockHistoryFormat.decode(it) } else emptyList()
        }.getOrNull() ?: return null
        return read.toMutableList().also { lines = it }
    }

    companion object {
        /** In device-protected storage (readable before the first unlock),
         *  and outside the warden's base dir, which a release clears in part. */
        fun fileIn(deviceProtectedFilesDir: File): File =
            File(deviceProtectedFilesDir, "screen_lock_history.jsonl")

        @Volatile private var shared: FileScreenLockHistory? = null

        fun get(context: Context): FileScreenLockHistory =
            shared ?: synchronized(this) {
                shared ?: FileScreenLockHistory(
                    fileIn(context.createDeviceProtectedStorageContext().filesDir),
                ).also { shared = it }
            }
    }
}

/** The JNI half. */
object CoreScreenLock : ScreenLockCore {
    override fun take(): CharterCore.ScreenLockDirective? = CharterCore.takeScreenLockReset()
    override fun report(id: String, result: String, atUnix: Long): Boolean =
        CharterCore.reportScreenLockReset(id, result, atUnix) != null
    override fun setState(secure: Boolean, resetReady: Boolean, resetSupported: Boolean) {
        if (CharterCore.setScreenLockState(secure, resetReady, resetSupported)) {
            Log.i(
                "ScreenLockReset",
                "screen lock: secure=$secure resetReady=$resetReady resetSupported=$resetSupported",
            )
        }
    }
}

/**
 * The child's notification (A4): public on the lock screen, so the child sees
 * why their old PIN is refused; ongoing; tapping it opens the home screen,
 * whose OK is the only way to acknowledge it. Ongoing is not relied on to
 * keep it up: every tick re-posts it if it is gone or its words changed.
 */
class NotificationScreenLockNotice(private val context: Context) : ScreenLockNoticeSurface {
    private val nm by lazy {
        context.getSystemService(Context.NOTIFICATION_SERVICE) as android.app.NotificationManager
    }
    private var posted: String? = null
    /** Unknown after a process start: the first quiet tick cancels once. */
    private var everChecked = false

    override fun show(text: String) {
        val up = runCatching { nm.activeNotifications.any { it.id == NOTIFICATION_ID } }.getOrDefault(false)
        if (up && posted == text) return
        nm.createNotificationChannel(
            android.app.NotificationChannel(
                CHANNEL,
                "Screen lock",
                android.app.NotificationManager.IMPORTANCE_DEFAULT,
            ).apply {
                description = "When your guardian changes or removes your screen lock"
                lockscreenVisibility = android.app.Notification.VISIBILITY_PUBLIC
            },
        )
        nm.notify(
            NOTIFICATION_ID,
            android.app.Notification.Builder(context, CHANNEL)
                .setContentTitle("Your screen lock")
                .setContentText(text)
                .setStyle(android.app.Notification.BigTextStyle().bigText(text))
                .setSmallIcon(android.R.drawable.ic_lock_lock)
                .setVisibility(android.app.Notification.VISIBILITY_PUBLIC)
                .setOngoing(true)
                .setOnlyAlertOnce(true)
                .setContentIntent(org.forgesworn.charter.service.CharterService.openApp(context))
                .build(),
        )
        posted = text
        everChecked = true
    }

    override fun hide() {
        if (posted == null && everChecked) return
        runCatching { nm.cancel(NOTIFICATION_ID) }
        posted = null
        everChecked = true
    }

    companion object {
        const val CHANNEL = "charter-screen-lock"
        const val NOTIFICATION_ID = 2006
    }
}
