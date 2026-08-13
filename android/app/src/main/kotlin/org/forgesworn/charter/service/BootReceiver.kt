package org.forgesworn.charter.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import org.forgesworn.charter.admin.Provisioning

/**
 * Re-arm enforcement on boot and after our own package is replaced (I5/I12).
 * Registered for both LOCKED_BOOT_COMPLETED (Direct Boot: device-protected
 * storage is readable before first unlock) and BOOT_COMPLETED.
 */
class BootReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        when (intent.action) {
            Intent.ACTION_LOCKED_BOOT_COMPLETED,
            Intent.ACTION_BOOT_COMPLETED,
            Intent.ACTION_MY_PACKAGE_REPLACED -> {
                if (Provisioning.isDeviceOwner(context)) {
                    CharterService.start(context)
                }
            }
        }
    }
}
