package org.forgesworn.mycharter.service

import org.forgesworn.mycharter.carrier.WardName
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
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
            "Mia used the emergency call button on Pixel 6. Their phone's emergency dialler opened.",
            Notifier.emergencyCallText(who),
        )
    }

    @Test fun emergencyCallNoHitNamesNobody() {
        assertEquals(
            "Your ward used the emergency call button. Their phone's emergency dialler opened.",
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
