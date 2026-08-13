package org.forgesworn.mycharter.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Re-arm the listener after boot or app update — provisioned carriers only.
 *
 * DELIBERATELY not directBootAware (decided 2026-07-23): alerting before the
 * guardian's first unlock would require the guardian secret in
 * device-protected storage — readable without the user credential — which
 * trades custody for a corner case (spec D2 forbids exactly that). Alerts
 * resume at first unlock; asks are never lost (48 h resubscribe window).
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_BOOT_COMPLETED, Intent.ACTION_MY_PACKAGE_REPLACED ->
                CarrierService.start(context)
        }
    }
}
