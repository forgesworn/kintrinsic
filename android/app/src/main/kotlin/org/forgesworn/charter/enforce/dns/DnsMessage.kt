package org.forgesworn.charter.enforce.dns

import java.io.ByteArrayOutputStream
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress

const val TYPE_A = 1
const val TYPE_AAAA = 28

/** The single question from a DNS query, plus the raw query for upstream relay. */
data class DnsQuestion(
    val name: String,
    val qtype: Int,
    val txnId: Int,
    val rawQuery: ByteArray,
) {
    val qnameEnd: Int get() = 12 + name.encodedLen() // offset just past QNAME
}

private fun String.encodedLen(): Int {
    if (isEmpty()) return 1
    // each label = 1 length byte + label bytes; + terminating root 0
    return split('.').sumOf { it.length + 1 } + 1
}

/**
 * Parse the first question of a DNS query. Returns null on any malformation —
 * the caller drops the packet (fail-closed: an unparseable query is not
 * resolved). Only single-question queries (the universal real-world case).
 */
fun parseQuestion(packet: ByteArray): DnsQuestion? {
    if (packet.size < 12 + 5) return null
    val txn = ((packet[0].toInt() and 0xff) shl 8) or (packet[1].toInt() and 0xff)
    val qdcount = ((packet[4].toInt() and 0xff) shl 8) or (packet[5].toInt() and 0xff)
    if (qdcount < 1) return null
    val sb = StringBuilder()
    var i = 12
    while (i < packet.size) {
        val len = packet[i].toInt() and 0xff
        if (len == 0) { i += 1; break }
        if (len and 0xc0 != 0) return null // compression pointer in a question: reject
        if (i + 1 + len > packet.size) return null
        if (sb.isNotEmpty()) sb.append('.')
        for (j in 0 until len) sb.append((packet[i + 1 + j].toInt() and 0xff).toChar())
        i += 1 + len
    }
    if (i + 4 > packet.size) return null
    val qtype = ((packet[i].toInt() and 0xff) shl 8) or (packet[i + 1].toInt() and 0xff)
    return DnsQuestion(sb.toString().lowercase(), qtype, txn, packet)
}

private fun header(txn: Int, flags: Int, an: Int): ByteArray = byteArrayOf(
    (txn shr 8).toByte(), txn.toByte(),
    (flags shr 8).toByte(), flags.toByte(),
    0x00, 0x01,                     // qdcount 1 (we echo the question)
    (an shr 8).toByte(), an.toByte(),
    0x00, 0x00, 0x00, 0x00,
)

/** The original question section bytes (offset 12 .. end of QCLASS). */
private fun questionSection(q: DnsQuestion): ByteArray {
    val end = q.qnameEnd + 4 // + qtype(2) + qclass(2)
    return q.rawQuery.copyOfRange(12, end)
}

/** NXDOMAIN response: QR=1, RD/RA, RCODE=3, echoes the question. */
fun buildNxdomain(q: DnsQuestion): ByteArray {
    val out = ByteArrayOutputStream()
    out.write(header(q.txnId, 0x8183, 0)) // QR|RD|RA + RCODE 3
    out.write(questionSection(q))
    return out.toByteArray()
}

/** A/AAAA answer: one record per matching-family address, name = pointer 0xC00C. */
fun buildAnswer(q: DnsQuestion, ips: List<InetAddress>, ttl: Int = 60): ByteArray {
    val matching = ips.filter {
        (q.qtype == TYPE_A && it is Inet4Address) || (q.qtype == TYPE_AAAA && it is Inet6Address)
    }
    val out = ByteArrayOutputStream()
    out.write(header(q.txnId, 0x8180, matching.size)) // QR|RD|RA, RCODE 0
    out.write(questionSection(q))
    for (ip in matching) {
        out.write(0xc0); out.write(0x0c)                       // name pointer -> offset 12
        out.write(0x00); out.write(q.qtype)                    // type
        out.write(0x00); out.write(0x01)                       // class IN
        out.write((ttl shr 24)); out.write((ttl shr 16)); out.write((ttl shr 8)); out.write(ttl)
        val addr = ip.address
        out.write(0x00); out.write(addr.size)                  // rdlength
        out.write(addr)
    }
    return out.toByteArray()
}

/** Build a minimal A-record query for `host` (txn id 0 — we control the socket). */
fun buildQuery(host: String): ByteArray {
    val out = java.io.ByteArrayOutputStream()
    out.write(byteArrayOf(0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00))
    for (label in host.split('.')) {
        out.write(label.length); for (c in label) out.write(c.code)
    }
    out.write(0x00)
    out.write(byteArrayOf(0x00, TYPE_A.toByte(), 0x00, 0x01))
    return out.toByteArray()
}

/** Extract A-record addresses from a DNS response (best-effort; ignores names). */
fun parseAddresses(resp: ByteArray): List<java.net.InetAddress> {
    val out = ArrayList<java.net.InetAddress>()
    if (resp.size < 12) return out
    val an = ((resp[6].toInt() and 0xff) shl 8) or (resp[7].toInt() and 0xff)
    var i = 12
    // skip the single question
    while (i < resp.size && resp[i].toInt() != 0) {
        val len = resp[i].toInt() and 0xff
        if (len and 0xc0 != 0) { i += 2; break }
        i += 1 + len
    }
    i += 1 + 4 // root + qtype + qclass
    var seen = 0
    while (seen < an && i + 12 <= resp.size) {
        // name (pointer or labels)
        if ((resp[i].toInt() and 0xc0) == 0xc0) i += 2 else { while (i < resp.size && resp[i].toInt() != 0) i += 1 + (resp[i].toInt() and 0xff); i += 1 }
        if (i + 10 > resp.size) break
        val type = ((resp[i].toInt() and 0xff) shl 8) or (resp[i + 1].toInt() and 0xff)
        val rdlen = ((resp[i + 8].toInt() and 0xff) shl 8) or (resp[i + 9].toInt() and 0xff)
        val rd = i + 10
        if (rd + rdlen > resp.size) break
        if (type == TYPE_A && rdlen == 4) out.add(java.net.InetAddress.getByAddress(resp.copyOfRange(rd, rd + 4)))
        i = rd + rdlen
        seen++
    }
    return out
}
