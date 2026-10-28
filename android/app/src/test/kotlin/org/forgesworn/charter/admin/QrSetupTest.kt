package org.forgesworn.charter.admin

import android.app.AppOpsManager
import android.app.admin.DevicePolicyManager
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** QR setup's decisions (docs/qr-setup.md), on the JVM. */
class QrSetupTest {

    private val pk = "ab".repeat(32)
    private val link = "bunker://$pk?relay=wss://relay.damus.io&kind=charter&token=${"0f".repeat(16)}"

    @Test
    fun `the extras key matches the guardian app's`() {
        // apps/charter-app/src/provision/qrPayload.ts PAIRING_LINK_EXTRA
        assertEquals("org.forgesworn.charter.PAIRING_LINK", QrSetup.PAIRING_LINK_EXTRA)
    }

    @Test
    fun `a readable pairing link is held as given`() {
        assertEquals(link, QrSetup.pairingLink(link))
        assertEquals(link, QrSetup.pairingLink("  $link\n"))
    }

    @Test
    fun `a missing or unreadable link is dropped`() {
        assertNull(QrSetup.pairingLink(null))
        assertNull(QrSetup.pairingLink(""))
        assertNull(QrSetup.pairingLink("   "))
        // Not a bunker link at all.
        assertNull(QrSetup.pairingLink("https://charter.mysignet.app/pair#$link"))
        // A key that is not 64 hex characters.
        assertNull(QrSetup.pairingLink("bunker://${"ab".repeat(31)}?relay=wss://r&kind=charter"))
        assertNull(QrSetup.pairingLink("bunker://${"zz".repeat(32)}?relay=wss://r&kind=charter"))
        // Whitespace inside the link: something has been spliced in.
        assertNull(QrSetup.pairingLink("bunker://$pk?relay=wss://r &kind=charter"))
    }

    @Test
    fun `provisioning mode is always fully managed`() {
        val fm = DevicePolicyManager.PROVISIONING_MODE_FULLY_MANAGED_DEVICE
        val wp = DevicePolicyManager.PROVISIONING_MODE_MANAGED_PROFILE
        assertEquals(fm, QrSetup.provisioningMode(null))
        assertEquals(fm, QrSetup.provisioningMode(emptyList()))
        assertEquals(fm, QrSetup.provisioningMode(listOf(wp, fm)))
        assertEquals(fm, QrSetup.provisioningMode(listOf(fm)))
    }

    @Test
    fun `no work profile, even when that is all that is offered`() {
        assertNull(QrSetup.provisioningMode(listOf(DevicePolicyManager.PROVISIONING_MODE_MANAGED_PROFILE)))
        assertNull(
            QrSetup.provisioningMode(
                listOf(DevicePolicyManager.PROVISIONING_MODE_MANAGED_PROFILE_ON_PERSONAL_DEVICE),
            ),
        )
    }

    @Test
    fun `usage access is asked for only on a Device Owner without it`() {
        assertTrue(QrSetup.needsUsageAccess(true, AppOpsManager.MODE_DEFAULT))
        assertTrue(QrSetup.needsUsageAccess(true, AppOpsManager.MODE_IGNORED))
        assertTrue(QrSetup.needsUsageAccess(true, AppOpsManager.MODE_ERRORED))
        assertFalse(QrSetup.needsUsageAccess(true, AppOpsManager.MODE_ALLOWED))
        assertFalse(QrSetup.needsUsageAccess(false, AppOpsManager.MODE_DEFAULT))
    }
}
