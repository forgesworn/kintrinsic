package org.forgesworn.charter.ui

import java.security.MessageDigest

/**
 * The short pairing code, and the guardian npub, that this phone shows before
 * it will pin anybody (S6, review 2026-08-07).
 *
 * WHY THIS IS HERE AND NOT ONLY ON THE WEB PAGE. The review found that
 * `/pair` copies whatever `bunker://…` sits in the URL fragment into its
 * open-app button, so anyone can send a ward a link on the legitimate domain
 * carrying a stranger's guardian identity. That page was fixed first — and it
 * turned out to be the wrong screen. In the ordinary flow the verified App
 * Link means the browser never renders anything: Android hands the URL
 * straight to this app, and the screen the ward actually confirms on is
 * `MainActivity`'s. (On top of which the live vhost SPA-rewrites every path,
 * so `/pair/index.html` has never been served at all.) A check placed where
 * the ward never looks is not a check.
 *
 * WHAT IT PROVES. Only that this screen and the grown-up's screen are talking
 * about the same (guardian, token) pair. An attacker's link produces a
 * perfectly self-consistent code of its own — it is worthless unless it is
 * COMPARED, which is why the copy asks for the comparison instead of
 * presenting the digits as a seal of approval.
 *
 * MUST AGREE, byte for byte, with `apps/charter-app/src/domain/pairingSas.ts`
 * and with the hand-written copy inside `public/pair/index.html`. Two screens
 * showing different codes for one link is worse than showing none: it teaches
 * a family that the check is broken and to tap through it. All three are
 * pinned to `core/crates/charter-testkit/vectors/pairing/sas_vectors.json`,
 * which this module's test reads off the shared test classpath.
 */
object PairingSas {
    /** Domain separator — a hash of these inputs must mean this and nothing else. */
    private const val DOMAIN = "charter-pair-sas:v1"

    /**
     * The six-digit code for a (guardian pubkey, one-time token) pair,
     * formatted `"123 456"`.
     *
     * Derived from BOTH: the pubkey alone would repeat across every pairing
     * this guardian ever does, so a code glimpsed once — over a shoulder, in a
     * photo of the QR — would be replayable forever.
     */
    fun code(guardianPubkeyHex: String, token: String): String {
        val input = "$DOMAIN:${guardianPubkeyHex.lowercase()}:$token"
        val d = MessageDigest.getInstance("SHA-256").digest(input.toByteArray(Charsets.UTF_8))
        // 24 bits -> 0..999999. The modulo bias on the largest residues is
        // irrelevant — this is a comparison code between two screens, not a
        // secret anyone has to guess — and "fixing" it would break agreement
        // with the other two implementations, which is the only property that
        // actually matters here.
        val n = (((d[0].toInt() and 0xff) shl 16) or
            ((d[1].toInt() and 0xff) shl 8) or
            (d[2].toInt() and 0xff)) % 1_000_000
        val s = n.toString().padStart(6, '0')
        return "${s.substring(0, 3)} ${s.substring(3)}"
    }

    /**
     * The same guardian key as a NIP-19 `npub…`, for the ward and the grown-up
     * to compare character by character when six digits are not enough.
     *
     * Hand-rolled bech32 (BIP-173): the ward app carries no Nostr encoding
     * library, and pulling one in for sixty characters of display text would
     * be a poor trade. Pinned to the shared vectors against nostr-tools'
     * output, which is what Kintrinsic shows on the other screen.
     */
    fun npub(guardianPubkeyHex: String): String? {
        val bytes = hexToBytes(guardianPubkeyHex) ?: return null
        val five = to5Bit(bytes)
        val data = five + checksum(HRP, five)
        return HRP + "1" + data.map { CHARSET[it] }.joinToString("")
    }

    /** What the ward's confirm screen shows for one pairing link. */
    data class Check(val code: String, val npub: String)

    /**
     * Read a pairing link and produce the check to put in front of the ward,
     * or `null` when the link carries no identity we can name.
     *
     * Accepts both forms the app is handed: the raw `bunker://<pk>?…&token=…`
     * and whatever a ward has pasted or edited into the field. Deliberately
     * total and side-effect free, so the screen's behaviour — including the
     * "say nothing rather than something reassuring" case — is testable
     * without an emulator.
     *
     * Returning `null` for an unreadable link matters: a half-rendered check,
     * or a code left over from a previous link, is a check that passes for the
     * wrong thing. `pair()` will refuse a bad link on its own merits a moment
     * later; this screen's job is only to not mislead in the meantime.
     */
    fun check(bunkerUri: String): Check? {
        if (!bunkerUri.startsWith("bunker://")) return null
        val pubkey = bunkerUri.removePrefix("bunker://").substringBefore('?').lowercase()
        if (!Regex("^[0-9a-f]{64}$").matches(pubkey)) return null
        val npub = npub(pubkey) ?: return null
        val token = Regex("[?&]token=([^&]*)").find(bunkerUri)?.groupValues?.getOrNull(1).orEmpty()
        return Check(code(pubkey, token), npub)
    }

    private const val HRP = "npub"
    private const val CHARSET = "qpzry9x8gf2tvdw0s3jn54khce6mua7l"

    private fun hexToBytes(hex: String): ByteArray? {
        if (hex.length != 64) return null
        val out = ByteArray(32)
        for (i in 0 until 32) {
            val hi = Character.digit(hex[i * 2], 16)
            val lo = Character.digit(hex[i * 2 + 1], 16)
            if (hi < 0 || lo < 0) return null
            out[i] = ((hi shl 4) or lo).toByte()
        }
        return out
    }

    /** 8-bit groups -> 5-bit groups, zero-padded on the right. */
    private fun to5Bit(bytes: ByteArray): List<Int> {
        var acc = 0
        var bits = 0
        val out = mutableListOf<Int>()
        for (b in bytes) {
            acc = (acc shl 8) or (b.toInt() and 0xff)
            bits += 8
            while (bits >= 5) {
                bits -= 5
                out.add((acc shr bits) and 31)
            }
        }
        if (bits > 0) out.add((acc shl (5 - bits)) and 31)
        return out
    }

    private fun polymod(values: List<Int>): Int {
        val gen = intArrayOf(0x3b6a57b2, 0x26508e6d, 0x1ea119fa, 0x3d4233dd, 0x2a1462b3)
        var c = 1
        for (v in values) {
            val top = c ushr 25
            c = ((c and 0x1ffffff) shl 5) xor v
            for (i in 0 until 5) if (((top ushr i) and 1) != 0) c = c xor gen[i]
        }
        return c
    }

    private fun hrpExpand(s: String): List<Int> =
        s.map { it.code ushr 5 } + listOf(0) + s.map { it.code and 31 }

    private fun checksum(hrp: String, data: List<Int>): List<Int> {
        val values = hrpExpand(hrp) + data + List(6) { 0 }
        val mod = polymod(values) xor 1
        return (0 until 6).map { (mod ushr (5 * (5 - it))) and 31 }
    }
}
