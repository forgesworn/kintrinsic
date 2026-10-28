package org.forgesworn.charter.admin

import android.app.admin.DeviceAdminReceiver
import android.app.admin.DevicePolicyManager
import android.content.Context
import android.content.Intent
import android.os.PersistableBundle
import android.util.Log
import org.forgesworn.charter.MainActivity
import org.forgesworn.charter.service.CharterService

/**
 * The Device Owner hook. Provisioned either from a factory reset by the
 * guardian's setup QR (docs/qr-setup.md), or once via
 * `adb shell dpm set-device-owner org.forgesworn.charter/.admin.CharterDeviceAdminReceiver`
 * on a factory-fresh device with no accounts (spike T0).
 */
class CharterDeviceAdminReceiver : DeviceAdminReceiver() {

    override fun onEnabled(context: Context, intent: Intent) {
        Log.i(TAG, "Kintrinsic device admin enabled")
    }

    /**
     * QR setup finished. Hold the pairing link (never pin it — see [QrSetup]),
     * make sure the warden is running, and open the pairing check. `dpm
     * set-device-owner` sends no such broadcast, so the cable path is
     * untouched.
     */
    override fun onProfileProvisioningComplete(context: Context, intent: Intent) {
        QrSetup.holdFrom(
            context,
            intent.getParcelableExtra(
                DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE,
                PersistableBundle::class.java,
            ),
        )
        if (!Provisioning.isDeviceOwner(context)) return
        runCatching { CharterService.start(context) }
            .onFailure { Log.e(TAG, "could not start the warden after provisioning", it) }
        runCatching {
            context.startActivity(
                Intent(context, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }.onFailure { Log.w(TAG, "could not open the pairing check after provisioning", it) }
    }

    companion object {
        private const val TAG = "CharterAdminReceiver"
    }
}
