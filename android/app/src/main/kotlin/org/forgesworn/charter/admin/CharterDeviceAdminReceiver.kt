package org.forgesworn.charter.admin

import android.app.admin.DeviceAdminReceiver
import android.content.Context
import android.content.Intent
import android.util.Log

/**
 * The Device Owner hook. Provisioned once via
 * `adb shell dpm set-device-owner org.forgesworn.charter/.admin.CharterDeviceAdminReceiver`
 * on a factory-fresh device with no accounts (spike T0).
 */
class CharterDeviceAdminReceiver : DeviceAdminReceiver() {

    override fun onEnabled(context: Context, intent: Intent) {
        Log.i(TAG, "Kintrinsic device admin enabled")
    }

    companion object {
        private const val TAG = "CharterAdminReceiver"
    }
}
