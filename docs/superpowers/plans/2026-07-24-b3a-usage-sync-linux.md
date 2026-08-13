# B3a — Usage-Sync (31115) + Pooled Union Enforcement: core + Linux warden

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Both wardens journal per-minute activity bitmaps, emit them in STATUS, and the Linux warden ingests guardian-signed USAGE_SYNC (31115) to enforce the pooled budget as `cap − |own ∪ elsewhere|` — falling back to scalars, degrading to local-only offline.

**Architecture:** All logic lands in `core/` (shared by both wardens): a `MinuteSet` bitmap type + journal in `charter-schedule`, wire structs in `charter-proto`, verification in `charter-verify`, transport poll in `charter-transport`, a `UsageSyncStore` port in `charter-sys`, and broker + enforcer wiring in `charter-spine`. `linux/charterd` only stamps the STATUS field and feeds the consolidated input; `android/jni` gets a compile-compat touch (full wiring is B3c). Spec: `spec/contract.md` §USAGE_SYNC (incl. the union-rule extension) and the design memo `docs/superpowers/specs/2026-07-24-per-device-rules-design.md`.

**Tech Stack:** Rust (core workspace + linux workspace), serde camelCase wire structs, hand-rolled base64url (no new deps in core), frozen cross-stack vectors under `core/crates/charter-testkit/vectors/`.

## Global Constraints

- Kind number: `CHARTER_DEVICE_USAGE_SYNC = 31115` (reserved at `charter-primitives/src/kinds.rs:26`; RELEASE=31116 already taken).
- Wire fields additive + optional only: `StatusPayload.active_minutes_today: Option<String>`, `UsageSyncPayload.elsewhere_minutes_today: Option<String>` — `#[serde(default, skip_serializing_if = "Option::is_none")]`, camelCase (`activeMinutesToday`, `elsewhereMinutesToday`). Existing payload bytes must stay identical.
- Bitmap: 1440 bits (one per local minute of `dayKey`), 180 bytes, base64url **no padding** (240 chars). Bit i = minute i active. Byte 0 bit 0 (LSB-first within byte) = minute 0.
- Monotonic-by-`ts`: an inbound USAGE_SYNC with `ts <=` the stored one for that subject is rejected (replay). Period roll: a stored sync whose `day_key`/`week_key` ≠ current key contributes 0 for that period (persist-last-known, per contract [decide] recommendation).
- Pooled daily used = `|own_minutes ∪ elsewhere_minutes| * 60` when BOTH bitmaps present and day keys align; else `used_today_secs + spent_elsewhere_today_secs`. Weekly is always scalar (`used_week + spent_elsewhere_week_secs`). Staleness/absence only ever under-counts.
- Trust: USAGE_SYNC inner event is **guardian-signed** — verified exactly like a CLAUSE (kind, parse, id integrity, schnorr, pinned author, monotonic ts). No new trust root.
- No new core dependencies (hand-roll base64url in `charter-schedule`); mock/real feature discipline unchanged.
- Gates: from `linux/` AND `core/`: `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --workspace`, `cargo test --workspace --features real` (core: granular real features), `cargo build --workspace --no-default-features --features real`. Android compile-compat: `cd android/jni && cargo check`, `cd android/jni-guardian && cargo check`.
- Commit after every task; conventional commits; session trailer.

---

### Task 1: `MinuteSet` bitmap in charter-schedule

**Files:**
- Create: `core/crates/charter-schedule/src/minutes.rs`
- Modify: `core/crates/charter-schedule/src/lib.rs` (add `pub mod minutes;`)
- Create: `core/crates/charter-testkit/vectors/usage_sync/minute_set_vectors.json`
- Create: `core/crates/charter-schedule/tests/minute_set_vectors.rs`

**Interfaces (Produces):**
```rust
/// 1440-bit day-of-minutes bitmap (LSB-first within each of 180 bytes).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MinuteSet { bits: [u8; 180] }

impl MinuteSet {
    pub const MINUTES: usize = 1440;
    pub fn set(&mut self, minute: usize);                  // no-op if >= 1440
    pub fn contains(&self, minute: usize) -> bool;
    pub fn count(&self) -> u32;                            // popcount
    pub fn union(&self, other: &MinuteSet) -> MinuteSet;
    pub fn is_empty(&self) -> bool;
    pub fn to_b64url(&self) -> String;                     // 240 chars, no pad
    pub fn from_b64url(s: &str) -> Option<MinuteSet>;      // strict: exactly 240 chars, valid alphabet
    /// Mark every local minute-of-day touched by [now-elapsed, now] (same local day only).
    pub fn mark_span(&mut self, tz: chrono_tz::Tz, now_unix: i64, elapsed_secs: u64);
}
```
Serde: serialize as the b64url string, deserialize via `from_b64url` (reject bad length/alphabet).

- [ ] Step 1: failing unit tests in `minutes.rs` `#[cfg(test)]`: `set_count_contains_roundtrip`, `b64url_roundtrip_240_chars_no_pad`, `from_b64url_rejects_bad_length_and_alphabet` (239/241 chars, `+`/`/` chars, padding `=`), `union_counts_overlap_once` (e.g. {0,1,2} ∪ {2,3} → count 4), `mark_span_marks_inclusive_minutes` (Europe/London, now=NOON, elapsed=90 → minutes for 11:58:30–12:00:00 marked: 718,719,720), `mark_span_clamps_to_day_start` (elapsed > seconds-since-midnight).
- [ ] Step 2: run `cd core && cargo test -p charter-schedule minutes` → FAIL (module missing).
- [ ] Step 3: implement `minutes.rs` (hand-rolled base64url alphabet `A-Za-z0-9-_`, encode 180 bytes → 240 chars; decode strict).
- [ ] Step 4: tests pass.
- [ ] Step 5: write `minute_set_vectors.json`: `{ "note": "...", "vectors": [ {name, minutes:[...], b64url:"..."}, ... ], "unions": [ {name, a:[...], b:[...], unionCount:N} ], "invalid": ["...bad strings..."] }` — generate the b64url strings FROM the passing Rust impl (print in a test, paste), including: empty set, {0}, {1439}, {718,719,720}, a dense 600-minute day. Vector test `minute_set_vectors.rs` loads via `charter_testkit::golden::load_json("usage_sync/minute_set_vectors.json")`, asserts encode+decode+unionCount, and that every `invalid` fails.
- [ ] Step 6: `cargo test -p charter-schedule` green → commit `feat(core): MinuteSet 1440-bit day bitmap with frozen b64url vectors`.

### Task 2: minute journal inside `UsageLedger`

**Files:**
- Modify: `core/crates/charter-schedule/src/usage.rs`

**Interfaces (Produces):** `UsageLedger` gains `#[serde(default)] minutes_today: MinuteSet`; new reader `pub fn minutes_today(&self, now_unix: i64) -> MinuteSet` (empty if the local day rolled, mirroring `used_today` at `usage.rs:160-167`).

- [ ] Step 1: failing tests in `usage.rs`: `active_screen_time_marks_minutes` (credit 120s Active at NOON → minutes_today count ≥ 2, contains 719/720), `minutes_clear_on_day_roll` (credit, then credit next local day → old minutes gone), `idle_and_learning_do_not_mark_minutes` (Idle credits nothing; Learning bucket marks — DECISION: learning time IS screen-active time for the union picture, so `Bucket::Learning` also marks minutes), `snapshot_without_minutes_field_restores` (old JSON snapshot → empty MinuteSet, no error).
- [ ] Step 2: run → FAIL.
- [ ] Step 3: implement — in `credit_bucket` (usage.rs:127-147) after the roll, when `activity.counts()`, call `self.minutes_today.mark_span(tz, now, elapsed)` for BOTH buckets; clear in `roll` (usage.rs:103-116).
- [ ] Step 4: all `charter-schedule` tests pass (incl. existing snapshot back-compat suite).
- [ ] Step 5: commit `feat(core): UsageLedger journals per-minute activity (survives snapshot, clears on day roll)`.

### Task 3: wire structs — kind 31115, `UsageSyncPayload`, STATUS `activeMinutesToday`

**Files:**
- Modify: `core/crates/charter-primitives/src/kinds.rs` (const + frozen-kinds test + drop the "reserved" comment)
- Create: `core/crates/charter-proto/src/usage_sync.rs`; modify `charter-proto/src/lib.rs`
- Modify: `core/crates/charter-proto/src/status.rs` (`active_minutes_today: Option<String>` after `learning_today_secs`)
- Modify: `core/crates/charter-spine/src/status_emit.rs` (`build_status` sets it to `None`; loop stamps it)
- Modify: `core/crates/charter-spine/tests/contract_constants.rs` (assert 31115)
- Create: `core/crates/charter-testkit/vectors/usage_sync/usage_sync_vectors.json`
- Create: `core/crates/charter-proto/tests/usage_sync_vectors.rs`

**Interfaces (Produces):**
```rust
pub const CHARTER_DEVICE_USAGE_SYNC: u16 = 31115;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageSyncPayload {
    pub v: u32,                                  // == 1
    pub subject: PubKey,
    pub ts: u64,
    pub day_key: String,
    pub spent_elsewhere_today_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spent_elsewhere_week_secs: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elsewhere_minutes_today: Option<String>, // MinuteSet b64url
}
impl UsageSyncPayload { pub fn from_json(&str) -> Option<Self>; pub fn to_json(&self) -> String; }
```
(Match `deny_unknown_fields` usage to whatever `ClausePayload` does — if clauses tolerate unknown fields, do the same here for forward-compat; check and mirror.)

- [ ] Step 1: failing tests — `usage_sync.rs` unit: `roundtrips_camel_case`, `optional_fields_omitted_when_absent`, `rejects_wrong_version`; `status.rs`: `active_minutes_today_serializes_camel_case_and_omits_when_none`; vectors file with ≥3 payload vectors (scalar-only; with bitmap; with week fields — reuse a bitmap string from Task 1 vectors) + `invalid` entries (v:2, missing dayKey, non-hex subject); consumer test asserts parse+reserialize == pinned bytes.
- [ ] Step 2: run → FAIL. Step 3: implement. Step 4: `cargo test -p charter-proto -p charter-primitives -p charter-spine` green.
- [ ] Step 5: commit `feat(core): USAGE_SYNC 31115 wire payload + STATUS activeMinutesToday (frozen vectors)`.

### Task 4: `verify_usage_sync` in charter-verify

**Files:**
- Create: `core/crates/charter-verify/src/usage_sync.rs`; modify `lib.rs` re-export
- Create: `core/crates/charter-verify/tests/verify_usage_sync.rs`

**Interfaces (Produces):**
```rust
pub struct VerifiedUsageSync { /* no public ctor */ }
impl VerifiedUsageSync { pub fn ts(&self) -> u64; pub fn payload(&self) -> &UsageSyncPayload; }

pub fn verify_usage_sync(
    event: &NostrEvent,
    pinned_guardian: &PubKey,
    prev_ts: Option<u64>,
    _now: u64,
) -> Result<VerifiedUsageSync, UsageSyncError>;
// UsageSyncError { Malformed, BadSignature, UntrustedSigner, StaleTs }
```
Order mirrors `verify_clause` (clause.rs:69-93): kind==31115 → payload parse (incl. v==1 and, when present, `MinuteSet::from_b64url` must succeed — malformed bitmap ⇒ Malformed, fail-closed) → event-id integrity → schnorr sig → pinned author → `ts > prev_ts`.

- [ ] Step 1: failing tests mirroring `verify_clause.rs:12-87`: `valid_usage_sync_authenticates`, `reject_untrusted_signer`, `reject_stale_or_equal_ts`, `reject_bad_signature`, `reject_kind_mismatch`, `reject_malformed_bitmap`. Build events with the same testkit builder used there.
- [ ] Step 2-4: FAIL → implement → green (`cargo test -p charter-verify` both mock and `--features real` if the suite splits).
- [ ] Step 5: commit `feat(core): verify_usage_sync — guardian-pinned, ts-monotonic, fail-closed`.

### Task 5: transport poll + persistence port

**Files:**
- Modify: `core/crates/charter-transport/src/transport.rs` (`ReceivedUsageSync { pub event: NostrEvent }` next to `ReceivedClause` at :64-69; `pub async fn poll_usage_syncs(...)` mirroring `poll_releases` :303-310 with `CHARTER_DEVICE_USAGE_SYNC`)
- Modify: `core/crates/charter-sys/src/persistence.rs` — new port:
```rust
pub trait UsageSyncStore: Send + Sync {
    /// Store the latest verified sync for a subject. Caller enforces ts-monotonicity.
    fn put_usage_sync(&self, subject_hex: &str, ts: u64, json: &str);
    fn get_usage_sync(&self, subject_hex: &str) -> Option<(u64, String)>;
}
```
  Mock impl beside the mock ChildClauseStore (persistence.rs:303-364 pattern, `BTreeMap<String,(u64,String)>`); real impl beside :665-728 at `<base>/children/<subject_hex>/usage_sync.json` reusing the hex-only `safe()` sanitizer.
- Tests: mock + real durability (`usage_sync_durable_across_reopen`, `usage_sync_per_subject_isolated`) mirroring persistence.rs:907-969 / :1110-1176.

- [ ] Step 1: failing tests. Step 2-4: implement → `cargo test -p charter-transport -p charter-sys` green (mock AND `--features real` for the real store).
- [ ] Step 5: commit `feat(core): poll_usage_syncs transport + UsageSyncStore port (mock+real)`.

### Task 6: pooled quota math + enforcer input

**Files:**
- Modify: `core/crates/charter-schedule/src/budget.rs`
- Modify: `core/crates/charter-schedule/src/enforcer.rs`

**Interfaces (Produces):**
```rust
/// The freshest verified elsewhere-view for one subject (from USAGE_SYNC).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConsolidatedUsage {
    pub day_key: String,
    pub spent_elsewhere_today_secs: u64,
    pub week_key: Option<String>,
    pub spent_elsewhere_week_secs: Option<u64>,
    pub elsewhere_minutes_today: Option<MinuteSet>,
}

pub fn quota_left_pooled(
    usage: &UsageLedger, budget: &GrantBudget,
    consolidated: Option<&ConsolidatedUsage>, now_unix: i64,
) -> QuotaStatus;
// existing quota_left(usage, budget, now) := quota_left_pooled(usage, budget, None, now)
```
`EnforcerInputs` (enforcer.rs:118-125) gains `pub consolidated: Option<ConsolidatedUsage>`; the budget dimension (enforcer.rs:240-252) calls `quota_left_pooled`.

Daily math inside `quota_left_pooled` when `Some(c)` and `c.day_key == usage current day key`:
- both `c.elsewhere_minutes_today` and non-empty `usage.minutes_today(now)` → `pooled_today = (own ∪ elsewhere).count() as u64 * 60`
- else → `pooled_today = usage.used_today(now) + c.spent_elsewhere_today_secs`
- daily left = `(daily*60).saturating_sub(pooled_today)`.
Weekly: `(weekly*60).saturating_sub(usage.used_week(now) + c.spent_elsewhere_week_secs.unwrap_or(0))` only when `c.week_key` matches the ledger's current week key; else local-only. Day-key mismatch ⇒ whole `c` contributes 0 (period rolled).

- [ ] Step 1: failing tests in budget.rs mirroring :38-100: `pooled_scalar_locks_when_elsewhere_plus_local_exceeds_cap` (cap 60m, local 30m, elsewhere 35m → Remaining(0)), `pooled_union_counts_simultaneous_once` (cap 60m, own minutes {0..40}, elsewhere {20..50} → union 50m → Remaining(600)), `stale_day_key_falls_back_to_local`, `no_consolidated_matches_old_behavior`, `week_scalar_pool`; enforcer test `pooled_budget_flows_into_effective` beside :436-506.
- [ ] Step 2-4: FAIL → implement → `cargo test -p charter-schedule` green. **Compile-compat:** update every `EnforcerInputs` construction site in core/linux (`..Default::default()` or `consolidated: None`) — and remember android/jni has one at `android/jni/src/warden.rs:1164-1171` (fixed in Task 8).
- [ ] Step 5: commit `feat(core): pooled quota — cap − |own ∪ elsewhere|, scalar fallback, period-roll safe`.

### Task 7: spine broker ingest + Linux runtime wiring

**Files:**
- Modify: `core/crates/charter-spine/src/transport_facade.rs` (facade method + `MockTransport::deliver_usage_sync` beside :60-70, impl :88-125)
- Modify: `core/crates/charter-spine/src/broker.rs` (`on_usage_sync` mirroring `on_clause` :353-420: parse subject from payload, floor = `usage_sync_store.get_usage_sync(subject).map(|(ts,_)| ts)`, `verify_usage_sync`, then put; call from `poll_once` :423-441)
- Modify: `linux/crates/charterd/src/runtime.rs`: (a) when building each child's `EnforcerInputs`, read `get_usage_sync(subject)` → parse `UsageSyncPayload` → build `ConsolidatedUsage` (bitmap decoded via `MinuteSet::from_b64url`); (b) stamp `status.active_minutes_today = Some(usage.minutes_today(now).to_b64url())` when non-empty, beside the `learning_today_secs` stamp at :1263.
- Create: `linux/crates/charterd/tests/usage_sync_pooling.rs` integration test mirroring `child_clause_routing.rs` (helper `broker(...)` at :45): deliver a guardian-signed USAGE_SYNC via MockTransport → assert the child's quota shrinks; deliver a stale-ts one → rejected; assert emitted STATUS json contains `activeMinutesToday` after crediting usage.

- [ ] Step 1: failing broker tests (`usage_sync_routes_verifies_and_stores`, `usage_sync_stale_ts_rejected`, `usage_sync_forged_signer_not_stored`) + the charterd integration test.
- [ ] Step 2-4: FAIL → implement → `cd core && cargo test --workspace` and `cd linux && cargo test --workspace` green.
- [ ] Step 5: commit `feat(warden): ingest USAGE_SYNC → pooled budget enforcement + STATUS activeMinutesToday`.

### Task 8: android compile-compat + full gates + contract status flip

**Files:**
- Modify: `android/jni/src/warden.rs` (`EnforcerInputs { .., consolidated: None }` at :1164-1171 — full wiring is B3c)
- Modify: `spec/contract.md` §USAGE_SYNC coordination note: "no device code consumes it" → Linux warden ingests + enforces as of B3a (date, this plan); STATUS `activeMinutesToday` emitted by both wardens (shared core).
- Modify: `docs/superpowers/specs/2026-07-24-per-device-rules-design.md` build order: B3 → "B3a LANDED (core+Linux)".

- [ ] Step 1: `cd android/jni && cargo check` + `cd android/jni-guardian && cargo check` → fix to green.
- [ ] Step 2: full gates — `cd core`: fmt/clippy/test/test-real/build-real (granular features per linux-ci.yml:25-44); `cd linux`: all five (linux-ci.yml:46-63).
- [ ] Step 3: docs edits above.
- [ ] Step 4: commit `feat(core+warden): B3a complete — usage-sync pooling live on Linux` + push main.

## Self-review notes
- Spec coverage: journal (T2), STATUS field (T3+T7b), 31115 payload+verify (T3+T4), transport+store (T5), union math + fallback + period roll (T6), broker+runtime (T7), android compile-compat (T8). PWA publish/consume = plan B3b; Android wiring = plan B3c.
- Type names used consistently: `MinuteSet`, `UsageSyncPayload`, `ConsolidatedUsage`, `VerifiedUsageSync`, `UsageSyncStore`, `ReceivedUsageSync`, `quota_left_pooled`.
- Learning-bucket decision recorded in T2 step 1 (learning marks minutes — it is screen time for the union picture; the learning *budget* exemption is unaffected, it lives in quota math not the journal).
