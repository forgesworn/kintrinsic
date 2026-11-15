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
            Notifier.emergencyCallText(who, released = true),
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

    // ---- C4: a device the guardian has since disconnected ---------------------------

    @Test fun aReleasedDevicesAlertNamesTheChildAndDeviceAndSaysItWasBeforeTheDisconnect() {
        assertEquals(
            "Mia used the emergency call button on Pixel 6, before you disconnected that phone.",
            Notifier.emergencyCallText(who, released = true),
        )
        assertEquals(
            "Mia opened their phone (Pixel 6) for 10 minutes, before you disconnected that phone.",
            Notifier.overrideText("full", 10, who, released = true),
        )
        assertEquals(
            "Mia: an emergency control was used on Pixel 6, before you disconnected that phone",
            Notifier.emergencyControlText(who, released = true),
        )
    }

    @Test fun aDeviceThatIsNotReleasedKeepsTheOriginalWording() {
        assertEquals("Mia used the emergency call button on Pixel 6.", Notifier.emergencyCallText(who, released = false))
        assertEquals("Mia opened their phone (Pixel 6) for 10 minutes.", Notifier.overrideText("full", 10, who, released = false))
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
