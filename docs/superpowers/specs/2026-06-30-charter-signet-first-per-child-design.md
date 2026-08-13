# Charter — Signet-first per-child enforcement + the device↔Signet contract

**Date:** 2026-06-30
**Branch:** `feat/charter-linux-warden-v1`
**Status:** design approved (brainstorm), ready for implementation plan
**Scope owner:** Charter for Linux (the device side). Signet is a separate,
user-excluded repo — this design writes the *contract* Signet must implement and
the *device code* that consumes its inbound half. It does **not** build Signet.

## Why

Today `charterd`'s `runtime.rs` runs the device-only `MultiChildEnforcer` as the
**primary** enforcement path; a paired guardian's `schedule`/`budget` clauses
(single-child, stored under one `ClauseStore` key) are effectively **dead** in the
multi-child loop — `multi.tick()` reads per-child `DeviceLimits` files and never
consults the guardian clause cache. The product decision (memory
`signet-first-architecture`) is the inverse: **remote (Signet + the MyCharter PWA
over the relay) is the primary parent path; device-only/local admin is the
backup.**

This work flips that precedence and, in the same pass, freezes the cross-repo
wire the Signet team needs — including the forward path to **multiple devices per
child** with **online usage consolidation**.

## Goals

1. **Multiple children on one device** (already partly built) — finished here:
   per-child guardian clauses become authoritative, device-only is the fallback.
2. **Multiple devices per child** (future, cross-repo) — the *contract* is fully
   specified now so Signet can build it later with no wire break; **no device
   code** for consolidation this session.

Both within the project's **anti-casual, fail-safe-offline** model.

## Non-goals (this session)

- Building any of Signet (the guardian/aggregator backend) — separate repo,
  excluded.
- Emitting the STATUS feed or consuming `USAGE_SYNC` in device code — both need
  Signet to exist; contract-only here.
- Re-opening frozen wire that golden vectors already pin (the additions are
  backward-compatible).

## Resolved design decisions (brainstorm)

| Fork | Decision |
|---|---|
| How a guardian clause names a child across device↔Signet | **Per-child subject pubkey** (== Signet `dependantId`). `ClausePayload` gains an optional `subject: Hex32`. The device holds an admin-set subject→local-username map. Opaque (no PII), Nostr-native, reuses the existing `RequestPayload.subject`. |
| Precedence when the guardian set *some* but not all of a child's clauses | **Whole-child.** If the guardian has set ANY clause for a child, the guardian is fully authoritative for that child (an unset dimension = "no constraint", their intent). Device-only applies ONLY to children the guardian has never touched. Matches existing single-child semantics (absent budget = no cap). |
| Device→guardian STATUS feed | **Contract now, device emission deferred.** |
| Multi-device consolidation | **Fully spec the protocol in the contract now; no device code.** |

---

## A. Contract changes — `spec/contract.md` (task 1, wire-only)

All additions are **backward-compatible**: optional fields keep existing
`charter-proto` golden vectors valid; absent `subject` means the pairing's sole
`subject_pubkey` (the single-child default).

### A1. Per-child CLAUSE (inbound, guardian → device)

`ClausePayload` (inner kind `CHARTER_DEVICE_CLAUSE = 31113`) gains `subject`:

```ts
interface ClausePayload {
  v: 1;
  kind: 'schedule' | 'budget';
  subject?: Hex32;   // the child this clause targets (== Signet dependantId).
                     // absent = the pairing's sole subject_pubkey (back-compat).
  issuedAt: number;
  body: object;      // GrantSchedule | GrantBudget (unchanged shapes)
}
```

- Rollback protection becomes **per-(subject, kind)** monotonic `issuedAt`.
- A clause for a `subject` not yet bound to a local account is still
  authenticated against the pinned guardian and cached — **inert** until the
  binding exists, then it applies. (Caching, not rejecting, avoids a
  binding-ordering race; the clause is guardian-signed so it is authentic.)
- Authority is unchanged: a CLAUSE is a fully-signed NIP-01 inner event by the
  **pinned guardian key**, sealed + gift-wrapped. Per-child changes the *key*, not
  the *trust root* — there is still exactly one pinned guardian.

### A2. STATUS feed (outbound, device → guardian) — serves display *and* aggregation

One feed, two consumers: the PWA renders it; the consolidation aggregator sums it.
New inner kind **`CHARTER_DEVICE_STATUS = 31114`**, machine-authored, gift-wrapped
to the guardian, **one event per child per device**:

```ts
interface StatusPayload {
  v: 1;
  subject: Hex32;       // which child
  machine: Hex32;       // which device (== Pairing.machine) — the aggregation key
  ts: number;
  dayKey: string;       // 'YYYY-MM-DD' in the budget clause's tz (reset boundary)
  weekKey?: string;     // ISO week key in the budget tz, when a weekly cap is set
  usedTodaySecs: number;    // THIS device's raw contribution today  → aggregation input
  usedWeekSecs?: number;    // THIS device's raw contribution this week
  windowLeftSecs: number;   // device's local view (schedule window left) → display
  quotaLeftSecs: number;    // device's local view (budget quota left)    → display
  effectiveSecs: number;    // min(window, quota) on this device           → display
  locked: boolean;
  lockReason?: 'schedule' | 'budget' | 'malformed';
  source: 'guardian' | 'device-only';  // which policy is in force for this child now
}
```

**Privacy:** numbers + enums only — **no** child content, exec path, `time.extend`
reason, name, or DOB. E2E gift-wrapped to the guardian. `source` tells the PWA
whether the guardian's clauses or the device-only fallback is currently active.

**`[decide]`** (Signet sign-off): emission cadence; whether to also wrap to the
dependant under `audit_transparency`; the exact field set.

### A3. Multi-device usage consolidation protocol (reserved; contract-complete)

**Keystone — identity is device-independent.** A child is one subject pubkey; N
devices each bind a local account to that subject. The guardian sends **one**
budget clause per subject, and every device caches it — so the **cap** is shared
for free. Only **usage** must be consolidated.

**Aggregation model.** The guardian's backend (Signet) sums each subject's
`usedTodaySecs` / `usedWeekSecs` across all its devices (from the A2 STATUS feed)
per `(subject, dayKey)` / `(subject, weekKey)`, and returns each device a
**spent-elsewhere** value — the sum of *all other* devices' usage. New reserved
inbound event, **guardian-signed** (same pinned authority as clauses — no new
trust root):

```ts
// CHARTER_DEVICE_USAGE_SYNC = 31115 (guardian → machine, inner event signed, gift-wrapped)
interface UsageSyncPayload {
  v: 1;
  subject: Hex32;
  ts: number;
  dayKey: string;             spentElsewhereTodaySecs: number;   // Σ usage of all OTHER devices today
  weekKey?: string;           spentElsewhereWeekSecs?: number;   // Σ usage of all OTHER devices this week
}
```

**Device enforcement extension (pure layer).** Budget enforcement moves from

```
quota_left = min over present caps of (cap − used)
```
to
```
quota_left = min over present caps of (cap − spentElsewhere − localUsed)   // saturating ≥ 0
```

`spentElsewhere` **excludes this device** (no double-count); `localUsed` stays the
device's own live `UsageLedger` and keeps accruing between syncs. This is one
optional additive input to `EnforcerInputs` (a `ConsolidatedUsage { daily, weekly }`)
— no rewrite of the enforcer math.

**Durability / freshness.** `spentElsewhere` is durable per `(subject, dayKey)` /
`(subject, weekKey)`, monotonic by `ts`, and implicitly **resets to 0 when the
period key rolls** (new day/week → 0 until that period's first sync). No `exp`
needed; a fresher `ts` supersedes; a stale relay replay (older `ts`) is rejected.

**Offline + reconcile semantics (the online constraint, made honest).** Offline, a
device enforces `cap − stale_spentElsewhere − localUsed` — siblings' *new* offline
usage is invisible, so a **bounded cross-device over-spend is possible** during an
offline window. On reconnect: every device reports its full local usage via
STATUS, the aggregator re-sums and pushes fresh spent-elsewhere; if the
consolidated total now exceeds the cap, every device's `quota_left` saturates to 0
and **all lock**. Over-spend is thus *caught and corrected on reconnect* — matching
the fail-safe-offline philosophy (offline you get per-device cap enforcement, not
consolidated).

**Trust boundary.** Usage is **device-self-reported**. `charterd` runs as root and
the managed child does not control it, so within-family device self-reports are
trusted (anti-casual threat model). A replay of an old *lower* spent-elsewhere
(which would grant more time) is resisted by `ts`-monotonic.

**`[decide]`** (Signet sign-off): the kind number `31115`; guardian-signed vs a
separately-pinned aggregator key (recommend **guardian-signed**); sync cadence; the
offline fail-direction — reset-to-0 vs **persist-last-known** (recommend
persist-last-known); a value-monotonic-within-period hardening on top of
`ts`-monotonic; the precise daily/weekly period-key derivation.

### A4. Authority + identity model (documented in the contract)

- One **pinned guardian** signs every CLAUSE and every USAGE_SYNC. Per-child and
  per-device keying changes *which subject/period* an event addresses, never the
  trust root.
- A **child = subject pubkey**, independent of device; an admin binds local
  account(s) to it. Device-only limits are the **fallback** (unpaired / offline /
  a child the guardian has never set). No-local-authority holds: only root (the
  parent acting locally) or the pinned guardian can set policy — never the child.
- **Whole-child precedence:** for a bound child with ≥1 guardian clause, the
  guardian owns both dimensions (absent dimension = unconstrained); otherwise
  device-only; otherwise unconstrained (existing absent-clause semantics).

---

## B. Device-side per-child clause storage + ingestion (task 2)

### B1. New `charter-sys` port — `ChildClauseStore`

Modeled on the existing `CuratorListStore` (opaque-key + monotonic-rollback
pattern), keyed by `(subject_hex, kind)`:

```rust
pub trait ChildClauseStore: Send + Sync {
    /// Store an authenticated clause for a child. Returns true if stored, false
    /// if rejected as a rollback (issued_at <= highest seen for (subject, kind)).
    fn put_child_clause(&self, subject_hex: &str, kind: u16, issued_at: u64, json: &str) -> SysResult<bool>;
    fn get_child_clause(&self, subject_hex: &str, kind: u16) -> SysResult<Option<String>>;
    fn highest_issued_at(&self, subject_hex: &str, kind: u16) -> SysResult<Option<u64>>;
    /// Every cached (kind → json) for a subject — the resolver's input.
    fn clauses_for(&self, subject_hex: &str) -> SysResult<Vec<(u16, String)>>;
}
```

- **Mock** impl in `charter-sys` (in-memory map) for the headless gate.
- **Real** impl FS-backed under `/var/lib/charter/children/<subject_hex>/clauses/<kind>.json`
  (+ a sibling `.high` for the monotonic `issued_at`), atomic-write via the
  existing `fsutil` helpers. Shipped with `--features real` tests.
- Wired into `SystemLayer` / `RealSystem` (`Default`) alongside the existing
  `clauses()` accessor as `child_clauses()`.

### B2. Broker ingestion routing — `broker.rs::on_clause`

`on_clause` already: parse `ClausePayload` → `verify_clause` (inner sig + author
== pinned guardian + monotonic via `prev`) → cache. Change:

- Read `payload.subject`.
- **Present** → route to `ChildClauseStore`, with `prev =
  child_clauses().highest_issued_at(subject, kind)` and
  `put_child_clause(subject, kind, …)`.
- **Absent** → unchanged single `ClauseStore` path (single-child / default
  back-compat).
- Same authenticate-then-cache; fail-closed (a forged/rolled-back clause is not
  cached). Per-child rollback is independent across subjects.

---

## C. Precedence resolution + multi-child rework (task 2 — the core)

### C1. New pure module — `charterd/src/child_policy.rs`

```rust
pub enum PolicySource { Guardian, DeviceOnly, Unconstrained }

pub struct EffectivePolicy {
    pub schedule: Option<GrantSchedule>,
    pub budget: Option<GrantBudget>,
    pub source: PolicySource,
}

/// Whole-child precedence. `guardian_clauses` = the cached (kind, json) for the
/// child's subject (empty if none / unbound). `device_only` = the local fallback.
pub fn resolve_effective(
    guardian_clauses: &[(u16, String)],
    device_only: Option<&DeviceLimits>,
) -> EffectivePolicy;
```

Rules (pure, fully unit-tested):

1. `guardian_clauses` non-empty → parse schedule (kind 1) / budget (kind 2) from
   them; absent dimension = `None` (unconstrained). `source = Guardian`.
   **Malformed-clause asymmetry (preserve the existing `enforcer_runtime`
   behavior, scoped per child):** a present-but-unparseable **schedule** yields
   the **fail-safe** locked schedule (`paused: true`) → that child locks; a
   present-but-unparseable **budget** yields `None` (no cap) — the schedule
   remains the fail-safe gate. (`load_schedule` fail-safes, `load_budget` fail-
   opens today; this keeps that contract.)
2. else `device_only` present → `to_schedule()` / `to_budget()`. `source = DeviceOnly`.
3. else both `None`. `source = Unconstrained`.

### C2. `MultiChildEnforcer` consumes resolved policy

- `ChildEnforcer` already stores `schedule` + `budget` directly. Add `source`
  (for the STATUS `source` field later; harmless now).
- `MultiChildEnforcer::sync` takes `&[(u32 /*uid*/, EffectivePolicy)]` instead of
  `&[(u32, DeviceLimits)]`. Re-limit keeps the accrued usage/extension ledgers
  across a source flip; the ledgers re-key on the new clause's tz/weekStart
  (existing reconcile behavior).
- The pure tick logic is otherwise unchanged (independent per-child decisions,
  active-session accrual).

### C3. `runtime.rs` loop rework

Per tick, per managed child:

1. Load `limits.d/<user>.json` → the per-child config (now: optional `subject`
   binding + optional device-only `DeviceLimits` fallback — see D).
2. If bound, pull `child_clauses().clauses_for(subject)`.
3. `resolve_effective(guardian_clauses, device_only)` → `EffectivePolicy`.
4. `multi.sync(resolved, …)`; `multi.tick(active, …)`; apply per-child freeze +
   the active child's lock (unchanged `apply_child_decisions`).

The single-child `EnforcerRuntime::tick` is **retired from the multi-child loop**
(it currently isn't even called there). `EnforcerRuntime` stays for the broker's
`time.extend` ledger seam (the shared `Arc<Mutex<EnforcerRuntime>>` the
`TimeExtendEnactor` pushes into). **[follow-up, noted not built]:** routing
`time.extend` grants to the right child's ledger is per-child future work; today
the broker keeps its single extension ledger. Flagged in the plan.

---

## D. Subject↔account binding + setup

- The per-child config file `/etc/charter/limits.d/<user>.json` gains an optional
  `subject` (hex32) and makes the device-only limit fields **optional**, so a file
  can be: *binding-only* (guardian-managed, no local fallback yet),
  *device-only* (today's shape), or *both*. Back-compat: existing flat
  `DeviceLimits` JSON parses as `subject = None` + limits present.
  - Concretely: introduce a `ChildConfig { subject: Option<String>, limits:
    Option<DeviceLimits> }` with a serde shape that still accepts the legacy flat
    `DeviceLimits` document (deserialize via an untagged/compat path; exact
    mechanics in the plan). `DeviceLimits::validate` is unchanged and only runs
    when `limits` is present.
- `charter-setup` (graphical, admin-only) writes the `subject` binding during
  pairing — the privileged step. No new live syscalls; the loop already reads this
  dir each tick.

---

## E. Testing + invariants (test-first)

- `child_policy::resolve_effective` — every branch: guardian-only, device-only,
  both (guardian wins), partial guardian (schedule-only / budget-only →
  unconstrained other dim), malformed guardian **schedule** (fail-safe lock that
  child) vs malformed guardian **budget** (fail-open, no cap, schedule still
  gates), unbound subject (empty guardian → device-only), neither
  (unconstrained).
- `ChildClauseStore` — mock + `--features real`: per-(subject, kind) rollback
  rejection, independence across subjects, durability across reopen, `clauses_for`
  ordering. Mirrors the existing `ClauseStore` / `CuratorListStore` tests.
- `broker::on_clause` routing — subject present → child store; absent → single
  store; forged author rejected per-child; rollback per-(subject, kind); a clause
  for an unbound subject is cached and inert.
- `MultiChildEnforcer` — a guardian clause supersedes device-only for one child
  while a sibling stays device-only; unpaired/offline → device-only fallback;
  ledgers survive a source flip.
- `ChildConfig` parse — legacy flat `DeviceLimits` file still loads; binding-only
  file loads; both loads; invalid limits still rejected.

**Invariants re-asserted by passing tests:**
- **No-local-authority** — neither source is child-writable; only root or the
  pinned guardian sets policy.
- **Fail-safe / fail-closed (two layers).** Envelope failure (bad inner sig,
  wrong author, rollback) → the clause is **never cached** (broker fail-closed),
  per-(subject, kind). An authenticated clause whose *body* is unparseable **is**
  cached but read-time fail-safes: an unparseable **schedule** locks **that child
  only** (per-child isolation), never the whole device. Both layers are scoped per
  subject — one child's bad clause can't lock a sibling.
- **Pinned authority** — every per-child clause is still authored by the single
  pinned guardian; per-child changes the addressed subject, not the trust root.
- **Rollback protection** — independent monotonic `issuedAt` per (subject, kind);
  a hostile relay cannot revert one child's clause or replay another's.

---

## Scope boundary (what ships this session)

| Item | This session |
|---|---|
| A1 per-child CLAUSE wire (`subject`) | **Contract + device consume (B/C)** |
| A2 STATUS feed wire | **Contract only** (emission deferred) |
| A3 consolidation protocol (`USAGE_SYNC`, aggregation, offline/reconcile) | **Contract only** (no device code) |
| A4 authority/identity doc | **Contract** |
| B `ChildClauseStore` + ingestion routing | **Device code** |
| C `child_policy` + multi-child rework + loop precedence flip | **Device code** |
| D `ChildConfig` (optional subject) + `charter-setup` binding write | **Device code** |
| E tests | **Device code** |
| Signet (guardian/aggregator backend), STATUS emission, USAGE_SYNC consumption | **Out of scope** |

## Forward-compat notes (so goal 2 lands with no wire break)

- `EnforcerInputs` gains an optional `ConsolidatedUsage` later — additive.
- STATUS already carries `machine` + `dayKey` + raw `usedTodaySecs`, the exact
  inputs an aggregator needs.
- `USAGE_SYNC` is guardian-signed → reuses the existing verify-against-pinned-
  guardian path; no new trust root or store pattern (same monotonic-by-ts shape).

## The gate (must stay green, run from `linux/`)

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace                                          # mock
cargo test -p charter-sys -p charterd --no-default-features --features real
cargo build --workspace --no-default-features --features real   # compile-only
```
