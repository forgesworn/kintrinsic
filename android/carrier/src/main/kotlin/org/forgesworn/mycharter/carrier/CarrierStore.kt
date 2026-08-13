package org.forgesworn.mycharter.carrier

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

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
class CarrierStore(context: Context) {
    private val prefs = context.getSharedPreferences("carrier", Context.MODE_PRIVATE)

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
                    .put("deviceLabel", e.deviceLabel),
            )
        }
        prefs.edit().putString(K_ROSTER, arr.toString()).apply()
    }

    /** Empty (never null, never throws) when absent or unreadable — the
     *  roster is a convenience, so a storage hiccup degrades it to "no
     *  names", not a [Notifier] crash. */
    fun roster(): List<RosterEntry> {
        val json = prefs.getString(K_ROSTER, null) ?: return emptyList()
        val arr = try { JSONArray(json) } catch (_: Exception) { return emptyList() }
        return (0 until arr.length()).mapNotNull { i ->
            val o = arr.optJSONObject(i) ?: return@mapNotNull null
            val machine = o.optString("machine", "")
            val childName = o.optString("childName", "")
            val deviceLabel = o.optString("deviceLabel", "")
            if (machine.isEmpty() || childName.isEmpty() || deviceLabel.isEmpty()) {
                null
            } else {
                RosterEntry(machine, childName, deviceLabel)
            }
        }
    }

    /** True if this reqId is NEW (recorded now); false if already seen. */
    @Synchronized
    fun markSeen(reqId: String): Boolean {
        val arr = try { JSONArray(prefs.getString(K_SEEN, "[]")) } catch (_: Exception) { JSONArray() }
        val seen = (0 until arr.length()).map { arr.optString(it, "") }
        if (reqId in seen) return false
        val next = (seen + reqId).takeLast(SEEN_CAP)
        prefs.edit().putString(K_SEEN, JSONArray(next).toString()).apply()
        return true
    }

    fun clear() = prefs.edit().clear().apply()

    private companion object {
        const val K_SK = "guardianSkHex"
        const val K_PK = "guardianPubkeyHex"
        const val K_RELAYS = "relays"
        const val K_ROSTER = "roster"
        const val K_SEEN = "seenReqIds"
        const val SEEN_CAP = 200
    }
}
