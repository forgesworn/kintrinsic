package org.forgesworn.charter.enforce

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File
import java.time.ZoneId

/**
 * The ward half of the guardian's screen-lock reset: the token decision, the
 * history's line format and fold, the notice derived from it, and the order of
 * a reset (no reset without a notice; a throw is `failed`).
 */
class ScreenLockResetTest {

    private val london: ZoneId = ZoneId.of("Europe/London")
    private val id1 = "aaaabbbbccccddddeeeeffff00009999"  // no "123456" in it: the PIN checks grep for that
    private val id2 = "fedcba9876543210fedcba9876543210"

    // 2026-11-14 19:42:00 Europe/London (GMT) = 19:42 UTC.
    private val at1942 = 1794685320L

    // ---- the token decision --------------------------------------------------

    @Test
    fun `no stored token is provisioned, but not again straight after a refusal`() {
        val p = ScreenLockTokenPolicy
        assertEquals(ScreenLockTokenStep.PROVISION, p.step(false, false, false, null, 1000))
        assertEquals(ScreenLockTokenStep.PROVISION, p.step(false, true, false, null, 1000))
        assertEquals(ScreenLockTokenStep.NONE, p.step(false, false, false, 990, 1000))
        assertEquals(ScreenLockTokenStep.PROVISION, p.step(false, false, false, 1000 - p.RETRY_SECS, 1000))
    }

    @Test
    fun `an active token is never set again`() {
        for (restored in listOf(false, true)) {
            assertEquals(ScreenLockTokenStep.NONE, ScreenLockTokenPolicy.step(true, true, restored, null, 1000))
        }
    }

    @Test
    fun `a stored token that is not active is set again once`() {
        val p = ScreenLockTokenPolicy
        assertEquals(ScreenLockTokenStep.RESTORE, p.step(true, false, false, null, 1000))
        assertEquals(ScreenLockTokenStep.NONE, p.step(true, false, true, null, 1000))
        // Refused: tried again after the pause, not every tick.
        assertEquals(ScreenLockTokenStep.NONE, p.step(true, false, false, 990, 1000))
        assertEquals(ScreenLockTokenStep.RESTORE, p.step(true, false, false, 900, 1000))
    }

    // ---- the history line format ---------------------------------------------

    @Test
    fun `a history line has no field for a PIN`() {
        val lines = listOf(
            ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
            ScreenLockLine(id1, at1942 + 1, outcome = ScreenLockOutcome.APPLIED),
            ScreenLockLine(id1, at1942 + 60, acknowledged = true),
        )
        val allowed = setOf("id", "at", "kind", "outcome", "acknowledged")
        for (l in lines) {
            val keys = org.json.JSONObject(ScreenLockHistoryFormat.encode(l)).keys().asSequence().toSet()
            assertTrue(keys.toString(), allowed.containsAll(keys))
            assertFalse(ScreenLockHistoryFormat.encode(l).contains("pin", ignoreCase = true))
        }
        assertEquals(
            """{"id":"$id1","at":$at1942,"kind":"changed","outcome":"pending"}""",
            ScreenLockHistoryFormat.encode(lines[0]),
        )
    }

    @Test
    fun `lines round-trip and junk is skipped`() {
        val l = ScreenLockLine(id1, at1942, kind = ScreenLockChange.REMOVED, outcome = ScreenLockOutcome.PENDING)
        assertEquals(l, ScreenLockHistoryFormat.decode(ScreenLockHistoryFormat.encode(l)))
        assertNull(ScreenLockHistoryFormat.decode(""))
        assertNull(ScreenLockHistoryFormat.decode("{\"id\":\"$id1\",\"at\":17946"))
        assertNull(ScreenLockHistoryFormat.decode("{\"at\":1}"))
    }

    @Test
    fun `the fold takes the latest outcome and an acknowledgement stays made`() {
        val records = ScreenLockHistoryFormat.fold(
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942 + 60, acknowledged = true),
                // An outcome written after the acknowledgement does not undo it.
                ScreenLockLine(id1, at1942 + 61, outcome = ScreenLockOutcome.APPLIED),
                ScreenLockLine(id2, at1942 + 100, kind = ScreenLockChange.REMOVED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id2, at1942 + 101, outcome = ScreenLockOutcome.FAILED),
            ),
        )
        assertEquals(2, records.size)
        assertEquals(ScreenLockRecord(id1, at1942, ScreenLockChange.CHANGED, ScreenLockOutcome.APPLIED, true), records[0])
        assertEquals(ScreenLockRecord(id2, at1942 + 100, ScreenLockChange.REMOVED, ScreenLockOutcome.FAILED, false), records[1])
    }

    // ---- the notice, derived from the history --------------------------------

    @Test
    fun `an applied reset not yet acknowledged is the notice`() {
        val records = ScreenLockHistoryFormat.fold(
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942, outcome = ScreenLockOutcome.APPLIED),
            ),
        )
        assertEquals(
            "Your guardian changed your screen lock at 19:42 on 14 Nov. Ask them for the new PIN.",
            ScreenLockCopy.currentNotice(records, london),
        )
        val removed = records.map { it.copy(kind = ScreenLockChange.REMOVED) }
        assertEquals(
            "Your guardian removed your screen lock at 19:42 on 14 Nov.",
            ScreenLockCopy.currentNotice(removed, london),
        )
    }

    @Test
    fun `a reset stuck at pending still tells the child it may have happened`() {
        val records = ScreenLockHistoryFormat.fold(
            listOf(ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING)),
        )
        val text = ScreenLockCopy.currentNotice(records, london)!!
        assertTrue(text, text.startsWith("Your guardian may have changed your screen lock at 19:42 on 14 Nov."))
    }

    @Test
    fun `acknowledged, failed and not-ready resets show no notice`() {
        for (lines in listOf(
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942, outcome = ScreenLockOutcome.APPLIED),
                ScreenLockLine(id1, at1942 + 5, acknowledged = true),
            ),
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942, outcome = ScreenLockOutcome.FAILED),
            ),
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942, outcome = ScreenLockOutcome.NOT_READY),
            ),
        )) {
            assertNull(ScreenLockCopy.currentNotice(ScreenLockHistoryFormat.fold(lines), london))
        }
    }

    @Test
    fun `the newest unacknowledged reset is the one shown`() {
        val records = ScreenLockHistoryFormat.fold(
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.REMOVED, outcome = ScreenLockOutcome.APPLIED),
                ScreenLockLine(id2, at1942 + 3600, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.APPLIED),
            ),
        )
        assertEquals(listOf(id2, id1), ScreenLockHistoryFormat.unacknowledged(records).map { it.id })
        assertTrue(ScreenLockCopy.currentNotice(records, london)!!.contains("changed your screen lock at 20:42"))
    }

    // ---- the file --------------------------------------------------------------

    @Test
    fun `the history file survives a torn last line and keeps every line`() {
        val dir = createTempDir()
        try {
            val f = FileScreenLockHistory.fileIn(dir)
            f.writeText("""{"id":"$id1","at":$at1942,"kind":"changed","outcome":"pending"}""" + "\n" + """{"id":"$id1","at":17""")
            val h = FileScreenLockHistory(f)
            assertTrue(h.append(ScreenLockLine(id1, at1942 + 1, outcome = ScreenLockOutcome.APPLIED)))
            val reread = FileScreenLockHistory(f).records()!!
            assertEquals(1, reread.size)
            assertEquals(ScreenLockOutcome.APPLIED, reread[0].outcome)
            assertEquals(3, f.readLines().size)
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun `the history lives outside the warden's base dir`() {
        // Provisioning.baseDir is <device-protected files>/charter; a release
        // clears the clauses and the pairing there (A5).
        val de = File("/data/user_de/0/org.forgesworn.charter/files")
        val f = FileScreenLockHistory.fileIn(de)
        assertFalse(f.canonicalPath.startsWith(File(de, "charter").canonicalPath + File.separator))
    }

    // ---- the step ----------------------------------------------------------------

    private class FakePlatform(
        var secure: Boolean = true,
        var active: Boolean = true,
        var resetResult: () -> Boolean = { true },
        var setTokenResult: () -> Boolean = { true },
    ) : ScreenLockPlatform {
        val tokensSet = mutableListOf<ByteArray>()
        val resets = mutableListOf<String?>()
        override fun isDeviceSecure() = secure
        override fun isTokenActive() = active
        override fun setToken(token: ByteArray): Boolean { tokensSet += token; return setTokenResult() }
        override fun resetWithToken(newPin: String?, token: ByteArray): Boolean { resets += newPin; return resetResult() }
    }

    private class FakeTokens(var token: ByteArray? = ByteArray(32) { 7 }) : ScreenLockTokenStore {
        override fun load() = token
        override fun save(token: ByteArray): Boolean { this.token = token; return true }
    }

    private class FakeHistory(var writable: Boolean = true, var readable: Boolean = true) : ScreenLockHistory {
        val lines = mutableListOf<ScreenLockLine>()
        override fun append(line: ScreenLockLine): Boolean { if (writable) lines += line; return writable }
        override fun records() = if (readable) ScreenLockHistoryFormat.fold(lines) else null
    }

    private class FakeCore(var pending: CharterCore.ScreenLockDirective? = null) : ScreenLockCore {
        /** The outcomes the core took. */
        val reports = mutableListOf<Pair<String, String>>()
        /** Every report attempt, taken or not. */
        val attempts = mutableListOf<Pair<String, String>>()
        var takes = true
        var state: Pair<Boolean, Boolean>? = null
        /** Every `resetSupported` reported, one per tick. */
        val supported = mutableListOf<Boolean>()
        override fun take() = pending.also { pending = null }
        override fun report(id: String, result: String, atUnix: Long): Boolean {
            attempts += id to result
            if (takes) reports += id to result
            return takes
        }
        override fun setState(secure: Boolean, resetReady: Boolean, resetSupported: Boolean) {
            state = secure to resetReady
            supported += resetSupported
        }
    }

    private class FakeNotice : ScreenLockNoticeSurface {
        var shown: String? = null
        override fun show(text: String) { shown = text }
        override fun hide() { shown = null }
    }

    private fun step(p: FakePlatform, t: FakeTokens, h: FakeHistory, c: FakeCore, n: FakeNotice, logs: MutableList<String>) =
        ScreenLockResetStep(p, t, h, c, n, zone = { london }, newToken = { ByteArray(32) { 9 } }, log = { logs += it })

    @Test
    fun `a reset is recorded, applied, reported and noticed`() {
        val p = FakePlatform(); val h = FakeHistory(); val n = FakeNotice(); val logs = mutableListOf<String>()
        val c = FakeCore(CharterCore.ScreenLockDirective(id1, "123456", at1942))
        val s = step(p, FakeTokens(), h, c, n, logs)
        s.platformTick(at1942)
        s.noticeTick()
        assertEquals(listOf<String?>("123456"), p.resets)
        assertEquals(listOf(id1 to ScreenLockOutcome.APPLIED), c.reports)
        assertEquals(true to true, c.state)
        assertEquals(listOf(ScreenLockOutcome.PENDING, ScreenLockOutcome.APPLIED), h.lines.map { it.outcome })
        assertTrue(n.shown!!.startsWith("Your guardian changed your screen lock at 19:42 on 14 Nov."))
        assertFalse(logs.joinToString().contains("123456"))
    }

    @Test
    fun `an empty PIN removes the screen lock`() {
        val p = FakePlatform(); val h = FakeHistory()
        val c = FakeCore(CharterCore.ScreenLockDirective(id1, "", at1942))
        step(p, FakeTokens(), h, c, FakeNotice(), mutableListOf()).platformTick(at1942)
        assertEquals(listOf<String?>(null), p.resets)
        assertEquals(ScreenLockChange.REMOVED, h.records()!!.single().kind)
    }

    @Test
    fun `no notice, no reset`() {
        val p = FakePlatform()
        val c = FakeCore(CharterCore.ScreenLockDirective(id1, "123456", at1942))
        step(p, FakeTokens(), FakeHistory(writable = false), c, FakeNotice(), mutableListOf()).platformTick(at1942)
        assertTrue(p.resets.isEmpty())
        assertEquals(listOf(id1 to ScreenLockOutcome.NOTICE_FAILED), c.reports)
    }

    @Test
    fun `an inactive token is reported not ready and nothing is changed`() {
        val p = FakePlatform(active = false); val h = FakeHistory(); val n = FakeNotice()
        val c = FakeCore(CharterCore.ScreenLockDirective(id1, "123456", at1942))
        val s = step(p, FakeTokens(), h, c, n, mutableListOf())
        s.platformTick(at1942)
        s.noticeTick()
        assertTrue(p.resets.isEmpty())
        assertEquals(listOf(id1 to ScreenLockOutcome.NOT_READY), c.reports)
        assertEquals(false, c.state!!.second)
        assertNull(n.shown)
    }

    @Test
    fun `a throw while resetting is failed, and its message is not logged`() {
        val p = FakePlatform(resetResult = { throw IllegalArgumentException("password 123456 too weak") })
        val c = FakeCore(CharterCore.ScreenLockDirective(id1, "123456", at1942))
        val logs = mutableListOf<String>()
        step(p, FakeTokens(), FakeHistory(), c, FakeNotice(), logs).platformTick(at1942)
        assertEquals(listOf(id1 to ScreenLockOutcome.FAILED), c.reports)
        assertFalse(logs.joinToString(), logs.joinToString().contains("123456"))
    }

    @Test
    fun `the token is provisioned when none is stored, and restored once when inactive`() {
        val p = FakePlatform(active = false); val t = FakeTokens(token = null)
        val s = step(p, t, FakeHistory(), FakeCore(), FakeNotice(), mutableListOf())
        s.platformTick(1000)
        assertEquals(1, p.tokensSet.size)
        assertEquals(9, t.token!![0].toInt())
        // Pending after a provision (a phone with a PIN, not unlocked since):
        // nothing more this process.
        s.platformTick(1002)
        assertEquals(1, p.tokensSet.size)
        // A new process (after a reboot that dropped the pending token) sets
        // the stored one again, once.
        val s2 = step(p, t, FakeHistory(), FakeCore(), FakeNotice(), mutableListOf())
        s2.platformTick(1003)
        s2.platformTick(1004)
        assertEquals(2, p.tokensSet.size)
        assertEquals(9, p.tokensSet[1][0].toInt())
        // Seen active, then dropped again: one more restore.
        p.active = true; s2.platformTick(1006)
        p.active = false; s2.platformTick(1008); s2.platformTick(1010)
        assertEquals(3, p.tokensSet.size)
    }

    @Test
    fun `a working phone reports reset supported`() {
        val c = FakeCore()
        step(FakePlatform(), FakeTokens(), FakeHistory(), c, FakeNotice(), mutableListOf()).platformTick(1000)
        assertEquals(listOf(true), c.supported)
        // A fresh token the platform takes but holds pending (a PIN, no
        // unlock since): not ready, but supported.
        val p = FakePlatform(active = false); val c2 = FakeCore()
        step(p, FakeTokens(token = null), FakeHistory(), c2, FakeNotice(), mutableListOf()).platformTick(1000)
        assertEquals(true to false, c2.state)
        assertEquals(listOf(true), c2.supported)
    }

    @Test
    fun `a platform that refuses the token outright reports unsupported, steadily, and says so once`() {
        // Escrow switched off before Kintrinsic was Device Owner: every
        // setResetPasswordToken throws SecurityException, for good.
        val p = FakePlatform(active = false, setTokenResult = { throw SecurityException("Escrow token is disabled on the current user") })
        val t = FakeTokens(token = null); val c = FakeCore(); val logs = mutableListOf<String>()
        val s = step(p, t, FakeHistory(), c, FakeNotice(), logs)
        // Ticks every 10 s for three minutes: retried each minute, refused each time.
        for (now in 1000L..1180L step 10) s.platformTick(now)
        assertTrue(p.tokensSet.size >= 3)
        assertTrue(c.supported.isNotEmpty())
        assertTrue("never flaps between retries: ${c.supported}", c.supported.all { !it })
        assertEquals(false, c.state!!.second)
        // Each kind of refusal said once: the throw, the first provision, the retries.
        assertEquals(logs.toString(), 3, logs.size)
        assertEquals(logs.toString(), logs.size, logs.toSet().size)
        assertTrue(logs[0].contains("SecurityException"))
        assertFalse(logs.joinToString().contains("Escrow token"))
    }

    @Test
    fun `a stored token refused when set again also reports unsupported`() {
        val p = FakePlatform(active = false, setTokenResult = { throw SecurityException("nope") })
        val c = FakeCore()
        step(p, FakeTokens(), FakeHistory(), c, FakeNotice(), mutableListOf()).platformTick(1000)
        assertEquals(listOf(false), c.supported)
    }

    @Test
    fun `only SecurityException means unsupported, and a later take clears it`() {
        val p = FakePlatform(active = false, setTokenResult = { throw IllegalStateException("busy") })
        val t = FakeTokens(token = null); val c = FakeCore()
        val s = step(p, t, FakeHistory(), c, FakeNotice(), mutableListOf())
        s.platformTick(1000)
        assertEquals(listOf(true), c.supported)
        // A plain false is a refusal, not "for good".
        p.setTokenResult = { false }
        s.platformTick(1100)
        assertEquals(listOf(true, true), c.supported)
        p.setTokenResult = { throw SecurityException("nope") }
        s.platformTick(1200)
        assertEquals(false, c.supported.last())
        // The platform takes one after all (or holds one active): supported again.
        p.setTokenResult = { true }
        s.platformTick(1300)
        assertEquals(true, c.supported.last())
        assertTrue(t.token != null)
        val c2 = FakeCore()
        val p2 = FakePlatform(active = false, setTokenResult = { throw SecurityException("nope") })
        val s2 = step(p2, FakeTokens(token = null), FakeHistory(), c2, FakeNotice(), mutableListOf())
        s2.platformTick(1000)
        p2.active = true
        s2.platformTick(1010)
        assertEquals(listOf(false, true), c2.supported)
    }

    // ---- review 3 -----------------------------------------------------------------

    @Test
    fun `acknowledged id X, then a new record reusing X, shows the notice`() {
        val records = ScreenLockHistoryFormat.fold(
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942, outcome = ScreenLockOutcome.APPLIED),
                ScreenLockLine(id1, at1942 + 60, acknowledged = true),
                ScreenLockLine(id1, at1942 + 3600, kind = ScreenLockChange.REMOVED, outcome = ScreenLockOutcome.PENDING),
                ScreenLockLine(id1, at1942 + 3600, outcome = ScreenLockOutcome.APPLIED),
            ),
        )
        assertEquals(2, records.size)
        assertTrue(records[0].acknowledged)
        assertEquals(ScreenLockRecord(id1, at1942 + 3600, ScreenLockChange.REMOVED, ScreenLockOutcome.APPLIED, false), records[1])
        assertEquals(
            "Your guardian removed your screen lock at 20:42 on 14 Nov.",
            ScreenLockCopy.currentNotice(records, london),
        )
        // An acknowledgement after the second record is that record's.
        val acked = ScreenLockHistoryFormat.fold(
            listOf(
                ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.APPLIED),
                ScreenLockLine(id1, at1942 + 3600, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.APPLIED),
                ScreenLockLine(id1, at1942 + 3660, acknowledged = true),
            ),
        )
        assertEquals(listOf(false, true), acked.map { it.acknowledged })
    }

    @Test
    fun `a reset reusing an id already in the history is refused, failed, and changes nothing`() {
        val p = FakePlatform(); val h = FakeHistory(); val n = FakeNotice(); val logs = mutableListOf<String>()
        h.lines += ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.PENDING)
        h.lines += ScreenLockLine(id1, at1942, outcome = ScreenLockOutcome.APPLIED)
        h.lines += ScreenLockLine(id1, at1942 + 60, acknowledged = true)
        val c = FakeCore(CharterCore.ScreenLockDirective(id1, "123456", at1942 + 7200))
        val s = step(p, FakeTokens(), h, c, n, logs)
        s.platformTick(at1942 + 7200)
        s.noticeTick()
        assertTrue(p.resets.isEmpty())
        assertEquals(listOf(id1 to ScreenLockOutcome.FAILED), c.reports)
        assertEquals(3, h.lines.size)
        assertNull(n.shown)
        assertFalse(logs.joinToString().contains("123456"))
        // A fresh id still applies and tells the child.
        c.pending = CharterCore.ScreenLockDirective(id2, "123456", at1942 + 7300)
        s.platformTick(at1942 + 7300)
        s.noticeTick()
        assertEquals(listOf<String?>("123456"), p.resets)
        assertTrue(n.shown!!.startsWith("Your guardian changed your screen lock"))
    }

    @Test
    fun `a history that will not read keeps the notice up and blocks any reset`() {
        val p = FakePlatform(); val h = FakeHistory(); val n = FakeNotice()
        h.lines += ScreenLockLine(id1, at1942, kind = ScreenLockChange.CHANGED, outcome = ScreenLockOutcome.APPLIED)
        val c = FakeCore()
        val s = step(p, FakeTokens(), h, c, n, mutableListOf())
        s.noticeTick()
        val shown = n.shown!!
        h.readable = false
        s.noticeTick()
        assertEquals("unreadable is not 'nothing to tell'", shown, n.shown)
        c.pending = CharterCore.ScreenLockDirective(id2, "123456", at1942 + 100)
        s.platformTick(at1942 + 100)
        assertTrue(p.resets.isEmpty())
        assertEquals(listOf(id2 to ScreenLockOutcome.NOTICE_FAILED), c.reports)
        // Readable and acknowledged: now it may go.
        h.readable = true
        h.lines += ScreenLockLine(id1, at1942 + 200, acknowledged = true)
        s.noticeTick()
        assertNull(n.shown)
    }

    @Test
    fun `a history file that will not read is null, not empty`() {
        val dir = createTempDir()
        try {
            val f = FileScreenLockHistory.fileIn(dir)
            assertEquals(emptyList<ScreenLockRecord>(), FileScreenLockHistory(f).records())
            f.mkdirs() // a directory where the file belongs: every read throws
            assertNull(FileScreenLockHistory(f).records())
        } finally {
            dir.deleteRecursively()
        }
    }

    @Test
    fun `the token is kept before it is set, and never set when it cannot be kept`() {
        val p = FakePlatform(active = false)
        val t = object : ScreenLockTokenStore {
            var token: ByteArray? = null
            var saves = true
            override fun load() = token
            override fun save(token: ByteArray): Boolean { if (saves) this.token = token; return saves }
        }
        t.saves = false
        val c = FakeCore()
        val s2 = ScreenLockResetStep(p, t, FakeHistory(), c, FakeNotice(), zone = { london }, newToken = { ByteArray(32) { 9 } })
        s2.platformTick(1000)
        assertTrue("never set without being kept", p.tokensSet.isEmpty())
        t.saves = true
        s2.platformTick(1100)
        assertEquals(1, p.tokensSet.size)
        assertEquals(9, t.token!![0].toInt())
    }

    @Test
    fun `ready needs the token active AND held by this app, and an active one it lacks is replaced aloud`() {
        // The platform holds an active token, but this app has none.
        val p = FakePlatform(active = true); val t = FakeTokens(token = null); val c = FakeCore(); val logs = mutableListOf<String>()
        val refusingSave = object : ScreenLockTokenStore {
            override fun load(): ByteArray? = null
            override fun save(token: ByteArray) = false
        }
        ScreenLockResetStep(p, refusingSave, FakeHistory(), c, FakeNotice(), zone = { london }, log = { logs += it })
            .platformTick(1000)
        assertEquals("active but not ours is not ready", true to false, c.state)
        assertTrue(p.tokensSet.isEmpty())
        assertTrue(logs.toString(), logs.any { it.contains("active reset token this app does not have") })
        // Kept and set: replaced, and said so.
        val logs2 = mutableListOf<String>()
        step(p, t, FakeHistory(), c, FakeNotice(), logs2).platformTick(1000)
        assertEquals(1, p.tokensSet.size)
        assertTrue(logs2.toString(), logs2.any { it.contains("replacing it") })
        assertEquals(true to true, c.state)
    }

    @Test
    fun `a token read that throws provisions nothing that tick`() {
        val p = FakePlatform(active = true)
        val throwing = object : ScreenLockTokenStore {
            override fun load(): ByteArray? = throw IllegalStateException("prefs")
            override fun save(token: ByteArray) = true
        }
        val c = FakeCore()
        ScreenLockResetStep(p, throwing, FakeHistory(), c, FakeNotice(), zone = { london }).platformTick(1000)
        assertTrue(p.tokensSet.isEmpty())
        assertEquals(true to false, c.state)
    }

    @Test
    fun `an outcome the core cannot take is sent again until it does`() {
        val p = FakePlatform(); val c = FakeCore(CharterCore.ScreenLockDirective(id1, "123456", at1942))
        c.takes = false
        val s = step(p, FakeTokens(), FakeHistory(), c, FakeNotice(), mutableListOf())
        s.platformTick(at1942)
        s.platformTick(at1942 + 10)
        assertEquals(listOf(id1 to ScreenLockOutcome.APPLIED, id1 to ScreenLockOutcome.APPLIED), c.attempts)
        assertTrue(c.reports.isEmpty())
        c.takes = true
        s.platformTick(at1942 + 20)
        s.platformTick(at1942 + 30)
        assertEquals(listOf(id1 to ScreenLockOutcome.APPLIED), c.reports)
        assertEquals("not sent again once taken", 3, c.attempts.size)
        assertEquals(listOf<String?>("123456"), p.resets)
    }

    @Test
    fun `the directive never prints its PIN`() {
        val d = CharterCore.ScreenLockDirective(id1, "123456", at1942)
        assertFalse(d.toString().contains("123456"))
    }

    @Test
    fun `a take answer parses, and nothing else does`() {
        val d = CharterCore.parseScreenLockDirective("""{"id":"$id1","newPin":"123456","ts":$at1942}""")!!
        assertEquals(id1, d.id); assertEquals("123456", d.newPin); assertEquals(at1942, d.ts)
        assertTrue(CharterCore.parseScreenLockDirective("""{"id":"$id1","newPin":"","ts":1}""")!!.removes)
        assertNull(CharterCore.parseScreenLockDirective(""))
        assertNull(CharterCore.parseScreenLockDirective("""{"error":"not initialized"}"""))
        assertNull(CharterCore.parseScreenLockDirective("""{"id":"$id1","newPin":"123"""))
        assertNull(CharterCore.parseScreenLockDirective("""{"id":"$id1","ts":1}"""))
    }
}
