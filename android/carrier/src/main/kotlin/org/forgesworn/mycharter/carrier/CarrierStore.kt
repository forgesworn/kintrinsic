package org.forgesworn.mycharter.carrier

import android.content.Context
import android.content.SharedPreferences
import org.json.JSONArray
import org.json.JSONObject
import java.security.SecureRandom

/**
 * App-private persistence for the carrier: the provisioned guardian key,
 * relays, and the seen-reqId LRU (dedupe across restarts — relays replay
 * stored wraps on every reconnect).
 *
 * Custody note (spec D2): the secret is stored app-private + FBE-at-rest,
 * the SAME posture as the WebView's localStorage copy one directory over.
 * Keystore-wrapping this copy would not raise the real bar while that copy
 * exists; proper custody arrives with the embedded-Signet vault component,
 * deliberately NOT half-built here.
 */
class CarrierStore internal constructor(
    private val prefs: SharedPreferences,
    /** The relay-delivered emergency audits held for the console (see [InboxSpool]). */
    private val spool: SpoolFile,
    /** Unix seconds; a seam for the roster's 14-day expiry and the stranger sound limit. */
    private val clock: () -> Long = { System.currentTimeMillis() / 1000 },
) {
    constructor(context: Context) : this(
        context.getSharedPreferences("carrier", Context.MODE_PRIVATE),
        SpoolFile(
            java.io.File(context.filesDir, "inbox-spool.json"),
            onDamaged = { android.util.Log.w("CarrierStore", "the emergency-audit spool was unreadable; set aside as ${it.name}") },
        ),
    )

    fun reconnectCap(): Long = org.forgesworn.mycharter.service.ReconnectPolicy.cap(
        runCatching { prefs.getLong("reconnect_cap_secs", 300) }.getOrDefault(300),
    )
    fun saveReconnectCap(seconds: Long) {
        prefs.edit().putLong("reconnect_cap_secs", org.forgesworn.mycharter.service.ReconnectPolicy.cap(seconds)).apply()
    }

    fun saveProvision(p: ProvisionPayload) {
        prefs.edit()
            .putString(K_SK, p.guardianSkHex)
            .putString(K_PK, p.guardianPubkeyHex)
            .putString(K_RELAYS, JSONArray(p.relays).toString())
            .apply()
    }

    fun provision(): ProvisionPayload? {
        val sk = prefs.getString(K_SK, null) ?: return null
        val pk = prefs.getString(K_PK, null) ?: return null
        val relaysJson = prefs.getString(K_RELAYS, null) ?: return null
        val arr = try { JSONArray(relaysJson) } catch (_: Exception) { return null }
        val relays = (0 until arr.length()).map { arr.optString(it, "") }.filter { it.isNotEmpty() }
        if (relays.isEmpty()) return null
        return ProvisionPayload(sk, pk, relays)
    }

    /** Persisted beside the provision so a roster pushed before a restart is
     *  still there to name a notification after it. */
    fun saveRoster(r: RosterPayload) {
        val arr = JSONArray()
        r.entries.forEach { e ->
            arr.put(
                JSONObject()
                    .put("machine", e.machine)
                    .put("childName", e.childName)
                    .put("deviceLabel", e.deviceLabel)
                    .put("paired", e.paired)
                    .put("released", e.released)
                    .also { o ->
                        e.releasedAt?.let { o.put("releasedAt", it) }
                        e.linkUntil?.let { o.put("linkUntil", it) }
                    },
            )
        }
        prefs.edit().putString(K_ROSTER, arr.toString()).apply()
    }

    /** Empty (never null, never throws) when absent or unreadable — the
     *  roster is a convenience, so a storage hiccup degrades it to "no
     *  names", not a [Notifier] crash. A released entry is dropped 14 days
     *  after its `releasedAt`, even if no newer push has replaced the stored
     *  roster; one unusable entry is skipped on its own. */
    fun roster(nowSecs: Long = clock()): List<RosterEntry> {
        val json = prefs.getString(K_ROSTER, null) ?: return emptyList()
        val arr = try { JSONArray(json) } catch (_: Exception) { return emptyList() }
        return (0 until arr.length()).mapNotNull { i ->
            RosterPayload.entryOf(arr.optJSONObject(i))?.takeUnless { it.expired(nowSecs) }
        }
    }

    /** True if this reqId is NEW (recorded now); false if already seen. */
    @Synchronized
    fun markSeen(reqId: String): Boolean = remember(K_SEEN, SEEN_CAP, reqId)

    /**
     * True if this audit key is NEW (recorded now); false if already seen. Audits
     * have their OWN bounded list, apart from the asks' reqIds, so a flood of one
     * can never evict the other's entries and re-alert a handled press or ask.
     * Keys are `audit:<wrapId>` and `press:<alertKey>`; only senders the roster
     * knows should be recorded here (see [noteUnpaired] for the rest).
     */
    @Synchronized
    fun markAuditSeen(key: String): Boolean {
        // Before this list existed audit keys shared the asks' list: an audit
        // already alerted on then must not re-alert once after an update.
        if (key.startsWith("audit:") && contains(K_SEEN, key)) return false
        return remember(K_SEEN_AUDIT, AUDIT_SEEN_CAP, key)
    }

    /**
     * Note a report from a sender that is not in the (known, non-empty) roster.
     * Returns how many distinct such reports have come since the shared row was
     * last dismissed (at least 1, and at most [UNPAIRED_CAP]; see [resetUnpaired]),
     * or null when this wrap id was already counted (a replay, which must not
     * re-alert). A separate small bounded set, so strangers can neither grow
     * without limit nor push a paired ward's entries out of [markAuditSeen]. A
     * null [wrapId] (unreadable) is not counted.
     */
    fun noteUnpaired(wrapId: String?): Int? = noteStranger(K_UNPAIRED, K_UNPAIRED_SINCE, wrapId)

    /**
     * The same for an ask from a phone that is not paired (a stranger or a released
     * machine), by request id. Never touches [markSeen]'s list, so a flood of
     * these cannot push a paired ward's request ids out and re-alert them.
     */
    fun noteUnpairedAsk(reqId: String?): Int? = noteStranger(K_UNPAIRED_ASKS, K_UNPAIRED_ASKS_SINCE, reqId)

    private fun noteStranger(listKey: String, sinceKey: String, id: String?): Int? = synchronized(LOCK) {
        val since = prefs.getInt(sinceKey, 0).coerceIn(0, UNPAIRED_CAP)
        if (id == null) return since.coerceAtLeast(1)
        val held = list(listKey)
        if (id in held) return null
        val next = (since + 1).coerceAtMost(UNPAIRED_CAP)
        prefs.edit()
            .putString(listKey, JSONArray((held + id).takeLast(UNPAIRED_CAP)).toString())
            .putInt(sinceKey, next)
            .apply()
        next
    }

    /**
     * The guardian dismissed the shared row: the next report counts from one.
     * The ids already seen are kept, so a relay replay of them is still no news.
     */
    fun resetUnpaired(asks: Boolean) = synchronized(LOCK) {
        prefs.edit().putInt(if (asks) K_UNPAIRED_ASKS_SINCE else K_UNPAIRED_SINCE, 0).apply()
    }

    /**
     * True (and noted) when the shared stranger emergency row may SOUND now: at most
     * once per [UNPAIRED_SOUND_SECS]. Otherwise the row updates silently. A last time
     * in the future (the clock went back) does not hold the sound back.
     */
    fun claimUnpairedSound(): Boolean = synchronized(LOCK) {
        val now = clock()
        val last = prefs.getLong(K_UNPAIRED_SOUND, -1L)
        if (last in 0..now && now - last < UNPAIRED_SOUND_SECS) return false
        prefs.edit().putLong(K_UNPAIRED_SOUND, now).apply()
        true
    }

    private fun list(key: String): List<String> {
        val arr = try { JSONArray(prefs.getString(key, "[]")) } catch (_: Exception) { JSONArray() }
        return (0 until arr.length()).map { arr.optString(it, "") }
    }

    private fun contains(key: String, value: String): Boolean = value in list(key)

    private fun remember(key: String, cap: Int, value: String): Boolean {
        val seen = list(key)
        if (value in seen) return false
        prefs.edit().putString(key, JSONArray((seen + value).takeLast(cap)).toString()).apply()
        return true
    }

    /**
     * The device pubkeys the console has told us about (paired, and released
     * within the grace period), or null when it has never pushed a roster (an
     * older console, or before first launch). Null means "unknown, so treat
     * everything as known": failing towards keeping. NOTE an empty roster is
     * returned as an empty set; callers that judge a sender by it must treat
     * empty like null (see [knownRoster]).
     */
    fun knownMachines(): Set<String>? =
        if (prefs.contains(K_ROSTER)) roster().map { it.machine }.toSet() else null

    /**
     * The roster to judge a sender by: null when it is absent OR empty. An empty
     * roster (a console that booted without its household) is not evidence that
     * any sender is a stranger.
     */
    fun knownRoster(): Set<String>? = knownMachines()?.takeIf { it.isNotEmpty() }

    /** The machines the roster names only as released (disconnected in the last 14 days). */
    fun releasedMachines(): Set<String> {
        val r = roster()
        return r.filter { it.released }.map { it.machine }.filter { isReleased(it, r) }.toSet()
    }

    /** Hold a relay-delivered emergency audit until the console consumes it.
     *  Synchronous (never `apply()`), so it is on disk before the alert. Throws on failure.
     *  An entry admitted with no usable roster stays unflagged and follows the next
     *  non-empty one; it is never stamped known. A released machine's entries have
     *  their own bucket, so they never evict a paired ward's. */
    fun spoolInboxWrap(id: String, machine: String, wrapJson: String, nowSecs: Long) =
        spool.add(id, machine, wrapJson, nowSecs, knownRoster(), releasedMachines())

    /** The spooled entries, as the console's inbox shape. Throws when the file
     *  cannot be read (an I/O error, which is never taken for damage) or a damaged
     *  one cannot be set aside or rewritten (the bridge degrades that to "link
     *  only"); a file whose text will not parse is set aside and salvaged. */
    fun inboxSpool(): String = spool.read()

    /** The console has persisted these ids (a JSON array of strings): forget them. */
    fun consumeInboxSpool(idsJson: String) = spool.remove(idsJson)

    /**
     * The local link's DNS-SD instance name, `Kintrinsic <4 hex>`: random,
     * chosen once per install and kept, so wards and network tools see one
     * stable name that says nothing about the family.
     */
    @Synchronized
    fun linkInstanceName(): String {
        prefs.getString(K_LINK_NAME, null)?.takeIf { LinkNames.isInstanceName(it) }?.let { return it }
        val bytes = ByteArray(2).also { SecureRandom().nextBytes(it) }
        val name = LinkNames.instanceName(bytes)
        prefs.edit().putString(K_LINK_NAME, name).apply()
        return name
    }

    fun clear() = prefs.edit().clear().apply()

    companion object {
        private const val K_SK = "guardianSkHex"
        private const val K_PK = "guardianPubkeyHex"
        private const val K_RELAYS = "relays"
        private const val K_ROSTER = "roster"
        private const val K_SEEN = "seenReqIds"
        private const val K_SEEN_AUDIT = "seenAuditKeys"
        private const val K_UNPAIRED = "unpairedWraps"
        private const val K_UNPAIRED_SINCE = "unpairedSinceDismissed"
        private const val K_UNPAIRED_ASKS = "unpairedAskIds"
        private const val K_UNPAIRED_ASKS_SINCE = "unpairedAsksSinceDismissed"
        private const val K_UNPAIRED_SOUND = "unpairedSoundAt"
        /** One lock for the counters: the service and the dismissal receiver each own a [CarrierStore]. */
        private val LOCK = Any()
        private const val K_LINK_NAME = "linkInstanceName"
        private const val SEEN_CAP = 200
        private const val AUDIT_SEEN_CAP = 200
        /** Most distinct unpaired-sender reports remembered (and the most the alert counts). */
        const val UNPAIRED_CAP = 50

        /** The shared stranger emergency row sounds at most once in this long. */
        const val UNPAIRED_SOUND_SECS = 60L * 60
    }
}
