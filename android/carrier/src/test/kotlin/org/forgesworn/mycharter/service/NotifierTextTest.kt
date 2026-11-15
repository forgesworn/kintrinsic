package org.forgesworn.mycharter.service

import android.app.NotificationManager
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.WardName
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.forgesworn.mycharter.carrier.RosterEntry
import org.junit.Test

/**
 * The pure text builders behind [Notifier.notifyOverride] and
 * [Notifier.notifyRequest]. Building a real [android.app.Notification]
 * needs a [android.content.Context] this module's host tests don't have —
 * these two functions are split out specifically so the wording (the part
 * that actually changed for the roster feature) is JVM-testable.
 */
class NotifierTextTest {
    private val who = WardName("Mia", "Pixel 6")

    // ---- overrideText ---------------------------------------------------

    @Test fun overrideNoHitIsExactlyTodaysString() {
        assertEquals(
            "Your ward opened their phone for 10 minutes.",
            Notifier.overrideText("full", 10, who = null),
        )
        assertEquals(
            "Your ward opened calls for 10 minutes.",
            Notifier.overrideText("calls", 10, who = null),
        )
    }

    @Test fun overrideHitNamesTheWardAndDevice() {
        assertEquals(
            "Mia opened their phone (Pixel 6) for 10 minutes.",
            Notifier.overrideText("full", 10, who),
        )
    }

    @Test fun overrideHitStillCarriesScopeAndMinutes() {
        assertEquals(
            "Mia opened calls (Pixel 6) for 3 minutes.",
            Notifier.overrideText("calls", 3, who),
        )
    }

    // ---- emergency call button ---------------------------------------------

    @Test fun emergencyCallHitNamesTheWardAndDevice() {
        assertEquals(
            "Mia used the emergency call button on Pixel 6.",
            Notifier.emergencyCallText(who),
        )
    }

    @Test fun emergencyCallNoHitNamesNobody() {
        assertEquals(
            "Your ward used the emergency call button.",
            Notifier.emergencyCallText(null),
        )
    }

    @Test fun emergencyCallTitleIsFixedAndNeverClaimsACall() {
        assertEquals("Emergency call button used", Notifier.EMERGENCY_CALL_TITLE)
    }

    @Test fun emergencyCallNeverUsesTheUnlockWording() {
        for (w in listOf(who, null)) {
            val text = Notifier.emergencyCallText(w)
            assertFalse(text.contains("opened calls"))
            assertFalse(text.contains("minutes"))
        }
    }

    @Test fun otherEmergencyControlIsGenericAndHonest() {
        assertEquals(
            "Mia: an emergency control was used on Pixel 6",
            Notifier.emergencyControlText(who),
        )
        assertEquals(
            "Your ward: an emergency control was used",
            Notifier.emergencyControlText(null),
        )
        assertFalse(Notifier.emergencyControlText(who).contains("opened"))
    }

    @Test fun noEmergencyCopyClaimsTheDiallerOpenedOrACallWasPlaced() {
        val all = listOf(
            Notifier.EMERGENCY_CALL_TITLE,
            Notifier.EMERGENCY_CONTROL_TITLE,
            Notifier.UNPAIRED_TITLE,
            Notifier.emergencyCallText(who),
            Notifier.emergencyCallText(who, Notifier.Release.BEFORE),
            Notifier.emergencyCallText(who, Notifier.Release.AFTER),
            Notifier.emergencyCallText(who, Notifier.Release.UNKNOWN),
            Notifier.emergencyCallText(null),
            Notifier.unpairedText(3),
        )
        for (t in all) {
            assertFalse(t, t.contains("dialler", ignoreCase = true))
            assertFalse(t, t.contains("a call was", ignoreCase = true))
            assertFalse(t, t.contains("called", ignoreCase = true))
        }
        assertTrue(Notifier.emergencyCallText(who).contains("used the emergency call button"))
    }

    // ---- M1/G10: a device the guardian has since disconnected ---------------------------

    private val before = Notifier.Release.BEFORE
    private val after = Notifier.Release.AFTER
    private val unknown = Notifier.Release.UNKNOWN

    @Test fun aPressBeforeTheDisconnectSaysSo() {
        assertEquals(
            "Mia used the emergency call button on Pixel 6, before you disconnected that phone.",
            Notifier.emergencyCallText(who, before),
        )
        assertEquals(
            "Mia opened their phone (Pixel 6) for 10 minutes, before you disconnected that phone.",
            Notifier.overrideText("full", 10, who, before),
        )
        assertEquals(
            "Mia: an emergency control was used on Pixel 6, before you disconnected that phone.",
            Notifier.emergencyControlText(who, before),
        )
    }

    @Test fun aPressAfterTheDisconnectSaysSoAndWarnsTheReleaseMayNotHaveArrived() {
        val warn = "The phone may not have received the disconnect."
        assertEquals(
            "Mia used the emergency call button on Pixel 6 after you disconnected that phone. $warn",
            Notifier.emergencyCallText(who, after),
        )
        assertEquals(
            "Mia opened calls (Pixel 6) for 3 minutes after you disconnected that phone. $warn",
            Notifier.overrideText("calls", 3, who, after),
        )
        assertEquals(
            "Mia used the emergency unlock on Pixel 6 after you disconnected that phone. $warn",
            Notifier.overrideText("", null, who, after),
        )
        assertEquals(
            "Mia: an emergency control was used on Pixel 6 after you disconnected that phone. $warn",
            Notifier.emergencyControlText(who, after),
        )
    }

    @Test fun aPressWithNoUsableTimeClaimsNoOrder() {
        assertEquals(
            "Mia used the emergency call button on Pixel 6, a phone you have disconnected.",
            Notifier.emergencyCallText(who, unknown),
        )
        assertEquals(
            "Mia opened their phone (Pixel 6) for 10 minutes, a phone you have disconnected.",
            Notifier.overrideText("full", 10, who, unknown),
        )
        assertEquals(
            "Mia: an emergency control was used on Pixel 6, a phone you have disconnected.",
            Notifier.emergencyControlText(who, unknown),
        )
        for (t in listOf(
            Notifier.emergencyCallText(who, unknown),
            Notifier.overrideText("full", 10, who, unknown),
            Notifier.emergencyControlText(who, unknown),
        )) {
            assertFalse(t, t.contains("before"))
            assertFalse(t, t.contains("after"))
        }
    }

    @Test fun theOrderIsJudgedByThePressTimeAgainstTheDisconnect() {
        assertEquals(before, Notifier.releaseOrder(at = 100, releasedAt = 200))
        assertEquals(before, Notifier.releaseOrder(at = 200, releasedAt = 200)) // at <= releasedAt
        assertEquals(after, Notifier.releaseOrder(at = 201, releasedAt = 200))
        assertEquals(unknown, Notifier.releaseOrder(at = null, releasedAt = 200))
        assertEquals(unknown, Notifier.releaseOrder(at = 100, releasedAt = null))
        assertEquals(unknown, Notifier.releaseOrder(at = null, releasedAt = null))
    }

    @Test fun onlyAReleasedMachineHasAnOrderAndItComesFromTheRoster() {
        val roster = listOf(
            RosterEntry("m1", "Mia", "Pixel 6", paired = false, released = true, releasedAt = 1000),
            RosterEntry("m2", "Leo", "Pixel 4a"),
            RosterEntry("m3", "Jo", "Old", paired = false, released = true),
        )
        assertEquals(before, Notifier.releaseOf("m1", roster, at = 999))
        assertEquals(after, Notifier.releaseOf("m1", roster, at = 1001))
        assertEquals(unknown, Notifier.releaseOf("m1", roster, at = null))
        assertEquals(unknown, Notifier.releaseOf("m3", roster, at = 5)) // no releasedAt on file
        assertNull(Notifier.releaseOf("m2", roster, at = 5))
        assertNull(Notifier.releaseOf("nobody", roster, at = 5))
    }

    @Test fun anAuditWithoutAStampedAtFromAReleasedMachineClaimsNoOrder() {
        // Only the audit's own `at` is compared; with none, the no-order form is used
        // (never a fallback such as the wrap's created_at or the receive time).
        val roster = listOf(RosterEntry("m1", "Mia", "Pixel 6", paired = false, released = true, releasedAt = 1000))
        val release = Notifier.releaseOf("m1", roster, at = null)
        assertEquals(unknown, release)
        assertEquals(
            "Mia used the emergency call button on Pixel 6, a phone you have disconnected.",
            Notifier.emergencyCallText(WardName("Mia", "Pixel 6"), release),
        )
        // A press in the same second as the disconnect counts as before.
        assertEquals(before, Notifier.releaseOf("m1", roster, at = 1000))
    }

    @Test fun aDeviceThatIsNotReleasedKeepsTheOriginalWording() {
        assertEquals("Mia used the emergency call button on Pixel 6.", Notifier.emergencyCallText(who, release = null))
        assertEquals("Mia opened their phone (Pixel 6) for 10 minutes.", Notifier.overrideText("full", 10, who, release = null))
    }

    // ---- G8: an unlock never invents a scope or a duration -------------------------------

    @Test fun anUnknownScopeSaysTheEmergencyUnlockWasUsedWithNoScopeOrDuration() {
        assertEquals("Mia used the emergency unlock on Pixel 6.", Notifier.overrideText("", 10, who))
        assertEquals("Mia used the emergency unlock on Pixel 6.", Notifier.overrideText("something-new", 1, who))
        assertEquals("Your ward used the emergency unlock.", Notifier.overrideText("", 10, null))
        for (t in listOf(Notifier.overrideText("", 10, who), Notifier.overrideText("x", 10, null))) {
            assertFalse(t, t.contains("minute"))
            assertFalse(t, t.contains("calls"))
        }
    }

    @Test fun oneMinuteIsSingularAndNoDurationLeavesItOut() {
        assertEquals("Mia opened calls (Pixel 6) for 1 minute.", Notifier.overrideText("calls", 1, who))
        assertEquals("Your ward opened their phone for 1 minute.", Notifier.overrideText("full", 1, null))
        assertEquals("Mia opened calls (Pixel 6).", Notifier.overrideText("calls", null, who))
        assertEquals("Your ward opened their phone.", Notifier.overrideText("full", null, null))
    }

    // ---- C2: unpaired senders share one row -------------------------------------------

    @Test fun theUnpairedRowSaysHowManyReportsWithoutNamingAnyone() {
        assertEquals("Reports from devices you haven't paired: 1", Notifier.unpairedText(1))
        assertEquals("Reports from devices you haven't paired: 7", Notifier.unpairedText(7))
        assertEquals("Reports from devices you haven't paired: 1", Notifier.unpairedText(0))
        val cap = CarrierStore.UNPAIRED_CAP
        assertEquals("Reports from devices you haven't paired: $cap or more", Notifier.unpairedText(cap))
        assertFalse(Notifier.unpairedText(2).contains("your ward", ignoreCase = true))
    }

    @Test fun onlyAKnownRosterWithNoEntryMakesASenderUnpaired() {
        val roster = listOf(RosterEntry("m1", "Mia", "Pixel 6"))
        assertFalse(Notifier.isUnpaired("m1", roster, rosterKnown = true))
        assertTrue(Notifier.isUnpaired("m2", roster, rosterKnown = true))
        // No roster pushed yet: the old "Your ward" wording, never "unpaired".
        assertFalse(Notifier.isUnpaired("m2", roster, rosterKnown = false))
        assertFalse(Notifier.isUnpaired("m2", emptyList(), rosterKnown = false))
        assertTrue(Notifier.isUnpaired("m2", emptyList(), rosterKnown = true))
    }

    @Test fun aReleasedDeviceIsNotUnpaired() {
        val roster = listOf(RosterEntry("m1", "Mia", "Pixel 6", paired = false, released = true))
        assertFalse(Notifier.isUnpaired("m1", roster, rosterKnown = true))
    }

    // ---- G1: only a PAIRED machine's ask is named ------------------------------------------

    @Test fun anAskIsUnpairedUnlessTheKnownRosterPairsTheMachine() {
        val roster = listOf(
            RosterEntry("m1", "Mia", "Pixel 6"),
            RosterEntry("m2", "Leo", "Pixel 4a", paired = false, released = true),
            RosterEntry("m3", "Jo", "Tab", paired = false),
        )
        assertFalse(Notifier.isUnpairedAsk("m1", roster, rosterKnown = true))
        assertTrue("a released machine", Notifier.isUnpairedAsk("m2", roster, rosterKnown = true))
        assertTrue("a machine the console says is not paired", Notifier.isUnpairedAsk("m3", roster, rosterKnown = true))
        assertTrue("a stranger", Notifier.isUnpairedAsk("zz", roster, rosterKnown = true))
        // No known roster: the old wording, never "unpaired".
        assertFalse(Notifier.isUnpairedAsk("zz", roster, rosterKnown = false))
        assertFalse(Notifier.isUnpairedAsk("zz", emptyList(), rosterKnown = false))
    }

    @Test fun theUnpairedAskRowSaysHowManyWithoutNamingAnyone() {
        assertEquals("Requests from phones that aren't paired: 1", Notifier.unpairedAskText(1))
        assertEquals("Requests from phones that aren't paired: 9", Notifier.unpairedAskText(9))
        assertEquals("Requests from phones that aren't paired: 1", Notifier.unpairedAskText(0))
        val cap = CarrierStore.UNPAIRED_CAP
        assertEquals("Requests from phones that aren't paired: $cap or more", Notifier.unpairedAskText(cap))
        assertEquals("ask-unpaired", Notifier.TAG_ASK_UNPAIRED)
        assertNotEquals(Notifier.TAG_ASK_UNPAIRED, Notifier.TAG_UNPAIRED)
    }

    @Test fun theOldestPairedAskRowsBeyondTheCapAreCancelledAndTheSharedRowIsNeverOne() {
        val rows = (1..25).map { Notifier.LiveRow("ask$it", it.toLong()) }.reversed() +
            Notifier.LiveRow(Notifier.TAG_ASK_UNPAIRED, 0L) + Notifier.LiveRow(null, 0L)
        val cancel = Notifier.askRowsToCancel(rows, keep = Notifier.ASK_ROW_CAP - 1)
        // 25 live, room for the 20th: the oldest six go, oldest first.
        assertEquals(listOf("ask1", "ask2", "ask3", "ask4", "ask5", "ask6"), cancel)
        assertEquals(20, Notifier.ASK_ROW_CAP)
        assertEquals(emptyList<String>(), Notifier.askRowsToCancel(rows.take(10), keep = 19))
        // Exactly at the cap before the new one: the oldest one makes room.
        assertEquals(listOf("ask1"), Notifier.askRowsToCancel((1..20).map { Notifier.LiveRow("ask$it", it.toLong()) }, keep = 19))
    }

    // ---- G4: a released machine's live emergency rows are counted by tag --------------------

    @Test fun aRowBelongsToAMachineByItsTagAndNeverByAPrefixOfIt() {
        assertTrue(Notifier.rowBelongsTo(Notifier.alertKey("override", "m1", 100, null), "m1"))
        assertTrue(Notifier.rowBelongsTo(Notifier.alertKey("emergency-call", "m1", null, "w"), "m1"))
        assertTrue(Notifier.rowBelongsTo(Notifier.alertKey("emergency-control", "m1", null, null), "m1"))
        assertFalse(Notifier.rowBelongsTo(Notifier.alertKey("emergency-call", "m10", 100, null), "m1"))
        assertFalse(Notifier.rowBelongsTo(Notifier.TAG_UNPAIRED, "m1"))
        assertFalse(Notifier.rowBelongsTo("ask-m1", "m1"))
        assertFalse(Notifier.rowBelongsTo(Notifier.alertKey("emergency-call", "m1", 1, null), ""))
    }

    @Test fun rowsOfCountsOnlyThatMachinesEmergencyRows() {
        val rows = listOf(
            Notifier.LiveRow("emergency-call-m1-1", 1), Notifier.LiveRow("override-m1-2", 2),
            Notifier.LiveRow("emergency-call-m2-1", 1), Notifier.LiveRow(Notifier.TAG_UNPAIRED, 3),
            Notifier.LiveRow(null, 4),
        )
        assertEquals(2, Notifier.rowsOf(rows, "m1"))
        assertEquals(1, Notifier.rowsOf(rows, "m2"))
        assertEquals(0, Notifier.rowsOf(rows, "m3"))
    }

    @Test fun everyRowUsesTheTagFormUnderAFixedId() {
        // Ids that are not derived from sender-influenced strings; the service row's is its own.
        val ids = setOf(Notifier.SERVICE_NOTIF_ID, Notifier.NOTIF_ID_EMERGENCY, Notifier.NOTIF_ID_REQUEST)
        assertEquals(3, ids.size)
    }

    // ---- C3: the Emergencies channel state, for the console ----------------------------

    @Test fun alertStateSaysWhetherNotificationsAndTheChannelAreOn() {
        val high = NotificationManager.IMPORTANCE_HIGH
        assertEquals("""{"notifications":true,"channel":true}""", Notifier.alertStateJson(true, high))
        assertEquals("""{"notifications":false,"channel":true}""", Notifier.alertStateJson(false, high))
        assertEquals(
            """{"notifications":true,"channel":false}""",
            Notifier.alertStateJson(true, NotificationManager.IMPORTANCE_NONE),
        )
        assertEquals("""{"notifications":true,"channel":false}""", Notifier.alertStateJson(true, null))
        assertEquals("""{"notifications":false,"channel":false}""", Notifier.alertStateJson(false, null))
    }

    @Test fun aQuietButNotMutedChannelCountsAsOn() {
        assertEquals(
            """{"notifications":true,"channel":true}""",
            Notifier.alertStateJson(true, NotificationManager.IMPORTANCE_LOW),
        )
    }

    // ---- one row per press, and the press time ---------------------------------

    @Test fun alertKeyIsPerPressWhenTheWardStampedATime() {
        assertEquals("emergency-call-m1-100", Notifier.alertKey("emergency-call", "m1", 100L, "w"))
        assertEquals("override-m1-100", Notifier.alertKey("override", "m1", 100L, null))
    }

    @Test fun aRebuiltWrapForTheSamePressReusesItsRow() {
        assertEquals(
            Notifier.alertKey("emergency-call", "m1", 100L, "wrapA"),
            Notifier.alertKey("emergency-call", "m1", 100L, "wrapB"),
        )
    }

    @Test fun twoPressesGetTwoRows() {
        assertNotEquals(
            Notifier.alertKey("emergency-call", "m1", 100L, null),
            Notifier.alertKey("emergency-call", "m1", 160L, null),
        )
    }

    // ---- C6: one press, whichever wrap carries it ---------------------------------

    @Test fun aPressKeyExistsOnlyWhenTheWardStampedATimeAndIgnoresTheWrapId() {
        assertNull(Notifier.pressKey(Notifier.AuditAction.EMERGENCY_CALL, "m1", null))
        assertEquals(
            "press:emergency-call-m1-100",
            Notifier.pressKey(Notifier.AuditAction.EMERGENCY_CALL, "m1", 100L),
        )
        assertEquals("press:override-m1-100", Notifier.pressKey(Notifier.AuditAction.UNLOCK, "m1", 100L))
        assertEquals("press:emergency-control-m1-100", Notifier.pressKey(Notifier.AuditAction.OTHER_CONTROL, "m1", 100L))
    }

    @Test fun aPressKeyIsPerMachineAndPerKind() {
        val call = Notifier.pressKey(Notifier.AuditAction.EMERGENCY_CALL, "m1", 100L)
        assertNotEquals(call, Notifier.pressKey(Notifier.AuditAction.EMERGENCY_CALL, "m2", 100L))
        assertNotEquals(call, Notifier.pressKey(Notifier.AuditAction.UNLOCK, "m1", 100L))
        assertNotEquals(call, Notifier.pressKey(Notifier.AuditAction.EMERGENCY_CALL, "m1", 101L))
    }

    @Test fun anUnlockAndACallNeverShareARow() {
        assertNotEquals(
            Notifier.alertKey("override", "m1", 100L, null),
            Notifier.alertKey("emergency-call", "m1", 100L, null),
        )
    }

    @Test fun withoutATimeTheRowFallsBackToTheWrapIdThenTheMachine() {
        assertEquals("emergency-call-m1-wrap-abc", Notifier.alertKey("emergency-call", "m1", null, "abc"))
        assertEquals("emergency-call-m1", Notifier.alertKey("emergency-call", "m1", null, null))
    }

    @Test fun whenIsThePressTimeInMillisWhenPlausible() {
        val now = 1_800_000_000L
        assertEquals((now - 60) * 1000, Notifier.plausibleWhenMillis(now - 60, now))
        assertEquals(now * 1000, Notifier.plausibleWhenMillis(now, now))
        assertEquals(
            (now - Notifier.MAX_PRESS_AGE_SECS) * 1000,
            Notifier.plausibleWhenMillis(now - Notifier.MAX_PRESS_AGE_SECS, now),
        )
    }

    @Test fun whenIsLeftToThePlatformWhenImplausible() {
        val now = 1_800_000_000L
        assertNull(Notifier.plausibleWhenMillis(null, now))
        assertNull(Notifier.plausibleWhenMillis(now + 1, now))
        assertNull(Notifier.plausibleWhenMillis(now - Notifier.MAX_PRESS_AGE_SECS - 1, now))
        assertNull(Notifier.plausibleWhenMillis(0L, now))
        assertNull(Notifier.plausibleWhenMillis(-1L, now))
    }

    // ---- auditAction (the dispatch within outcome=override) ------------------

    @Test fun overrideWithBreakGlassOrNoOpIsTheUnlockAlert() {
        assertEquals(Notifier.AuditAction.UNLOCK, Notifier.auditAction("override", "unlock.breakglass"))
        assertEquals(Notifier.AuditAction.UNLOCK, Notifier.auditAction("override", ""))
    }

    @Test fun overrideWithCallEmergencyIsTheEmergencyCallAlert() {
        assertEquals(Notifier.AuditAction.EMERGENCY_CALL, Notifier.auditAction("override", "call.emergency"))
    }

    @Test fun overrideWithAnyOtherOpIsTheGenericAlert() {
        assertEquals(Notifier.AuditAction.OTHER_CONTROL, Notifier.auditAction("override", "something.new"))
        assertEquals(Notifier.AuditAction.OTHER_CONTROL, Notifier.auditAction("override", "Call.Emergency"))
    }

    @Test fun anEmergencyCallAlertsWhateverItsOutcome() {
        for (outcome in listOf("override", "applied", "failed", "enacted", "", "Override", "something-new")) {
            assertEquals(outcome, Notifier.AuditAction.EMERGENCY_CALL, Notifier.auditAction(outcome, "call.emergency"))
        }
    }

    @Test fun anyOtherOutcomeIsIgnoredWhateverTheOtherOp() {
        for (op in listOf("unlock.breakglass", "", "screenlock.reset")) {
            for (outcome in listOf("applied", "failed", "enacted", "", "Override")) {
                assertEquals(Notifier.AuditAction.IGNORE, Notifier.auditAction(outcome, op))
            }
        }
    }

    // ---- requestText ------------------------------------------------------

    @Test fun requestNoHitIsExactlyTodaysStrings() {
        assertEquals(
            "Your ward asks for 30 more minutes",
            Notifier.requestText("time.extend", 30, who = null),
        )
        assertEquals(
            "Your ward asks to install an app",
            Notifier.requestText("install.apk", null, who = null),
        )
        assertEquals(
            "Your ward sent a request",
            Notifier.requestText("something.else", null, who = null),
        )
    }

    @Test fun requestHitNamesTheWardAndDevice() {
        assertEquals(
            "Mia (Pixel 6) asks for 30 more minutes",
            Notifier.requestText("time.extend", 30, who),
        )
        assertEquals(
            "Mia (Pixel 6) asks to install an app",
            Notifier.requestText("install.apk", null, who),
        )
        assertEquals(
            "Mia (Pixel 6) sent a request",
            Notifier.requestText("something.else", null, who),
        )
    }

    @Test fun requestTimeExtendWithNoMinutesFallsBackToGenericSentRequest() {
        // op == "time.extend" but minutes == null misses the first branch —
        // same behaviour before and after the roster change.
        assertEquals(
            "Your ward sent a request",
            Notifier.requestText("time.extend", null, who = null),
        )
    }
}
