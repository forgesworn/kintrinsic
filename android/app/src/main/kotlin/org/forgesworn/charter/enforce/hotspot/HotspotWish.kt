package org.forgesworn.charter.enforce.hotspot

/**
 * The WARD's own switch for the guest hotspot.
 *
 * A `filtered` tethering clause means "you MAY run a guest hotspot", not "run
 * one continuously". Held up on nothing but a clause, the AP stays up for as
 * long as the grant does — the Wi-Fi chip parked in AP mode, a foreground
 * service pinned out of Doze, all day, whether or not a single guest ever
 * joined. That is a flat battery for a capability nobody is using (decented,
 * 2026-07-26, watching it burn on Rob's phone).
 *
 * So the posture is a permission and this is the switch, the same shape as a
 * time bucket: the guardian grants the capability, the ward spends it when they
 * actually need it.
 *
 * Deliberately in-memory and default-OFF:
 *  - a reboot, a crash, or a process restart lands on OFF, never on a hotspot
 *    nobody asked for (the same instinct as Android's own hotspot, which does
 *    not come back after a reboot);
 *  - no storage means nothing to migrate, and nothing that can disagree with
 *    the live AP it describes.
 * MainActivity and WardenController share one process, so a plain static is the
 * whole mechanism.
 */
object HotspotWish {

    @Volatile private var wanted = false

    /** Why it last went off, when that wasn't the ward's own doing — so the app
     *  can say "nobody used it for 15 minutes" instead of just showing "Off". */
    @Volatile var switchedOffReason: String? = null
        private set

    val on: Boolean get() = wanted

    fun turnOn() {
        wanted = true
        switchedOffReason = null
    }

    /** [reason] is ward-facing copy; null when the ward switched it off. */
    fun turnOff(reason: String? = null) {
        wanted = false
        switchedOffReason = reason
    }

    /**
     * The clause no longer allows a filtered hotspot. Forget the wish entirely:
     * a wish that outlived its grant would bring an AP straight back up the
     * moment a guardian re-granted, which the ward never asked for and would
     * not be watching for.
     */
    fun reset() {
        wanted = false
        switchedOffReason = null
    }
}

/**
 * When to switch a live-but-unused guest AP off by itself. Split out as a pure
 * function so the policy is unit-testable without a radio.
 *
 * Mirrors what Android's own tethered hotspot does ("turn off hotspot
 * automatically" when no devices have connected) — a local-only AP gets no such
 * courtesy from the platform, so Kintrinsic has to supply it.
 */
object HotspotIdle {

    /** How long an AP may sit with nobody on it before it switches itself off. */
    const val GRACE_MS = 15 * 60 * 1000L

    /** Ward-facing explanation, kept next to the policy it describes. */
    const val REASON = "nobody used it for 15 minutes"

    /**
     * @param liveSocketCount open guest sockets right now (client + upstream
     *   halves both count; any non-zero value means somebody is on it).
     * @param idleMs since the last guest connection was accepted — or since
     *   bring-up, so an AP nobody ever joined ages out too.
     */
    fun shouldSwitchOff(liveSocketCount: Int, idleMs: Long): Boolean =
        liveSocketCount == 0 && idleMs >= GRACE_MS
}
