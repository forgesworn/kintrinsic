package org.forgesworn.mycharter.carrier

import org.json.JSONObject

/**
 * One `(device pubkey → child name, device label)` pair, pushed PWA → carrier
 * over `CarrierBridge.roster` so [org.forgesworn.mycharter.service.Notifier]
 * can name the ward instead of saying "Your ward" (design memo 2026-08-04,
 * part 1). The PWA already knows every child's devices; the carrier does not
 * — it only ever sees a device's pubkey.
 */
data class RosterEntry(val machine: String, val childName: String, val deviceLabel: String)

/** What a resolved roster hit gives [org.forgesworn.mycharter.service.Notifier]
 *  to compose text with. */
data class WardName(val childName: String, val deviceLabel: String)

/**
 * The web console's roster push. Strict like [ProvisionPayload]: `v == 1` and
 * every entry non-empty, or the WHOLE push is rejected — never a
 * half-parsed roster silently missing entries. Rejection means "keep
 * whatever was there before", not "wipe it" — see [CarrierBridge.roster].
 */
data class RosterPayload(val entries: List<RosterEntry>) {
    companion object {
        fun parse(json: String): RosterPayload? {
            val o = try { JSONObject(json) } catch (_: Exception) { return null }
            if (o.optInt("v", 0) != 1) return null
            val arr = o.optJSONArray("entries") ?: return null
            val entries = (0 until arr.length()).map { i ->
                val e = arr.optJSONObject(i) ?: return null
                val machine = e.optString("machine", "")
                val childName = e.optString("childName", "")
                val deviceLabel = e.optString("deviceLabel", "")
                if (machine.isEmpty() || childName.isEmpty() || deviceLabel.isEmpty()) return null
                RosterEntry(machine, childName, deviceLabel)
            }
            return RosterPayload(entries)
        }
    }
}

/**
 * Pure lookup — no [android.content.Context], so it runs in the plain JVM
 * unit suite. An empty/unknown machine or a roster with no matching entry
 * both return null: the caller falls back to today's generic wording, NEVER
 * a guessed name — the roster is a convenience, never a correctness
 * dependency (standing rule against fabricated user-facing values).
 */
fun describeWard(machine: String, roster: List<RosterEntry>): WardName? {
    if (machine.isEmpty()) return null
    val hit = roster.firstOrNull { it.machine == machine } ?: return null
    return WardName(hit.childName, hit.deviceLabel)
}
