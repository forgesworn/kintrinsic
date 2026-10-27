package org.forgesworn.charter.enforce

import android.os.UserManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * 05-G2: the baseline and the tethering posture are level-triggered against
 * the platform's real restriction state, so drift is healed and a failed
 * write is retried rather than latched.
 */
class RestrictionWritesTest {

    @Test
    fun `only the missing restrictions are written`() {
        val set = setOf("a", "c")
        val writes = restrictionWrites(listOf("a" to true, "b" to true, "c" to true)) { it in set }
        assertEquals(listOf("b" to true), writes)
    }

    @Test
    fun `a restriction that drifted away is put back`() {
        val writes = restrictionWrites(listOf(UserManager.DISALLOW_SAFE_BOOT to true)) { false }
        assertEquals(listOf(UserManager.DISALLOW_SAFE_BOOT to true), writes)
    }

    @Test
    fun `an unreadable state writes everything, never assumes it is right`() {
        val desired = listOf("a" to true, "b" to false)
        assertEquals(desired, restrictionWrites(desired, null))
    }

    @Test
    fun `nothing is written when the state already matches`() {
        val set = setOf("a")
        assertTrue(restrictionWrites(listOf("a" to true, "b" to false)) { it in set }.isEmpty())
    }

    @Test
    fun `filtered locks the system hotspot before it frees tethering config`() {
        val writes = restrictionWrites(tetherTargets("filtered"), null)
        assertEquals(
            listOf(
                UserManager.DISALLOW_WIFI_TETHERING to true,
                UserManager.DISALLOW_CONFIG_TETHERING to false,
            ),
            writes,
        )
    }

    @Test
    fun `blocked locks config before it frees the hotspot key`() {
        assertEquals(
            listOf(
                UserManager.DISALLOW_CONFIG_TETHERING to true,
                UserManager.DISALLOW_WIFI_TETHERING to false,
            ),
            tetherTargets("blocked"),
        )
    }

    @Test
    fun `an unknown mode is blocked`() {
        assertEquals(tetherTargets("blocked"), tetherTargets("some-future-mode"))
    }

    @Test
    fun `a tether posture that drifted is healed on the next pass`() {
        // Blocked was applied, then config tethering was lifted from outside.
        val set = setOf<String>()
        assertEquals(
            listOf(UserManager.DISALLOW_CONFIG_TETHERING to true),
            restrictionWrites(tetherTargets("blocked")) { it in set },
        )
    }
}
