package org.forgesworn.charter.enforce.hotspot

import org.forgesworn.charter.native.CharterCore.DnsPlan
import org.forgesworn.charter.native.CharterCore.DnsRewrite
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The Kintrinsic Hotspot doorman verdict — the guest-traffic filter for the
 * app-hosted local-only AP. It reuses the SAME DnsResolver the on-phone DNS
 * filter uses (one policy, one source of truth): a guest's CONNECT target is
 * refused, dialed as-requested, or dialed to the SafeSearch rewrite host.
 */
class HotspotFilterTest {

    private fun blocklistPlan() = DnsPlan(
        revision = "r1", mode = "blocklist",
        allowDomains = emptyList(),
        blockDomains = listOf("bad.example"),
        blockCategories = emptyList(),
        allowExceptions = listOf("ok.example"),
        safeSearch = true, youtubeRestrict = "moderate",
        rewrites = listOf(DnsRewrite("www.google.com", "forcesafesearch.google.com")),
    )

    private fun allowlistPlan() = blocklistPlan().copy(
        mode = "allowlist", allowDomains = listOf("school.example"),
    )

    @Test fun blocked_host_is_refused() {
        val v = HotspotFilter(blocklistPlan()).verdict("bad.example")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun blocked_subdomain_is_refused() {
        val v = HotspotFilter(blocklistPlan()).verdict("cdn.bad.example")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun unlisted_host_dials_as_requested() {
        val v = HotspotFilter(blocklistPlan()).verdict("news.example")
        assertEquals(HotspotVerdict.Dial("news.example"), v)
    }

    @Test fun rewrite_dials_the_safesearch_host() {
        // The tunnel goes to the SafeSearch host — exactly how the DNS rewrite
        // enforces it (that host serves a cert valid for the original SNI).
        val v = HotspotFilter(blocklistPlan()).verdict("www.google.com")
        assertEquals(HotspotVerdict.Dial("forcesafesearch.google.com"), v)
    }

    @Test fun allowlist_refuses_everything_not_listed() {
        val f = HotspotFilter(allowlistPlan())
        assertTrue(f.verdict("evil.example") is HotspotVerdict.Refuse)
        assertEquals(HotspotVerdict.Dial("school.example"), f.verdict("school.example"))
    }

    @Test fun host_is_matched_case_insensitively() {
        val v = HotspotFilter(blocklistPlan()).verdict("BAD.EXAMPLE")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    // --- IP-literal targets can't be domain-filtered → must fail closed ------

    @Test fun ipv4_literal_is_refused_in_blocklist_mode() {
        // The escape the review caught: a raw IP matches no domain, so a naive
        // filter would pass it through and defeat the blocklist entirely.
        val v = HotspotFilter(blocklistPlan()).verdict("1.2.3.4")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun ipv6_literal_is_refused_in_blocklist_mode() {
        val v = HotspotFilter(blocklistPlan()).verdict("2606:4700::1")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun bracketed_ipv6_literal_is_refused() {
        val v = HotspotFilter(blocklistPlan()).verdict("[2606:4700::1]")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun ipv4_literal_is_refused_in_allowlist_mode() {
        val v = HotspotFilter(allowlistPlan()).verdict("1.2.3.4")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun ip_literal_dials_only_when_unrestricted() {
        // Unrestricted = the guardian imposes no web constraint, so an IP is fine.
        val unrestricted = blocklistPlan().copy(mode = "unrestricted", blockDomains = emptyList())
        assertEquals(HotspotVerdict.Dial("1.2.3.4"), HotspotFilter(unrestricted).verdict("1.2.3.4"))
    }

    // --- trailing-dot normalization: one extra dot must not dodge the filter ---

    @Test fun trailing_dot_blocked_host_is_refused() {
        // "bad.example." resolves the same as "bad.example" but matches neither
        // the blocklist entry nor an IP shape — it must normalize and be refused.
        val v = HotspotFilter(blocklistPlan()).verdict("bad.example.")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun trailing_dot_blocked_subdomain_is_refused() {
        val v = HotspotFilter(blocklistPlan()).verdict("cdn.bad.example.")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun trailing_dot_ipv4_literal_is_refused() {
        val v = HotspotFilter(blocklistPlan()).verdict("1.2.3.4.")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun trailing_dot_allowlist_host_out_of_list_is_refused() {
        val v = HotspotFilter(allowlistPlan()).verdict("evil.example.")
        assertTrue(v is HotspotVerdict.Refuse)
    }

    @Test fun trailing_dot_allowed_host_dials_the_normalized_name() {
        // The vetted (dot-stripped) host is what we dial, so the proxy connects
        // to exactly what passed the filter — not the raw guest string.
        val v = HotspotFilter(blocklistPlan()).verdict("news.example.")
        assertEquals(HotspotVerdict.Dial("news.example"), v)
    }

    @Test fun empty_or_dot_only_host_is_refused() {
        assertTrue(HotspotFilter(blocklistPlan()).verdict(".") is HotspotVerdict.Refuse)
        assertTrue(HotspotFilter(blocklistPlan()).verdict("") is HotspotVerdict.Refuse)
    }
}
