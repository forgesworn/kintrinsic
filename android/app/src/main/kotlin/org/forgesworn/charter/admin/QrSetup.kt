package org.forgesworn.charter.admin

import android.app.AppOpsManager
import android.app.admin.DevicePolicyManager
import android.content.Context
import android.os.PersistableBundle
import android.util.Log
import org.forgesworn.charter.ui.PairingSas

/**
 * QR setup (docs/qr-setup.md): Android's setup wizard scans the guardian's
 * code, installs this app, makes it Device Owner and hands it the pairing
 * link in `PROVISIONING_ADMIN_EXTRAS_BUNDLE`.
 *
 * The link is HELD here, never pinned. It is attacker-supplyable exactly as a
 * scanned or pasted link is — anyone can print a provisioning code carrying
 * their own guardian key — so it only ever pre-fills the pairing field, where
 * [org.forgesworn.charter.MainActivity.showPairingCheck] puts the six-digit
 * code in front of the family and nothing is pinned until someone taps
 * **Pair with guardian**.
 *
 * The decisions are pure functions over plain values so they run on the JVM
 * (a `PersistableBundle` or `Intent` is a no-op stub there).
 */
object QrSetup {

    /** The admin-extras key the guardian app's QR carries the link under. */
    const val PAIRING_LINK_EXTRA = "org.forgesworn.charter.PAIRING_LINK"

    private const val PREFS = "qr_setup"
    private const val KEY_LINK = "held_pairing_link"
    private const val TAG = "CharterQrSetup"

    /**
     * The pairing link to hold, from the raw admin-extras value, or `null` for
     * one that is missing or that the pairing check cannot read. An unreadable
     * link is dropped, not held: the ward app then stays at its normal
     * unpaired screen, where scanning or pasting still works.
     */
    fun pairingLink(raw: String?): String? {
        val link = raw?.trim().orEmpty()
        if (link.isEmpty() || link.any { it.isWhitespace() }) return null
        return link.takeIf { PairingSas.check(it) != null }
    }

    /**
     * The answer to `GET_PROVISIONING_MODE`: always a fully managed device,
     * never a work profile. [allowed] is the platform's
     * `EXTRA_PROVISIONING_ALLOWED_PROVISIONING_MODES`; absent or empty means
     * the platform named no restriction. `null` when fully managed is not on
     * offer — the caller cancels rather than settle for a work profile.
     */
    fun provisioningMode(allowed: List<Int>?): Int? {
        val fullyManaged = DevicePolicyManager.PROVISIONING_MODE_FULLY_MANAGED_DEVICE
        return if (allowed.isNullOrEmpty() || fullyManaged in allowed) fullyManaged else null
    }

    /**
     * Whether to ask for Usage access: only on a Device Owner (the meter only
     * runs there) whose app-op is not yet allowed. The cable setup grants it
     * over adb, so on that path this stays false.
     */
    fun needsUsageAccess(isOwner: Boolean, opMode: Int): Boolean =
        isOwner && opMode != AppOpsManager.MODE_ALLOWED

    // ---- platform glue (not exercised on the JVM) ----------------------------

    /** Hold the link from a provisioning extras bundle, if it carries one. */
    fun holdFrom(context: Context, extras: PersistableBundle?) {
        val link = pairingLink(extras?.getString(PAIRING_LINK_EXTRA))
        if (link == null) {
            if (extras != null) Log.w(TAG, "provisioning extras carried no readable pairing link")
            return
        }
        prefs(context).edit().putString(KEY_LINK, link).apply()
    }

    /** The held link, re-checked on the way out. */
    fun held(context: Context): String? = pairingLink(prefs(context).getString(KEY_LINK, null))

    /** Forget the held link — once paired it has done its job. */
    fun clear(context: Context) {
        val p = prefs(context)
        if (p.contains(KEY_LINK)) p.edit().remove(KEY_LINK).apply()
    }

    /** The usage-stats app-op mode, read the way `WardenController` reads it. */
    fun usageOpMode(context: Context): Int {
        val appOps = context.getSystemService(Context.APP_OPS_SERVICE) as AppOpsManager
        return appOps.unsafeCheckOpNoThrow(
            AppOpsManager.OPSTR_GET_USAGE_STATS,
            android.os.Process.myUid(),
            context.packageName,
        )
    }

    // Device-protected, like the rest of the app's state: the admin receiver
    // is direct-boot aware, and the link must survive the reboot some setup
    // wizards finish with.
    private fun prefs(context: Context) =
        context.createDeviceProtectedStorageContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE)
}
