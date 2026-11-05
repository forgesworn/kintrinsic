package org.forgesworn.mycharter.carrier

import org.json.JSONObject

/** One key the carrier listens on: wraps p-tagged to [pubkeyHex] are
 *  unwrapped with [skHex]. */
data class CarrierKey(val skHex: String, val pubkeyHex: String)

/**
 * The key hand-off from the web console (CarrierBridge). Strict: v==1,
 * 64-char lowercase hex keys, ≥1 wss:// relay — anything else is null
 * (never a partial parse; a bad hand-off must not half-provision).
 *
 * `guardianSkHex` / `guardianPubkeyHex` is the primary key (the one the
 * relays' NIP-42 AUTH and the local link use). A phone-held root that has
 * minted a working key (D5) is written to on both: wards write to the
 * working key once they list it, and to the root before then. The page then
 * also sends `keys: [{skHex, pubkeyHex}, …]`, the further keys to listen on;
 * a payload without `keys` (every console before this) is the primary alone.
 */
data class ProvisionPayload(
    val guardianSkHex: String,
    val guardianPubkeyHex: String,
    val relays: List<String>,
    /** Further keys beyond the primary, never repeating it. */
    val extraKeys: List<CarrierKey> = emptyList(),
) {
    /** Every key listened on, the primary first. */
    val keys: List<CarrierKey>
        get() = listOf(CarrierKey(guardianSkHex, guardianPubkeyHex)) + extraKeys

    /** The pubkeys a subscription names in `#p`, the primary first. */
    val pubkeys: List<String>
        get() = keys.map { it.pubkeyHex }

    /** The secret for a wrap p-tagged to [pubkeyHex]; the primary's when the
     *  wrap names none of ours (or names none at all). */
    fun secretFor(pubkeyHex: String?): String =
        keys.firstOrNull { it.pubkeyHex == pubkeyHex }?.skHex ?: guardianSkHex

    companion object {
        private val HEX64 = Regex("^[0-9a-f]{64}$")

        /** A root and its working keys: more than this is a bad hand-off. */
        const val MAX_KEYS = 4

        fun parse(json: String): ProvisionPayload? {
            val o = try { JSONObject(json) } catch (_: Exception) { return null }
            if (o.optInt("v", 0) != 1) return null
            val sk = o.optString("guardianSkHex", "")
            val pk = o.optString("guardianPubkeyHex", "")
            if (!HEX64.matches(sk) || !HEX64.matches(pk)) return null
            val arr = o.optJSONArray("relays") ?: return null
            val relays = (0 until arr.length()).map { arr.optString(it, "") }
            if (relays.isEmpty() || relays.any { !it.startsWith("wss://") }) return null
            val extra = if (o.has("keys")) parseKeys(o.optJSONArray("keys") ?: return null) ?: return null else emptyList()
            return of(sk, pk, relays, extra)
        }

        /** The `keys` list, or null if any entry is malformed. */
        fun parseKeys(arr: org.json.JSONArray): List<CarrierKey>? {
            if (arr.length() > MAX_KEYS) return null
            return (0 until arr.length()).map { i ->
                val k = arr.optJSONObject(i) ?: return null
                val ksk = k.optString("skHex", "")
                val kpk = k.optString("pubkeyHex", "")
                if (!HEX64.matches(ksk) || !HEX64.matches(kpk)) return null
                CarrierKey(ksk, kpk)
            }
        }

        /** The payload with [extra] cut to the keys that are not the primary
         *  and not repeated; null if one pubkey comes with two secrets. */
        fun of(sk: String, pk: String, relays: List<String>, extra: List<CarrierKey>): ProvisionPayload? {
            val all = listOf(CarrierKey(sk, pk)) + extra
            if (all.groupBy { it.pubkeyHex }.values.any { same -> same.map { it.skHex }.toSet().size > 1 }) {
                return null
            }
            val rest = extra.filter { it.pubkeyHex != pk }.distinctBy { it.pubkeyHex }
            if (rest.size + 1 > MAX_KEYS) return null
            return ProvisionPayload(sk, pk, relays, rest)
        }
    }
}
