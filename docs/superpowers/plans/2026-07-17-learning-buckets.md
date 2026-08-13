# Learning Buckets ("school apps are time-free") Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Time spent in guardian-designated learning apps (Khan Academy as a pinned Chromium app window; native apps like the son's own project) credits a separate *learning* bucket instead of draining the child's screen-time budget.

**Architecture:** The spine's usage meter grows a second per-day bucket (`learning`) fed by a new `Bucket` parameter on the credit path. A new frozen `learning` wire clause carries the guardian's app list (site apps with a verified domain closure; native apps by executable identity) plus an optional learning cap. On Linux, charterd attributes each tick to a bucket by resolving the focused X11 window → owning process → cmdline/exe identity; a new standing enactor materialises site apps as root-owned resolver-pinned Chromium `--app` launchers. MyCharter gets a Learning section (curated catalogue + native-app picker fed by a device-published app inventory). Everything is inert until a `learning` clause exists (I17 pattern).

**Tech Stack:** Rust (charter-schedule / charter-proto / charter-spine / charterd), x11rb (already used by charter-lock), Chromium `--app` + `--host-resolver-rules`, React/TS MyCharter PWA, vitest + cargo test golden vectors.

## Global Constraints

- **Buckets v1 = two:** `screen` (drains the budget, exactly today's behaviour) and `learning` (tracked separately; optional aggregate cap). Per-app sub-buckets are v2 — do NOT build them.
- **Over-cap semantics:** when the learning cap is exhausted, learning-attributed time credits the `screen` bucket instead (soft fallback, no new lock UX).
- **Fail-closed attribution:** any attribution failure (no display, no PID, unreadable cmdline) charges `screen`. A tick can never be *free* by error.
- **Identity strength ladder:** site apps match ONLY on the launcher's cmdline markers (`--class=charter-<id>` AND `--host-resolver-rules` present). Native apps match on root-owned absolute exe path or flatpak app id. Home-dir native paths are allowed but the clause marks them `trusted` (guardian-vouched, advisory) — no pretending they're enforced.
- **Inert-until-clause:** no learning clause → zero behaviour change, no launchers written, no attribution probing beyond the existing active-uid check.
- **Curated domain closures must be verified, never guessed** (no-fabricated-values rule). Khan Academy seed: verify the domain set live before committing the catalogue entry.
- **Versioning:** new clause `learning` v1 frozen via golden vectors (mirror `apps_clause_vectors.rs`). Deb ships as **0.3.0** (capability jump per release convention).
- **Wardship lexicon** in all copy: guardian/ward, "learning time", never "parental controls".
- Linux gates (fmt/clippy -D warnings/test/real-test/real-build) green in `core/` and `linux/` before each merge; root vitest green; PWA typecheck+test green.

## File Structure

- `core/crates/charter-schedule/src/usage.rs` — `Bucket` enum; `UsageLedger` gains `learning_today_secs` + `credit_bucket()`; snapshot forward/backward compatible.
- `core/crates/charter-proto/src/learning.rs` (new) — `GrantLearning` clause body v1 + parsing; `core/crates/charter-proto/tests/learning_clause_vectors.rs` (new) golden vectors; `spec/vectors/learning-v1.json` (new).
- `core/crates/charter-spine/src/multi_child.rs` — `tick()` takes the active tick's `Bucket`; learning cap fallback logic; `learning_today()` accessor; `child_policy.rs` carries the parsed learning clause.
- `linux/crates/charterd/src/focus.rs` (new) — focused-window→process→`Bucket` attribution (x11rb, timeout-guarded).
- `linux/crates/charterd/src/enactors/learning_apps.rs` (new) — standing enactor: materialise/remove root-owned `.desktop` launchers + per-app Chromium profiles dir.
- `linux/crates/charterd/src/runtime.rs` — wire `focus::bucket_for_tick()` into the loop; extend TimeLeft snapshots with `learning_today`; inventory publish on slow tick.
- `apps/charter-app/src/screens/Learning.tsx` (new) + `src/data/learning_catalogue.ts` (new) + wire types — guardian UI.
- `apps/charter-console/ui/app.html` — Learning section (backup path).
- `spec/contract.md` — `learning` clause documented.

---

### Task A1: `Bucket` + learning counter in the usage meter (charter-schedule)

**Files:** Modify `core/crates/charter-schedule/src/usage.rs`, `core/crates/charter-schedule/src/lib.rs` (re-export).

**Interfaces — Produces:**
```rust
pub enum Bucket { Screen, Learning }               // Copy, Eq, Serialize
impl UsageLedger {
    /// Credit `elapsed` seconds of `activity` into `bucket` at `now`.
    /// Bucket::Screen == the pre-existing credit() semantics (day+week).
    /// Bucket::Learning feeds learning_today_secs only (day-keyed, same rollover).
    pub fn credit_bucket(&mut self, now_unix: i64, activity: Activity, bucket: Bucket, elapsed_secs: u64);
    pub fn learning_today_secs(&self, now_unix: i64) -> u64;
}
// credit() stays and delegates to credit_bucket(.., Bucket::Screen, ..).
```

- [ ] Failing tests first (same file's tests module): `learning_credit_does_not_drain_screen`, `learning_rolls_over_at_local_midnight`, `snapshot_without_learning_field_restores` (old snapshot JSON string → ledger with learning=0; serde `#[serde(default)]` on the new field).
- [ ] Run: `cargo test -p charter-schedule usage` → new tests FAIL.
- [ ] Implement (`learning_today_secs: u64` field with `#[serde(default)]`; day-rollover shared with existing `roll()` path).
- [ ] Run: PASS. `cargo fmt` + clippy clean.
- [ ] Commit `feat(schedule): learning bucket in the usage ledger`.

### Task A2: `learning` clause v1 (charter-proto) + golden vectors

**Files:** Create `core/crates/charter-proto/src/learning.rs`; modify `lib.rs`, `clause.rs` (ClauseKind::Learning); create `core/crates/charter-proto/tests/learning_clause_vectors.rs`, `spec/vectors/learning-v1.json`.

**Interfaces — Produces:**
```rust
pub const LEARNING_VERSION: u32 = 1;
#[serde(rename_all = "lowercase")]
pub enum LearningAppKind { Site, Native }
#[serde(rename_all = "camelCase")]
pub struct LearningApp {
    pub id: String,            // slug, e.g. "khan-academy"
    pub label: String,         // display name
    pub kind: LearningAppKind,
    #[serde(default)] pub domains: Vec<String>, // site: verified closure
    #[serde(default, skip_serializing_if = "Option::is_none")] pub url: Option<String>,   // site: launch URL
    #[serde(default, skip_serializing_if = "Option::is_none")] pub exec: Option<String>,  // native: abs path or flatpak id
    #[serde(default)] pub trusted: bool,        // native home-dir path: guardian-vouched only
}
#[serde(rename_all = "camelCase")]
pub struct GrantLearning {
    pub v: u32,
    pub apps: Vec<LearningApp>,
    #[serde(default, skip_serializing_if = "Option::is_none")] pub cap_minutes: Option<u32>, // None = uncapped
    #[serde(default, skip_serializing_if = "Option::is_none")] pub paused: Option<bool>,
    pub issued_at: u64,
}
impl GrantLearning { pub fn from_value(&Value) -> Result<GrantLearning, ProtoError>; pub fn is_paused(&self) -> bool; }
```

- [ ] Write vectors JSON first (valid site app, valid native, capped, paused, bad-version→error, site-with-no-domains→error) mirroring `apps_clause_vectors.rs` harness; run → FAIL (module missing).
- [ ] Implement `learning.rs` (validation: site apps require non-empty `domains` + `url`; native require `exec`; fail-closed).
- [ ] Tests PASS; fmt/clippy; commit `feat(proto): learning clause v1 + golden vectors`.

### Task A3: spine plumbs the bucket + cap fallback (charter-spine)

**Files:** Modify `core/crates/charter-spine/src/multi_child.rs`, `child_policy.rs`, `enforcer_runtime.rs`, `status_emit.rs`.

**Interfaces — Produces:**
```rust
// multi_child:
pub fn tick(&mut self, active_uid: Option<u32>, now: i64, elapsed: u64, bucket: Bucket) -> Vec<ChildDecision>;
pub fn learning_today(&self, uid: u32, now: i64) -> Option<u64>;
// EffectivePolicy gains: pub learning: Option<GrantLearning>;
```
Cap fallback lives here: if `bucket == Learning` and `cap_minutes` is Some and `learning_today_secs >= cap*60`, credit as `Bucket::Screen` instead.

- [ ] Failing tests: `learning_tick_does_not_reduce_remaining`, `learning_over_cap_charges_screen`, `paused_learning_clause_charges_screen`; existing `tick()` callers updated with `Bucket::Screen` (behaviour-identical).
- [ ] Implement; all spine tests PASS; commit `feat(spine): bucketed tick + learning cap fallback`.

### Task B1: focused-window attribution (charterd `focus.rs`)

**Files:** Create `linux/crates/charterd/src/focus.rs`; modify `lib.rs`, `Cargo.toml` (x11rb dep, real feature only — copy version from `charter-lock/Cargo.toml`).

**Interfaces — Produces:**
```rust
/// Classify the current foreground window for `display`/`xauth` against the
/// child's learning apps. Pure matcher split from the X11 probe for testing:
pub fn classify(cmdline: &[String], exe: Option<&str>, apps: &[LearningApp]) -> Bucket;
/// Probe + classify; ANY failure => Bucket::Screen. Hard 2s guard like loginctl.
pub fn bucket_for_tick(display: &str, xauth: Option<&str>, apps: &[LearningApp]) -> Bucket;
```
Site match: cmdline contains `--class=charter-<app.id>` AND any `--host-resolver-rules=` arg. Native match: `exe == app.exec` (absolute, root-owned check via metadata uid==0 unless `trusted`) or flatpak: cmdline[0] ends `/flatpak` + contains `run` + app id.
X11 probe: `_NET_ACTIVE_WINDOW` on root → `_NET_WM_PID` on that window → `/proc/<pid>/cmdline`,`/proc/<pid>/exe`. Reuse the display/xauth the loop already resolves via `active_session_x()`.

- [ ] Failing tests for `classify` (pure): khan launcher cmdline→Learning; plain chromium→Screen; forged `--class` without resolver pin→Screen; native exe match root-owned→Learning; home-dir exe untrusted→Screen; home-dir exe trusted→Learning; empty apps→Screen.
- [ ] Implement matcher; PASS. X11 probe compiles under `--features real` (on-metal check is decented's round; degrade path unit-tested by feeding probe failure).
- [ ] Commit `feat(charterd): foreground bucket attribution`.

### Task B2: learning-apps standing enactor

**Files:** Create `linux/crates/charterd/src/enactors/learning_apps.rs`; modify `enactors/mod.rs`, `runtime.rs` (reconcile call on slow tick, same level-triggered style as `web_content.rs`).

**Interfaces — Produces:**
```rust
/// Reconcile materialised launchers with the clause (level-triggered, idempotent).
/// Writes /usr/share/applications/charter-learn-<id>.desktop (root 0644) with
/// Exec=<chromium> --app=<url> --class=charter-<id> --user-data-dir=/var/lib/charter/apps/<uid>/<id> --host-resolver-rules=<pin> ;
/// removes stale charter-learn-*.desktop not in the clause. Returns actions taken (for tests).
pub fn reconcile(clause: Option<&GrantLearning>, uid: u32, fs: &mut dyn LearnFs) -> Vec<LearnAction>;
```
`pin` = `MAP * ~NOTFOUND, EXCLUDE <domain1>, EXCLUDE <domain2>, …` from `app.domains`. Chromium binary resolved at enact time (`/usr/bin/chromium` else `/usr/bin/chromium-browser`); absent → skip with logged warning (level-triggered retry). `LearnFs` trait mock for tests; `RealLearnFs` under `real`.

- [ ] Failing tests (mock fs): clause with khan → one desktop file with exact Exec line; clause removed → file removed; idempotent second reconcile → no actions; native apps produce NO launcher; paused clause → all launchers removed.
- [ ] Implement; PASS; commit `feat(charterd): pinned learning-app launchers`.

### Task B3: loop + status wiring

**Files:** Modify `linux/crates/charterd/src/runtime.rs` (attribution call + `multi.tick(active, now, elapsed, bucket)`), `status_emit.rs`/`dbus_service.rs` (TimeLeft snapshot + STATUS relay payload gain `learningToday` secs), `charter-cli` status output ("Learning today: 32m").

- [ ] Failing test: snapshot serialisation includes `learningToday` when learning clause active, absent otherwise (backward-compatible).
- [ ] Wire; full linux gates green; commit `feat(charterd): learning bucket in loop + status`.

### Task B4: app inventory publish

**Files:** Create `linux/crates/charterd/src/app_inventory.rs`; modify `runtime.rs` (slow tick), STATUS payload (`installedApps: [{id, label, exec, flatpak}]`, deduped, root-owned entries only).

- [ ] Failing test: parse fixture .desktop dir → inventory excludes NoDisplay=true, dedups by exec, ignores user-writable dirs.
- [ ] Implement (`/usr/share/applications` + flatpak system dir scan); PASS; commit `feat(charterd): publish installed-app inventory`.

### Task C1: catalogue seed (verified) + wire types

**Files:** Create `apps/charter-app/src/data/learning_catalogue.ts`; modify `src/wire/types.ts` (`LearningClause`, `LearningApp` mirroring A2 camelCase), `src/domain/types.ts`.

- [ ] VERIFY Khan domain closure live (load site, capture required origins; expect khanacademy.org, kastatic.org, kasandbox.org, cdn.kastatic.org + auth origin) — record findings in the catalogue entry as comments with date.
- [ ] Vitest: catalogue entries satisfy clause validation shape (site⇒domains+url non-empty).
- [ ] Commit `feat(app): learning catalogue seed (verified closures)`.

### Task C2: MyCharter Learning screen

**Files:** Create `apps/charter-app/src/screens/Learning.tsx`; modify `screens/Limits.tsx` nav or child screen (entry point + "Learning today" display from STATUS `learningToday`/`installedApps`), store publish path (same signed-clause rail as existing clauses, kind 31113 body `learning`).

- [ ] Vitest (jsdom, app's own runner): renders catalogue + inventory lists; toggling Khan produces a clause payload matching a golden JSON; cap input serialises `capMinutes`; untick → app removed from payload.
- [ ] Implement in app design language; `npm run typecheck && npm test` in `apps/charter-app` green; commit `feat(app): Learning section (choose time-free apps, optional cap)`.

### Task D1: console backup section + contract doc

> **STATUS 2026-07-17:** contract doc DONE (shipped with D2); the console
> device-only learning editor + `charter-settings --set-learning` are a
> FAST-FOLLOW (guardian-phone path shipped complete; device-only families
> get learning next session).

**Files:** Modify `apps/charter-console/ui/app.html` + `src` bridge (Learning list writes the device-only clause via `charter-settings --set-learning <user> <json>` new headless flag), `linux/crates/charterd/src/bin/charter-settings.rs`, `spec/contract.md` (clause table + example), README touch-ups.

- [ ] Test: `charter-settings --set-learning` round-trips the JSON into the limits dir (existing device-only pattern); contract example validates against A2 vectors.
- [ ] Commit `feat(console): learning apps backup editor + contract docs`.

### Task D2: release 0.3.0

- [ ] All gates (core, linux, root vitest, app) green; bump `linux/Cargo.toml` + `apps/charter-console/Cargo.toml` to 0.3.0; postinst unchanged; `Suggests: chromium` added to control (NOT Depends — feature is optional).
- [ ] `cargo run -p xtask -- deb`; verify contents (new files present, version 0.3.0); copy versioned + `charter-latest.deb` into `apps/charter-app/public/`, drop 0.2.1 per convention.
- [ ] PR → merge → deploy → live checksum check; hand decented the hardware round (Khan 10 min: leisure unchanged; Minecraft: drains; ExampleGame once registered: free; forged-launcher spot-check).

## Self-Review

- Spec coverage: buckets (A1/A3), clause (A2), attribution (B1), pinned launchers (B2), status (B3), inventory (B4), catalogue+UI (C1/C2), console+docs (D1), ship (D2). decented's asks all land: Khan separate bucket ✓, native app (ExampleGame) ✓, PWA-escape closed by resolver pin ✓, Firefox stays general browser (no change needed — absence of match ⇒ Screen) ✓.
- Types consistent: `Bucket` defined once (schedule), re-exported through spine to charterd; `GrantLearning`/`LearningApp` defined in proto, mirrored camelCase in TS.
- No placeholders: every task carries its tests and exact interfaces; code-level detail intentionally lives at interface precision since executor holds full session context.
