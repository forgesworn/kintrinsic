package org.forgesworn.mycharter.relay

import org.json.JSONArray
import org.json.JSONObject

/**
 * Minimal NIP-01 client framing for the carrier's one job: subscribe to
 * gift-wraps p-tagged to the guardian. Parsing is defensive — a relay frame
 * we don't understand is null, never a throw in the websocket callback.
 */
object RelayFraming {
    /**
     * NIP-59 randomizes wrap `created_at` up to ~2 days into the past, so a
     * tight `since` silently loses jittered wraps. Reach back 48 h on every
     * (re)subscribe; the persisted seen-reqId LRU makes redelivery free.
     */
    const val SINCE_WINDOW_SECS = 172_800L

    fun reqMessage(subId: String, guardianPubkeyHex: String, sinceUnix: Long): String =
        reqMessage(subId, listOf(guardianPubkeyHex), sinceUnix)

    /**
     * One subscription for gift-wraps p-tagged to ANY of [pubkeysHex].
     * Repeats are dropped; the order is kept. The carrier's sockets each name
     * one key (see [RelayRoute]): a relay that hands wraps only to the
     * signed-in recipient may refuse a filter naming a key it has not seen
     * sign in on that connection.
     */
    fun reqMessage(subId: String, pubkeysHex: List<String>, sinceUnix: Long): String =
        JSONArray()
            .put("REQ")
            .put(subId)
            .put(
                JSONObject()
                    .put("kinds", JSONArray().put(1059))
                    .put("#p", JSONArray(pubkeysHex.distinct()))
                    .put("since", sinceUnix),
            )
            .toString()

    /** The 64-hex key a gift-wrap's first `p` tag names, or null: which of
     *  the carrier's keys the wrap is encrypted to. */
    fun recipient(eventJson: String): String? {
        val tags = try { JSONObject(eventJson).optJSONArray("tags") } catch (_: Exception) { return null } ?: return null
        for (i in 0 until tags.length()) {
            val t = tags.optJSONArray(i) ?: continue
            if (t.optString(0) == "p") return t.optString(1).takeIf { it.length == 64 }
        }
        return null
    }

    fun parseEvent(frame: String): String? {
        val arr = try { JSONArray(frame) } catch (_: Exception) { return null }
        if (arr.length() < 3 || arr.optString(0) != "EVENT") return null
        return arr.optJSONObject(2)?.toString()
    }

    /**
     * The outer wrap's event id — the one handle on a delivery that is stable
     * across relays AND redeliveries (the ward builds one wrap and publishes it
     * everywhere), so it is what redelivery dedupe keys on. Null for anything
     * that isn't a 64-hex id, so a caller can fall back to fail-loud.
     */
    fun eventId(eventJson: String): String? {
        val id = try { JSONObject(eventJson).optString("id", "") } catch (_: Exception) { return null }
        return id.takeIf { it.length == 64 }
    }
}
