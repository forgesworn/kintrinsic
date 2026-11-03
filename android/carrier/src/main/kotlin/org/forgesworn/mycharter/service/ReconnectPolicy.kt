package org.forgesworn.mycharter.service

/** Keep sockets and four-minute pings in every mode; only outage retry cost changes. */
internal object ReconnectPolicy {
    fun cap(seconds: Long): Long = seconds.takeIf { it in listOf(60L, 300L, 600L) } ?: 300L
    fun next(previous: Long, livedMs: Long, capSecs: Long): Long =
        ((if (livedMs >= 60_000) 0 else previous.coerceIn(0, 600)) * 2).coerceIn(1, cap(capSecs))
    fun shouldConnect(hasSocket: Boolean, queuedRetry: Boolean) = !hasSocket && !queuedRetry
}
