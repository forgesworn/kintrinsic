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
 * Pure (no Android types) so it is host-testable. Persistence is
 * [CarrierStore]'s.
 */
object InboxSpool {
    /** Most entries kept; the oldest go first. */
    const val CAP = 50

    /** An entry older than this can no longer be read by the console (the wrap
     *  jitter window is two days), so it is dropped rather than kept forever. */
    const val MAX_AGE_SECS = 3L * 24 * 60 * 60

    private fun parse(json: String?): JSONArray =
        try { JSONArray(json ?: "[]") } catch (_: Exception) { JSONArray() }

    private fun entries(arr: JSONArray): List<JSONObject> =
        (0 until arr.length()).mapNotNull { arr.optJSONObject(it) }

    /**
     * [spool] with the wrap added. Unchanged when the id is already held (a
     * relay replay is a no-op) or the wrap is not a JSON object. Entries older
     * than [MAX_AGE_SECS] are dropped and the list is capped at [CAP].
     */
    fun add(spool: String?, id: String, machine: String, wrapJson: String, nowSecs: Long): String {
        val wrap = try { JSONObject(wrapJson) } catch (_: Exception) { return spool ?: "[]" }
        val kept = entries(parse(spool)).filter {
            nowSecs - it.optLong("receivedAt", nowSecs) <= MAX_AGE_SECS
        }
        if (kept.any { it.optString("id") == id }) return JSONArray(kept).toString()
        val entry = JSONObject()
            .put("id", id)
            .put("machine", machine)
            .put("wrap", wrap)
            .put("receivedAt", nowSecs)
        return JSONArray((kept + entry).takeLast(CAP)).toString()
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
