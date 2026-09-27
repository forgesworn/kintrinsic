# Kintrinsic

**Libre, self-hosted digital wardship — a guardian grants scoped, revocable
screen-time, app, content and comms permissions to a child's devices, signed
with the family's own keys, enforced on-device, with no platform account in the
middle.**

Kintrinsic is a companion-parenting tool, not a surveillance one. The guardian
and ward see the same picture; every limit is transparent; the aim is to hand
autonomy back over time, not to build a cage. The design principle is that we
**do not design around circumvention** — where enforcement is incomplete, the
docs say so plainly.

> **Status: pre-1.0, alpha.** Kintrinsic is dogfooded daily on the
> maintainers' own family's devices, but it has not had independent security
> review and is still growing enforcement coverage — read
> [Known limitations](#known-limitations) before you rely on it. Android
> release builds are currently signed with a debug-derived key as a
> documented, temporary bridge (see
> [`android/keystore/README.md`](android/keystore/README.md)). Kintrinsic is a
> supervision aid, not a substitute for actually supervising your child, and
> it cannot stop a ward who has root/admin on their own device (see below).

> **A note on the name.** The product is **Kintrinsic**. The code, the daemon,
> and some binaries and identifiers keep the earlier working name **`charter`**
> (e.g. `charterd`, the Android application id) — a deliberate, permanent
> continuity choice, the same way Signal's package id is still
> `org.thoughtbot.securesms`-lineage. "A charter" is also the in-product word
> for the agreement a family writes. Product = Kintrinsic; the charter = the
> agreement; `charter*` in code = the engine that enforces it.

## What's here

A working system across three enforced platforms plus a guardian app and a wire
protocol — not a spec or an SDK alone.

| Component | What it is |
|---|---|
| **Ward APK** (`android/app`, `org.forgesworn.charter`) | An Android **Device Owner** enforcer for a child's phone (GrapheneOS and stock). Enforces schedules, budgets, per-app limits, install windows, web content and an always-available **Lifeline** emergency-calling path. Self-updates from a guardian-signed instruction. |
| **Linux warden** (`linux/`, `charterd`) | A Rust daemon that enforces the same charter on a Linux machine: per-child time, an on-display lock, Firefox-only web filtering (see [Known limitations](#known-limitations)), and honest foreground-app attribution. Ships as a `.deb`. |
| **Guardian app** (`apps/charter-app`, the carrier APK) | Where a guardian issues and amends clauses, approves "can I have longer?" asks, and pairs devices. Holds the family signing key locally; backs it up encrypted. |
| **Wire contract** (`spec/contract.md`) | The Nostr-based protocol: gift-wrapped (NIP-59) clauses, grants, status and usage-sync between guardian and device. No server holds family data. |
| **Consumer SDK** (`@forgesworn/charter` on npm) | Lets a Nostr-aware app honour a schedule clause directly (see below). |

Enforcement, pairing, clauses and — as of the decentralized-stack work — software
updates all travel over Nostr relays and Blossom blob servers. The only external
services the running product needs are relays and Blossom; the long-term goal is to
run none of our own.

## The lexicon

Kintrinsic is **wardship**, not "parental controls" — controls frame the
relationship as surveillance imposed on a subject; wardship is protective,
accountable, time-limited authority that grants scoped powers trending toward
*more* autonomy.

| Term | What it is |
|---|---|
| **Guardian** | The adult who holds the wardship and grants clauses. |
| **Ward** | The child the wardship protects. |
| **Charter** | The agreement a family writes — the set of clauses in force. |
| **Warden** | The on-device enforcer (`charterd` on Linux; the Device Owner service on Android). |
| **Clause / grant** | How a guardian acts — you *grant* clauses, you don't *set controls*. |

In one sentence: **a guardian holds wardship over a ward; the charter defines it;
the warden enforces it.**

## Consumer SDK (`@forgesworn/charter` on npm)

Any Nostr-aware app can honour a schedule clause without the rest of Kintrinsic:

```bash
npm install @forgesworn/charter nostr-tools
```

```typescript
import { evaluateSchedule, createCharterEvaluator } from "@forgesworn/charter";

const ev = createCharterEvaluator({ appKeypair: { pubkey: APP_PUB, privkey: APP_PRIV } });
await ev.init({
  charterAuthors: [guardianPubkey],
  charterRelays: ["wss://relay.example.com"],
  subjectPubkey: signedInChildPubkey,
});
const decision = ev.check(signedInChildPubkey);
if (!decision.allow) showLockoutScreen(decision.reason);
```

`evaluateSchedule` (clause payload + `Date` → `{allow, reason}`) is also exported
standalone for dashboards. See [`docs/integrating.md`](docs/integrating.md) for the
protocol semantics.

## Repository layout

- `spec/contract.md` — the wire protocol (the source of truth for message shapes).
- `core/` — the shared Rust verification/crypto/transport crates (used by Linux and, via JNI, Android).
- `linux/` — the `charterd` warden, tray, lock UI, and `.deb` packaging.
- `android/` — the ward enforcer (`app`) and guardian carrier (`carrier`) modules.
- `apps/charter-app/` — the guardian PWA bundled into the carrier APK.
- `src/` + `docs/` — the consumer SDK and its integration docs.

## Known limitations

Kintrinsic is alpha software. Some of these are architectural (true for any
on-device parental-control system); some are gaps we intend to close.

- **A ward with root/administrator access on their own device can defeat any
  on-device enforcement.** Kintrinsic enforces at the OS layer (Android Device
  Owner, or a privileged Linux daemon); it assumes the guardian, not the ward,
  controls that layer. This is a property of on-device enforcement generally,
  not a bug we can patch away.
- **Linux web-content filtering is partial.** The clause covers block/allow
  lists, SafeSearch and a YouTube restricted-mode setting, but on Linux this
  is currently enforced only inside Firefox (via browser policy); Chromium,
  Brave, Electron-based apps and other non-browser network clients are not
  yet filtered. The Android ward enforces web content at the network (VPN)
  layer instead and is not affected by this gap. Until the Linux applier is
  finished, treat "web filtering" on a Linux ward as Firefox-only.
- **Supported ward platforms are Android (Device Owner mode, GrapheneOS and
  stock) and Linux.** There is no ward enforcer for iOS, ChromeOS, Windows or
  macOS, and none is currently planned; a family with wards on those
  platforms cannot use Kintrinsic to enforce clauses there.
- **No independent third-party security audit has been done.** The project
  has had internal review, but nothing external and adversarial. Given that
  the guardian app holds a family signing key, treat key backup and device
  loss recovery with care until an audit exists.
- **Android release signing is on a temporary debug-derived key**, a
  documented bridge for already-paired field devices until the signing
  identity is rotated — see
  [`android/keystore/README.md`](android/keystore/README.md) for the
  mitigations in force meanwhile.

## Provenance

Kintrinsic has been in private development since spring 2026 as a family-safety
tool built and tested on the maintainers' own household devices. It is being opened
at launch rather than developed in public — the development history was kept private
precisely because a child-safety product built on a real family should not carry
that family's details into a public log. The consumer SDK's npm publish history and
the live deployment provide independent timestamps of the work.

## Part of the ForgeSworn Toolkit

[ForgeSworn](https://forgesworn.dev) builds open-source cryptographic identity,
payments, and coordination tools for Nostr.

| Library | What it does |
|---|---|
| signet-app | Identity verification + guardian–ward management |
| [dominion](https://github.com/forgesworn/dominion) | Epoch-based encrypted access control |
| [toll-booth](https://github.com/forgesworn/toll-booth) | L402 payment middleware |
| [nostr-attestations](https://github.com/forgesworn/nostr-attestations) | NIP-VA verifiable attestations |
| [canary-kit](https://github.com/forgesworn/canary-kit) | Coercion-resistant spoken verification |

## Security

See [SECURITY.md](SECURITY.md) for how to report a vulnerability (please use the
private advisory channel, not a public issue).

## Licence

[MIT](LICENSE)
