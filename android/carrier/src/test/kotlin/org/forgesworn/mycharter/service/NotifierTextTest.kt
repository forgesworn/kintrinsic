package org.forgesworn.mycharter.service

import org.forgesworn.mycharter.carrier.WardName
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
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
            Notifier.alertKey("emergency-call", "m1", 100L, null).hashCode(),
            Notifier.alertKey("emergency-call", "m1", 160L, null).hashCode(),
        )
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

    @Test fun anyOtherOutcomeIsIgnoredWhateverTheOp() {
        for (op in listOf("call.emergency", "unlock.breakglass", "", "screenlock.reset")) {
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
