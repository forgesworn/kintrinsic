package org.forgesworn.charter

import android.Manifest
import android.content.Context
import android.net.wifi.WifiManager
import android.os.HandlerThread
import android.os.UserManager
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.forgesworn.charter.admin.Provisioning
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import java.net.Inet4Address
import java.net.NetworkInterface
import java.net.ServerSocket
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * Kintrinsic Hotspot spike (world-first target): the system hotspot stays locked,
 * while Kintrinsic hosts its OWN fail-closed AP via startLocalOnlyHotspot and a
 * userspace filtering proxy. These tests pin the two restriction interactions
 * the whole design hangs on (verified in AOSP source, proven here on-device):
 *
 *  - DISALLOW_CONFIG_TETHERING (our baseline) also blocks LOHS → a filtered
 *    session must swap to DISALLOW_WIFI_TETHERING for its window.
 *  - DISALLOW_WIFI_TETHERING blocks the SYSTEM Wi-Fi hotspot but leaves LOHS
 *    alive — and the phone keeps its own Wi-Fi client connection (STA+AP).
 */
@RunWith(AndroidJUnit4::class)
class CharterHotspotSpikeTest {

    private val context: Context get() = ApplicationProvider.getApplicationContext()
    private val admin get() = Provisioning.adminComponent(context)
    private lateinit var dpm: android.app.admin.DevicePolicyManager
    private lateinit var wifi: WifiManager

    @Before
    fun setUp() {
        dpm = Provisioning.dpm(context)
        wifi = context.getSystemService(Context.WIFI_SERVICE) as WifiManager
        assumeTrue("needs Device Owner", Provisioning.isDeviceOwner(context))
        // The DO grants itself the nearby-Wi-Fi runtime permission — the same
        // self-grant the production Kintrinsic Hotspot path will use.
        dpm.setPermissionGrantState(
            admin, context.packageName, Manifest.permission.NEARBY_WIFI_DEVICES,
            android.app.admin.DevicePolicyManager.PERMISSION_GRANT_STATE_GRANTED,
        )
        // Leave no tethering restrictions from prior runs.
        dpm.clearUserRestriction(admin, UserManager.DISALLOW_CONFIG_TETHERING)
        dpm.clearUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
    }

    /** The baseline lock (DISALLOW_CONFIG_TETHERING) must also refuse our own
     *  LOHS — this is WHY a filtered session needs the restriction swap. */
    @Test
    fun baselineRestrictionAlsoBlocksLocalOnlyHotspot() {
        dpm.addUserRestriction(admin, UserManager.DISALLOW_CONFIG_TETHERING)
        try {
            val result = startLohs()
            assertEquals(
                "LOHS must be refused under DISALLOW_CONFIG_TETHERING",
                WifiManager.LocalOnlyHotspotCallback.ERROR_TETHERING_DISALLOWED,
                (result as LohsResult.Failed).reason,
            )
        } finally {
            dpm.clearUserRestriction(admin, UserManager.DISALLOW_CONFIG_TETHERING)
        }
    }

    /** Under the narrower DISALLOW_WIFI_TETHERING: the system hotspot is locked
     *  but Kintrinsic's own AP starts, exposes a QR-able join config, coexists with
     *  the phone's Wi-Fi connection, and accepts a proxy listener socket. */
    @Test
    fun charterHotspotRunsUnderWifiTetheringRestriction() {
        dpm.addUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
        var reservation: WifiManager.LocalOnlyHotspotReservation? = null
        try {
            val staBefore = wifi.connectionInfo?.networkId != -1
            val result = startLohs()
            assertTrue(
                "LOHS must start under DISALLOW_WIFI_TETHERING, got $result",
                result is LohsResult.Started,
            )
            reservation = (result as LohsResult.Started).reservation

            // Join config must be readable → the guardian-side QR works.
            val cfg = reservation.softApConfiguration
            assertNotNull("SSID must be readable for the join QR", cfg.wifiSsid)
            assertNotNull("passphrase must be readable for the join QR", cfg.passphrase)

            // STA+AP concurrency: our AP must not have kicked the phone off Wi-Fi.
            if (staBefore) {
                assertTrue(
                    "phone must keep its own Wi-Fi connection while hosting the AP",
                    wifi.connectionInfo?.networkId != -1,
                )
            }

            // The doorman's socket: bind a listener on the AP interface address.
            val apAddr = apInterfaceAddress()
            assertNotNull("AP interface must expose an IPv4 address", apAddr)
            ServerSocket(0, 8, apAddr).use { srv ->
                assertTrue("proxy listener must bind on the AP address", srv.isBound)
            }
        } finally {
            reservation?.close()
            dpm.clearUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING)
        }
    }

    private sealed class LohsResult {
        data class Started(val reservation: WifiManager.LocalOnlyHotspotReservation) : LohsResult()
        data class Failed(val reason: Int) : LohsResult()
    }

    private fun startLohs(): LohsResult {
        val latch = CountDownLatch(1)
        var outcome: LohsResult? = null
        val thread = HandlerThread("lohs-spike").apply { start() }
        val handler = android.os.Handler(thread.looper)
        wifi.startLocalOnlyHotspot(
            object : WifiManager.LocalOnlyHotspotCallback() {
                override fun onStarted(r: WifiManager.LocalOnlyHotspotReservation) {
                    outcome = LohsResult.Started(r); latch.countDown()
                }
                override fun onFailed(reason: Int) {
                    outcome = LohsResult.Failed(reason); latch.countDown()
                }
                override fun onStopped() { /* teardown, not an outcome */ }
            },
            handler,
        )
        assertTrue("LOHS callback within 20s", latch.await(20, TimeUnit.SECONDS))
        thread.quitSafely()
        return outcome!!
    }

    /** The local-only AP's IPv4 (the address guests reach the proxy on): the
     *  first non-loopback IPv4 that is not the STA's own address. */
    private fun apInterfaceAddress(): Inet4Address? {
        val staIp = wifi.connectionInfo?.ipAddress ?: 0
        val staAddr = if (staIp != 0)
            "%d.%d.%d.%d".format(staIp and 0xff, staIp shr 8 and 0xff, staIp shr 16 and 0xff, staIp shr 24 and 0xff)
        else null
        for (nif in NetworkInterface.getNetworkInterfaces()) {
            if (!nif.isUp || nif.isLoopback) continue
            for (addr in nif.inetAddresses) {
                if (addr is Inet4Address && addr.hostAddress != staAddr && addr.isSiteLocalAddress) {
                    return addr
                }
            }
        }
        return null
    }
}
