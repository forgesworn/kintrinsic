# Android full-traffic TUN — design (web-content filtering that actually works)

**Date:** 2026-07-10
**Status:** approved direction (decented, 2026-07-10) — supersedes the split-tunnel DNS filter for phone web-content enforcement.
**Origin:** the 2026-07-10 hardware gate on the real Pixel proved the split-tunnel MVP can't filter on real networks. This is the robust, on-device, values-aligned replacement.

## Why (the two on-metal findings this fixes)

The split-tunnel DNS-filter VpnService (routes only the virtual DNS IPs) failed on-metal in two converging ways:

1. **Lockdown breaks all internet.** Always-on + `lockdownEnabled=true` + a tunnel that routes only DNS ⇒ Android's lockdown firewall drops every non-DNS packet (only the virtual DNS server is reachable; even `ping 8.8.8.8` blocked). Verified: `tun0` had only the DNS `/32` routes, no default. Worked around for now by dropping lockdown (fail-soft, commit `d0396de`), which restores internet but weakens fail-closed.
2. **Private DNS (DoT) bypasses the filter.** On a network with private DNS (the test Wi-Fi does DoT to `1.1.1.1`/`8.8.8.8`, `UsePrivateDns=true`), the system sends DNS-over-TLS **direct to the network resolver** — the virtual DNS server (`10.111.0.53`) is never in the path. A content clause (block youtube, SafeSearch) had **no effect** on-metal: youtube.com resolved normally, google was not rewritten. A Device Owner **cannot** force private DNS off (`setGlobalPrivateDns` supports only opportunistic/hostname).

Both point to the same root: **you must route ALL traffic through the tunnel** to (a) let lockdown coexist with a working internet and (b) capture the system's DNS regardless of the network's private-DNS setting. That is a full-traffic TUN — the thing the earlier spec deferred as a non-goal, now proven necessary.

## What stays (unchanged, already on-metal-proven)

- The wire contract (`content` clause, store_key 3), the shared `charter-content` evaluator, and the `charter-webpolicy` `DnsFilterPlan` renderer — all correct, keep as-is.
- The Kotlin `DnsResolver` (plan → Block/Rewrite/PassThrough, suffix-aware) and `DnsMessage` codec — reusable verbatim as the DNS-decision layer inside the new engine.
- Pairing, STATUS, the DO app, `charterDnsPlan()` JNI export, the `inert-until-paired` pin gate (commit `271f61b`), schedule/budget/app enforcement — all working on-metal.

## Architecture

A full-traffic `VpnService` whose TUN carries a **default route** (`0.0.0.0/0` + `::/0`), backed by a **Rust userspace TCP/IP engine** (via the existing JNI) that:

```
tun fd (all packets)
  → Rust packet engine (smoltcp-based)
      • UDP/TCP :53  → DnsResolver decision (shared plan): Block→NXDOMAIN, Rewrite→answer, Pass→relay to a real upstream via a protected socket
      • :853 (DoT)   → DROP the connection, forcing the system's opportunistic private DNS to fall back to plain :53 (which we then capture). DISALLOW_CONFIG_PRIVATE_DNS stops the ward re-enabling strict DoT; the DoH canary stays.
      • everything else (TCP + UDP, any dst) → transparently FORWARD to the underlying network via protected sockets (userspace NAT)
```

### Why Rust + smoltcp (not a Kotlin TCP stack, not a bundled C tun2socks)

- A userspace TCP stack in Kotlin is huge and fragile. A bundled C engine (badvpn/hev-socks5-tunnel) is opaque + a supply-chain surface.
- **`smoltcp`** is a mature, `no_std`-friendly Rust userspace TCP/IP stack. Charter already ships a Rust `.so` via JNI, so the packet engine lives in the shared Rust core — one language for decide + forward, testable on the host, values-aligned (auditable, no opaque blob).
- The DNS decision reuses the SAME `charter-content` + `charter-webpolicy` logic the engine can call directly in Rust (no JNI round-trip per query).

### The protected-socket boundary

Forwarded/upstream sockets must be `VpnService.protect()`ed so they egress the real network instead of looping into the TUN. Rust can't call `protect()` (an Android API). Design: **Kotlin owns socket creation + protect(), Rust owns the packet logic.** Two viable seams (decide during Task 1 spike):
- (a) Rust requests "protect this fd" via a JNI upcall; Kotlin calls `protect(fd)` and returns. smoltcp's socket set uses raw fds Kotlin protected.
- (b) Kotlin runs the raw fd relay for forwarded flows (a thin protected-socket pump), Rust does DNS + connection tracking. More Kotlin, simpler Rust.
Preference: (a) if the JNI upcall is clean; (b) as the fallback.

### VpnService config

- `addAddress(10.111.0.2/32, fd00:…::2/128)`; `addRoute(0.0.0.0/0)` + `addRoute(::/0)` (full tunnel); `addDnsServer(10.111.0.53, fd00:…::53)`; `addDisallowedApplication(self)` (Charter's relay traffic stays off the TUN).
- Fail posture: with full routing, **lockdown becomes safe** (all traffic has a tunnel path). Revisit the fail-soft-vs-lockdown choice once forwarding works — hard-fail-closed (lockdown=true) is now achievable and was decented's original preference. Default to lockdown ON once the forwarder is proven to pass traffic.

## Testing

- **Host (Rust):** feed the engine crafted IP packets (the `IpUdpDatagram` fixtures extend naturally) — assert DNS decisions, DoT-drop, and that a forwarded TCP/UDP flow round-trips through a stubbed protected socket. smoltcp is host-testable.
- **Instrumented / hardware:** the real gate. On the test Pixel, on a **DoT network**: pair → content clause → confirm youtube.com is NXDOMAIN, google is SafeSearch-rewritten, example.com passes, AND general internet (browser, apps, `ping 8.8.8.8`) works — the combination the split-tunnel could never achieve.

## Build shape (for the plan)

1. **Spike:** prove the Rust engine can take the tun fd over JNI, forward ONE TCP flow + ONE UDP flow through a protected socket, and pass a real page load on the emulator. This de-risks the protect() seam + smoltcp integration before building breadth. (Mirrors the original Android spike discipline.)
2. Full-route VpnService + fd handoff to Rust.
3. DNS interception in the engine (reuse DnsResolver/DnsMessage/plan) incl. DoT-drop.
4. UDP forwarding (flow table + timeouts).
5. TCP forwarding (smoltcp sockets ↔ protected sockets).
6. Fail-closed (restore lockdown), idempotent plan apply, lifecycle/shutdown.
7. Host tests + the on-metal DoT-network gate.

## Risks / open questions

- The `protect()` seam (a vs b) — settle in the spike.
- Battery/throughput of a userspace forwarder — measure on-metal; smoltcp is efficient but a full forwarder has cost. Acceptable for a managed ward device.
- DoH-over-443 to arbitrary resolvers (a determined browser) still isn't caught by DNS interception — same anti-casual-tier limit as before; documented, not solved here.
- IPv6 parity throughout.
- This is a **multi-session build**; the spike (step 1) is the gate that says "the approach is sound" before the breadth.
