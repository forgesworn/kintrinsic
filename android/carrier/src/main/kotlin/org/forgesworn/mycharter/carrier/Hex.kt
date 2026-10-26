package org.forgesworn.mycharter.carrier

/**
 * 64 lowercase-or-uppercase hex chars -> 32 bytes, or null. Deliberately no
 * String intermediate: the point is to hand the JNI something the caller can
 * WIPE (`fill(0)`) the moment the call returns (S12).
 */
fun hexToKeyBytes(hex: String): ByteArray? {
    if (hex.length != 64) return null
    val out = ByteArray(32)
    for (i in 0 until 32) {
        val hi = Character.digit(hex[i * 2], 16)
        val lo = Character.digit(hex[i * 2 + 1], 16)
        if (hi < 0 || lo < 0) {
            out.fill(0)
            return null
        }
        out[i] = ((hi shl 4) or lo).toByte()
    }
    return out
}
