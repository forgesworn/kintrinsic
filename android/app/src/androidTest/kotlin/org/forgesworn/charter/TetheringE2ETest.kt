package org.forgesworn.charter

import android.app.admin.DevicePolicyManager
import android.content.Context
import android.os.UserManager
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.enforce.DpmAppGateOps
import org.forgesworn.charter.enforce.DpmRestrictionOps
import org.forgesworn.charter.enforce.FakeDnsFilterOps
import org.forgesworn.charter.enforce.UsageSource
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.service.WardenController
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Tethering is a first-class clause category (default OFF): the hotspot is a
 * clean bypass of the DNS-filter VPN — tethered clients get raw upstream
 * internet — so the baseline must close it and a grant must be able to open it.
 */
@RunWith(AndroidJUnit4::class)
class TetheringE2ETest {

    private val context: Context get() = ApplicationProvider.getApplicationContext()
    private lateinit var dpm: DevicePolicyManager
    private val admin get() = Provisioning.adminComponent(context)

    // From: cargo run --example gen_fixture --features charter-verify/mock
    private val guardian = "defdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34"
    private val subject = "70e6b44a2ac6083ab673bacb5cb7ca554b795b416e702c1c980bb7b87c78b8e9"
    private val openClause =
        """{"id":"cccf73d06e74c87652b3f29bd98ae0c670ac3ee5d19ec3cf78d10fab06981d87","pubkey":"defdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34","created_at":2,"kind":31113,"tags":[["t","charter-device"],["d","schedule"]],"content":"{\"body\":{\"issuedAt\":2,\"tz\":\"Europe/London\",\"v\":1,\"weekly\":{}},\"issuedAt\":2,\"kind\":\"schedule\",\"subject\":\"70e6b44a2ac6083ab673bacb5cb7ca554b795b416e702c1c980bb7b87c78b8e9\",\"v\":1}","sig":"22dc542fbbedba576612c4ff152e3d17f3b6256193fff4587aa2020d4e86e70c78b97e01316929e359dec39dd601a2d00598dc501e38e1fcd546c1be0759fa21"}"""
    private val tetherRaw =
        """{"id":"722ea7fa57fe4b8eff91207e1f7fe019b095c7b58354109f46787a854262d266","pubkey":"defdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34","created_at":3,"kind":31113,"tags":[["t","charter-device"],["d","tethering"]],"content":"{\"body\":{\"allow\":\"raw\",\"issuedAt\":3,\"until\":4000000000,\"v\":1},\"issuedAt\":3,\"kind\":\"tethering\",\"subject\":\"70e6b44a2ac6083ab673bacb5cb7ca554b795b416e702c1c980bb7b87c78b8e9\",\"v\":1}","sig":"26920f958b5818417bd8902b98bfbe4f246afda25e59d58ddddaae33c7a4fb4c85d52ee3b01ad18b93338e3bcc671046714c503e6d4e1150753259ebaa26d00b"}"""
    private val tetherFiltered =
        """{"id":"0df8ae65e5c399967fb6cfce9be1ee0c468772822dc451476825764548537093","pubkey":"defdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34","created_at":5,"kind":31113,"tags":[["t","charter-device"],["d","tethering"]],"content":"{\"body\":{\"allow\":\"filtered\",\"issuedAt\":5,\"v\":1},\"issuedAt\":5,\"kind\":\"tethering\",\"subject\":\"70e6b44a2ac6083ab673bacb5cb7ca554b795b416e702c1c980bb7b87c78b8e9\",\"v\":1}","sig":"262e9eef08430c44787b0153c8da99c711aadada72c6272831af5b5a61392ef0e01d7603b1d0aca4df49e76eeb97aca900f239857531f01c6b15b0a1b2225423"}"""
    private val tetherNone =
        """{"id":"8cbafc47067d9e7fd9542e4414fc5c83b0d4e9bebd56581142e808bf82a9e9c0","pubkey":"defdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34","created_at":6,"kind":31113,"tags":[["t","charter-device"],["d","tethering"]],"content":"{\"body\":{\"allow\":\"none\",\"issuedAt\":6,\"v\":1},\"issuedAt\":6,\"kind\":\"tethering\",\"subject\":\"70e6b44a2ac6083ab673bacb5cb7ca554b795b416e702c1c980bb7b87c78b8e9\",\"v\":1}","sig":"2fd6ce31095ab923f31cb926fecb3c55c3b2a3af692ac93329b9d07a28f73574ba34631960d3598c60d24676ea9e090f452d0c12300a049f9bd575d09117a666"}"""
    private val tetherExpired =
        """{"id":"d6a4b0334d7c92dededb73168761073f5f1b45748950088bc882df1eb266c3e3","pubkey":"defdea4cdb677750a420fee807eacf21eb9898ae79b9768766e4faa04a2d4a34","created_at":7,"kind":31113,"tags":[["t","charter-device"],["d","tethering"]],"content":"{\"body\":{\"allow\":\"raw\",\"issuedAt\":7,\"until\":2000,\"v\":1},\"issuedAt\":7,\"kind\":\"tethering\",\"subject\":\"70e6b44a2ac6083ab673bacb5cb7ca554b795b416e702c1c980bb7b87c78b8e9\",\"v\":1}","sig":"0a2a21d12bba61851c1a7c9c203533c6c67df4f08980a85489236e4f9bbf3fe4fb9b676e9e4c2af8317b4d22ef059332cc49069bd18ab47894e75b1c176a89c8"}"""

    @Before
    fun setUp() {
        dpm = Provisioning.dpm(context)
        assumeTrue("needs Device Owner (dpm set-device-owner)", Provisioning.isDeviceOwner(context))
        // Clean slate so re-runs are deterministic (mirrors EnforcementE2ETest).
        java.io.File(Provisioning.baseDir(context)).deleteRecursively()
    }

    @Test
    fun baselineClosesTetheringBypass() {
        val restrictions = DpmRestrictionOps(dpm, admin)
        restrictions.applyBaseline()
        try {
            assertTrue(
                "baseline must lock tethering config (hotspot = unfiltered-internet bypass)",
                restrictions.isRestrictionActive(UserManager.DISALLOW_CONFIG_TETHERING),
            )
        } finally {
            restrictions.clearBaseline()
        }
        assertFalse(
            "clearBaseline must release the tethering lock",
            restrictions.isRestrictionActive(UserManager.DISALLOW_CONFIG_TETHERING),
        )
    }

    /** The whole posture ladder, driven by REAL guardian-signed clauses through
     *  the controller: default blocked → raw grant frees tethering → filtered
     *  grant locks the system Wi-Fi hotspot (Kintrinsic Hotspot window) → revoke
     *  re-locks → an expired grant stays locked. */
    @Test
    fun signedTetheringClauseWalksThePostureLadder() {
        val core = CharterCore.init(Provisioning.baseDir(context), "enforce", Provisioning.ownVersionCode(context))
        assertTrue("init error: ${core.error}", core.error == null)
        val restrictions = DpmRestrictionOps(dpm, admin)
        val controller = WardenController(
            context, dpm, admin,
            DpmAppGateOps(context, dpm, admin), restrictions,
            object : UsageSource {
                override fun foregroundPackage() = subject
                override fun screenInteractive() = true
            },
            org.forgesworn.charter.enforce.FakeApkInstallOps(),
            FakeDnsFilterOps(),
            object : WardenController.LockController {
                override fun show(reason: String) {}
                override fun hide() {}
            },
        )
        controller.init()
        CharterCore.setPairing(guardian, subject)

        fun config() = restrictions.isRestrictionActive(UserManager.DISALLOW_CONFIG_TETHERING)
        fun wifi() = restrictions.isRestrictionActive(UserManager.DISALLOW_WIFI_TETHERING)
        fun ingest(clause: String) =
            assertTrue("clause rejected", CharterCore.ingestClause(clause, now()).accepted)

        try {
            // A charter with no tethering clause: default-closed.
            ingest(openClause)
            controller.tickAndApply(now())
            assertTrue("default posture must lock tethering config", config())
            assertFalse("wifi-only lock is not part of the default", wifi())

            // Raw grant (far-future until): tethering config free for the window.
            ingest(tetherRaw)
            controller.tickAndApply(now())
            assertFalse("raw grant must clear the tethering lock", config())
            assertFalse("raw grant must not leave the wifi-only lock", wifi())

            // Filtered grant: system Wi-Fi hotspot locked, Kintrinsic's own AP legal.
            ingest(tetherFiltered)
            controller.tickAndApply(now())
            assertFalse("filtered session must free LOHS (no config lock)", config())
            assertTrue("filtered session must lock the system Wi-Fi hotspot", wifi())

            // Revoke: fully locked again (and the wifi-only lock cleaned up).
            ingest(tetherNone)
            controller.tickAndApply(now())
            assertTrue("revoke must restore the tethering lock", config())
            assertFalse("revoke must clear the wifi-only lock", wifi())

            // An expired grant (until in the past) never opens anything.
            ingest(tetherExpired)
            controller.tickAndApply(now())
            assertTrue("expired grant must stay locked", config())
            assertFalse("expired grant must not leave the wifi-only lock", wifi())
        } finally {
            restrictions.clearBaseline()
            runCatching { dpm.clearUserRestriction(admin, UserManager.DISALLOW_WIFI_TETHERING) }
        }
    }

    private fun now() = System.currentTimeMillis() / 1000
}
