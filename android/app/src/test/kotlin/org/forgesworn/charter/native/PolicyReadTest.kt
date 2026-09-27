package org.forgesworn.charter.native

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Test

/**
 * F1: a per-app enforcement answer the core could not give is never read as
 * the empty answer ("no policy", "suspend nothing").
 */
class PolicyReadTest {

    @Test
    fun anEmptyReplyIsNoPolicy() {
        assertNull(CharterCore.parseAppPolicy(""))
    }

    @Test
    fun aPolicyParses() {
        val p = CharterCore.parseAppPolicy(
            """{"v":1,"posture":"allowlist","allowed":["a.b"],"blocked":[],"hidden":["c.d"]}""",
        )!!
        assertEquals("allowlist", p.posture)
        assertEquals(listOf("a.b"), p.allowed)
        assertEquals(listOf("c.d"), p.hidden)
    }

    @Test
    fun anErrorReplyIsUnreadableNotAnEmptyBlocklist() {
        assertThrows(CharterCore.Unreadable::class.java) {
            CharterCore.parseAppPolicy("""{"error":"apps clause unreadable: Io"}""")
        }
        assertThrows(CharterCore.Unreadable::class.java) {
            CharterCore.parseAppPolicy("""{"error":"internal error (panic recovered)"}""")
        }
    }

    @Test
    fun aMalformedPolicyIsUnreadable() {
        assertThrows(CharterCore.Unreadable::class.java) { CharterCore.parseAppPolicy("{not json") }
        assertThrows(CharterCore.Unreadable::class.java) {
            CharterCore.parseAppPolicy("""{"posture":"blocklist","blocked":[1,{}]}""")
        }
    }

    @Test
    fun aPackageListParses() {
        assertEquals(emptyList<String>(), CharterCore.parsePackageList("rules", "[]"))
        assertEquals(listOf("a.b", "c.d"), CharterCore.parsePackageList("rules", """["a.b","c.d"]"""))
    }

    @Test
    fun aPackageListErrorIsUnreadableNotEmpty() {
        for (raw in listOf(
            """{"error":"appRules clause unreadable"}""",
            """{"error":"not initialized"}""",
            "",
            "garbage",
            "[1,{}]",
        )) {
            assertThrows(raw, CharterCore.Unreadable::class.java) {
                CharterCore.parsePackageList("rules", raw)
            }
        }
    }
}
