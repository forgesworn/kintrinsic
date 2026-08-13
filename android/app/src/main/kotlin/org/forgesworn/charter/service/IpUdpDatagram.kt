package org.forgesworn.charter.service

/**
 * Minimal IPv4/IPv6 + UDP parse/rebuild for DNS datagrams flowing through the
 * TUN. We only ever handle packets addressed to our own virtual DNS IPs, so the
 * response is the same packet with src/dst swapped and a new UDP payload.
 */
class IpUdpDatagram private constructor(
    private val packet: ByteArray,
    private val isV6: Boolean,
    private val ipHeaderLen: Int,
    val dstPort: Int,
    val payload: ByteArray,
) {
    /** Rebuild an IP+UDP datagram back to the querier with `dns` as the payload. */
    fun swapAndWrapUdp(dns: ByteArray): ByteArray {
        // Reuse the incoming header; swap addresses; recompute lengths+checksums.
        return if (isV6) buildV6(packet, ipHeaderLen, dns) else buildV4(packet, ipHeaderLen, dns)
    }

    companion object {
        fun parse(p: ByteArray): IpUdpDatagram? {
            if (p.isEmpty()) return null
            return when (p[0].toInt() ushr 4) {
                4 -> parseV4(p)
                6 -> parseV6(p)
                else -> null
            }
        }

        private fun u16(p: ByteArray, o: Int) = ((p[o].toInt() and 0xff) shl 8) or (p[o + 1].toInt() and 0xff)

        private fun parseV4(p: ByteArray): IpUdpDatagram? {
            if (p.size < 20) return null
            val ihl = (p[0].toInt() and 0x0f) * 4
            if (p[9].toInt() != 17) return null // not UDP
            if (p.size < ihl + 8) return null
            val dstPort = u16(p, ihl + 2)
            val udpLen = u16(p, ihl + 4)
            val payloadLen = udpLen - 8
            if (payloadLen < 0 || ihl + 8 + payloadLen > p.size) return null
            val payload = p.copyOfRange(ihl + 8, ihl + 8 + payloadLen)
            return IpUdpDatagram(p, false, ihl, dstPort, payload)
        }

        private fun parseV6(p: ByteArray): IpUdpDatagram? {
            if (p.size < 40) return null
            if (p[6].toInt() != 17) return null // next header != UDP (no ext headers handled)
            if (p.size < 48) return null
            val dstPort = u16(p, 40 + 2)
            val udpLen = u16(p, 40 + 4)
            val payloadLen = udpLen - 8
            if (payloadLen < 0 || 48 + payloadLen > p.size) return null
            val payload = p.copyOfRange(48, 48 + payloadLen)
            return IpUdpDatagram(p, true, 40, dstPort, payload)
        }

        private fun buildV4(orig: ByteArray, ihl: Int, dns: ByteArray): ByteArray {
            val total = ihl + 8 + dns.size
            val out = ByteArray(total)
            System.arraycopy(orig, 0, out, 0, ihl)
            // total length
            out[2] = (total ushr 8).toByte(); out[3] = total.toByte()
            out[8] = 64 // TTL
            // swap src/dst (bytes 12..15 <-> 16..19)
            for (k in 0 until 4) { val t = out[12 + k]; out[12 + k] = orig[16 + k]; out[16 + k] = t }
            // zero IP checksum then recompute
            out[10] = 0; out[11] = 0
            val ipck = checksum(out, 0, ihl)
            out[10] = (ipck ushr 8).toByte(); out[11] = ipck.toByte()
            // UDP header: swap ports, set length, zero checksum (legal for IPv4)
            val udp = ihl
            out[udp] = orig[udp + 2]; out[udp + 1] = orig[udp + 3]
            out[udp + 2] = orig[udp]; out[udp + 3] = orig[udp + 1]
            val ulen = 8 + dns.size
            out[udp + 4] = (ulen ushr 8).toByte(); out[udp + 5] = ulen.toByte()
            out[udp + 6] = 0; out[udp + 7] = 0
            System.arraycopy(dns, 0, out, udp + 8, dns.size)
            return out
        }

        private fun buildV6(orig: ByteArray, ihl: Int, dns: ByteArray): ByteArray {
            val ulen = 8 + dns.size
            val total = 40 + ulen
            val out = ByteArray(total)
            System.arraycopy(orig, 0, out, 0, 40)
            out[4] = (ulen ushr 8).toByte(); out[5] = ulen.toByte() // payload length
            // swap src (8..23) and dst (24..39)
            for (k in 0 until 16) { val t = out[8 + k]; out[8 + k] = orig[24 + k]; out[24 + k] = t }
            val udp = 40
            out[udp] = orig[udp + 2]; out[udp + 1] = orig[udp + 3]
            out[udp + 2] = orig[udp]; out[udp + 3] = orig[udp + 1]
            out[udp + 4] = (ulen ushr 8).toByte(); out[udp + 5] = ulen.toByte()
            // UDP checksum mandatory in IPv6; compute over pseudo-header.
            out[udp + 6] = 0; out[udp + 7] = 0
            System.arraycopy(dns, 0, out, udp + 8, dns.size)
            val ck = udpV6Checksum(out)
            out[udp + 6] = (ck ushr 8).toByte(); out[udp + 7] = ck.toByte()
            return out
        }

        private fun checksum(b: ByteArray, off: Int, len: Int): Int {
            var sum = 0L; var i = off
            while (i < off + len - 1) { sum += (((b[i].toInt() and 0xff) shl 8) or (b[i + 1].toInt() and 0xff)); i += 2 }
            if ((len and 1) == 1) sum += ((b[off + len - 1].toInt() and 0xff) shl 8).toLong()
            while (sum shr 16 != 0L) sum = (sum and 0xffff) + (sum shr 16)
            return (sum.inv() and 0xffff).toInt()
        }

        private fun udpV6Checksum(p: ByteArray): Int {
            val udpLen = 8 + (p.size - 48)
            var sum = 0L
            for (k in 8 until 40 step 2) sum += u16(p, k).toLong() // src+dst addrs
            sum += udpLen.toLong()
            sum += 17L // next header
            var i = 40
            while (i < p.size - 1) { sum += u16(p, i).toLong(); i += 2 }
            if (((p.size - 40) and 1) == 1) sum += ((p[p.size - 1].toInt() and 0xff) shl 8).toLong()
            while (sum shr 16 != 0L) sum = (sum and 0xffff) + (sum shr 16)
            var r = (sum.inv() and 0xffff).toInt()
            if (r == 0) r = 0xffff
            return r
        }
    }
}
