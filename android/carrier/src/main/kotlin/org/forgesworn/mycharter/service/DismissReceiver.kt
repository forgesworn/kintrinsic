package org.forgesworn.mycharter.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import org.forgesworn.mycharter.carrier.CarrierStore

/**
 * The guardian swiped away a shared "not paired" row (the stranger emergency row
 * or the unpaired asks row): its count starts again from one. Only the app's own
 * explicit PendingIntent reaches this receiver (it is not exported). The ids
 * already counted are kept, so a relay replay of them is still no news.
 */
class DismissReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val asks = intent.getBooleanExtra(EXTRA_ASKS, false)
        runCatching { CarrierStore(context).resetUnpaired(asks) }
    }

    companion object {
        const val EXTRA_ASKS = "asks"
    }
}
