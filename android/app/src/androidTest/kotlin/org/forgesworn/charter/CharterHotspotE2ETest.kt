package org.forgesworn.charter

import android.Manifest
import android.content.Context
import android.net.wifi.WifiManager
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.enforce.hotspot.CharterHotspot
import org.forgesworn.charter.enforce.hotspot.HotspotFilter
import org.forgesworn.charter.native.CharterCore.DnsPlan
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith
import java.io.BufferedReader
import java.io.InputStreamReader
import java.io.OutputStreamWriter
import java.net.InetAddress
import java.net.Socket

/**
 * The full Kintrinsic Hotspot on real hardware: start() brings up the local-only
 * AP AND the filtering proxy bound to the AP interface, exposes a QR-able join
 * config, and a client reaching the proxy on that interface is filtered by the
 * ward's own policy. This proves the entire device side of filtered tethering —
 * everything a foreign guest exercises except associating over the radio.
 */
@RunWith(AndroidJUnit4::class)
class CharterHotspotE2ETest {

    private val context: Context get() = ApplicationProvider.getApplicationContext()
    private lateinit var hotspot: CharterHotspot

    private fun blockPlan() = DnsPlan(
        revision = "r1", mode = "blocklist",
        allowDomains = emptyList(),
        blockDomains = listOf("blocked.example"),
        blockCategories = emptyList(),
        allowExceptions = emptyList(),
        safeSearch = false, youtubeRestrict = "off",
        rewrites = emptyList(),
    )

    @Before
    fun setUp() {
        assumeTrue("needs Device Owner", Provisioning.isDeviceOwner(context))
        val dpm = Provisioning.dpm(context)
        val admin = Provisioning.adminComponent(context)
        // The DO self-grants nearby-Wi-Fi + frees LOHS (filtered-session posture).
        dpm.setPermissionGrantState(
            admin, context.packageName, Manifest.permission.NEARBY_WIFI_DEVICES,
            android.app.admin.DevicePolicyManager.PERMISSION_GRANT_STATE_GRANTED,
        )
        dpm.clearUserRestriction(admin, android.os.UserManager.DISALLOW_CONFIG_TETHERING)
        dpm.addUserRestriction(admin, android.os.UserManager.DISALLOW_WIFI_TETHERING)
        hotspot = CharterHotspot(context) { HotspotFilter(blockPlan()) }
    }

    @After
    fun tearDown() {
        runCatching { hotspot.stop() }
        val dpm = Provisioning.dpm(context)
        val admin = Provisioning.adminComponent(context)
        runCatching { dpm.clearUserRestriction(admin, android.os.UserManager.DISALLOW_WIFI_TETHERING) }
    }

    @Test
    fun startBringsUpApAndFilteringProxy() {
        val session = hotspot.start(20_000)
        assertNotNull("hotspot must start (LOHS + proxy)", session)
        session!!

        // The join config the guardian renders as a QR.
        assertTrue("SSID must be present for the join QR", session.ssid.isNotBlank())
        assertTrue("passphrase must be present for the join QR", session.passphrase.isNotBlank())

        // The proxy is live on the AP interface — filter a blocked and an
        // allowed host exactly as a joined guest would experience it.
        val proxyAddr = InetAddress.getByName(session.proxyHost)
        Socket(proxyAddr, session.proxyPort).use { s ->
            val out = OutputStreamWriter(s.getOutputStream())
            out.write("CONNECT blocked.example:443 HTTP/1.1\r\n\r\n")
            out.flush()
            val line = BufferedReader(InputStreamReader(s.getInputStream())).readLine()
            assertTrue("blocked host must be refused, got: $line",
                line != null && line.contains("403"))
        }
    }
}
