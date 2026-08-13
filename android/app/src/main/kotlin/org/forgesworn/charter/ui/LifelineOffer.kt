package org.forgesworn.charter.ui

import android.content.Context
import android.content.pm.PackageManager

/**
 * Whether this device can honour the lifeline at all.
 *
 * The lifeline exists because a locked PHONE is still a phone. A Wi-Fi-only
 * tablet is not: it has no cellular radio, so `ACTION_CALL` goes nowhere and
 * there is no emergency number to resolve. Drawing the buttons anyway meant a
 * child could hold "Call Mum" for two seconds — the deliberate anti-pocket-dial
 * gate — and be told "Couldn't start the call". A dead button on a safety
 * affordance is worse than no button, because a child in trouble tries it and
 * believes it should work.
 *
 * So on a device that cannot call we offer nothing and say why. The
 * break-glass unlock is untouched: it needs no radio, and on such a device it
 * becomes the meaningful escape.
 */
object LifelineOffer {

    data class Offer(
        val showCallButtons: Boolean,
        val showEmergency: Boolean,
        /** Shown in place of the buttons; null when there is nothing to explain. */
        val note: String?,
    )

    /** Pure decision, so it can be tested without a device. */
    fun decide(canCall: Boolean, numbers: Int, emergencyOn: Boolean): Offer {
        val configured = numbers > 0 || emergencyOn
        if (canCall) {
            return Offer(
                showCallButtons = numbers > 0,
                showEmergency = emergencyOn,
                note = null,
            )
        }
        return Offer(
            showCallButtons = false,
            showEmergency = false,
            // Only worth saying when a lifeline WAS set: otherwise silence is
            // simply "no lifeline here", not a broken one.
            note = if (configured) "This device can't make calls — find your guardian." else null,
        )
    }

    /**
     * Does this device have voice calling? `FEATURE_TELEPHONY_CALLING` is the
     * precise question from API 33 (a device can carry a modem for data and
     * still not place calls); below that the coarse telephony feature is the
     * best available answer.
     */
    fun canCall(ctx: Context): Boolean {
        val pm = ctx.packageManager
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            pm.hasSystemFeature(PackageManager.FEATURE_TELEPHONY_CALLING)
        } else {
            @Suppress("DEPRECATION")
            pm.hasSystemFeature(PackageManager.FEATURE_TELEPHONY)
        }
    }
}
