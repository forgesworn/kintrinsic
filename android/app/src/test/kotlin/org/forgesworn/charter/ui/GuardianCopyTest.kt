package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The guardian list and the Guardian presence screen's words and parsing. */
class GuardianCopyTest {

    private val root = "11".repeat(32)
    private val other = "13".repeat(32)

    @Test
    fun aGuardianIsNamedByLabelAndShortNpub() {
        val name = GuardianCopy.guardianName("Mum", root)
        assertTrue(name, name.startsWith("Mum (npub1"))
        assertTrue(name, name.contains("…"))
        assertTrue(GuardianCopy.guardianName(null, root).startsWith("A guardian (npub1"))
        assertTrue(GuardianCopy.guardianName("  ", root).startsWith("A guardian ("))
    }

    @Test
    fun theSummaryParsesAndReadsAsLines() {
        val raw = """{"available":true,"paired":true,"pending":1,
            "pin":{"set":true,"failures":5,"waitSeconds":42},
            "roots":[{"root":"$root","label":null,"keys":2,"keyLabels":["phone"],"rules":["budget","schedule"]}]}"""
        val s = CharterCore.parseGuardiansSummary(raw)
        assertNotNull(s)
        s!!
        assertTrue(s.available && s.paired && s.pinSet)
        assertEquals(42L, s.pinWaitSeconds)
        assertEquals(5L, s.pinFailures)
        assertEquals(1, s.roots.size)
        assertNull(s.roots[0].label)
        val lines = GuardianCopy.rootLines(s.roots[0])
        assertTrue(lines, lines.contains("2 Kintrinsic apps linked"))
        assertTrue(lines, lines.contains("Set: daily time, schedule"))
        assertNull(CharterCore.parseGuardiansSummary("""{"error":"not initialized"}"""))
    }

    @Test
    fun pendingStatementsParseAndReadAsSentences() {
        val raw = """[{"d":"kintrinsic/guardian/$other","kind":"coguardian","signer":"$root",
            "key":"$other","label":"Dad","issuedAt":100,"expiresAt":7300,"takesEffectAt":null,"confirmed":false}]"""
        val list = CharterCore.parsePendingStatements(raw)
        assertEquals(1, list.size)
        val p = list[0]
        assertNull(p.takesEffectAt)
        assertTrue(GuardianCopy.pendingTitle(p).startsWith("Add Dad (npub1"))
        val detail = GuardianCopy.pendingDetail(p, 100)
        assertTrue(detail, detail.startsWith("Asked by A guardian (npub1"))
        assertTrue(detail, detail.contains("Expires in 2 h"))
        assertEquals(emptyList<CharterCore.PendingStatement>(), CharterCore.parsePendingStatements("garbage"))
    }

    @Test
    fun aMintIsATokenOrAnErrorNeverBoth() {
        val ok = CharterCore.parsePresenceResult(
            """{"code":"charter://presence?m=aa&t=bb","shortForm":"bb","expiresAt":700,"qr":{"size":21,"modules":"${"0".repeat(441)}"}}""",
        )
        assertTrue(ok.ok)
        assertNull(ok.error)
        assertEquals(21, ok.token!!.qrSize)

        val wrong = CharterCore.parsePresenceResult("""{"ok":false,"failures":5,"waitSeconds":60,"error":"wrongPin"}""")
        assertFalse(wrong.ok)
        assertNull(wrong.token)
        assertEquals("wrongPin", wrong.error)
        assertTrue(GuardianCopy.mintError(wrong).contains("1 min"))

        val noPin = CharterCore.parsePresenceResult("""{"ok":false,"noPin":true,"waitSeconds":0,"error":"noPin"}""")
        assertEquals(GuardianCopy.NO_PIN, GuardianCopy.mintError(noPin))
        assertEquals("failed", CharterCore.parsePresenceResult("not json").error)
    }

    @Test
    fun theBackoffAndCountdownRead() {
        assertNull(GuardianCopy.waitLine(0))
        assertEquals("Too many wrong PINs. Try again in 59 s.", GuardianCopy.waitLine(59))
        assertEquals("Too many wrong PINs. Try again in 2 min.", GuardianCopy.waitLine(61))
        assertEquals("10:00", GuardianCopy.countdown(600))
        assertEquals("0:05", GuardianCopy.countdown(5))
    }
}
