package org.forgesworn.charter.admin

import android.app.admin.DevicePolicyManager
import android.content.ComponentName
import android.content.Context

/** Device Owner plumbing shared by the service, UI, and tests. */
object Provisioning {

    fun adminComponent(context: Context): ComponentName =
        ComponentName(context.packageName, "org.forgesworn.charter.admin.CharterDeviceAdminReceiver")

    fun dpm(context: Context): DevicePolicyManager =
        context.getSystemService(Context.DEVICE_POLICY_SERVICE) as DevicePolicyManager

    fun isDeviceOwner(context: Context): Boolean =
        dpm(context).isDeviceOwnerApp(context.packageName)

    /**
     * Kintrinsic state lives in device-protected storage (D6): enforcement must run
     * before first unlock (Direct Boot), and FBE device-key encryption + the app
     * sandbox is the 0600-root analog. The machine key never leaves here and is
     * excluded from every backup path.
     */
    /** This build's own versionCode — reported in STATUS so the guardian can
     *  see update state (#44). 0 only if the platform can't read ourselves
     *  (never observed; fail-quiet keeps init unconditional). */
    fun ownVersionCode(context: Context): Long = runCatching {
        context.packageManager.getPackageInfo(context.packageName, 0).longVersionCode
    }.getOrDefault(0L)

    fun baseDir(context: Context): String {
        val ctx = context.createDeviceProtectedStorageContext()
        val dir = java.io.File(ctx.filesDir, "charter")
        dir.mkdirs()
        return dir.absolutePath
    }
}
