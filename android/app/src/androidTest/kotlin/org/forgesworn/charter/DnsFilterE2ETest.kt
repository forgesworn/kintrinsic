package org.forgesworn.charter

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.forgesworn.charter.enforce.dns.DnsDecision
import org.forgesworn.charter.enforce.dns.DnsResolver
import org.forgesworn.charter.enforce.dns.TYPE_A
import org.forgesworn.charter.enforce.dns.DnsQuestion
import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class DnsFilterE2ETest {
    private fun q(n: String) = DnsQuestion(n, TYPE_A, 1, ByteArray(0))

    @Test fun blocklist_plan_blocks_and_rewrites_on_device() {
        val plan = CharterCore.DnsPlan(
            revision = "e2e", mode = "blocklist",
            allowDomains = emptyList(), blockDomains = listOf("bad.example"),
            blockCategories = emptyList(), allowExceptions = emptyList(),
            safeSearch = true, youtubeRestrict = "off",
            rewrites = listOf(CharterCore.DnsRewrite("www.google.com", "forcesafesearch.google.com")),
        )
        val r = DnsResolver(plan)
        assertTrue(r.decide(q("bad.example")) is DnsDecision.Block)
        val g = r.decide(q("www.google.com"))
        assertTrue(g is DnsDecision.Rewrite && g.target == "forcesafesearch.google.com")
        assertTrue(r.decide(q("news.example")) is DnsDecision.PassThrough)
    }
}
