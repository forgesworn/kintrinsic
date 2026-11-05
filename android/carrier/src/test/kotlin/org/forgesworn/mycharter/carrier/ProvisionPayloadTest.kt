package org.forgesworn.mycharter.carrier

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ProvisionPayloadTest {
    private val sk = "a".repeat(64)
    private val pk = "b".repeat(64)
    private val wSk = "c".repeat(64)
    private val wPk = "d".repeat(64)

    private fun json(
        v: Int = 1,
        sk: String = this.sk,
        pk: String = this.pk,
        relays: String = """["wss://relay.trotters.cc"]""",
        keys: String? = null,
    ) = """{"v":$v,"guardianSkHex":"$sk","guardianPubkeyHex":"$pk","relays":$relays""" +
        (keys?.let { ""","keys":$it""" } ?: "") + "}"

    @Test fun parsesAValidPayload() {
        val p = ProvisionPayload.parse(json())!!
        assertEquals(sk, p.guardianSkHex)
        assertEquals(pk, p.guardianPubkeyHex)
        assertEquals(listOf("wss://relay.trotters.cc"), p.relays)
        // A console from before the key list: the primary alone.
        assertTrue(p.extraKeys.isEmpty())
        assertEquals(listOf(pk), p.pubkeys)
    }

    /** D5: a phone-held root and its working key, both listened on. */
    @Test fun parsesARootAndItsWorkingKey() {
        val p = ProvisionPayload.parse(json(keys = """[{"skHex":"$wSk","pubkeyHex":"$wPk"}]"""))!!
        assertEquals(listOf(pk, wPk), p.pubkeys)
        assertEquals(wSk, p.secretFor(wPk))
        assertEquals(sk, p.secretFor(pk))
        // A wrap naming none of ours, or none at all: the primary's secret.
        assertEquals(sk, p.secretFor("e".repeat(64)))
        assertEquals(sk, p.secretFor(null))
        // The primary repeated in the list, or a key twice: listed once.
        val dup = ProvisionPayload.parse(
            json(keys = """[{"skHex":"$sk","pubkeyHex":"$pk"},{"skHex":"$wSk","pubkeyHex":"$wPk"},{"skHex":"$wSk","pubkeyHex":"$wPk"}]"""),
        )!!
        assertEquals(listOf(pk, wPk), dup.pubkeys)
        assertEquals(json(keys = "[]").let { ProvisionPayload.parse(it)!!.pubkeys }, listOf(pk))
    }

    @Test fun rejectsABadKeyList() {
        assertNull(ProvisionPayload.parse(json(keys = """[{"skHex":"zz","pubkeyHex":"$wPk"}]""")))
        assertNull(ProvisionPayload.parse(json(keys = """[{"skHex":"$wSk"}]""")))
        assertNull(ProvisionPayload.parse(json(keys = """["$wPk"]""")))
        assertNull(ProvisionPayload.parse(json(keys = """{"skHex":"$wSk","pubkeyHex":"$wPk"}""")))
        // One pubkey with two secrets: refused whole, never half-provisioned.
        assertNull(ProvisionPayload.parse(json(keys = """[{"skHex":"$wSk","pubkeyHex":"$pk"}]""")))
        // More keys than a family of working keys needs.
        val many = (1..5).joinToString(",", "[", "]") {
            val h = "%064x".format(it)
            """{"skHex":"$h","pubkeyHex":"$h"}"""
        }
        assertNull(ProvisionPayload.parse(json(keys = many)))
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
