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
 * to a PIN they choose, or removing it, and the child is always told. The
 * core verifies the signed reset and parks it; this file applies it through
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
 *  it arrived (its first line). */
data class ScreenLockRecord(
    val id: String,
    val at: Long,
    val kind: String,
    val outcome: String,
    val acknowledged: Boolean,
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
     * Fold lines into records, oldest first. Field by field, never "last line
     * wins": [ScreenLockRecord.at] and kind come from the id's first line, the
     * outcome from its latest line that has one, and an acknowledgement, once
     * made, stays made. An id whose record line was lost still shows (as a
     * change, `pending` until an outcome says otherwise) rather than vanishing.
     */
    fun fold(lines: List<ScreenLockLine>): List<ScreenLockRecord> {
        val order = mutableListOf<String>()
        val byId = mutableMapOf<String, ScreenLockRecord>()
        for (l in lines) {
            val prev = byId[l.id]
            if (prev == null) {
                order += l.id
                byId[l.id] = ScreenLockRecord(
                    id = l.id,
                    at = l.at,
                    kind = l.kind ?: ScreenLockChange.CHANGED,
                    outcome = l.outcome ?: ScreenLockOutcome.PENDING,
                    acknowledged = l.acknowledged,
                )
            } else {
                byId[l.id] = prev.copy(
                    outcome = l.outcome ?: prev.outcome,
                    acknowledged = prev.acknowledged || l.acknowledged,
                )
            }
        }
        return order.mapNotNull { byId[it] }
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

    /** One row of "Screen lock history". */
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
    fun records(): List<ScreenLockRecord>
}

/** The core's half (JNI). Seam for tests. */
interface ScreenLockCore {
    fun take(): CharterCore.ScreenLockDirective?
    fun report(id: String, result: String, atUnix: Long)
    fun setState(secure: Boolean, resetReady: Boolean)
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

    private fun logRefusal(message: String) {
        if (refusalsLogged.add(message)) log(message)
    }

    /** Device Owner only, with the core up. */
    fun platformTick(nowUnix: Long) {
        syncToken(nowUnix)
        val secure = runCatching { platform.isDeviceSecure() }.getOrDefault(false)
        val ready = tokenActive()
        core.setState(secure, ready)
        drain(nowUnix)
    }

    /** Every tick. */
    fun noticeTick() {
        val text = ScreenLockCopy.currentNotice(history.records(), zone())
        if (text == null) notice.hide() else notice.show(text)
    }

    private fun tokenActive(): Boolean = runCatching { platform.isTokenActive() }.getOrDefault(false)

    private fun syncToken(nowUnix: Long) {
        val stored = runCatching { tokens.load() }.getOrNull()
        val active = tokenActive()
        if (active) restored = false
        when (ScreenLockTokenPolicy.step(stored != null, active, restored, lastRefusedAt, nowUnix)) {
            ScreenLockTokenStep.NONE -> Unit
            ScreenLockTokenStep.PROVISION -> {
                val token = newToken()
                val taken = setToken(token)
                if (taken && tokens.save(token)) {
                    // Just set: on a phone with a PIN it is pending until the
                    // next unlock, and setting it again would change nothing.
                    restored = true
                    lastRefusedAt = null
                    refusalsLogged.clear()
                    log("screen-lock reset token provisioned")
                } else {
                    lastRefusedAt = nowUnix
                    logRefusal("screen-lock reset token not provisioned (platform took it: $taken)")
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
        platform.setToken(token)
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
        val recorded = runCatching {
            history.append(ScreenLockLine(d.id, nowUnix, kind = kind, outcome = ScreenLockOutcome.PENDING))
        }.getOrDefault(false)
        if (!recorded) {
            log("screen-lock reset $tag…: no notice could be recorded, so nothing was changed")
            core.report(d.id, ScreenLockOutcome.NOTICE_FAILED, nowUnix)
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
        core.report(id, outcome, nowUnix)
        log("screen-lock reset $tag…: $outcome")
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
    override fun records(): List<ScreenLockRecord> = ScreenLockHistoryFormat.fold(loaded())

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
        if (ok) current += line else lines = null // re-read: the file is the truth
        return ok
    }

    private fun endsMidLine(): Boolean {
        if (!file.exists() || file.length() == 0L) return false
        RandomAccessFile(file, "r").use { f ->
            f.seek(f.length() - 1)
            return f.read() != '\n'.code
        }
    }

    private fun loaded(): MutableList<ScreenLockLine> {
        lines?.let { return it }
        val read = runCatching {
            if (file.exists()) file.readLines(Charsets.UTF_8).mapNotNull { ScreenLockHistoryFormat.decode(it) } else emptyList()
        }.getOrNull()
        // An unreadable file is not cached as empty: the next call tries again.
        return (read ?: return mutableListOf()).toMutableList().also { lines = it }
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
    override fun report(id: String, result: String, atUnix: Long) {
        CharterCore.reportScreenLockReset(id, result, atUnix)
    }
    override fun setState(secure: Boolean, resetReady: Boolean) {
        if (CharterCore.setScreenLockState(secure, resetReady)) {
            Log.i("ScreenLockReset", "screen lock: secure=$secure resetReady=$resetReady")
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
