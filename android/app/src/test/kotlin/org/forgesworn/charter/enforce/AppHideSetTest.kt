package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The set logic deciding what Kintrinsic HIDES — "remove from device" (2026-08-27).
 *
 * Born from a ward's Samsung tablet arriving full of OEM bloatware with no cable
 * path to sweep it off, since 0.6.9 locks USB debugging by design. Pinned here
 * for two reasons the suspend set does not share: hiding is the one thing a ward
 * cannot see happen and cannot undo from the device, and the deny-list
 * subtraction is all that stands between a tidy clause and a phone with no
 * launcher on it.
 */
class AppHideSetTest {

    /** What the phone actually has — bloatware plus the load-bearing stack. */
    private val installed = listOf(
        "com.samsung.bloat",
        "com.samsung.morebloat",
        "com.game",
        "com.android.settings",
        "com.android.dialer",
    )

    /** Stands in for [denyListPackages], which needs a real `Context`. */
    private val deny = setOf(
        "org.forgesworn.charter",
        "com.android.settings",
        "com.android.dialer",
    )

    private fun policy(
        hidden: List<String> = emptyList(),
        posture: String = "blocklist",
        allowed: List<String> = emptyList(),
        blocked: List<String> = emptyList(),
    ) = CharterCore.AppPolicy(
        posture = posture,
        allowed = allowed,
        blocked = blocked,
        hidden = hidden,
    )

    @Test
    fun `no policy hides nothing`() {
        assertTrue(appHideSet(null, installed, deny).isEmpty())
    }

    /** The additive default: every charter written before this clause existed
     *  must mean exactly what it always meant. */
    @Test
    fun `a policy with no hidden list hides nothing`() {
        assertTrue(appHideSet(policy(blocked = listOf("com.game")), installed, deny).isEmpty())
    }

    @Test
    fun `it hides what the clause names`() {
        val out = appHideSet(policy(hidden = listOf("com.samsung.bloat")), installed, deny)
        assertEquals(setOf("com.samsung.bloat"), out)
    }

    /**
     * A guardian pasting a list of bloatware names that only half matches this
     * particular model must not produce a failing hide call every tick forever.
     */
    @Test
    fun `a package this phone does not have is ignored`() {
        val out = appHideSet(
            policy(hidden = listOf("com.samsung.bloat", "com.never.installed")),
            installed,
            deny,
        )
        assertEquals(setOf("com.samsung.bloat"), out)
    }

    /**
     * The one that would brick the device. Unlike a suspension there is no grey
     * badge to explain a hidden launcher, dialer or Settings — the clause is not
     * permitted to strand the ward on a phone she cannot use or call out of.
     */
    @Test
    fun `the deny-list is never hidden, however the clause asks`() {
        val out = appHideSet(
            policy(hidden = listOf("com.android.settings", "com.android.dialer", "com.samsung.bloat")),
            installed,
            deny,
        )
        assertEquals(setOf("com.samsung.bloat"), out)
        assertFalse("Settings must survive", out.contains("com.android.settings"))
        assertFalse("the dialer must survive", out.contains("com.android.dialer"))
    }

    @Test
    fun `hiding the warden itself is refused`() {
        val out = appHideSet(
            policy(hidden = listOf("org.forgesworn.charter")),
            installed + "org.forgesworn.charter",
            deny,
        )
        assertTrue("Kintrinsic may never hide itself", out.isEmpty())
    }

    /**
     * Hiding answers "does this belong on the phone at all?", never "may she
     * open it now?" — so it reads NOTHING but `hidden`. A pause lifts BLOCKING;
     * a paused clause reaches here with its blocked list already dissolved and
     * its hidden list intact, and the bloatware stays gone.
     */
    @Test
    fun `a paused policy still hides`() {
        val paused = policy(hidden = listOf("com.samsung.bloat"), blocked = emptyList())
        assertEquals(setOf("com.samsung.bloat"), appHideSet(paused, installed, deny))
    }

    /** Independent of posture: an allowlist that names an app does not un-hide
     *  it, and a blocklist that omits one does not spare it. */
    @Test
    fun `posture does not move the hide set`() {
        val allow = policy(
            hidden = listOf("com.samsung.bloat"),
            posture = "allowlist",
            allowed = listOf("com.samsung.bloat", "com.game"),
        )
        assertEquals(setOf("com.samsung.bloat"), appHideSet(allow, installed, deny))
        val block = policy(hidden = listOf("com.samsung.bloat"), blocked = listOf("com.game"))
        assertEquals(setOf("com.samsung.bloat"), appHideSet(block, installed, deny))
    }

    /** A set, not a list: the same name twice is one hide, one call. */
    @Test
    fun `duplicates collapse`() {
        val out = appHideSet(
            policy(hidden = listOf("com.samsung.bloat", "com.samsung.bloat", "com.samsung.morebloat")),
            installed,
            deny,
        )
        assertEquals(setOf("com.samsung.bloat", "com.samsung.morebloat"), out)
    }

    /** Dropping every name is the undo — the reconcile unhides what it hid. */
    @Test
    fun `an emptied clause asks for nothing hidden`() {
        assertTrue(appHideSet(policy(hidden = emptyList()), installed, deny).isEmpty())
    }

    /** Nothing installed, nothing to hide — a phone mid-provisioning must not
     *  produce a set full of names it cannot act on. */
    @Test
    fun `an empty device hides nothing`() {
        assertTrue(appHideSet(policy(hidden = listOf("com.samsung.bloat")), emptyList(), deny).isEmpty())
    }
}
