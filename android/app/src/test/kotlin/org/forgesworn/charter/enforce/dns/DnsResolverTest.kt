package org.forgesworn.charter.enforce.dns

import org.forgesworn.charter.native.CharterCore.DnsPlan
import org.forgesworn.charter.native.CharterCore.DnsRewrite
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DnsResolverTest {
    private fun q(name: String) = DnsQuestion(name, TYPE_A, 1, ByteArray(0))

    private fun blocklistPlan() = DnsPlan(
        revision = "r1", mode = "blocklist",
        allowDomains = emptyList(),
        blockDomains = listOf("bad.example"),
        blockCategories = emptyList(),
        allowExceptions = listOf("ok.example"),
        safeSearch = true, youtubeRestrict = "moderate",
        rewrites = listOf(
            DnsRewrite("www.google.com", "forcesafesearch.google.com"),
            DnsRewrite("www.youtube.com", "restrictmoderate.youtube.com"),
        ),
    )

    @Test fun blocklist_blocks_listed_domain_and_subdomains() {
        val r = DnsResolver(blocklistPlan())
        assertTrue(r.decide(q("bad.example")) is DnsDecision.Block)
        assertTrue(r.decide(q("www.bad.example")) is DnsDecision.Block) // subdomain
    }

    @Test fun blocklist_passes_unlisted() {
        val r = DnsResolver(blocklistPlan())
        assertTrue(r.decide(q("news.example")) is DnsDecision.PassThrough)
    }

    @Test fun rewrite_wins_over_passthrough() {
        val d = DnsResolver(blocklistPlan()).decide(q("www.google.com"))
        assertTrue(d is DnsDecision.Rewrite && d.target == "forcesafesearch.google.com")
    }

    @Test fun exception_overrides_block() {
        val plan = blocklistPlan().copy(blockDomains = listOf("ok.example"))
        // ok.example is also in allowExceptions -> must pass.
        assertTrue(DnsResolver(plan).decide(q("ok.example")) is DnsDecision.PassThrough)
    }

    @Test fun locked_blocks_everything_except_exceptions() {
        val plan = blocklistPlan().copy(mode = "locked", allowExceptions = listOf("school.example"))
        val r = DnsResolver(plan)
        assertTrue(r.decide(q("anything.example")) is DnsDecision.Block)
        assertTrue(r.decide(q("school.example")) is DnsDecision.PassThrough)
    }

    @Test fun allowlist_blocks_everything_outside_allow() {
        val plan = blocklistPlan().copy(
            mode = "allowlist", allowDomains = listOf("kids.example"), blockDomains = emptyList())
        val r = DnsResolver(plan)
        assertTrue(r.decide(q("kids.example")) is DnsDecision.PassThrough)
        assertTrue(r.decide(q("sub.kids.example")) is DnsDecision.PassThrough)
        assertTrue(r.decide(q("evil.example")) is DnsDecision.Block)
    }

    @Test fun false_suffix_lookalike_is_not_blocked() {
        val r = DnsResolver(blocklistPlan()) // blocks "bad.example"
        // A lookalike that merely CONTAINS the blocked string is not a subdomain.
        assertTrue(r.decide(q("notbad.example")) is DnsDecision.PassThrough)
        assertTrue(r.decide(q("bad.example.evil.com")) is DnsDecision.PassThrough)
    }

    @Test fun unrestricted_passes_all() {
        val plan = blocklistPlan().copy(mode = "unrestricted", rewrites = emptyList())
        assertTrue(DnsResolver(plan).decide(q("anything.example")) is DnsDecision.PassThrough)
    }

    @Test fun unknown_mode_fails_closed() {
        // Version skew: a mode this APK doesn't recognize must block, not pass.
        val plan = blocklistPlan().copy(mode = "some-future-mode", rewrites = emptyList())
        assertTrue(DnsResolver(plan).decide(q("anything.example")) is DnsDecision.Block)
    }
}
