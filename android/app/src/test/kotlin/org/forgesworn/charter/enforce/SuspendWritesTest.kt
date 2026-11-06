package org.forgesworn.charter.enforce

import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * A locked phone must not re-suspend its whole app list every tick: the
 * platform announces a no-op re-suspend to the launcher as a change, which kept
 * waking it all night.
 */
class SuspendWritesTest {

    private val launchable = listOf("a.game", "b.chat", "c.video", "d.notes")

    @Test
    fun `a settled lock writes nothing`() {
        val set = launchable.toSet()
        assertEquals(emptyList<String>(), suspendWrites(launchable, set) { true })
    }

    @Test
    fun `only the packages not yet suspended are written`() {
        val set = setOf("a.game", "b.chat", "c.video")
        val already = setOf("a.game")
        assertEquals(listOf("b.chat", "c.video"), suspendWrites(launchable, set) { it in already })
    }

    @Test
    fun `a package outside the suspend set is never written, suspended or not`() {
        assertEquals(listOf("b.chat"), suspendWrites(launchable, setOf("b.chat")) { false })
        assertEquals(emptyList<String>(), suspendWrites(launchable, emptySet()) { true })
    }

    @Test
    fun `an unknown state is written, never assumed suspended`() {
        // launchableSuspension reports missing flags as false: the write happens.
        val state = mapOf("a.game" to true)
        assertEquals(
            listOf("b.chat", "c.video", "d.notes"),
            suspendWrites(launchable, launchable.toSet()) { state[it] == true },
        )
    }

    @Test
    fun `query order is kept`() {
        val set = setOf("d.notes", "a.game")
        assertEquals(listOf("a.game", "d.notes"), suspendWrites(launchable, set) { false })
    }
}
