package org.forgesworn.charter.admin

import android.app.Activity
import android.app.admin.DevicePolicyManager
import android.content.Intent
import android.os.Bundle
import android.os.PersistableBundle

/**
 * `ACTION_GET_PROVISIONING_MODE` (Android 12+ QR setup, docs/qr-setup.md):
 * answer "fully managed device", never a work profile. Only the platform can
 * start this (`BIND_DEVICE_ADMIN`). No UI; it answers and finishes.
 */
class ProvisioningModeActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val extras = intent.getParcelableExtra(
            DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE,
            PersistableBundle::class.java,
        )
        QrSetup.holdFrom(this, extras)
        val mode = QrSetup.provisioningMode(
            intent.getIntegerArrayListExtra(DevicePolicyManager.EXTRA_PROVISIONING_ALLOWED_PROVISIONING_MODES),
        )
        if (mode == null) {
            setResult(RESULT_CANCELED)
        } else {
            val result = Intent().putExtra(DevicePolicyManager.EXTRA_PROVISIONING_MODE, mode)
            // Hand the extras back so the platform carries them on to the
            // policy-compliance step and the completion broadcast.
            if (extras != null) result.putExtra(DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE, extras)
            setResult(RESULT_OK, result)
        }
        finish()
    }
}
