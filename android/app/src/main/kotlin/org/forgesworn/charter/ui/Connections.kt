package org.forgesworn.charter.ui

import android.app.admin.DevicePolicyManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.provider.Settings
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.enforce.appFlagsOf
import org.forgesworn.charter.enforce.lockedRestrictionsInForce
import org.forgesworn.charter.enforce.systemPackagesOnly

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
 * can reach the rest of Settings while locked. The tamper-relevant parts stay
 * closed by the baseline restrictions (date/time, VPN, private DNS, factory
 * reset, users, install sources), and while locked also by
 * [org.forgesworn.charter.enforce.LOCKED_RESTRICTIONS] (accounts, app controls,
 * default apps). The row is only offered while those are really in force
 * ([lockedSurfaceClosed]). Back returns to the shade.
 *
 * Only a system app can serve these pages. Any app can declare an intent filter
 * for them, and whichever package resolves joins the allowlist, so a store app
 * must not be able to put itself there by claiming to be the Wi-Fi page.
 *
 * What the ward may JOIN there is the guardian's `connections` clause, which
 * the warden applies device-wide as restrictions the Settings pages honour
 * (WardenController, `connectionsTargets`); [limitNote] says it on the shade.
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
     * to a system app (the shade then shows no row rather than a dead one).
     */
    fun settingsPackages(ctx: Context): List<String> =
        systemSettingsPackages(
            listOf(resolvedPackage(ctx, wifiIntent()), resolvedPackage(ctx, bluetoothIntent())),
            appFlagsOf(ctx),
        )

    /**
     * [intent] pinned to the system app that serves it, or null when nothing
     * does (no button for that page). Pinned so the tap opens the package that
     * was checked, whatever has been installed since the row was painted.
     */
    fun systemPage(ctx: Context, intent: Intent): Intent? =
        systemSettingsPackages(listOf(resolvedPackage(ctx, intent)), appFlagsOf(ctx))
            .firstOrNull()
            ?.let { intent.setPackage(it) }

    /**
     * The pure half of [settingsPackages]. A failed lookup (null) and the
     * platform's own chooser (`android`, itself a system package, which appears
     * when several apps answer) both count as no result.
     */
    internal fun systemSettingsPackages(
        resolved: List<String?>,
        appFlags: (String) -> Int?,
    ): List<String> =
        systemPackagesOnly(
            resolved.filterNotNull().filter { it.isNotEmpty() && it != "android" }.distinct(),
            appFlags,
        )

    private fun resolvedPackage(ctx: Context, intent: Intent): String? = runCatching {
        ctx.packageManager.resolveActivity(intent, PackageManager.MATCH_DEFAULT_ONLY)
            ?.activityInfo?.packageName
    }.getOrNull()

    /**
     * Whether the while-locked restrictions are really in force right now, read
     * from the platform rather than from what the warden last asked for. The
     * row is offered only while this holds.
     */
    fun lockedSurfaceClosed(ctx: Context): Boolean = runCatching {
        val dpm = ctx.getSystemService(Context.DEVICE_POLICY_SERVICE) as DevicePolicyManager
        lockedRestrictionsInForce(dpm, Provisioning.adminComponent(ctx))
    }.getOrDefault(false)

    /**
     * One plain line on what the guardian's `connections` clause allows, or
     * null when nothing is limited (or the posture is unknown). Switching
     * either radio on and off is always allowed, so the line is only about
     * joining new things.
     */
    fun limitNote(wifi: String?, bluetooth: String?): String? {
        val parts = buildList {
            when (wifi) {
                "saved" -> add("Wi-Fi: networks already saved on this phone")
                "secured" -> add("Wi-Fi: only networks with a password")
            }
            if (bluetooth == "paired") add("Bluetooth: devices already paired")
        }
        return if (parts.isEmpty()) null else parts.joinToString("\n")
    }

    /** Whether the device has the radio at all — no Bluetooth button on a
     *  device without Bluetooth, no Wi-Fi button on one without Wi-Fi. */
    fun hasWifi(ctx: Context): Boolean =
        ctx.packageManager.hasSystemFeature(PackageManager.FEATURE_WIFI)

    fun hasBluetooth(ctx: Context): Boolean =
        ctx.packageManager.hasSystemFeature(PackageManager.FEATURE_BLUETOOTH)
}
