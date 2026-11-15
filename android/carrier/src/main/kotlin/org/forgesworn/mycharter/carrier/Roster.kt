package org.forgesworn.mycharter.carrier

import org.json.JSONObject

/**
 * One `(device pubkey → child name, device label)` pair, pushed PWA → carrier
 * over `CarrierBridge.roster` so [org.forgesworn.mycharter.service.Notifier]
 * can name the ward instead of saying "Your ward" (design memo 2026-08-04,
 * part 1). The PWA already knows every child's devices; the carrier does not
 * — it only ever sees a device's pubkey.
 */
data class RosterEntry(
    val machine: String,
    val childName: String,
    val deviceLabel: String,
    /** Only paired devices are admitted to the local link's handshake.
     *  Absent on the wire (an older console) means paired; see [pairedField]. */
    val paired: Boolean = true,
    /** A device the guardian disconnected (unpaired, removed, or its child removed)
     *  in the last 14 days. The console keeps it in the roster for that grace
     *  period so the carrier can still name it and hold its audits as known; the
     *  local link never admits it. A released entry is never [paired]. */
    val released: Boolean = false,
)

/**
 * The roster's `paired` flag. Absent means paired (consoles older than the
 * local link never sent it); anything present that is not the JSON boolean
 * `true` means NOT paired. This feeds the local link's admission list, so a
 * malformed value must deny, never admit.
 */
fun pairedField(o: JSONObject): Boolean = !o.has("paired") || o.opt("paired") == true

/**
 * The roster's `released` flag. Absent means false; anything present that is not
 * the JSON boolean `true` is also false (a malformed value never promotes an
 * entry, and admission is decided by [pairedField], which a release overrides).
 */
fun releasedField(o: JSONObject): Boolean = o.opt("released") == true

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
                val released = releasedField(e)
                // A released device is never admitted, whatever `paired` says.
                RosterEntry(machine, childName, deviceLabel, paired = pairedField(e) && !released, released = released)
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
    // A live pairing outranks a released one for the same machine.
    val hit = roster.firstOrNull { it.machine == machine && !it.released }
        ?: roster.firstOrNull { it.machine == machine }
        ?: return null
    return WardName(hit.childName, hit.deviceLabel)
}

/**
 * True when the roster names [machine] only as a RELEASED device: the guardian
 * disconnected it, so its press happened before that. A machine that is also
 * paired (re-paired since) is not released.
 */
fun isReleased(machine: String, roster: List<RosterEntry>): Boolean {
    if (machine.isEmpty()) return false
    val hits = roster.filter { it.machine == machine }
    return hits.isNotEmpty() && hits.all { it.released }
}
