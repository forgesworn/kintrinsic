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
 * and the console's `(machine, at)` folding handle every duplicate. An audit
 * with `op=call.emergency` is held whatever its outcome.
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
 * every machine counts as known (fail towards keeping); an EMPTY roster says
 * nothing either, so it is treated the same way: entries admitted under it carry
 * no flag and follow the next non-empty roster.
 *
 * Once the console consumes an entry its id is kept as a tombstone for
 * [TOMBSTONE_SECS] (relays replay 48 h of wraps on every reconnect), so a replay
 * is not written back into the spool only to be drained and consumed again.
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

    /** How long a consumed id is remembered: the relays' replay window. */
    const val TOMBSTONE_SECS = 48L * 60 * 60

    /** Most tombstones kept (oldest out), so the file stays bounded. */
    const val MAX_TOMBSTONES = 1000

    private fun parse(json: String?): JSONArray =
        try { JSONArray(json ?: "[]") } catch (_: Exception) { JSONArray() }

    private fun entries(arr: JSONArray): List<JSONObject> =
        (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }

    /**
     * [spool] with the wrap added. Unchanged when the id is already held (a
     * relay replay is a no-op), the wrap is not a JSON object, or it is over
     * [MAX_ENTRY_BYTES]. Entries older than [MAX_AGE_SECS] are dropped, then
     * each group is capped (oldest out): known machines at [KNOWN_CAP],
     * unknown at [UNKNOWN_CAP]. `known == null` (or empty) means no usable roster
     * has been pushed and everything counts as known. An id in [consumed] is
     * never re-admitted.
     */
    fun add(
        spool: String?,
        id: String,
        machine: String,
        wrapJson: String,
        nowSecs: Long,
        known: Set<String>? = null,
        consumed: Set<String> = emptySet(),
    ): String {
        val roster = known?.takeIf { it.isNotEmpty() }
        val current = spool ?: "[]"
        if (wrapJson.toByteArray(Charsets.UTF_8).size > MAX_ENTRY_BYTES) return current
        val wrap = try { JSONObject(wrapJson) } catch (_: Exception) { return current }
        val kept = entries(parse(spool)).filter {
            nowSecs - it.optLong("receivedAt", nowSecs) <= MAX_AGE_SECS
        }
        if (id in consumed || kept.any { it.optString("id") == id }) return cap(kept, roster)
        val entry = JSONObject()
            .put("id", id)
            .put("machine", machine)
            .put("wrap", wrap)
            .put("receivedAt", nowSecs)
            .put("verifiedAt", nowSecs)
        // Decided ONCE, now, against the roster of the moment, and kept with the
        // entry: a later roster (even an empty one, from a console that booted
        // without its household) never moves a held press into the small group
        // or evicts it. No roster yet means no flag, and the entry is judged by
        // whatever roster is current when the caps next run.
        if (roster != null) entry.put("known", machine in roster)
        return cap(kept + entry, roster)
    }

    /** The admission-time verdict when the entry has one, else the current roster's. */
    private fun isKnown(e: JSONObject, known: Set<String>?): Boolean =
        if (e.has("known")) e.optBoolean("known", true) else known == null || e.optString("machine") in known

    /** Apply the two caps, keeping order and dropping the oldest of each group. */
    internal fun cap(all: List<JSONObject>, rosterNow: Set<String>?): String {
        val known = rosterNow?.takeIf { it.isNotEmpty() }
        val knownEntries = all.filter { isKnown(it, known) }
        val unknownEntries = all.filterNot { isKnown(it, known) }
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

    // ---- tombstones and the file's document -------------------------------------

    /** [consumed] plus every id in [idsJson] stamped [nowSecs], with the old and the surplus dropped. */
    fun tombstone(consumed: Map<String, Long>, idsJson: String, nowSecs: Long): Map<String, Long> {
        val arr = try { JSONArray(idsJson) } catch (_: Exception) { return pruneTombstones(consumed, nowSecs) }
        val next = LinkedHashMap(consumed)
        for (i in 0 until arr.length()) {
            val id = arr.optString(i, "")
            if (id.isNotEmpty()) { next.remove(id); next[id] = nowSecs }
        }
        return pruneTombstones(next, nowSecs)
    }

    /** Drop tombstones older than [TOMBSTONE_SECS], then the oldest beyond [MAX_TOMBSTONES]. */
    fun pruneTombstones(consumed: Map<String, Long>, nowSecs: Long): Map<String, Long> =
        consumed.entries
            .filter { nowSecs - it.value <= TOMBSTONE_SECS }
            .takeLast(MAX_TOMBSTONES)
            .associate { it.key to it.value }

    /** What the spool file holds: the held entries (a JSON array) and the tombstones. */
    class Doc(val entries: String, val consumed: Map<String, Long>)

    /** The file text for [doc]: `{"v":2,"entries":[…],"consumed":{"<id>":<unix>}}`. */
    fun encode(doc: Doc): String =
        JSONObject()
            .put("v", 2)
            .put("entries", parse(doc.entries))
            .put("consumed", JSONObject().also { o -> doc.consumed.forEach { (k, v) -> o.put(k, v) } })
            .toString()

    /**
     * The document in [text], or null when it is damaged. A bare JSON array is the
     * older format (entries, no tombstones) and still reads. An object needs an
     * `entries` array; `consumed` is optional and a malformed one is read as empty.
     */
    fun decode(text: String): Doc? {
        try { JSONArray(text); return Doc(text, emptyMap()) } catch (_: Exception) { }
        val o = try { JSONObject(text) } catch (_: Exception) { return null }
        val entries = o.optJSONArray("entries") ?: return null
        val c = o.optJSONObject("consumed")
        val consumed = LinkedHashMap<String, Long>()
        c?.keys()?.forEach { k -> c.optLong(k, -1L).takeIf { it >= 0 }?.let { consumed[k] = it } }
        return Doc(entries.toString(), consumed)
    }

    /**
     * Every complete entry that can still be read out of a damaged [text] (a
     * truncated or partly overwritten file), as a JSON array in file order,
     * duplicates dropped and the caps applied. An entry is an object with a
     * non-empty `id`, a `machine` and a `wrap` object; anything else is skipped.
     */
    fun salvage(text: String): String {
        val found = LinkedHashMap<String, JSONObject>()
        val starts = ArrayList<Int>()
        var inString = false
        var escaped = false
        for (i in text.indices) {
            val c = text[i]
            if (inString) {
                if (escaped) escaped = false else if (c == '\\') escaped = true else if (c == '"') inString = false
                continue
            }
            when (c) {
                '"' -> inString = true
                '{' -> starts.add(i)
                '}' -> {
                    val start = starts.removeLastOrNull() ?: continue
                    if (i + 1 - start > MAX_ENTRY_BYTES + 1024) continue
                    val o = try { JSONObject(text.substring(start, i + 1)) } catch (_: Exception) { continue }
                    val id = o.optString("id", "")
                    if (id.isNotEmpty() && o.has("machine") && o.optJSONObject("wrap") != null) found.putIfAbsent(id, o)
                }
            }
        }
        return cap(found.values.toList(), null)
    }

    /**
     * The native inbox, then every spooled entry it does not hold. When BOTH hold
     * the same wrap id the SPOOLED entry is emitted, in the native entry's place:
     * it carries `verifiedAt` (the bytes the carrier itself classified), so a
     * press that also came over the home network still lands days later.
     */
    fun merge(nativeInbox: String, spool: String?): String {
        val native = parse(nativeInbox)
        val spooled = entries(parse(spool)).associateBy { it.optString("id") }
        val out = JSONArray()
        val emitted = HashSet<String>()
        for (i in 0 until native.length()) {
            val n = native.optJSONObject(i)
            val id = n?.optString("id")
            val replacement = if (id != null) spooled[id] else null
            out.put(replacement ?: native.get(i))
            if (id != null) emitted.add(id)
        }
        for ((id, e) in spooled) if (id !in emitted) out.put(e)
        return out.toString()
    }
}
