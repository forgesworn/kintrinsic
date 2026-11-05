package org.forgesworn.mycharter.relay

/**
 * One relay socket: [relay], subscribed for gift-wraps to [pubkeyHex] alone
 * and answering NIP-42 AUTH with that key alone.
 *
 * One socket per (relay, key), not one per relay with an AUTH per key: a
 * relay that hands kind-1059 wraps only to the signed-in recipient commonly
 * holds ONE authenticated pubkey per connection (a second AUTH replaces the
 * first) and checks a REQ's `#p` against it, so a shared socket would lose
 * one key's wraps. A connection per key needs nothing beyond what every
 * NIP-42 relay already does. With a single key this is one socket per relay,
 * exactly as before.
 */
data class RelayRoute(val relay: String, val pubkeyHex: String)

object RelayRoutes {
    /** Every socket the carrier wants: each relay for each key, primary first. */
    fun wanted(relays: List<String>, pubkeysHex: List<String>): List<RelayRoute> =
        relays.distinct().flatMap { relay -> pubkeysHex.distinct().map { RelayRoute(relay, it) } }

    /** The open sockets to close: a relay or key no longer provisioned. */
    fun stale(open: Collection<RelayRoute>, relays: List<String>, pubkeysHex: List<String>): List<RelayRoute> {
        val keep = wanted(relays, pubkeysHex).toSet()
        return open.filter { it !in keep }
    }
}
