package org.forgesworn.charter.enforce.dns

import org.forgesworn.charter.native.CharterCore.DnsPlan

/** What to do with one DNS query, decided purely from the plan. */
sealed class DnsDecision {
    /** Answer NXDOMAIN — the domain is blocked. */
    object Block : DnsDecision()
    /** Resolve `target` upstream and answer under the queried name (SafeSearch/YouTube). */
    data class Rewrite(val target: String) : DnsDecision()
    /** Relay the query upstream unchanged. */
    object PassThrough : DnsDecision()
}

/**
 * Decides each DNS query from the effective plan. Precedence, top-down:
 *   1. rewrite (forced SafeSearch / YouTube) — even in locked/allowlist modes a
 *      rewrite target is a controlled host, so honoring it first is safe and
 *      keeps search working under a tight policy;
 *   2. explicit exception (parent-allow) — overrides a block;
 *   3. mode: locked ⇒ block; allowlist ⇒ pass only allow-listed (+subdomains);
 *      blocklist ⇒ block listed (+subdomains); unrestricted ⇒ pass.
 * Any unrecognized mode fails CLOSED (block) — future-proofing against version
 * skew between the Rust DnsMode enum and this APK; the shipped enum is a closed
 * 4-variant set, so this fallback is unreachable today.
 * Domain matching is suffix-aware: "bad.example" also blocks "x.bad.example".
 */
class DnsResolver(private val plan: DnsPlan) {
    private val rewrites = plan.rewrites.associate { it.host.lowercase() to it.answer }

    fun decide(q: DnsQuestion): DnsDecision {
        val name = q.name.lowercase()
        rewrites[name]?.let { return DnsDecision.Rewrite(it) }
        if (matches(name, plan.allowExceptions)) return DnsDecision.PassThrough
        return when (plan.mode) {
            "locked" -> DnsDecision.Block
            "allowlist" -> if (matches(name, plan.allowDomains)) DnsDecision.PassThrough else DnsDecision.Block
            "blocklist" -> if (matches(name, plan.blockDomains)) DnsDecision.Block else DnsDecision.PassThrough
            "unrestricted" -> DnsDecision.PassThrough
            else -> DnsDecision.Block // unknown mode -> fail closed (version skew)
        }
    }

    /** True if `name` equals or is a subdomain of any entry in `set`. */
    private fun matches(name: String, set: List<String>): Boolean =
        set.any { d ->
            val dl = d.lowercase()
            name == dl || name.endsWith(".$dl")
        }
}
