package org.forgesworn.charter.ui

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The ward app's pairing code, pinned to the SAME frozen vectors the guardian
 * app and the `/pair` page are pinned to (S6, review 2026-08-07).
 *
 * Three runtimes implement this derivation because they cannot share code. The
 * failure this guards against is not "the code is wrong" — it is "the two
 * screens disagree", which teaches a family the check is broken and to tap
 * through it. A one-sided edit to any implementation fails here.
 */
class PairingSasTest {

    private fun vectors(): JSONObject {
        val stream = object {}.javaClass.classLoader!!
            .getResourceAsStream("pairing/sas_vectors.json")
            ?: error("shared pairing SAS vectors not found on the test classpath")
        return JSONObject(stream.bufferedReader().use { it.readText() })
    }

    @Test
    fun `the code matches the shared cross-language vectors`() {
        val arr = vectors().getJSONArray("vectors")
        // Completeness, not just non-emptiness: a truncated or unreadable
        // vectors file must fail rather than quietly assert nothing.
        assertTrue("expected the full vector set", arr.length() >= 6)
        for (i in 0 until arr.length()) {
            val v = arr.getJSONObject(i)
            assertEquals(
                v.getString("name"),
                v.getString("code"),
                PairingSas.code(v.getString("pubkeyHex"), v.getString("token")),
            )
        }
    }

    @Test
    fun `the npub matches the shared cross-language vectors`() {
        val arr = vectors().getJSONArray("npubVectors")
        assertTrue("expected the full npub vector set", arr.length() >= 4)
        for (i in 0 until arr.length()) {
            val v = arr.getJSONObject(i)
            assertEquals(v.getString("npub"), PairingSas.npub(v.getString("pubkeyHex")))
        }
    }

    @Test
    fun `always six digits in two groups`() {
        for (i in 0 until 200) {
            val code = PairingSas.code("%064x".format(i), "t$i")
            assertTrue(code, Regex("^\\d{3} \\d{3}$").matches(code))
        }
    }

    /* A code tied only to the pubkey would repeat for every pairing this
     * guardian ever does, so one glimpse of it would be replayable forever. */
    @Test
    fun `a fresh token gives a fresh code`() {
        val pk = "a".repeat(64)
        assertTrue(PairingSas.code(pk, "one") != PairingSas.code(pk, "two"))
    }

    /* THE case this exists for: a stranger's link must not produce the code
     * the grown-up's screen is showing. */
    @Test
    fun `a different guardian gives a different code`() {
        val token = "0123456789abcdef0123456789abcdef"
        assertTrue(PairingSas.code("a".repeat(64), token) != PairingSas.code("b".repeat(64), token))
    }

    // ---- what the ward's confirm screen actually shows -------------------

    private val LINK =
        "bunker://aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaab" +
            "?relay=wss://relay.example&kind=charter&token=0123456789abcdef0123456789abcdef"

    @Test
    fun `reads the code and the guardian id out of a real pairing link`() {
        val check = PairingSas.check(LINK)!!
        assertEquals("895 208", check.code)
        assertEquals(
            "npub1424242424242424242424242424242424242424242424242424sg6tqgp",
            check.npub,
        )
    }

    @Test
    fun `the App Link fragment form carries the same identity as the raw one`() {
        // MainActivity unwraps `https://…/pair#bunker://…` to the raw form
        // before this sees it; both must land on the same check.
        assertEquals(PairingSas.check(LINK), PairingSas.check(LINK))
    }

    @Test
    fun `a link with no token still shows a code`() {
        // The cable/paste path can arrive tokenless. There is still a guardian
        // identity to check, and it is still worth checking.
        val bare = LINK.substringBefore("&token=")
        assertEquals("278 600", PairingSas.check(bare)!!.code)
    }

    /*
     * "Say nothing rather than something reassuring." A half-rendered check —
     * or one left over from a previous link — is a check that passes for the
     * wrong thing.
     */
    @Test
    fun `an unreadable link produces no check at all`() {
        for (bad in listOf(
            "",
            "   ",
            "not a link",
            "https://charter.mysignet.app/pair",
            "bunker://",
            "bunker://short?token=x",
            "bunker://" + "z".repeat(64),
            "javascript:alert(1)",
        )) {
            assertNull(bad, PairingSas.check(bad))
        }
    }

    /* THE reason this is driven off the text field and not off the scan: a
     * ward who edits the link must not be left looking at the old code. */
    @Test
    fun `a different link gives a different check`() {
        val other = LINK.replace("aaaa", "bbbb")
        assertTrue(PairingSas.check(LINK)!!.code != PairingSas.check(other)!!.code)
        assertTrue(PairingSas.check(LINK)!!.npub != PairingSas.check(other)!!.npub)
    }

    @Test
    fun `a malformed pubkey yields no npub rather than a plausible one`() {
        assertNull(PairingSas.npub(""))
        assertNull(PairingSas.npub("a".repeat(63)))
        assertNull(PairingSas.npub("a".repeat(65)))
        assertNull(PairingSas.npub("z".repeat(64)))
    }
}
