package org.forgesworn.charter.service

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress

/**
 * Pure-JVM tests for [IpUdpDatagram]: hand-built IPv4/IPv6 + UDP datagrams,
 * no Android APIs. This is the riskiest code in the DNS-filtering TUN (IP/UDP
 * framing + Internet checksums), so it gets direct byte-level coverage.
 */
class IpUdpDatagramTest {
    private val dnsQuery = byteArrayOf(
        0x12, 0x34,             // txn id
        0x01, 0x00,             // flags: standard query, RD
        0x00, 0x01,             // qdcount 1
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x03, 'w'.code.toByte(), 'w'.code.toByte(), 'w'.code.toByte(),
        0x07, 'e'.code.toByte(), 'x'.code.toByte(), 'a'.code.toByte(),
              'm'.code.toByte(), 'p'.code.toByte(), 'l'.code.toByte(), 'e'.code.toByte(),
        0x03, 'c'.code.toByte(), 'o'.code.toByte(), 'm'.code.toByte(),
        0x00,                   // root
        0x00, 0x01,             // qtype A
        0x00, 0x01,             // qclass IN
    )

    private val dnsResponse = byteArrayOf(
        0x12, 0x34, 0x81.toByte(), 0x80.toByte(), 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
        0xc0.toByte(), 0x0c, 0x00, 0x01, 0x00, 0x01,
        0x00, 0x00, 0x00, 0x3c,
        0x00, 0x04,
        93, 184.toByte(), 216.toByte(), 34,
    )

    // --- IPv4 helpers ---------------------------------------------------

    private fun buildV4Packet(
        payload: ByteArray,
        srcIp: String = "10.111.0.2",
        dstIp: String = "10.111.0.53",
        srcPort: Int = 54321,
        dstPort: Int = 53,
        protocol: Int = 17,
    ): ByteArray {
        val ihl = 20
        val udpLen = 8 + payload.size
        val total = ihl + udpLen
        val out = ByteArray(total)
        out[0] = 0x45 // version 4, IHL 5 (x4 = 20 bytes)
        out[1] = 0x00
        out[2] = (total ushr 8).toByte(); out[3] = total.toByte()
        out[4] = 0x00; out[5] = 0x00 // identification
        out[6] = 0x00; out[7] = 0x00 // flags/fragment offset
        out[8] = 64 // TTL
        out[9] = protocol.toByte()
        out[10] = 0; out[11] = 0 // header checksum (parse() does not validate)
        val src = InetAddress.getByName(srcIp).address
        val dst = InetAddress.getByName(dstIp).address
        System.arraycopy(src, 0, out, 12, 4)
        System.arraycopy(dst, 0, out, 16, 4)
        out[20] = (srcPort ushr 8).toByte(); out[21] = srcPort.toByte()
        out[22] = (dstPort ushr 8).toByte(); out[23] = dstPort.toByte()
        out[24] = (udpLen ushr 8).toByte(); out[25] = udpLen.toByte()
        out[26] = 0; out[27] = 0 // UDP checksum (optional/0 under IPv4)
        System.arraycopy(payload, 0, out, 28, payload.size)
        return out
    }

    /** Internet checksum validation: sum all 16-bit words (incl. the checksum
     *  field itself); a correct checksum folds the total to all-ones (0xffff). */
    private fun foldedSum(b: ByteArray, off: Int, len: Int): Int {
        var sum = 0L
        var i = off
        while (i < off + len - 1) {
            sum += (((b[i].toInt() and 0xff) shl 8) or (b[i + 1].toInt() and 0xff))
            i += 2
        }
        if ((len and 1) == 1) sum += ((b[off + len - 1].toInt() and 0xff) shl 8).toLong()
        while (sum shr 16 != 0L) sum = (sum and 0xffff) + (sum shr 16)
        return (sum and 0xffff).toInt()
    }

    @Test fun v4_parses_dst_port_and_payload() {
        val pkt = buildV4Packet(dnsQuery)
        val ip = IpUdpDatagram.parse(pkt)!!
        assertEquals(53, ip.dstPort)
        assertArrayEquals(dnsQuery, ip.payload)
    }

    @Test fun v4_round_trip_swaps_addresses_and_ports_and_carries_response() {
        val query = buildV4Packet(dnsQuery, srcIp = "10.111.0.2", dstIp = "10.111.0.53", srcPort = 54321, dstPort = 53)
        val parsed = IpUdpDatagram.parse(query)!!

        val rebuilt = parsed.swapAndWrapUdp(dnsResponse)
        val reparsed = IpUdpDatagram.parse(rebuilt)!!

        // dstPort of the reply is the original querier's src port.
        assertEquals(54321, reparsed.dstPort)
        assertArrayEquals(dnsResponse, reparsed.payload)

        // Addresses swapped: new src == old dst, new dst == old src.
        assertArrayEquals(InetAddress.getByName("10.111.0.53").address, rebuilt.copyOfRange(12, 16))
        assertArrayEquals(InetAddress.getByName("10.111.0.2").address, rebuilt.copyOfRange(16, 20))
    }

    @Test fun v4_rebuilt_header_checksum_validates() {
        val query = buildV4Packet(dnsQuery)
        val parsed = IpUdpDatagram.parse(query)!!
        val rebuilt = parsed.swapAndWrapUdp(dnsResponse)
        val ihl = (rebuilt[0].toInt() and 0x0f) * 4
        assertEquals(0xffff, foldedSum(rebuilt, 0, ihl))
    }

    @Test fun v4_non_udp_protocol_returns_null() {
        val tcpPkt = buildV4Packet(dnsQuery, protocol = 6)
        assertNull(IpUdpDatagram.parse(tcpPkt))
    }

    @Test fun v4_too_short_returns_null() {
        assertNull(IpUdpDatagram.parse(byteArrayOf(0x45, 0x00, 0x00, 0x14)))
    }

    // --- IPv6 helpers -----------------------------------------------------

    private fun buildV6Packet(
        payload: ByteArray,
        srcIp: String = "fd00:6368:6172:74::2",
        dstIp: String = "fd00:6368:6172:74::53",
        srcPort: Int = 54321,
        dstPort: Int = 53,
        nextHeader: Int = 17,
    ): ByteArray {
        val udpLen = 8 + payload.size
        val total = 40 + udpLen
        val out = ByteArray(total)
        out[0] = 0x60 // version 6
        out[4] = (udpLen ushr 8).toByte(); out[5] = udpLen.toByte() // payload length
        out[6] = nextHeader.toByte()
        out[7] = 64 // hop limit
        val src = InetAddress.getByName(srcIp).address
        val dst = InetAddress.getByName(dstIp).address
        System.arraycopy(src, 0, out, 8, 16)
        System.arraycopy(dst, 0, out, 24, 16)
        out[40] = (srcPort ushr 8).toByte(); out[41] = srcPort.toByte()
        out[42] = (dstPort ushr 8).toByte(); out[43] = dstPort.toByte()
        out[44] = (udpLen ushr 8).toByte(); out[45] = udpLen.toByte()
        out[46] = 0; out[47] = 0 // UDP checksum (unset on the inbound test packet)
        System.arraycopy(payload, 0, out, 48, payload.size)
        return out
    }

    /** Validates the mandatory IPv6 UDP checksum over the pseudo-header + segment. */
    private fun v6UdpChecksumValidates(p: ByteArray): Boolean {
        fun u16(o: Int) = ((p[o].toInt() and 0xff) shl 8) or (p[o + 1].toInt() and 0xff)
        val udpLen = 8 + (p.size - 48)
        var sum = 0L
        for (k in 8 until 40 step 2) sum += u16(k).toLong() // src + dst addrs
        sum += udpLen.toLong()
        sum += 17L // next header
        var i = 40
        while (i < p.size - 1) { sum += u16(i).toLong(); i += 2 }
        if (((p.size - 40) and 1) == 1) sum += ((p[p.size - 1].toInt() and 0xff) shl 8).toLong()
        while (sum shr 16 != 0L) sum = (sum and 0xffff) + (sum shr 16)
        return (sum and 0xffff) == 0xffffL
    }

    @Test fun v6_round_trip_swaps_addresses_and_checksum_validates() {
        val query = buildV6Packet(dnsQuery)
        val parsed = IpUdpDatagram.parse(query)!!
        assertEquals(53, parsed.dstPort)
        assertArrayEquals(dnsQuery, parsed.payload)

        val rebuilt = parsed.swapAndWrapUdp(dnsResponse)
        val reparsed = IpUdpDatagram.parse(rebuilt)!!
        assertEquals(54321, reparsed.dstPort)
        assertArrayEquals(dnsResponse, reparsed.payload)

        // Addresses swapped.
        assertArrayEquals(InetAddress.getByName("fd00:6368:6172:74::53").address, rebuilt.copyOfRange(8, 24))
        assertArrayEquals(InetAddress.getByName("fd00:6368:6172:74::2").address, rebuilt.copyOfRange(24, 40))

        // The UDP checksum is mandatory (non-zero) in IPv6, and must validate.
        val storedChecksum = ((rebuilt[46].toInt() and 0xff) shl 8) or (rebuilt[47].toInt() and 0xff)
        assertTrue("IPv6 UDP checksum must be non-zero", storedChecksum != 0)
        assertTrue("IPv6 UDP checksum must validate", v6UdpChecksumValidates(rebuilt))
    }

    @Test fun v6_non_udp_next_header_returns_null() {
        val icmpv6Pkt = buildV6Packet(dnsQuery, nextHeader = 58)
        assertNull(IpUdpDatagram.parse(icmpv6Pkt))
    }

    @Test fun v6_too_short_returns_null() {
        assertNull(IpUdpDatagram.parse(ByteArray(20)))
    }
}
