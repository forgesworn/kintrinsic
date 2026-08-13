package org.forgesworn.mycharter.carrier

import org.json.JSONObject

/**
 * The key hand-off from the web console (CarrierBridge). Strict: v==1,
 * 64-char lowercase hex keys, ≥1 wss:// relay — anything else is null
 * (never a partial parse; a bad hand-off must not half-provision).
 */
data class ProvisionPayload(
    val guardianSkHex: String,
    val guardianPubkeyHex: String,
    val relays: List<String>,
) {
    companion object {
        private val HEX64 = Regex("^[0-9a-f]{64}$")

        fun parse(json: String): ProvisionPayload? {
            val o = try { JSONObject(json) } catch (_: Exception) { return null }
            if (o.optInt("v", 0) != 1) return null
            val sk = o.optString("guardianSkHex", "")
            val pk = o.optString("guardianPubkeyHex", "")
            if (!HEX64.matches(sk) || !HEX64.matches(pk)) return null
            val arr = o.optJSONArray("relays") ?: return null
            val relays = (0 until arr.length()).map { arr.optString(it, "") }
            if (relays.isEmpty() || relays.any { !it.startsWith("wss://") }) return null
            return ProvisionPayload(sk, pk, relays)
        }
    }
}
