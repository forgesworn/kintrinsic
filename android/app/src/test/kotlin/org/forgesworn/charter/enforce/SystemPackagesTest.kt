package org.forgesworn.charter.enforce

import android.content.pm.ApplicationInfo
import org.forgesworn.charter.ui.Connections
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Only system apps may join the lock by virtue of a role or an intent filter:
 * both are things a ward, or an app the ward installed, can claim.
 */
class SystemPackagesTest {

    private val settings = "com.android.settings"
    private val systemDialer = "com.oem.dialer"
    private val thirdParty = "com.example.shell"
    private val unreadable = "com.example.gone"

    private val flags: (String) -> Int? = { pkg ->
        when (pkg) {
            settings, systemDialer, "android" -> ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_INSTALLED
            thirdParty -> ApplicationInfo.FLAG_INSTALLED
            unreadable -> throw IllegalStateException("not found")
            else -> null
        }
    }

    // ---- the filter ---------------------------------------------------------

    @Test
    fun `an app without the system flag is dropped`() {
        assertEquals(listOf(settings), systemPackagesOnly(listOf(settings, thirdParty), flags))
    }

    @Test
    fun `an updated system app still counts`() {
        val updated: (String) -> Int? = {
            ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_UPDATED_SYSTEM_APP
        }
        assertEquals(listOf(settings), systemPackagesOnly(listOf(settings), updated))
    }

    @Test
    fun `a package that cannot be read is dropped`() {
        assertEquals(emptyList<String>(), systemPackagesOnly(listOf(unreadable, "com.null"), flags))
    }

    // ---- the Settings allowlist --------------------------------------------

    @Test
    fun `an app claiming the Wi-Fi page does not reach the allowlist`() {
        assertEquals(
            listOf(settings),
            Connections.systemSettingsPackages(listOf(thirdParty, settings), flags),
        )
    }

    @Test
    fun `no system result means no row`() {
        assertEquals(emptyList<String>(), Connections.systemSettingsPackages(listOf(thirdParty), flags))
        assertEquals(emptyList<String>(), Connections.systemSettingsPackages(listOf(unreadable), flags))
        assertEquals(emptyList<String>(), Connections.systemSettingsPackages(listOf(null, null), flags))
    }

    /** `android` is a system package, so the flag alone would let the chooser in. */
    @Test
    fun `the platform chooser is not a settings app`() {
        assertEquals(emptyList<String>(), Connections.systemSettingsPackages(listOf("android", ""), flags))
    }

    @Test
    fun `one app serving both pages is listed once`() {
        assertEquals(listOf(settings), Connections.systemSettingsPackages(listOf(settings, settings), flags))
    }

    // ---- the dialers on the allowlist --------------------------------------

    @Test
    fun `a third-party default dialer is not allowlisted, the system dialer is`() {
        assertEquals(listOf(systemDialer), lockTaskDialers(thirdParty, systemDialer, flags))
    }

    @Test
    fun `a default dialer that cannot be read is not allowlisted`() {
        assertEquals(listOf(systemDialer), lockTaskDialers(unreadable, systemDialer, flags))
    }

    @Test
    fun `a system default dialer stays, listed once when it is also the system dialer`() {
        assertEquals(listOf(systemDialer), lockTaskDialers(systemDialer, systemDialer, flags))
        assertEquals(listOf(settings, systemDialer), lockTaskDialers(settings, systemDialer, flags))
    }

    @Test
    fun `the system dialer stays whatever the default is`() {
        assertEquals(listOf(systemDialer), lockTaskDialers(null, systemDialer, flags))
    }

    @Test
    fun `a third-party default with no system dialer leaves no dialer on the allowlist`() {
        assertEquals(emptyList<String>(), lockTaskDialers(thirdParty, null, flags))
    }

    // ---- the dialers exempt from suspension --------------------------------

    /** Telecom opens the system dialer for an emergency number. Suspending it
     *  would break 999 on any phone whose dialer has an OEM package name. */
    @Test
    fun `the system dialer is exempt from suspension under any package name`() {
        assertTrue(systemDialer in suspensionExemptDialers(systemDialer, flags))
    }

    @Test
    fun `a third-party default dialer gets no exemption`() {
        assertFalse(thirdParty in suspensionExemptDialers(systemDialer, flags))
    }

    @Test
    fun `the AOSP dialer name is exempt only when it really is a system app`() {
        val aosp = "com.android.dialer"
        val squatted: (String) -> Int? = { ApplicationInfo.FLAG_INSTALLED }
        val genuine: (String) -> Int? = { ApplicationInfo.FLAG_SYSTEM }
        assertFalse(aosp in suspensionExemptDialers(systemDialer, squatted))
        assertTrue(aosp in suspensionExemptDialers(null, genuine))
    }

    /** What the lock lets in, it can also open. */
    @Test
    fun `what is allowlisted as a dialer is never suspended`() {
        for (default in listOf(systemDialer, thirdParty, unreadable, null)) {
            val allowlisted = lockTaskDialers(default, systemDialer, flags)
            assertTrue(suspensionExemptDialers(systemDialer, flags).containsAll(allowlisted))
        }
    }
}
