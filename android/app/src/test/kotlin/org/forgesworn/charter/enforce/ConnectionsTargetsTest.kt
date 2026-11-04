package org.forgesworn.charter.enforce

import android.app.admin.DevicePolicyManager
import android.os.UserManager
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.native.parseConnections
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** The `connections` clause's tiers as the device writes them. */
class ConnectionsTargetsTest {

    private fun restriction(t: ConnectionsTargets, key: String) =
        t.restrictions.first { it.first == key }.second

    @Test
    fun `saved closes new networks and leaves open networks the ward already has`() {
        val t = connectionsTargets("saved", "any")
        assertEquals(true, restriction(t, UserManager.DISALLOW_ADD_WIFI_CONFIG))
        assertEquals(DevicePolicyManager.WIFI_SECURITY_OPEN, t.minWifiSecurity)
    }

    @Test
    fun `secured allows new networks but only ones with a password`() {
        val t = connectionsTargets("secured", "any")
        assertEquals(false, restriction(t, UserManager.DISALLOW_ADD_WIFI_CONFIG))
        assertEquals(DevicePolicyManager.WIFI_SECURITY_PERSONAL, t.minWifiSecurity)
    }

    @Test
    fun `any clears everything`() {
        val t = connectionsTargets("any", "any")
        assertEquals(false, restriction(t, UserManager.DISALLOW_ADD_WIFI_CONFIG))
        assertEquals(false, restriction(t, UserManager.DISALLOW_CONFIG_BLUETOOTH))
        assertEquals(DevicePolicyManager.WIFI_SECURITY_OPEN, t.minWifiSecurity)
    }

    @Test
    fun `paired closes new Bluetooth pairing`() {
        assertEquals(
            true,
            restriction(connectionsTargets("any", "paired"), UserManager.DISALLOW_CONFIG_BLUETOOTH),
        )
    }

    @Test
    fun `an unknown tier is the careful one`() {
        val t = connectionsTargets("bogus", "bogus")
        assertEquals(true, restriction(t, UserManager.DISALLOW_ADD_WIFI_CONFIG))
        assertEquals(true, restriction(t, UserManager.DISALLOW_CONFIG_BLUETOOTH))
    }

    @Test
    fun `the core's answer parses, and an error or an unknown value is no answer`() {
        assertEquals(
            CharterCore.Connections("secured", "paired"),
            parseConnections("""{"wifi":"secured","bluetooth":"paired"}"""),
        )
        assertNull(parseConnections("""{"error":"connections clause unreadable"}"""))
        assertNull(parseConnections("""{"wifi":"open","bluetooth":"any"}"""))
        assertNull(parseConnections(""))
    }
}
