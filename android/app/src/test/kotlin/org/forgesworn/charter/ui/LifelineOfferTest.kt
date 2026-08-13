package org.forgesworn.charter.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * The lifeline on a device that cannot call.
 *
 * decented is putting Kintrinsic on a Wi-Fi-only Galaxy Tab A9 (SM-X110) for home
 * screen time (2026-07-28). It has no cellular radio, so ACTION_CALL goes
 * nowhere — and until this, the shade still drew "Call Mum", took the child's
 * two-second hold, and answered "Couldn't start the call". A dead button on a
 * safety affordance is worse than no button: a child in trouble would try it
 * and believe it ought to work.
 */
class LifelineOfferTest {

    @Test fun aPhoneOffersEveryNumberTheGuardianSet() {
        val o = LifelineOffer.decide(canCall = true, numbers = 3, emergencyOn = true)
        assertTrue(o.showCallButtons)
        assertTrue(o.showEmergency)
        assertEquals(null, o.note)
    }

    @Test fun aTabletOffersNoCallButtonsAtAll() {
        val o = LifelineOffer.decide(canCall = false, numbers = 3, emergencyOn = true)
        assertFalse("a call that cannot connect must not be offered", o.showCallButtons)
        assertFalse(o.showEmergency)
    }

    /** Silence would read as the guardian forgetting to set a lifeline up. */
    @Test fun aTabletSaysWhyThereIsNoOneToCall() {
        val o = LifelineOffer.decide(canCall = false, numbers = 3, emergencyOn = true)
        assertEquals("This device can't make calls — find your guardian.", o.note)
    }

    /** With no lifeline configured there is nothing to explain either way. */
    @Test fun nothingIsSaidWhenNoLifelineWasSet() {
        assertEquals(null, LifelineOffer.decide(false, numbers = 0, emergencyOn = false).note)
        assertFalse(LifelineOffer.decide(true, numbers = 0, emergencyOn = false).showCallButtons)
    }

    /** Emergency-services-only is still a lifeline worth explaining. */
    @Test fun emergencyOnlyCountsAsALifeline() {
        val o = LifelineOffer.decide(canCall = false, numbers = 0, emergencyOn = true)
        assertEquals("This device can't make calls — find your guardian.", o.note)
    }
}
