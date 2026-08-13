package org.forgesworn.charter.enforce.dns

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.net.InetAddress

class DnsMessageTest {
    // A DNS query for "www.google.com" A, txn id 0x1234.
    private fun googleQuery(): ByteArray = byteArrayOf(
        0x12, 0x34,             // txn id
        0x01, 0x00,             // flags: standard query, RD
        0x00, 0x01,             // qdcount 1
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x03, 'w'.code.toByte(), 'w'.code.toByte(), 'w'.code.toByte(),
        0x06, 'g'.code.toByte(), 'o'.code.toByte(), 'o'.code.toByte(),
              'g'.code.toByte(), 'l'.code.toByte(), 'e'.code.toByte(),
        0x03, 'c'.code.toByte(), 'o'.code.toByte(), 'm'.code.toByte(),
        0x00,                   // root
        0x00, 0x01,             // qtype A
        0x00, 0x01,             // qclass IN
    )

    @Test fun parses_name_type_and_txn() {
        val q = parseQuestion(googleQuery())!!
        assertEquals("www.google.com", q.name)
        assertEquals(TYPE_A, q.qtype)
        assertEquals(0x1234, q.txnId)
    }

    @Test fun nxdomain_echoes_txn_and_sets_rcode3() {
        val q = parseQuestion(googleQuery())!!
        val resp = buildNxdomain(q)
        assertEquals(0x12, resp[0].toInt() and 0xff)
        assertEquals(0x34, resp[1].toInt() and 0xff)
        // QR=1 (response) bit and RCODE=3 in the low nibble of byte 3.
        assertEquals(0x03, resp[3].toInt() and 0x0f)   // NXDOMAIN
        assertEquals(0x80, resp[2].toInt() and 0x80)   // QR set
    }

    @Test fun answer_carries_the_a_record() {
        val q = parseQuestion(googleQuery())!!
        val ip = InetAddress.getByName("216.239.38.120")   // forcesafesearch.google.com
        val resp = buildAnswer(q, listOf(ip), ttl = 60)
        // ancount at bytes 6-7 == 1
        assertEquals(1, ((resp[6].toInt() and 0xff) shl 8) or (resp[7].toInt() and 0xff))
        // last 4 bytes are the A record data.
        val n = resp.size
        assertArrayEquals(ip.address, resp.copyOfRange(n - 4, n))
    }

    @Test fun malformed_returns_null() {
        assertNull(parseQuestion(byteArrayOf(0x00, 0x01)))
    }
}
