package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The ward's account of a held app. The rules that matter: both edges are told,
 * a hold merely counting down says nothing more, and a restart does not re-nag.
 */
class AppHoldNoticesTest {

    private fun hold(pkg: String, state: String = "allowed", until: Long = 5000L) =
        CharterCore.AppHold(pkg, state, until)

    private val nothingBlocked: (String) -> Boolean = { false }
    private val everythingBlocked: (String) -> Boolean = { true }

    @Test
    fun a_new_allow_hold_says_the_app_is_open() {
        val out = holdNotices(emptyMap(), listOf(hold("org.chromium.vanadium")), nothingBlocked)
        assertEquals(1, out.size)
        assertEquals(HoldNotice.Kind.OPENED, out[0].kind)
        assertEquals("org.chromium.vanadium", out[0].pkg)
        assertEquals(5000L, out[0].untilUnix)
    }

    @Test
    fun a_new_block_hold_says_the_app_is_paused() {
        val out = holdNotices(emptyMap(), listOf(hold("com.mojang", state = "blocked")), nothingBlocked)
        assertEquals(listOf(HoldNotice.Kind.PAUSED), out.map { it.kind })
    }

    /** The common case, on every tick. Saying it again would be nagging. */
    @Test
    fun a_hold_that_is_merely_running_says_nothing() {
        val live = listOf(hold("a.b"))
        assertTrue(holdNotices(memoryOf(live), live, nothingBlocked).isEmpty())
    }

    /**
     * The end wording comes from what the app actually IS afterwards, not from
     * the hold's direction — a guardian who changed the standing rule mid-hold
     * must not be quoted saying the opposite of what the phone now does.
     */
    @Test
    fun the_end_of_a_hold_reads_the_rule_it_lands_on() {
        val was = memoryOf(listOf(hold("a.b")))
        assertEquals(
            listOf(HoldNotice.Kind.BACK_CLOSED),
            holdNotices(was, emptyList(), everythingBlocked).map { it.kind },
        )
        assertEquals(
            listOf(HoldNotice.Kind.BACK_OPEN),
            holdNotices(was, emptyList(), nothingBlocked).map { it.kind },
        )
    }

    /** A guardian who extends an hour to two has said something new. */
    @Test
    fun a_changed_expiry_is_a_new_hold() {
        val was = memoryOf(listOf(hold("a.b", until = 5000L)))
        val out = holdNotices(was, listOf(hold("a.b", until = 9000L)), nothingBlocked)
        assertEquals(listOf(HoldNotice.Kind.OPENED), out.map { it.kind })
        assertEquals(9000L, out[0].untilUnix)
    }

    /** …as has one who flips an open hold into a paused one. */
    @Test
    fun a_changed_direction_is_a_new_hold() {
        val was = memoryOf(listOf(hold("a.b", state = "allowed")))
        val out = holdNotices(was, listOf(hold("a.b", state = "blocked")), nothingBlocked)
        assertEquals(listOf(HoldNotice.Kind.PAUSED), out.map { it.kind })
    }

    /** One tick can carry both edges — one hold starting, another ending. */
    @Test
    fun starts_and_ends_are_reported_together() {
        val was = memoryOf(listOf(hold("gone.app")))
        val out = holdNotices(was, listOf(hold("new.app")), everythingBlocked)
        assertEquals(
            setOf("new.app" to HoldNotice.Kind.OPENED, "gone.app" to HoldNotice.Kind.BACK_CLOSED),
            out.map { it.pkg to it.kind }.toSet(),
        )
    }

    /**
     * A phone restarts more than a guardian's does. Carrying the memory across
     * is what stops a reboot re-announcing every live hold.
     */
    @Test
    fun a_restart_with_the_memory_intact_re_announces_nothing() {
        val live = listOf(hold("a.b"), hold("c.d", state = "blocked"))
        val remembered = memoryOf(live) // what the store handed back after restart
        assertTrue(holdNotices(remembered, live, nothingBlocked).isEmpty())
    }

    // --- blockedUnder: the reading `appSuspendSet` already uses ---------------

    @Test
    fun blocked_under_follows_the_posture() {
        val blocklist = CharterCore.AppPolicy("blocklist", listOf("a.b"), emptyList())
        assertTrue(blockedUnder(blocklist, "a.b"))
        assertTrue(!blockedUnder(blocklist, "c.d"))

        val allowlist = CharterCore.AppPolicy("allowlist", emptyList(), listOf("a.b"))
        assertTrue(!blockedUnder(allowlist, "a.b"))
        assertTrue(blockedUnder(allowlist, "c.d"))

        // No policy at all blocks nothing — the same fail-safe as the gate.
        assertTrue(!blockedUnder(null, "a.b"))
    }

    // --- appRules: apps with their own agreed hours -------------------------
    //
    // Announced nowhere at all until the 2026-08-02 audit: these turn on and
    // off with the clock on every tick, so "it just stopped working" was the
    // ward's entire experience of the clause.

    @Test
    fun a_scheduled_app_is_announced_at_both_edges() {
        val shut = ruleNotices(emptySet(), setOf("a.b"), firstRun = false, blockedByPolicy = { false })
        assertEquals(listOf(HoldNotice("a.b", HoldNotice.Kind.RULE_CLOSED, 0L)), shut)

        val open = ruleNotices(setOf("a.b"), emptySet(), firstRun = false, blockedByPolicy = { false })
        assertEquals(listOf(HoldNotice("a.b", HoldNotice.Kind.RULE_OPEN, 0L)), open)
    }

    @Test
    fun an_unchanged_tick_says_nothing() {
        val same = setOf("a.b", "c.d")
        assertTrue(ruleNotices(same, same, firstRun = false, blockedByPolicy = { false }).isEmpty())
    }

    /**
     * The first tick on a fresh install records and says nothing. Otherwise
     * adopting Kintrinsic greets the ward with a burst of notices about rules that
     * were already in force — and a feature meant to explain becomes a nag.
     */
    @Test
    fun the_first_run_seeds_silently() {
        assertTrue(
            ruleNotices(emptySet(), setOf("a.b", "c.d"), firstRun = true, blockedByPolicy = { false })
                .isEmpty(),
        )
    }

    /**
     * An app the standing `apps` policy blocks anyway is skipped BOTH ways:
     * "closed for now" is not news when it was already closed, and "open again"
     * would be a plain lie told by the wrong clause.
     */
    @Test
    fun an_app_the_standing_policy_blocks_is_never_spoken_for() {
        val always = { _: String -> true }
        assertTrue(ruleNotices(emptySet(), setOf("a.b"), firstRun = false, blockedByPolicy = always).isEmpty())
        assertTrue(ruleNotices(setOf("a.b"), emptySet(), firstRun = false, blockedByPolicy = always).isEmpty())
    }
}
