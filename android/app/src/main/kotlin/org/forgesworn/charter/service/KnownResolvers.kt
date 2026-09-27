package org.forgesworn.charter.service

/**
 * Well-known public resolvers the DNS filter routes into its own tunnel
 * (05-G4).
 *
 * The tunnel used to carry only its own virtual resolver, so an app that sent
 * plain DNS straight to `8.8.8.8:53` or `1.1.1.1:53` went round the filter
 * with no cleverness needed. Routing these addresses into the tunnel closes
 * that for the resolvers people actually use: their UDP/53 is answered by the
 * filter under the same plan (the reply comes back from the address the app
 * asked), and every other packet to them, DoH on 443, DoT on 853 and DNS over
 * TCP, is dropped, so the app falls back to the system resolver, which is the
 * filter. The DoH host names themselves are refused by
 * [org.forgesworn.charter.enforce.dns.DnsResolver].
 *
 * This is a list, not a full-traffic tunnel: a resolver that is not on it,
 * reached directly by an app, is still not seen. Closing that needs the
 * deferred full-traffic forwarder.
 *
 * The filter's own upstream lookups use protected sockets, which bypass the
 * tunnel, so routing its upstream ([CharterVpnService.UPSTREAM]) here is safe.
 */
object KnownResolvers {
    data class Route(val address: String, val prefix: Int)

    private val v4 = listOf(
        // Google
        "8.8.8.8", "8.8.4.4",
        // Cloudflare (plain, malware, family)
        "1.1.1.1", "1.0.0.1", "1.1.1.2", "1.0.0.2", "1.1.1.3", "1.0.0.3",
        // Quad9
        "9.9.9.9", "149.112.112.112", "9.9.9.10", "149.112.112.10", "9.9.9.11", "149.112.112.11",
        // OpenDNS
        "208.67.222.222", "208.67.220.220", "208.67.222.123", "208.67.220.123",
        // AdGuard
        "94.140.14.14", "94.140.15.15", "94.140.14.15", "94.140.15.16", "94.140.14.140", "94.140.14.141",
        // CleanBrowsing
        "185.228.168.9", "185.228.169.9", "185.228.168.168", "185.228.169.168",
        // Control D
        "76.76.2.0", "76.76.10.0",
        // Mullvad
        "194.242.2.2",
        // Verisign, Comodo, Level3
        "64.6.64.6", "64.6.65.6", "8.26.56.26", "8.20.247.20", "4.2.2.1", "4.2.2.2",
    ).map { Route(it, 32) }

    private val v6 = listOf(
        "2001:4860:4860::8888", "2001:4860:4860::8844",
        "2606:4700:4700::1111", "2606:4700:4700::1001",
        "2606:4700:4700::1112", "2606:4700:4700::1002",
        "2606:4700:4700::1113", "2606:4700:4700::1003",
        "2620:fe::fe", "2620:fe::9", "2620:fe::10", "2620:fe::11",
        "2620:119:35::35", "2620:119:53::53",
        "2a10:50c0::ad1:ff", "2a10:50c0::ad2:ff",
        "2a0d:2a00:1::", "2a0d:2a00:2::",
        "2606:1a40::", "2606:1a40:1::",
        "2a07:e340::2",
    ).map { Route(it, 128) }

    /** NextDNS answers on whole anycast blocks, per profile. */
    private val blocks = listOf(Route("45.90.28.0", 24), Route("45.90.30.0", 24))

    val routes: List<Route> = v4 + v6 + blocks
}
