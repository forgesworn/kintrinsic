package org.forgesworn.charter.service

/** One serial network chain. Injected queue is the service's slow worker. */
internal class RelayRoundLoop(
    private val now: () -> Long,
    private val active: () -> Boolean,
    private val interactive: () -> Boolean,
    private val interval: () -> Long,
    private val post: (Runnable, Long) -> Unit,
    private val remove: (Runnable) -> Unit,
    private val work: () -> Unit,
) {
    private val refreshLock = Any()
    private var refreshQueued = false
    private var promptRequested = false
    private var completedAt = -1L
    val round: Runnable = object : Runnable {
        override fun run() {
            if (!active()) return
            try { work() } finally {
                completedAt = now()
                remove(this)
                if (active()) post(this, interval())
            }
        }
    }
    fun refresh(prompt: Boolean) {
        val requestedAt = now()
        synchronized(refreshLock) {
            promptRequested = promptRequested || prompt
            if (refreshQueued) return
            refreshQueued = true
        }
        post(Runnable {
            val wantsPrompt = synchronized(refreshLock) {
                refreshQueued = false
                val value = promptRequested
                promptRequested = false
                value
            }
            if (active()) {
                remove(round)
                post(round, if (wantsPrompt && interactive() && (completedAt < 0 || requestedAt - completedAt >= 5_000)) 0 else interval())
            }
        }, 0)
    }
    /** Caller rechecks the effective watchdog deadline on this same queue. */
    fun overdueRound() { remove(round); round.run() }
}
