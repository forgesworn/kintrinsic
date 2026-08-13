package org.forgesworn.charter.ui

/**
 * Ward-facing time-left copy (decented, 2026-07-23): hours + minutes reads
 * better than raw minutes, and under five minutes the seconds matter.
 * "2h 15m" / "37m" / "4m 32s" / "48s".
 */
object TimeText {
    fun timeLeft(totalSecs: Long): String {
        val s = totalSecs.coerceAtLeast(0)
        val h = s / 3600
        val m = (s % 3600) / 60
        val sec = s % 60
        return when {
            h > 0 -> "${h}h ${m}m"
            s >= 5 * 60 -> "${m}m"
            m > 0 -> "${m}m ${sec}s"
            else -> "${sec}s"
        }
    }
}
