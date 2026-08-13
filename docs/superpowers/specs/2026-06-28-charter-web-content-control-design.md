# Charter — Web Content Control (the `content` clause, Linux enforcement)

**Status:** draft design (pre-implementation)
**Date:** 2026-06-28
**Author:** decented
**Scope:** how Charter gates a managed child's *web* activity on a Linux Mint
machine — the device-side enforcement arm of the reserved `content` clause —
built by orchestrating commodity open-source filters under `charterd`, with
**parent-overridable, signed, curator-led** allow/deny lists distributed over
Nostr.

> Companion to: `spec/contract.md` (the `content` clause is reserved there:
> "Rating cap + category filters"), `linux/README.md` (the `charterd` warden
> core), and `README.md` (Charter product positioning).

---

## 1. Problem & framing

`charterd` already turns a stock Mint box into a *cooperatively* locked-down
machine: it brokers installs and execs (exec-allowlist by hash via fapolicyd /
trust-DB), owns the managed user account, enforces schedule + budget, and can
freeze sessions. What it does **not** yet gate is **web activity inside the one
browser the child is allowed to run.**

Two facts shaped this design (full landscape research in
`internal/research/`, summarised here):

1. **There is no kid-specific desktop-Linux browser worth adopting.** KidZui,
   Pikluk, Chameleon/ZAC, DoudouLinux, Qimo are all abandoned. GNOME's own
   `malcontent-webd` web-filter backend (late-2025) is backend-only, no UI,
   blocklist-only. **We do not build or fork a browser.**
2. **Every commodity web-filtering control is trivially bypassable on its own —
   except that `charterd` is already the thing that makes them unbypassable.**
   A root broker that controls which executables run, owns the user account, and
   can make config files root-owned + immutable is exactly what turns ordinary
   Firefox policy + DNS filtering into a robust gate.

So the product is **not** a new filter engine. It is the **integrator and
enforcer**: `charterd` orchestrates existing OSS filters, exposes them as the
`content` clause, wires them to the existing signed-event + audit rails, and
adds the genuine differentiator — **signed, curator-led, parent-authoritative
allow/deny lists** carried over Nostr.

### Non-goals (YAGNI — explicitly out of v1)

- ❌ **MITM page-content proxy (e2guardian).** The only thing that can block one
  bad video on an *allowed* domain, but ECH + QUIC + HSTS make it leaky and
  high-maintenance in 2026. Deferred to an opt-in "deep filtering" toggle later.
- ❌ **Automated trust-graph / reputation scoring** (PageRank/EigenTrust/
  GrapeRank/NIP-85/Vertex). v1 = the parent explicitly picks curators. If we
  ever want computed trust, we *consume* a Nostr trust DVM — we never author one.
- ❌ **Subjective per-page rating** (the PICS / POWDER / ICRA graveyard).
  Domain-level labels only.
- ❌ Shipping the child's browsing stream off-device (the "Web of Trust"
  extension privacy scandal). All per-URL data stays local.

---

## 2. Decisions locked in (from brainstorming)

| Decision | Choice | Why |
|---|---|---|
| **Posture** | **Age-tiered: both** allowlist (young) and category-blocklist (older), selected per child via the `content` clause | One stack serves both; parent picks per child |
| **Lists** | **Curator-led in v1**, with **parent override + additions always authoritative** | The tractable differentiator; parent is final authority |
| **List → enforcement** | **Materialize-to-config**, not live-broker | Fail-closed, offline-capable, browsing stream never leaves device |
| **Sanctioned browser** | **Firefox ESR** | Machine `policies.json` is profile-independent + highest-precedence; full uBO "hard mode" survives (Chromium MV3 killed it); ships on Mint; privacy-aligned. No native SafeSearch policy — handled at DNS anyway |
| **v1 enforcement depth** | **exec-lock (have it) + DNS (AdGuard Home) + Firefox machine policy** | Robust, all-OSS, single-laptop, no MITM |

---

## 3. Architecture overview

Three independent locked layers, with `charterd`'s exec-allowlist as the
linchpin underneath:

```
 content clause (signed by parent, synced via existing rail)
        │
        ▼
 curator label/list ingest  ──►  content evaluator (PURE)
 (subscribed curator pubkeys)     (clause + cached lists + clock
                                   → EffectiveWebPolicy)
        │                                   │
        │ materialize-to-config             │
        ▼                                   ▼
 ┌─────────────────────┐   ┌─────────────────────┐   ┌──────────────────────┐
 │ web_policy enactor  │   │ dns_filter enactor  │   │ exec-lock (REUSE)    │
 │ → Firefox           │   │ → AdGuard Home      │   │ → only the approved  │
 │   policies.json     │   │   + blocklists      │   │   Firefox may run;   │
 │   (allowlist /      │   │   + forced          │   │   block every other  │
 │   blocklist, lock   │   │   SafeSearch;       │   │   browser / VPN /    │
 │   DoH/ECH/QUIC,     │   │   pin resolv.conf;  │   │   DoH client /       │
 │   devtools, Tor)    │   │   firewall :53/:853 │   │   AppImage / portable│
 │                     │   │   /UDP-443 + DoH IPs│   │                      │
 └─────────────────────┘   └─────────────────────┘   └──────────────────────┘
  HOW the browser behaves     WHERE it can connect       WHAT can run
```

**No in-browser layer is self-sufficient** — the exec-lock is what makes the
other two trustworthy. A child who can run an unmanaged AppImage browser or a
DoH/VPN client defeats both policy and DNS; `charterd` already prevents that.

---

## 4. Components

Naming follows the repo's `charter-*` crate convention and the existing
`charter-sys` port + `mock`/`real` discipline.

### 4.1 `content` clause (new type in `charter-proto`)

Device-enforced, exactly like `budget`: the bunker validates + stores + syncs
it; `charterd` caches the signed clause and enforces it on-device. It is **not**
a bunker sign-time clause (it gates the machine's browser, not Signet signs).

```ts
interface GrantContent {
  v: 1;
  /** IANA tz — reserved for future time-based content rules; unused in v1. */
  tz?: string;
  /** Enforcement posture for this child. */
  posture: 'allowlist' | 'blocklist';
  /** Coarse, parent-set policy selector. NOT derived from DOB and never a
   *  birthDate — a policy knob only (see Privacy). */
  ageTier: 'young' | 'older';
  /** Curator pubkeys (64-hex) the parent subscribes to. Their signed lists
   *  feed the effective decision. May be empty (parent-only curation). */
  curators: string[];
  /** Min independent curators required to auto-admit a domain into an
   *  allowlist. Default 2. Ignored in blocklist posture. */
  quorumN?: number;
  /** Category keys to block in blocklist posture, e.g. 'porn','gambling'. */
  blockCategories?: string[];
  /** Force SafeSearch across engines (default true). */
  safeSearch?: boolean;
  /** YouTube restricted mode. Default 'moderate' for young, 'off' otherwise. */
  youtubeRestrict?: 'off' | 'moderate' | 'strict';
  /** Parent overrides — ALWAYS authoritative. Domains or URL prefixes. */
  parentAllow?: string[];
  parentDeny?: string[];
  /** Block all web (lock the browser). Distinct from absent. */
  paused?: boolean;
  /** Soft tombstone — parent revoked content control. */
  revoked?: boolean;
  /** Unix seconds — LWW + rollback protection (monotonic per kind). */
  issuedAt: number;
}
```

Carried in the existing `CLAUSE (31113)` envelope by adding `'content'` to the
`ClausePayload.kind` union (`'schedule' | 'budget' | 'content'`). Set via the
already-reserved NIP-46 methods `charter_set_content` /
`charter_set_default_content` `(dependantId, [...scope keys], GrantContent|null)`.

**Scope composition** follows the contract's existing rule: per-origin and
dep-default `content` clauses compose by **intersection** — a per-origin clause
can never widen the dep-default envelope (e.g. a per-origin allow can't unblock
a dep-default `parentDeny`).

### 4.2 Curator lists on Nostr (signed, replaceable)

Two Nostr primitives, single-sourced as kind constants in `charter-primitives`
(numbers are **[decide]**, see §10):

- **Curator web list** — an **addressable** event (NIP-51 set semantics), one
  per `(curator, listId)`, keyed by `d`. Carries signed `r`-tagged domain
  entries each with a rating/category tag (`kid-safe`, `block`,
  `category:<key>`), namespaced (`app.charter.web`). Replaceable, so a re-sync or
  revocation is just the latest event — no append-only poisoning surface.
- **Per-site rating / flag** — a **NIP-32 label** (kind `1985`): `L =
  app.charter.web`, `l = kid-safe|block|category:<key>`, `r = <url/domain>`.
  Used for the lightweight "flag this site" + third-party rating flow, and (via
  **NIP-56** kind `1984` reports) for parent-visible flags. Advisory, never
  auto-acting.

A parent's **subscribed curators** are themselves a NIP-51 set on the parent's
identity; the `content` clause's `curators[]` is the materialized subscription.

### 4.3 Content evaluator (new pure crate `charter-content`)

Mirrors `charter-schedule`'s pure, headless-testable, zero-privilege evaluator
with golden vectors.

```
evaluateContent(clause: GrantContent,
                lists: CuratorList[],     // cached, signature-verified
                now: Instant)
  -> EffectiveWebPolicy {
       posture,
       allowDomains: Set<Domain>,   // allowlist posture
       blockDomains: Set<Domain>,   // explicit blocks
       blockCategories: Set<Key>,   // blocklist posture
       allowExceptions: Set<Domain>,// parentAllow, surfaced so the DNS layer can
                                    // override CATEGORY blocks (categories expand
                                    // to domains only at that layer)
       safeSearch: bool,
       youtubeRestrict: Mode,
       locked: bool,                // paused / fail-closed
     }
```

**Resolution order (parent always wins):**

1. If `paused` → `locked = true` (browser blocked entirely). If `revoked` → no
   content constraint.
2. **allowlist posture:**
   `allowDomains = { d : ≥quorumN subscribed curators rate d kid-safe } ∪ parentAllow − parentDeny`
3. **blocklist posture:**
   `blockDomains = (curator-blocked domains ∪ category-expansion(blockCategories)) − parentAllow ∪ parentDeny`
4. Apply `safeSearch` / `youtubeRestrict`.

`parentDeny` ⊇-beats `parentAllow` ⊇-beats curators, in both postures. The
quorum applies **only to the dangerous direction** (admitting a domain into a
young child's allowlist), so one compromised curator cannot inject an adult
site; a single curator `block` is sufficient (fail-safe), and the parent can
allow-override a specific over-block.

### 4.4 Enactors (new, in `charterd`) + new `charter-sys` ports

Same trait + `mock`/`real` pattern as `flatpak`, `exec.allow`, etc. Each takes
the `EffectiveWebPolicy` and writes it down.

- **`web_policy` enactor** → `WebPolicyOps` port. Writes root-owned, `0644`,
  immutable Firefox `policies.json` at `/etc/firefox/policies/`:
  - allowlist: `WebsiteFilter { Block: ["<all_urls>"], Exceptions: allowDomains }`.
  - blocklist: `WebsiteFilter { Block: blockDomains }` (categories handled at DNS).
  - always: force-install + lock the safety extension; `BlockAboutConfig`,
    `DisablePrivateBrowsing`, `DisableDeveloperTools`, `DisableSafeMode`,
    `Incognito/guest off`, locked homepage/search, `DNSOverHTTPS {Enabled:false,
    Locked:true}`, `DisableEncryptedClientHello`, `network.http.http3.enabled`
    locked false, and (Brave, if ever used) `TorDisabled`.
  - verifies the file is root-owned + immutable after write; re-asserts on drift.
- **`dns_filter` enactor** → `DnsFilterOps` port. Configures **AdGuard Home**
  (GPL-3.0) bound to `127.0.0.1:53`:
  - blocklist posture: enable **local** HaGeZi/StevenBlack blocklists + **local**
    SafeSearch DNS-rewrites + YouTube-restrict CNAMEs — all on-device, no external
    calls. AGH's built-in *Parental Control* / *Safe Browsing* category toggles
    query AdGuard's cloud (privacy-preserving SHA-256 hash-*prefix* lookups, not
    full hostnames) and are therefore **optional, off by default** — local lists
    are the privacy-pure path.
  - allowlist posture: AGH default-deny + allow only `allowDomains` (belt &
    suspenders with the browser policy).
  - pins `/etc/resolv.conf` → `127.0.0.1`, `chattr +i`; firewalls outbound
    `:53` (DNAT→localhost), `:853` (drop), **UDP 443** (drop, kills QUIC bypass),
    and a curated DoH-IP blocklist.
- **exec-lock (REUSE, no new code path):** the approved-exec store allowlists
  only the sanctioned Firefox binary and **constrains its launch arguments**
  (strip `--disable-extensions`, `--user-data-dir`, etc.); every other browser /
  VPN / DoH client / portable / AppImage / interpreter-as-server is denied by the
  existing hash-not-path gate.

**Fail-closed contract:** if curator lists can't be fetched, the evaluator uses
the **last-known-good cached** `EffectiveWebPolicy` and never widens an
allowlist on failure. If an enactor cannot assert policy (write/immutability
fails), it **locks the browser** (treat as `paused`) rather than leave it open.

### 4.5 Parent app surface (`apps/charter-app`)

- Per-child: choose `posture` + `ageTier`; `paused` toggle.
- **Curator discovery + subscribe** (the v1 headline): browse curators, see what
  a list contains and *why* a domain is rated (which curators signed it),
  subscribe/unsubscribe, set `quorumN`.
- **Parent override** UI: add/remove `parentAllow` / `parentDeny`; these visibly
  outrank curators.
- **Be a curator (minimal):** publish a signed curator list (a parent curating
  for their own family bootstraps the ecosystem; sharing it makes them a
  curator). Full curator-authoring UX is **[decide]** for scope (§10).
- **On-device block view (optional):** AdGuard Home's local query log can power
  a "what got blocked" view **on the machine only** — never gift-wrapped
  off-device (privacy, §6).

---

### 4.6 Packaging & distribution

Charter ships as a **lean `.deb` (served from an APT repo for auto-updates)**
containing only Charter's own code — `charterd`, the `charter` CLI, the GUI, the
systemd unit, the `web_policy` / `dns_filter` enactors + policy templates, and the
`charter-setup` first-run. It does **NOT** bundle Firefox or AdGuard Home:

- **Firefox ESR** is a declared apt dependency — pulled from the distro / Mozilla
  APT repo, so Mozilla owns its (weekly) security updates. Charter never ships the
  binary, which avoids the trademark issue *and* avoids becoming the slow
  middleman for browser CVEs. On Mint, Firefox is already present; standardize on
  the ESR package for policy stability.
- **AdGuard Home** isn't in distro repos, so `charter-setup` **fetches it from
  upstream** at setup time via the existing brokered-install pattern — no GPL
  redistribution burden (Charter points at upstream, it is not the distributor).

Setup is admin-run and explicit (matching the existing admin-only first-run
pairing): `charter-setup` ensures Firefox ESR + AGH are present, writes the
root-owned `policies.json` + AGH config, pins DNS (immutable `resolv.conf` +
firewall :53/:853/UDP-443), exec-allowlists the one sanctioned Firefox binary, and
pairs with the parent's Signet. This **extends the existing machine-setup flow**,
not a new distribution mechanism.

**Mental model:** Charter is the *controller and config-writer*, not a software
bundle; Firefox and AdGuard Home stay owned and patched by their upstreams.

## 5. Enforcement mapping summary

| Posture | Browser policy layer | DNS layer | Exec layer |
|---|---|---|---|
| **allowlist (young)** | `WebsiteFilter` allow-only `allowDomains` | AGH default-deny + allow `allowDomains`; SafeSearch on; YT moderate | only approved Firefox runs |
| **blocklist (older)** | `WebsiteFilter` block `blockDomains`; all hardening locks | AGH category blocklists + SafeSearch + YT restrict | only approved Firefox runs |

---

## 6. Trust, safety & privacy model

- **Parent is final authority, always.** Curators are advisory inputs;
  `parentAllow`/`parentDeny` override them either way.
- **Quorum for allowlist admission** (default 2 independent curators) blocks a
  single compromised curator from injecting an adult site; parent override is the
  escape hatch in both directions.
- **Signed, reviewable sync.** Curator updates are signed replaceable events,
  applied on a short delay with the diff logged — never silent live mutation.
  Revocation = the curator's latest replacement event.
- **Privacy by construction.** Evaluation + override are 100% local; the child's
  browsing stream never leaves the device. Curators publish lists *blind* — they
  never learn who subscribes or what any child visits. This is the deliberate
  counter-design to the "Web of Trust" extension scandal. DNS filtering uses only
  local blocklists + local SafeSearch rewrites; AdGuard Home's cloud category
  toggles stay **off**, so no hostnames — not even hashed prefixes — leave the
  device by default.
- **No ambient child PII.** `ageTier` is a coarse, parent-set policy selector,
  **not** derived from a date of birth and never written to systemd's
  world-readable `birthDate` (consistent with the existing privacy guard). Audit
  events keep the contract's "`content` is always empty" rule — per-URL visits
  never appear in any off-device log.

---

## 7. Licensing & open-source alignment

Charter is MIT. This design keeps that clean while leaning entirely on
open-source filters and an open protocol. Two things carry it:

**Every dependency is OSS or an open protocol — no proprietary or account-gated
service is required.**

| Piece | License / openness |
|---|---|
| Firefox ESR + enterprise `policies.json` | MPL-2.0 |
| AdGuard Home (DNS engine) | GPL-3.0, self-hosted, **no cloud account** |
| Blocklists (StevenBlack / HaGeZi) | MIT / GPL-3.0 |
| uBlock Origin (force-installed safety ext) | GPL-3.0 |
| fapolicyd / trust-DB (exec-lock, existing) | GPL |
| e2guardian (deferred deep layer) | GPL-2.0 |
| Curator lists + ratings | open data over Nostr (NIP-32/51/56), signed + portable |

The proprietary options surfaced in research — NextDNS, Cloudflare for Families,
OpenDNS — were **deliberately not chosen**; they would violate "on your own keys,
no platform required." Self-hosted AdGuard Home is the libre substitute.

**`charterd` stays MIT despite orchestrating GPL tools — because it orchestrates
them at arm's length, it does not link them.** charterd writes a config file
(Firefox), configures and talks to a separate daemon (AdGuard Home), and invokes
separate programs (fapolicyd) — all distinct processes communicating over
files / sockets / CLI. Under the GPL "mere aggregation" rule, separate programs
at arm's length are separate works, so no copyleft propagates into charterd's own
source (the same reason a package manager or systemd can be any license while
managing GPL programs).

**Provisioning, not bundling.** To avoid even redistribution obligations,
`charterd` should **fetch/install** AdGuard Home (and any GPL component) from
upstream via the existing brokered install path rather than re-bundle binaries —
Charter points at upstream, it is not the distributor (open question §10.3).

**No Firefox trademark issue:** we drop a system policy file next to a stock,
unmodified Firefox — never repackaged or rebranded.

**Swappable by design — and not locked to one corporate steward.** AdGuard Home
is GPL-3.0 and runs fully locally with no account, but it is stewarded by a
single company (AdGuard Software Ltd) some communities are wary of, and its cloud
category toggles (§4.4) are kept off for that reason. Because the DNS engine sits
behind the `DnsFilterOps` port, AGH (the capability-rich default) can be swapped
without touching charterd for a **fully vendor-neutral** stack — **CoreDNS**
(Apache-2.0, CNCF-governed), **Blocky** (Apache-2.0), or **unbound + RPZ/lists**
(BSD) — or **Pi-hole** (EUPL-1.2). The trade-off is losing AGH's UI + built-in
SafeSearch/category convenience for zero company dependency. Blocklist sources
stay configurable. Default engine is open question §10.

> Honesty flag: a few popular blocklists (e.g. OISD) have ambiguous licensing;
> default to unambiguous sources (StevenBlack MIT, HaGeZi GPL-3.0) and keep the
> source list user-configurable.

## 8. Audit

Reuse the existing device audit (kind `31000`, machine-authored,
gift-wrapped-to-guardian, `content` always empty). Audit events describe
**policy enactment**, not per-URL browsing:

- `enacted` — a new/updated `content` policy was materialized (tags carry
  posture + list-version fingerprints, no URLs).
- `failed` / `locked` — enactor could not assert policy → browser locked.

Per-URL block statistics, if surfaced at all, stay **on-device** in AdGuard
Home's local log — never wrapped off-device.

---

## 9. Build sequence (feeds the implementation plan)

1. `charter-proto`: `GrantContent` type + validation invariants + golden
   vectors; extend `ClausePayload.kind` union.
2. `charter-content`: pure evaluator + golden vectors (allowlist, blocklist,
   quorum, parent-override precedence, fail-closed).
3. `charter-primitives`: single-source the new kind constants for curator lists.
4. `charter-transport`: subscribe to subscribed curators' lists/labels; verify
   signatures; cache; feed evaluator.
5. `charter-sys`: `WebPolicyOps` + `DnsFilterOps` ports (`mock` + compile-only
   `real`).
6. `charterd`: `web_policy` + `dns_filter` enactors + materialize loop +
   fail-closed locking; wire exec-lock arg-constraints for the browser.
7. `apps/charter-app`: posture/ageTier, curator subscribe + override UI, minimal
   curator-publish.
8. (Reserved) opt-in e2guardian "deep filtering"; consume-only trust scoring.

Each step compiles + unit-tests headless under mocks with zero privilege, per
the workspace's existing discipline.

---

## 10. Open questions / [decide]

1. **Curator-list kind numbers** — exact addressable kind(s) for curator web
   lists, single-sourced in `charter-primitives` (and whether to use a custom
   addressable kind vs. overloading NIP-51 bookmark sets).
2. **Allowlist > 1000 entries** — Firefox `WebsiteFilter` caps each array at
   1000. For large young-child allowlists, push overflow to the DNS allow layer,
   or split — pick one.
3. **AdGuard Home provisioning — RESOLVED (see §4.6):** `charterd` does NOT bundle
   AGH; `charter-setup` fetches it from upstream via the brokered-install pattern,
   and Firefox ESR is a declared apt dependency. Charter ships as a lean `.deb`
   controller, not a software bundle.
4. **Curator-authoring surface** — minimal publish inside `charter-app` vs. a
   separate "Charter Curator" tool/CLI. v1 needs *some* path since curators lead.
5. **Category vocabulary** — the canonical `blockCategories` key set and how it
   maps onto HaGeZi/OISD list segments + AGH parental categories.
6. **Per-origin vs dep-default for `content`** — confirm intersection semantics
   make sense for web (likely dep-default-only in v1; per-origin is more relevant
   to games).
7. **Default DNS engine** — ship **AdGuard Home** (capability-rich UI + built-in
   SafeSearch/category, GPL-3.0, but a single corporate steward) as the default,
   or a **vendor-neutral** engine (CoreDNS / Blocky / unbound) as default with AGH
   optional? The `DnsFilterOps` port makes either viable; this is a values + UX
   call, not a technical blocker.

---

## 11. References

- `spec/contract.md` — clause model, device-enforced clause pattern (`budget`),
  CLAUSE envelope, audit contract, privacy rules.
- `linux/README.md` — `charterd` warden core, exec-allowlist, port + mock/real
  discipline, no-on-device-DOB guard.
- `internal/research/` — full web-filtering landscape (browser policy, DNS,
  proxies, TLS/QUIC reality, decentralized/web-of-trust precedents) and sources.
- NIP-32 (labeling), NIP-51 (lists/sets), NIP-56 (reporting), NIP-59 (gift
  wrap) — the Nostr primitives carrying signed lists + flags.
- AdGuard Home (GPL-3.0), Firefox enterprise policies, HaGeZi/OISD blocklists.
