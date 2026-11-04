package org.forgesworn.charter.enforce

import android.content.pm.ApplicationInfo
import org.forgesworn.charter.ui.Connections
import org.junit.Assert.assertEquals
import org.junit.Test

/**
 * Only system apps may join the lock by virtue of an intent filter, which an
 * app the ward installed can claim.
 */
class SystemPackagesTest {

    private val settings = "com.android.settings"
    private val thirdParty = "com.example.shell"
    private val unreadable = "com.example.gone"

    private val flags: (String) -> Int? = { pkg ->
        when (pkg) {
            settings, "android" -> ApplicationInfo.FLAG_SYSTEM or ApplicationInfo.FLAG_INSTALLED
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
}
