package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The set logic deciding what Kintrinsic suspends. Its own comment called it "the
 * JVM-testable seam" and it had no tests at all until 2026-07-29 — this is the
 * one function standing between a guardian's intent and a child's device going
 * dark, so it gets pinned.
 */
class AppSuspendSetTest {

    private val launchable = listOf("com.game", "com.book", "com.school", "com.chat")

    private fun policy(posture: String, allowed: List<String> = emptyList(), blocked: List<String> = emptyList()) =
        CharterCore.AppPolicy(posture = posture, allowed = allowed, blocked = blocked)

    @Test
    fun `a lock takes everything`() {
        assertEquals(launchable.toSet(), appSuspendSet(launchable, locked = true, appPolicy = null, ruleSuspensions = emptySet()))
    }

    @Test
    fun `no policy and no lock suspends nothing`() {
        assertTrue(appSuspendSet(launchable, locked = false, appPolicy = null, ruleSuspensions = emptySet()).isEmpty())
    }

    @Test
    fun `an allowlist suspends everything it does not name`() {
        val out = appSuspendSet(launchable, false, policy("allowlist", allowed = listOf("com.school")), emptySet())
        assertEquals(setOf("com.game", "com.book", "com.chat"), out)
    }

    @Test
    fun `a blocklist suspends only what it names`() {
        val out = appSuspendSet(launchable, false, policy("blocklist", blocked = listOf("com.game")), emptySet())
        assertEquals(setOf("com.game"), out)
    }

    /** The two dimensions must never cancel each other out. */
    @Test
    fun `per-app rules union with the posture`() {
        val out = appSuspendSet(launchable, false, policy("blocklist", blocked = listOf("com.game")), setOf("com.chat"))
        assertEquals(setOf("com.game", "com.chat"), out)
    }

    // --- listening through the lock (spec 2026-07-29) ------------------------

    @Test
    fun `an agreed story keeps playing through a lock`() {
        val out = appSuspendSet(launchable, locked = true, appPolicy = null, ruleSuspensions = emptySet(), listeningExempt = setOf("com.book"))
        assertFalse("the audiobook must survive the lock", out.contains("com.book"))
        assertTrue("everything else still goes", out.containsAll(listOf("com.game", "com.school", "com.chat")))
    }

    /**
     * The exemption is permission to keep playing THROUGH A LOCK — never a way
     * to dodge a standing block. An app the guardian blocked outright stays
     * blocked whether it is making a noise or not.
     */
    @Test
    fun `listening never unblocks an app while unlocked`() {
        val out = appSuspendSet(
            launchable,
            locked = false,
            appPolicy = policy("blocklist", blocked = listOf("com.book")),
            ruleSuspensions = emptySet(),
            listeningExempt = setOf("com.book"),
        )
        assertTrue("a blocked app is blocked, playing or not", out.contains("com.book"))
    }

    @Test
    fun `an empty exemption changes nothing`() {
        assertEquals(
            appSuspendSet(launchable, true, null, emptySet()),
            appSuspendSet(launchable, true, null, emptySet(), emptySet()),
        )
    }

    // --- named-times buckets (2026-08-02) -------------------------------------

    /**
     * Spending a bucket's own allowance is its own charter dimension, exactly
     * like the per-app rule suspensions — it unions in, never cancels the
     * others out, and never touches an app outside the bucket.
     */
    @Test
    fun `a spent bucket unions with the posture and rule suspensions`() {
        val out = appSuspendSet(
            launchable,
            locked = false,
            appPolicy = policy("blocklist", blocked = listOf("com.game")),
            ruleSuspensions = setOf("com.chat"),
            bucketSuspensions = setOf("com.school"),
        )
        assertEquals(setOf("com.game", "com.chat", "com.school"), out)
    }

    /** Spending a bucket must NEVER lock the whole device — only its own apps. */
    @Test
    fun `a spent bucket suspends only its own apps, never the device`() {
        val out = appSuspendSet(
            launchable,
            locked = false,
            appPolicy = null,
            ruleSuspensions = emptySet(),
            bucketSuspensions = setOf("com.game"),
        )
        assertEquals(setOf("com.game"), out)
        assertFalse("everything outside the spent bucket must stay up", out.contains("com.book"))
    }

    /**
     * The listening exemption is permission to keep an agreed story playing
     * through a LOCK — never a way to dodge a bucket's own confiscation while
     * otherwise unlocked. A bucket-suspended app stays suspended whether or
     * not it happens to be named as the listening exemption.
     */
    @Test
    fun `listening never reopens a spent bucket while unlocked`() {
        val out = appSuspendSet(
            launchable,
            locked = false,
            appPolicy = null,
            ruleSuspensions = emptySet(),
            listeningExempt = setOf("com.game"),
            bucketSuspensions = setOf("com.game"),
        )
        assertTrue("a spent bucket's app stays suspended", out.contains("com.game"))
    }

    // --- always available (spec 2026-08-03) -----------------------------------

    /**
     * The shipped `listening` exemption subtracted from the WHOLE union, so a
     * guardian-blocked app that happened to be playing came back at the lock —
     * contradicting this function's own documented contract. The existing test
     * for it runs unlocked and never saw this.
     */
    @Test
    fun `an exemption never beats a standing block, even while locked`() {
        val out = appSuspendSet(
            launchable,
            locked = true,
            appPolicy = policy("blocklist", blocked = listOf("com.book")),
            ruleSuspensions = emptySet(),
            listeningExempt = setOf("com.book"),
        )
        assertTrue("a blocked app is blocked, playing or not", out.contains("com.book"))
    }

    @Test
    fun `an allowlist still excludes an app it does not name, even when exempt`() {
        val out = appSuspendSet(
            launchable,
            locked = true,
            appPolicy = policy("allowlist", allowed = listOf("com.school")),
            ruleSuspensions = emptySet(),
            alwaysAvailable = setOf("com.book"),
        )
        assertTrue("outside the allowlist stays out", out.contains("com.book"))
    }

    @Test
    fun `an always-available app survives a lock with no audio playing`() {
        val out = appSuspendSet(
            launchable,
            locked = true,
            appPolicy = null,
            ruleSuspensions = emptySet(),
            alwaysAvailable = setOf("com.book"),
        )
        assertFalse("she must be able to START it", out.contains("com.book"))
        assertTrue("everything else still goes", out.containsAll(listOf("com.game", "com.chat")))
    }

    /**
     * Nothing to exempt while unlocked — the clause is about surviving a lock,
     * never about dodging a per-app rule during allowed hours.
     */
    @Test
    fun `always-available never unblocks during allowed hours`() {
        val out = appSuspendSet(
            launchable,
            locked = false,
            appPolicy = null,
            ruleSuspensions = setOf("com.book"),
            alwaysAvailable = setOf("com.book"),
        )
        assertTrue("a live per-app rule still applies", out.contains("com.book"))
    }

    @Test
    fun `an empty always-available set changes nothing`() {
        assertEquals(
            appSuspendSet(launchable, true, null, emptySet()),
            appSuspendSet(launchable, true, null, emptySet(), emptySet(), emptySet(), emptySet()),
        )
    }
}
