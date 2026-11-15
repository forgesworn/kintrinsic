package org.forgesworn.mycharter.carrier

import org.json.JSONArray
import org.json.JSONObject

/**
 * The carrier's own hold for emergency audits that arrived over a RELAY.
 *
 * A wrap that came over the home-network link waits in the native inbox until
 * the console has persisted it. A relay wrap used to be classified, alerted on
 * and forgotten, so a guardian who dismissed the alert and opened the console
 * more than a relay-window later never saw the press in Activity. Every
 * `outcome=override` wrap (any op) is therefore kept here, in the same
 * `{id, machine, wrap, receivedAt}` shape the console already drains with
 * `localInbox` / `localConsume`, until it consumes it. Consume-after-persist
 * and the console's `(machine, at)` folding handle every duplicate.
 *
 * Each entry also carries `verifiedAt`: the moment THIS carrier checked the wrap
 * (seal, author, two-day freshness) before holding it. The console then checks
 * freshness against that moment rather than against whenever the guardian next
 * opens it, so a press still lands days later.
 *
 * Because any key can send a validly sealed audit to the guardian's public
 * key, entries are bounded so a stranger cannot push out a real press: each
 * entry is capped in size, and machines the console does not know have their
 * own small share ([UNKNOWN_CAP]) that can never evict a known machine's
 * entries ([KNOWN_CAP]). Before the console has said which machines it knows,
 * every machine counts as known (fail towards keeping).
 *
 * Pure (no Android types) so it is host-testable. Persistence is [SpoolFile]'s.
 */
object InboxSpool {
    /** Most entries kept for machines the console knows. */
    const val KNOWN_CAP = 200

    /** Most entries kept for machines the console has not paired (oldest out). */
    const val UNKNOWN_CAP = 10

    /** Largest wrap kept, in bytes. A real audit wrap is 1-2 KB. */
    const val MAX_ENTRY_BYTES = 16 * 1024

    /** How long an unconsumed entry is held. The console checks freshness
     *  against `verifiedAt`, so this is a storage limit, not a validity one. */
    const val MAX_AGE_SECS = 14L * 24 * 60 * 60

    private fun parse(json: String?): JSONArray =
        try { JSONArray(json ?: "[]") } catch (_: Exception) { JSONArray() }

    private fun entries(arr: JSONArray): List<JSONObject> =
        (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }

    /**
     * [spool] with the wrap added. Unchanged when the id is already held (a
     * relay replay is a no-op), the wrap is not a JSON object, or it is over
     * [MAX_ENTRY_BYTES]. Entries older than [MAX_AGE_SECS] are dropped, then
     * each group is capped (oldest out): known machines at [KNOWN_CAP],
     * unknown at [UNKNOWN_CAP]. `known == null` means the roster has not been
     * pushed yet and everything counts as known.
     */
    fun add(
        spool: String?,
        id: String,
        machine: String,
        wrapJson: String,
        nowSecs: Long,
        known: Set<String>? = null,
    ): String {
        val current = spool ?: "[]"
        if (wrapJson.toByteArray(Charsets.UTF_8).size > MAX_ENTRY_BYTES) return current
        val wrap = try { JSONObject(wrapJson) } catch (_: Exception) { return current }
        val kept = entries(parse(spool)).filter {
            nowSecs - it.optLong("receivedAt", nowSecs) <= MAX_AGE_SECS
        }
        if (kept.any { it.optString("id") == id }) return cap(kept, known)
        val entry = JSONObject()
            .put("id", id)
            .put("machine", machine)
            .put("wrap", wrap)
            .put("receivedAt", nowSecs)
            .put("verifiedAt", nowSecs)
        return cap(kept + entry, known)
    }

    private fun isKnown(machine: String, known: Set<String>?): Boolean =
        known == null || machine in known

    /** Apply the two caps, keeping order and dropping the oldest of each group. */
    internal fun cap(all: List<JSONObject>, known: Set<String>?): String {
        val knownEntries = all.filter { isKnown(it.optString("machine"), known) }
        val unknownEntries = all.filterNot { isKnown(it.optString("machine"), known) }
        val keep = HashSet<JSONObject>(
            knownEntries.takeLast(KNOWN_CAP) + unknownEntries.takeLast(UNKNOWN_CAP),
        )
        return JSONArray(all.filter { it in keep }).toString()
    }

    /** [spool] without the entries whose id is in [idsJson] (a JSON array of strings). */
    fun remove(spool: String?, idsJson: String): String {
        val arr = try { JSONArray(idsJson) } catch (_: Exception) { return spool ?: "[]" }
        val ids = (0 until arr.length()).map { arr.optString(it, "") }.toSet()
        return JSONArray(entries(parse(spool)).filter { it.optString("id") !in ids }).toString()
    }

    /** The native inbox followed by every spooled entry whose id it does not already hold. */
    fun merge(nativeInbox: String, spool: String?): String {
        val native = parse(nativeInbox)
        val held = entries(native).map { it.optString("id") }.toSet()
        val out = JSONArray()
        for (i in 0 until native.length()) out.put(native.get(i))
        for (e in entries(parse(spool))) if (e.optString("id") !in held) out.put(e)
        return out.toString()
    }
}
