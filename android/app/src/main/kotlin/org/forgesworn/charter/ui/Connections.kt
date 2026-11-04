package org.forgesworn.charter.ui

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.provider.Settings

/**
 * Wi-Fi and Bluetooth from the lock shade.
 *
 * LockTask hides the status bar and Quick Settings, so a locked ward had no
 * way at all to join a Wi-Fi network or connect their headphones. That made
 * the shade's own Open row half-broken: Duolingo "open at any hour" is no use
 * on a phone that can't get online, and an audiobook is no use with the
 * headphones stuck in the drawer. Connectivity is plumbing, not the thing a
 * lock is about — locking a child out of it is enforcement spilling into
 * things that were never the point (the same reasoning as the vitals strip).
 *
 * The shade hands off to the platform's own Wi-Fi and Bluetooth pages rather
 * than rebuilding a network picker or a pairing flow in-app: those pages
 * already handle passwords, captive portals, WPA3, pairing PINs and every OEM
 * quirk, and a home-grown picker that half-works would be a dead control.
 *
 * What that costs, said plainly: LockTask allows by PACKAGE, so the settings
 * app joins the allowlist, and a ward who navigates away from the Wi-Fi page
 * can reach the rest of Settings while locked. That is the same Settings they
 * have during allowed hours (Kintrinsic never suspends it), the tamper-relevant
 * parts stay closed by the baseline restrictions (date/time, VPN, private DNS,
 * factory reset, users, install sources), and nothing launched FROM Settings
 * gets past the pin unless it is itself allowlisted. Back returns to the shade.
 */
object Connections {

    /** The Wi-Fi page: the network list, the toggle, join-with-password. */
    fun wifiIntent(): Intent = Intent(Settings.ACTION_WIFI_SETTINGS)

    /** The Bluetooth page: the toggle, paired devices, pair new device. */
    fun bluetoothIntent(): Intent = Intent(Settings.ACTION_BLUETOOTH_SETTINGS)

    /**
     * The package(s) that actually serve those pages on THIS device — resolved
     * live rather than assuming `com.android.settings`, so an OEM that hosts
     * them elsewhere still gets a working button. Empty when nothing resolves
     * (the shade then shows no row rather than a dead one).
     */
    fun settingsPackages(ctx: Context): List<String> {
        val pm = ctx.packageManager
        return listOf(wifiIntent(), bluetoothIntent())
            .mapNotNull { intent ->
                runCatching {
                    pm.resolveActivity(intent, PackageManager.MATCH_DEFAULT_ONLY)
                        ?.activityInfo?.packageName
                }.getOrNull()
            }
            .filter { it.isNotEmpty() && it != "android" }
            .distinct()
    }

    /** Whether the device has the radio at all — no Bluetooth button on a
     *  device without Bluetooth, no Wi-Fi button on one without Wi-Fi. */
    fun hasWifi(ctx: Context): Boolean =
        ctx.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI)

    fun hasBluetooth(ctx: Context): Boolean =
        ctx.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH)
}
