package org.forgesworn.charter.service

import android.os.SystemClock
import org.json.JSONObject

data class BatterySettings(
    val screenOffSecs: Long = 300,
    val ordinarySecs: Long = 60,
    val requestBoost: Boolean = true,
    val openingBoost: Boolean = true,
    val boostSecs: Long = 120,
) {
    /** The guardian's "Longer battery" preset exactly (and nothing near it). */
    val isLongerBattery: Boolean get() = this == LONGER_BATTERY
    val mode: String get() = when (this) {
        LONGER_BATTERY -> "Longer battery life"
        BatterySettings() -> "Balanced (recommended)"
        BatterySettings(120,30,true,true,180) -> "Faster responses"
        else -> "Custom"
    }
    companion object {
        val LONGER_BATTERY = BatterySettings(600, 120, true, false, 60)
        fun parse(o: JSONObject?): BatterySettings = BatterySettings(
            screenOffSecs = o?.optLong("screenOffSecs")?.takeIf { it in listOf(120L, 300L, 600L) } ?: 300,
            ordinarySecs = o?.optLong("ordinarySecs")?.takeIf { it in listOf(30L, 60L, 120L) } ?: 60,
            requestBoost = o?.optBoolean("requestBoost", true) ?: true,
            openingBoost = o?.optBoolean("openingBoost", true) ?: true,
            boostSecs = o?.optLong("boostSecs")?.takeIf { it in listOf(60L, 120L, 180L) } ?: 120,
        )
    }
}

/** Pure monotonic policy. Pending records never create or renew a deadline. */
internal class RelaySchedule {
    var settings = BatterySettings()
    private var openingUntil = 0L
    private var lastOpening = Long.MIN_VALUE
    private val requests = mutableMapOf<String, Long>()
    fun opened(now: Long): Boolean {
        // Debounce overlapping activities, configuration changes and lock redraws.
        if (lastOpening != Long.MIN_VALUE && now - lastOpening < 60_000) return false
        lastOpening = now
        openingUntil = now + 15_000
        return true
    }
    fun submitted(id: String, now: Long) {
        requests.entries.removeAll { now >= it.value }
        if (requests.size < 128) requests.putIfAbsent(id, now + settings.boostSecs * 1000)
    }
    fun reconcile(states: Map<String, String>) {
        requests.keys.removeAll { states[it] != null && states[it] !in setOf("pending", "submitted", "sent") }
    }
    fun waiting(now: Long) = settings.requestBoost && requests.values.any { now < it }
    fun interval(interactive: Boolean, now: Long): Long = when {
        !interactive -> settings.screenOffSecs * 1000
        waiting(now) || (settings.openingBoost && now < openingUntil) -> 5_000
        else -> settings.ordinarySecs * 1000
    }
}

/** All state is synchronised: UI, network and enforcement use separate workers. */
internal object RelayPollPace {
    const val USING_MS = 60_000L
    const val ASLEEP_MS = 300_000L
    private val policy = RelaySchedule()
    private val attention = RelayAttention()
    @Volatile var onChange: (() -> Unit)? = null
    /** Called (outside the lock) when the guardian's setting moves into or out
     *  of "Longer battery", so the liveness alarm can be re-timed at once. */
    @Volatile var onModeChange: (() -> Unit)? = null
    @Synchronized fun show(owner: Any) {
        val now = SystemClock.elapsedRealtime()
        val opening = !attention.visible()
        if (attention.show(owner, now)) policy.opened(now)
        if (opening) onChange?.invoke()
    }
    @Synchronized fun hide(owner: Any) { attention.hide(owner, SystemClock.elapsedRealtime()) }
    @Synchronized fun submitted(id: String) {
        policy.submitted(id, SystemClock.elapsedRealtime())
        onChange?.invoke()
    }
    @Synchronized fun reconcile(states: Map<String, String>) { policy.reconcile(states) }
    fun configure(settings: BatterySettings) {
        val flipped = synchronized(this) {
            val was = policy.settings.isLongerBattery
            policy.settings = settings
            was != settings.isLongerBattery
        }
        if (flipped) onModeChange?.invoke()
    }
    @Synchronized fun longerBattery(): Boolean = policy.settings.isLongerBattery
    @Synchronized fun interval(interactive: Boolean) = policy.interval(interactive, SystemClock.elapsedRealtime())
    @Synchronized fun waiting() = policy.waiting(SystemClock.elapsedRealtime())
    @Synchronized fun status(): String = "${policy.settings.mode}. Relay fallback: ${policy.settings.ordinarySecs}s in use / ${policy.settings.screenOffSecs}s screen off. " +
        if (policy.waiting(SystemClock.elapsedRealtime())) "Waiting: short fast-check window active." else "No active request fast-check window."
}

/** A continuous visible session is one opening, including overlapping activities. */
internal class RelayAttention {
    private val viewers = mutableSetOf<Any>()
    private var lastHidden: Long? = null
    fun visible() = viewers.isNotEmpty()
    fun show(owner: Any, now: Long): Boolean {
        val first = viewers.isEmpty()
        return viewers.add(owner) && first && (lastHidden?.let { now - it >= 30_000 } ?: true)
    }
    fun hide(owner: Any, now: Long) {
        if (viewers.remove(owner) && viewers.isEmpty()) lastHidden = now
    }
}
