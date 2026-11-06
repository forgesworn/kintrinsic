package org.forgesworn.charter.service

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test

class RelayPollPaceTest {
    @Test fun `overlapping owners and configuration changes are one attention session`() {
        val a = RelayAttention(); val first = Any(); val second = Any()
        assertTrue(a.show(first, 0)); assertFalse(a.show(first, 1))
        assertFalse(a.show(second, 60000))
        a.hide(first, 60001); assertFalse(a.show(first, 60002))
        a.hide(first, 60003); a.hide(second, 60004)
        assertFalse(a.show(Any(), 60005)) // rotation, no new boost
        val fresh = RelayAttention(); val owner = Any()
        assertTrue(fresh.show(owner, 0)); fresh.hide(owner, 1)
        assertTrue(fresh.show(owner, 30001))
    }
    @Test fun `request survives leaving UI but dark overrides and wake does not renew`() {
        val p = RelaySchedule()
        p.submitted("a", 1000)
        assertEquals(5000L, p.interval(true, 2000))
        assertEquals(300000L, p.interval(false, 3000))
        assertEquals(5000L, p.interval(true, 120000))
        assertEquals(60000L, p.interval(true, 121000))
        assertEquals(60000L, p.interval(true, 900000))
    }
    @Test fun `answers are per request and duplicate submission never renews`() {
        val p = RelaySchedule()
        p.submitted("a", 0); p.submitted("b", 30000)
        p.reconcile(mapOf("a" to "denied", "b" to "pending"))
        assertTrue(p.waiting(120001))
        p.submitted("b", 140000)
        assertFalse(p.waiting(150000))
        p.submitted("c", 160000)
        p.reconcile(mapOf("c" to "enacted"))
        assertFalse(p.waiting(160001))
    }
    @Test fun `opening expires even when visible and re-rendering does not extend it`() {
        val p = RelaySchedule()
        assertTrue(p.opened(0)); assertFalse(p.opened(10000))
        assertEquals(5000L, p.interval(true, 14999))
        assertEquals(60000L, p.interval(true, 15000))
        assertFalse(p.opened(59000))
    }
    @Test fun `settings disable boosts and process restoration never revives pending asks`() {
        val p = RelaySchedule()
        p.settings = BatterySettings(600, 120, false, false, 60)
        p.opened(0); p.submitted("a", 0)
        assertEquals(120000L, p.interval(true, 1))
        assertEquals(600000L, p.interval(false, 1))
        val restored = RelaySchedule()
        restored.reconcile(mapOf("a" to "pending"))
        assertFalse(restored.waiting(1))
    }
    @Test fun `legacy and malformed persisted settings restore recommended steps`() {
        assertEquals(BatterySettings(), BatterySettings.parse(null))
        assertEquals(BatterySettings(), BatterySettings.parse(JSONObject("""{"screenOffSecs":-1,"ordinarySecs":999,"boostSecs":0}""")))
        val valid = JSONObject("""{"screenOffSecs":600,"ordinarySecs":120,"boostSecs":60,"requestBoost":false,"openingBoost":false}""")
        assertEquals(BatterySettings(600,120,false,false,60), BatterySettings.parse(valid))
    }
    @Test fun `only the exact longer-battery preset counts as longer battery`() {
        assertTrue(BatterySettings(600, 120, true, false, 60).isLongerBattery)
        assertFalse(BatterySettings().isLongerBattery)
        assertFalse(BatterySettings(120, 30, true, true, 180).isLongerBattery)
        assertFalse(BatterySettings(600, 120, true, true, 60).isLongerBattery)
    }
}
