package org.forgesworn.mycharter.carrier

/** Pure naming for the local link's DNS-SD advertisement (JVM-testable). */
object LinkNames {
    private val INSTANCE = Regex("^Kintrinsic [0-9a-f]{4}$")

    /** `Kintrinsic <4 hex>` from the first two of [random]. */
    fun instanceName(random: ByteArray): String {
        require(random.size >= 2) { "need two random bytes" }
        return "Kintrinsic %02x%02x".format(random[0].toInt() and 0xff, random[1].toInt() and 0xff)
    }

    fun isInstanceName(s: String): Boolean = INSTANCE.matches(s)
}
