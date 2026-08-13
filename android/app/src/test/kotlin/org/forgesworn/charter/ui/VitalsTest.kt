package org.forgesworn.charter.ui

import android.telephony.ServiceState
import android.telephony.TelephonyManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The pure half of the shade's vital signs — thresholds, normalisation and
 * labels. The drawing and the platform reads live in views; everything that
 * can be got WRONG (a red battery that should be green, a 5-level radio
 * squashed onto 4 bars) is a plain function, and is tested here.
 */
class VitalsTest {

    @Test fun batteryTintBandsByCharge() {
        assertEquals(Vitals.COLOR_OK, Vitals.batteryColor(100))
        assertEquals(Vitals.COLOR_OK, Vitals.batteryColor(31))
        assertEquals(Vitals.COLOR_LOW, Vitals.batteryColor(30))
        assertEquals(Vitals.COLOR_LOW, Vitals.batteryColor(16))
        assertEquals(Vitals.COLOR_CRITICAL, Vitals.batteryColor(15))
        assertEquals(Vitals.COLOR_CRITICAL, Vitals.batteryColor(0))
    }

    @Test fun batteryTintIsMutedWhenUnknown() {
        assertEquals(Vitals.COLOR_MUTED, Vitals.batteryColor(null))
    }

    @Test fun batteryTextIsHonestWhenUnknown() {
        assertEquals("82%", Vitals.batteryText(82))
        assertEquals("—", Vitals.batteryText(null))
    }

    @Test fun batteryPercentIsScaledAndClamped() {
        assertEquals(50, Vitals.batteryPercent(50, 100))
        // Some devices report a scale of 255, not 100.
        assertEquals(50, Vitals.batteryPercent(128, 255))
        assertEquals(100, Vitals.batteryPercent(200, 100))
        assertEquals(0, Vitals.batteryPercent(0, 100))
    }

    /**
     * Any negative level is the platform declining to say — NOT a flat
     * battery. Rendering it as "0%" in red would invent the most alarming
     * reading there is out of missing data.
     */
    @Test fun batteryPercentIsNullWhenThePlatformWontSay() {
        assertNull(Vitals.batteryPercent(-1, 100))
        assertNull(Vitals.batteryPercent(-3, 100))
        assertNull(Vitals.batteryPercent(50, 0))
        assertNull(Vitals.batteryPercent(50, -1))
    }

    /** Radios report 0..max where max is NOT always 4; the gauge draws 4. */
    @Test fun signalLevelsNormaliseOntoFourBars() {
        assertEquals(4, Vitals.bars(4, 4))
        assertEquals(0, Vitals.bars(0, 4))
        assertEquals(2, Vitals.bars(2, 4))
        // A 0..5 radio (some Wi-Fi stacks report maxSignalLevel = 5).
        assertEquals(4, Vitals.bars(5, 5))
        assertEquals(2, Vitals.bars(3, 5))
        assertEquals(0, Vitals.bars(0, 5))
    }

    @Test fun signalLevelsClampOutOfRangeInput() {
        assertEquals(4, Vitals.bars(9, 4))
        assertEquals(0, Vitals.bars(-2, 4))
        // A nonsense max must not divide by zero or invent bars.
        assertEquals(0, Vitals.bars(3, 0))
    }

    @Test fun unknownSignalStaysUnknown() {
        assertNull(Vitals.bars(null, 4))
    }

    @Test fun networkGenerationLabels() {
        assertEquals("5G", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_NR))
        assertEquals("4G", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_LTE))
        assertEquals("3G", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_UMTS))
        assertEquals("3G", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_HSPAP))
        assertEquals("2G", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_EDGE))
        assertEquals("2G", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_GSM))
    }

    /** Never invent a generation we can't name — the chip just says "Mobile". */
    @Test fun unknownNetworkTypeFallsBackToPlainMobile() {
        assertEquals("Mobile", Vitals.mobileGeneration(TelephonyManager.NETWORK_TYPE_UNKNOWN))
        assertEquals("Mobile", Vitals.mobileGeneration(-99))
    }

    /**
     * A SIM being present is NOT the same as being able to reach anyone: in
     * airplane mode the SIM still reads READY. Keying the chip off SIM state
     * alone showed a healthy "Mobile" on a phone that could not place a call —
     * the exact lie this strip exists to prevent.
     */
    @Test fun radioReachComesFromServiceNotSimPresence() {
        assertEquals(
            Vitals.Reach(mobile = true, emergencyOnly = false),
            Vitals.reachFrom(ServiceState.STATE_IN_SERVICE),
        )
        assertEquals(
            Vitals.Reach(mobile = false, emergencyOnly = false),
            Vitals.reachFrom(ServiceState.STATE_OUT_OF_SERVICE),
        )
        // Airplane mode / radio off.
        assertEquals(
            Vitals.Reach(mobile = false, emergencyOnly = false),
            Vitals.reachFrom(ServiceState.STATE_POWER_OFF),
        )
        // Unknowable: claim nothing.
        assertEquals(
            Vitals.Reach(mobile = false, emergencyOnly = false),
            Vitals.reachFrom(null),
        )
    }

    /**
     * "Emergency calls only" is its own state and must survive to the glass.
     * On a lifeline screen the difference between "you cannot call anyone" and
     * "you can still call 999" is the whole point of the feature.
     */
    @Test fun emergencyOnlyIsItsOwnState() {
        val reach = Vitals.reachFrom(ServiceState.STATE_EMERGENCY_ONLY)
        assertEquals(Vitals.Reach(mobile = false, emergencyOnly = true), reach)
        assertEquals(
            "Emergency calls only",
            Vitals.mobileChipLabel(reach, airplane = false, generation = "4G"),
        )
    }

    @Test fun chipLabelNamesTheActualCause() {
        val inService = Vitals.Reach(mobile = true, emergencyOnly = false)
        assertEquals("4G", Vitals.mobileChipLabel(inService, false, "4G"))

        // Airplane mode is actionable in a way "No signal" is not — say it.
        val dead = Vitals.Reach(mobile = false, emergencyOnly = false)
        assertEquals("Airplane mode", Vitals.mobileChipLabel(dead, airplane = true, generation = "4G"))
        assertEquals("", Vitals.mobileChipLabel(dead, airplane = false, generation = "4G"))
    }

    /**
     * The offline case is the one that matters most on a locked phone: if the
     * ward has neither radio, the shade must SAY so rather than show a silent
     * empty corner — that is the difference between "my phone is broken" and
     * "I'm out of signal, walk to the window".
     */
    @Test fun offlineIsStatedNotImplied() {
        val dark = Vitals.Snapshot(
            batteryPercent = 44, charging = false,
            wifi = false, wifiLevel = null,
            mobile = false, emergencyOnly = false,
            mobileLevel = null, mobileLabel = "",
        )
        assertEquals(true, dark.offline)

        val onWifi = dark.copy(wifi = true, wifiLevel = 3)
        assertEquals(false, onWifi.offline)

        // Radio in service but zero bars is still "not offline" — the phone is
        // registered; it just can't reach much from here.
        val onRadio = dark.copy(mobile = true, mobileLevel = 0)
        assertEquals(false, onRadio.offline)

        // Emergency-only is NOT offline: 999 still works, and saying "No
        // signal" there could stop a child trying the one call that'd connect.
        val emergency = dark.copy(emergencyOnly = true)
        assertEquals(false, emergency.offline)
    }
}
