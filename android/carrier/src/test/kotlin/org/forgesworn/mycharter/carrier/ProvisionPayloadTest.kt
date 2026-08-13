package org.forgesworn.mycharter.carrier

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ProvisionPayloadTest {
    private val sk = "a".repeat(64)
    private val pk = "b".repeat(64)

    private fun json(
        v: Int = 1,
        sk: String = this.sk,
        pk: String = this.pk,
        relays: String = """["wss://relay.trotters.cc"]""",
    ) = """{"v":$v,"guardianSkHex":"$sk","guardianPubkeyHex":"$pk","relays":$relays}"""

    @Test fun parsesAValidPayload() {
        val p = ProvisionPayload.parse(json())!!
        assertEquals(sk, p.guardianSkHex)
        assertEquals(pk, p.guardianPubkeyHex)
        assertEquals(listOf("wss://relay.trotters.cc"), p.relays)
    }

    @Test fun rejectsBadVersionBadHexAndEmptyRelays() {
        assertNull(ProvisionPayload.parse(json(v = 2)))
        assertNull(ProvisionPayload.parse(json(sk = "zz".repeat(32))))   // not hex
        assertNull(ProvisionPayload.parse(json(sk = "aa")))              // wrong len
        assertNull(ProvisionPayload.parse(json(pk = "B".repeat(64))))    // uppercase
        assertNull(ProvisionPayload.parse(json(relays = "[]")))
        assertNull(ProvisionPayload.parse(json(relays = """["http://x"]"""))) // not wss
        assertNull(ProvisionPayload.parse("not json"))
    }
}
