package org.forgesworn.charter.admin

import android.app.Activity
import android.app.admin.DevicePolicyManager
import android.os.Bundle
import android.os.PersistableBundle
import android.util.Log
import org.forgesworn.charter.service.CharterService

/**
 * `ACTION_ADMIN_POLICY_COMPLIANCE` (Android 12+ QR setup, docs/qr-setup.md):
 * the app is Device Owner now. Apply what the cable setup applies after
 * `dpm set-device-owner` and hand back `RESULT_OK`.
 *
 * On the cable path that is the warden starting up: `WardenController.init`
 * self-grants the runtime permissions and usage access and opens LockTask to
 * the phone stack. The restriction baseline itself is charter-gated by design
 * (`Enforcement.applyBaseline`, I17) and lands when the first charter does,
 * on either path. Usage access may still need its one switch (MainActivity
 * asks), because only adb can flip that app-op without a tap.
 */
class PolicyComplianceActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        QrSetup.holdFrom(
            this,
            intent.getParcelableExtra(
                DevicePolicyManager.EXTRA_PROVISIONING_ADMIN_EXTRAS_BUNDLE,
                PersistableBundle::class.java,
            ),
        )
        if (Provisioning.isDeviceOwner(this)) {
            runCatching { CharterService.start(this) }
                .onFailure { Log.e(TAG, "could not start the warden after provisioning", it) }
        } else {
            Log.w(TAG, "policy compliance reached without Device Owner")
        }
        setResult(RESULT_OK)
        finish()
    }

    private companion object {
        const val TAG = "CharterCompliance"
    }
}
