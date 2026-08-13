package org.forgesworn.charter.enforce.hotspot

import org.forgesworn.charter.enforce.dns.DnsDecision
import org.forgesworn.charter.enforce.dns.DnsQuestion
import org.forgesworn.charter.enforce.dns.DnsResolver
import org.forgesworn.charter.enforce.dns.TYPE_A
import org.forgesworn.charter.native.CharterCore.DnsPlan

/** What the Kintrinsic Hotspot does with one guest CONNECT target. */
sealed class HotspotVerdict {
    /** Block the guest's request — the host is off-policy. */
    object Refuse : HotspotVerdict()
    /** Tunnel to [host] (the requested host, or a SafeSearch rewrite target). */
    data class Dial(val host: String) : HotspotVerdict()
}

/**
 * The Kintrinsic Hotspot doorman: filters guest traffic through the SAME policy as
 * the on-phone DNS filter, by delegating the per-host decision to [DnsResolver].
 * One source of truth — the guest network can never be more permissive than the
 * ward's own device. Fail-closed: a blocked host (or unknown mode) is Refused.
 *
 * A `Rewrite` becomes `Dial(rewriteTarget)` — the CONNECT tunnel goes to the
 * SafeSearch host, which serves a certificate valid for the original SNI, so
 * forced-SafeSearch is enforced for guests exactly as the DNS rewrite enforces
 * it on the phone.
 *
 * **IP-literal targets fail closed.** A raw IPv4/IPv6 destination can never be
 * matched against a domain policy, so a guest could otherwise defeat a blocklist
 * by connecting to a hard-coded / out-of-band-resolved IP. Such targets are
 * Refused unless the guardian imposes no web constraint at all (`unrestricted`).
 */
class HotspotFilter(plan: DnsPlan) {
    private val resolver = DnsResolver(plan)
    private val mode = plan.mode

    fun verdict(host: String): HotspotVerdict {
        // Normalize BEFORE any policy decision: strip an IPv6 literal's brackets
        // and any trailing dot(s). A trailing-dot FQDN ("bad.example.") resolves
        // exactly like the bare name but matches NEITHER the blocklist (the
        // resolver compares against dot-free entries) NOR the IP-literal shape —
        // so without this a guest could dodge the whole filter with one extra dot
        // (`bad.example.` or `1.2.3.4.`). The normalized host is what we also hand
        // back to Dial, so the proxy connects to exactly what we vetted.
        val h = host.removeSurrounding("[", "]").trimEnd('.')
        if (h.isEmpty()) return HotspotVerdict.Refuse
        if (isIpLiteral(h)) {
            // Can't be domain-filtered; only safe when nothing is being filtered.
            return if (mode == "unrestricted") HotspotVerdict.Dial(h) else HotspotVerdict.Refuse
        }
        return when (val d = resolver.decide(DnsQuestion(h, TYPE_A, 1, ByteArray(0)))) {
            is DnsDecision.Block -> HotspotVerdict.Refuse
            is DnsDecision.PassThrough -> HotspotVerdict.Dial(h)
            is DnsDecision.Rewrite -> HotspotVerdict.Dial(d.target)
        }
    }

    /** True for an IPv4 or IPv6 literal — pure (no DNS lookup), so it is unit-
     *  testable off-device. Domains never contain ':', so any colon ⇒ IPv6. */
    private fun isIpLiteral(host: String): Boolean {
        if (host.contains(':')) return true // IPv6 literal
        val octets = host.split('.')
        return octets.size == 4 &&
            octets.all { o -> o.isNotEmpty() && o.toIntOrNull()?.let { it in 0..255 } == true }
    }
}
