# Named Times Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** One family time model — named groups of apps, each Free / Counted (daily and/or weekly) / On request — unified in the guardian UI, enforced on both platforms, and speakable in asks and gifts.

**Architecture:** Approach A from `docs/superpowers/specs/2026-08-02-named-times-design.md` — the UI unifies now; the wire keeps `learning` (5), `buckets` (10), `apps` (4) underneath. Weekly allowances extend `buckets`; group asks/gifts extend `time.extend`/`gift`; On request = `apps.blocked` + new `askFirst` presentation hint + new `app.open` ask op whose grant is an ordinary `AppHold` re-sign.

**Tech Stack:** Rust (core crates + charterd + JNI), Kotlin (Android ward), TypeScript/React (MyCharter PWA), serde/vitest wire vectors.

## Global Constraints

- **Fail directions are law:** `learning` fails closed-to-Screen; `buckets` fails OPEN (caps nothing); `apps` blocking fails closed. Never invert one.
- **Wire additivity:** existing fields/strings are frozen; additions are `#[serde(default)]` / optional. `deny_unknown_fields` is not used — old consumers ignore new fields.
- **Buckets version rule:** PWA emits `v:1` when every bucket has `dailyMinutes`; `v:2` when any bucket is weekly-only. Device accepts 1 and 2.
- **Week convention:** identical to budget — `week_key_of` walk-back, `weekStart` default `mon`, enforcement tz. Never reimplement.
- **Identity vocabulary:** group members use `appRule.pkg` strings (Android package id / Linux exec path / flatpak id). Exact match in core; fuzzy process match stays in the enforcer.
- **One probe per tick:** a tick credits exactly one bucket (`attribute_tick` stays the single X probe on Linux; `foregroundPackage()` the single probe on Android).
- **Readout sentinel:** DTO "cap unset" is `-1` with `serde(default = -1)`, never 0.
- **Stand-down caps everything;** group extends/gifts sit under it and never touch the device budget.
- **Bucket time also charges the day** (existing `tick_attributed` behaviour — do not change).
- **Copy lexicon:** guardian/ward/wardship; warm child-facing copy. UI name: "Named times".
- **Gates:** Linux `cd linux && cargo fmt --check && cargo clippy && cargo test` + real-build; core crates likewise from their workspace. Kotlin/JNI are NOT in CI — build/verify locally (`source ~/Android/env.sh`, `android/scripts/build-jni.sh`). Never hand-bump app versions on main (bot-cut); `.deb`/APK publish via `scripts/publish-deb.sh` / `android/scripts/publish-apk.sh` + `scripts/sync-front-door-downloads.sh`.
- **No instrumented `@Before` grant may paper over a production grant** (audit vs `WardenController`).
- **Sequential execution only** — one file-mutating agent at a time, commit between tasks (shared-tree hazard).

---

## Phase 1 — core crates

### Task 1: Weekly allowances in the buckets clause

**Files:**
- Modify: `core/crates/charter-schedule/src/clause.rs:171-269` (AppBucket, GrantBuckets, consts)
- Modify: `core/crates/charter-schedule/src/buckets.rs` (all three pure fns + tests)

**Interfaces:**
- Produces: `AppBucket { id, label, apps, daily_minutes: Option<u16>, weekly_minutes: Option<u16> }`; `GrantBuckets` + `week_start: Option<WeekStart>`; `BUCKETS_VERSION_WEEKLY: u32 = 2`; `MAX_BUCKET_WEEK_MINUTES: u16 = 10080`; `pub struct BucketSpent { pub day_secs: u64, pub week_secs: u64 }`; `bucket_for_app(&GrantBuckets, &str) -> Option<&AppBucket>` (unchanged); `spent_bucket_apps<F: Fn(&str) -> BucketSpent>(&GrantBuckets, F) -> Vec<String>`; `bucket_day_remaining_secs(&AppBucket, u64) -> Option<u64>`; `bucket_week_remaining_secs(&AppBucket, u64) -> Option<u64>`; `bucket_remaining_secs(&AppBucket, BucketSpent) -> u64` (binding min).

- [ ] **Step 1: Write failing tests** in `buckets.rs` (extend the existing module; update `play()` helper to `daily_minutes: Some(minutes), weekly_minutes: None`):

```rust
fn play_weekly(daily: Option<u16>, weekly: Option<u16>) -> GrantBuckets {
    GrantBuckets {
        v: if daily.is_none() { 2 } else { 1 },
        buckets: vec![AppBucket {
            id: "play".into(),
            label: "Play".into(),
            apps: vec!["com.mojang.Minecraft".into()],
            daily_minutes: daily,
            weekly_minutes: weekly,
        }],
        paused: None,
        week_start: None,
        tz: "Europe/London".into(),
        issued_at: 1,
    }
}

#[test]
fn weekly_cap_bites_even_with_daily_headroom() {
    let b = play_weekly(Some(60), Some(300));
    // 20m today but the week is spent: the group closes.
    let spent = |_: &str| BucketSpent { day_secs: 20 * 60, week_secs: 300 * 60 };
    assert_eq!(spent_bucket_apps(&b, spent), vec!["com.mojang.Minecraft"]);
}

#[test]
fn weekly_only_group_is_valid_at_v2_and_enforced() {
    let b = play_weekly(None, Some(300));
    assert!(b.is_valid());
    assert!(spent_bucket_apps(&b, |_| BucketSpent { day_secs: 0, week_secs: 299 * 60 }).is_empty());
    assert_eq!(spent_bucket_apps(&b, |_| BucketSpent { day_secs: 0, week_secs: 300 * 60 }).len(), 1);
}

#[test]
fn a_capless_bucket_is_invalid() {
    assert!(!play_weekly(None, None).is_valid());
}

#[test]
fn v1_wire_from_old_mycharter_still_reads() {
    // The pinned pre-weekly bytes must keep working verbatim.
    let body: GrantBuckets = serde_json::from_str(
        r#"{"v":1,"buckets":[{"id":"play","label":"Play","apps":["mc"],"dailyMinutes":60}],"tz":"Europe/London","issuedAt":1700}"#,
    ).unwrap();
    assert!(body.is_valid());
    assert_eq!(body.buckets[0].daily_minutes, Some(60));
    assert_eq!(body.buckets[0].weekly_minutes, None);
}

#[test]
fn binding_remaining_is_the_min_axis() {
    let b = play_weekly(Some(60), Some(300));
    let bucket = &b.buckets[0];
    assert_eq!(bucket_day_remaining_secs(bucket, 15 * 60), Some(45 * 60));
    assert_eq!(bucket_week_remaining_secs(bucket, 290 * 60), Some(10 * 60));
    assert_eq!(
        bucket_remaining_secs(bucket, BucketSpent { day_secs: 15 * 60, week_secs: 290 * 60 }),
        10 * 60
    );
}
```

- [ ] **Step 2:** `cd core && cargo test -p charter-schedule buckets` — expect compile FAIL (`daily_minutes` type, `BucketSpent` missing).
- [ ] **Step 3: Implement.** In `clause.rs`: `daily_minutes: Option<u16>` + `weekly_minutes: Option<u16>` (both `#[serde(default, skip_serializing_if = "Option::is_none")]`), `week_start: Option<WeekStart>` on `GrantBuckets` (same serde), consts `BUCKETS_VERSION_WEEKLY = 2`, `MAX_BUCKET_WEEK_MINUTES: u16 = 10080`. `is_valid`: accept `self.v == BUCKETS_VERSION || self.v == BUCKETS_VERSION_WEEKLY`; per bucket require `daily_minutes.is_some() || weekly_minutes.is_some()`; daily in `1..=MAX_BUCKET_MINUTES` when set; weekly in `1..=MAX_BUCKET_WEEK_MINUTES` when set. In `buckets.rs`: add `BucketSpent`; `spent_bucket_apps` blocks when ANY set axis is spent:

```rust
pub struct BucketSpent { pub day_secs: u64, pub week_secs: u64 }

pub fn spent_bucket_apps<F>(body: &GrantBuckets, spent: F) -> Vec<String>
where F: Fn(&str) -> BucketSpent {
    if body.is_paused() || !body.is_valid() { return Vec::new(); }
    let mut out = Vec::new();
    for b in &body.buckets {
        let s = spent(&b.id);
        let day_gone = b.daily_minutes.is_some_and(|m| s.day_secs >= u64::from(m) * 60);
        let week_gone = b.weekly_minutes.is_some_and(|m| s.week_secs >= u64::from(m) * 60);
        if day_gone || week_gone { out.extend(b.apps.iter().cloned()); }
    }
    out.sort(); out.dedup(); out
}
```

  Per-axis remainders return `None` when that cap is unset; `bucket_remaining_secs` is the min of set axes. Fix every existing test/caller in the crate (`enforcer.rs`, `multi_child.rs` if they touch `daily_minutes`) to the `Option` shape.
- [ ] **Step 4:** `cd core && cargo test -p charter-schedule` — PASS, including untouched invariant tests.
- [ ] **Step 5:** `git add -A core/crates/charter-schedule && git commit -m "feat(buckets): weekly allowances — daily and/or weekly caps per group"`

### Task 2: Week meter in the usage ledger

**Files:**
- Modify: `core/crates/charter-schedule/src/usage.rs` (~60-66 map, ~117-137 roll, ~174-195 credit/accessors)
- Modify: `core/crates/charter-spine/src/multi_child.rs:223` area (`tick_attributed` pass-through, if it copies the day figure)

**Interfaces:**
- Produces: `UsageLedger::credit_app_bucket(now, id, secs)` now also credits `bucket_week_secs`; new accessor `bucket_week_secs(&self, now, id) -> u64` beside existing `bucket_today_secs(...)`; both zeroed on their respective rolls; both survive snapshot (`#[serde(default)]` map).

- [ ] **Step 1: Failing test** in `usage.rs`:

```rust
#[test]
fn bucket_week_meter_survives_the_day_roll_and_dies_at_the_week_roll() {
    // NOON is the existing Mon-noon fixture; week_start Mon.
    let mut u = ledger_at(NOON); // use the file's existing constructor helper
    u.credit_app_bucket(NOON, "play", 1200);
    let tue = NOON + 24 * 3600;
    assert_eq!(u.bucket_today_secs(tue, "play"), 0);      // day rolled
    assert_eq!(u.bucket_week_secs(tue, "play"), 1200);     // week persists
    u.credit_app_bucket(tue, "play", 600);
    assert_eq!(u.bucket_week_secs(tue, "play"), 1800);
    let next_mon = NOON + 7 * 24 * 3600;
    assert_eq!(u.bucket_week_secs(next_mon, "play"), 0);   // week rolled
}
```

  (Match the file's actual test-constructor idiom; if `ledger_at` doesn't exist, use whatever the neighbouring `bucket_today_secs` tests use.)
- [ ] **Step 2:** `cargo test -p charter-schedule usage` — FAIL (`bucket_week_secs` missing).
- [ ] **Step 3: Implement:** `#[serde(default)] bucket_week_secs: BTreeMap<String, u64>`; `roll()` clears it in the week-change branch (beside `used_week_secs = 0`), and the day branch leaves it alone; `credit_app_bucket` adds to both maps; accessor mirrors `bucket_today_secs` (roll-checked, 0 for missing id).
- [ ] **Step 4:** `cargo test -p charter-schedule && cargo test -p charter-spine` — PASS.
- [ ] **Step 5:** Commit `feat(usage): week-keyed bucket meters beside the day meters`.

### Task 3: Per-group extension pool (asks + gifts land somewhere)

**Files:**
- Modify: `core/crates/charter-schedule/src/extension.rs`
- Modify: `core/crates/charter-proto/src/clause.rs:902-955` (`GiftBody`)

**Interfaces:**
- Produces: `ExtensionLedger::apply_bucket(now, req_id, minutes, bucket_id) -> bool` (idempotent via the SHARED `applied` list, today-only, cleared on day roll); `bucket_extra_secs(now, bucket_id) -> u64`; `GiftBody.group_id: Option<String>` (`#[serde(default, skip_serializing_if = "Option::is_none")]`, camelCase `groupId`, stays `v:1`). Semantics for consumers (Tasks 6/8): effective spent per axis = `spent.saturating_sub(bucket_extra_secs)` — extra lifts BOTH walls today.

- [ ] **Step 1: Failing tests** in `extension.rs`:

```rust
#[test]
fn bucket_pool_is_per_group_idempotent_and_today_only() {
    let mut l = ExtensionLedger::new(TZ, NOON);
    assert!(l.apply_bucket(NOON, "req-p", 30, "play"));
    assert!(!l.apply_bucket(NOON, "req-p", 30, "play")); // replay
    l.apply_bucket(NOON, "req-s", 15, "social");
    assert_eq!(l.bucket_extra_secs(NOON, "play"), 30 * 60);
    assert_eq!(l.bucket_extra_secs(NOON, "social"), 15 * 60);
    assert_eq!(l.bucket_extra_secs(NOON, "reading"), 0);
    assert_eq!(l.bucket_extra_secs(NOON + 24 * 3600, "play"), 0); // day roll
}

#[test]
fn one_reqid_cannot_be_both_device_and_bucket() {
    let mut l = ExtensionLedger::new(TZ, NOON);
    assert!(l.apply(NOON, "req-x", 10, Dimension::Budget));
    assert!(!l.apply_bucket(NOON, "req-x", 10, "play"));
}

#[test]
fn bucket_pool_survives_snapshot() {
    let mut l = ExtensionLedger::new(TZ, NOON);
    l.apply_bucket(NOON, "req-p", 30, "play");
    let r = ExtensionLedger::from_snapshot(&l.snapshot()).unwrap();
    assert_eq!(r.bucket_extra_secs(NOON, "play"), 30 * 60);
}
```

  And in proto's gift tests: `groupId` round-trips, absent on old wire, and `minutes_now` is unaffected by its presence.
- [ ] **Step 2:** Run both crates' tests — FAIL.
- [ ] **Step 3: Implement:** `#[serde(default)] bucket_extra: std::collections::BTreeMap<String, u64>` on `ExtensionLedger`; cleared in `roll()`; `apply_bucket` mirrors `apply` (shared `applied` dedupe); accessor day-key-checked like `budget_extra_secs`. `GiftBody` gains the optional field only — validation untouched.
- [ ] **Step 4:** `cargo test -p charter-schedule -p charter-proto` — PASS (incl. `pre_burn`-style old-snapshot restore still green thanks to `serde(default)`).
- [ ] **Step 5:** Commit `feat(extension): per-group additive pools; gift learns groupId`.

### Task 4: Ask vocabulary — `bucket` limit, `app.open` op, `askFirst` hint, STATUS groups

**Files:**
- Modify: `core/crates/charter-proto/src/op.rs` (LimitHit + OpType)
- Modify: `core/crates/charter-proto/src/params.rs:114-155` (extend params; add AppOpen params)
- Modify: `core/crates/charter-proto/src/apps.rs:67` area (`GrantApps.ask_first`)
- Modify: `core/crates/charter-proto/src/status.rs` (~63, ~124) (`groups`)

**Interfaces:**
- Produces: `LimitHit::Bucket` (wire `"bucket"`); `OpType::AppOpen` (wire `"app.open"`); `TimeExtendRequestParams.bucket_id: Option<String>` and `TimeExtendGrantParams.bucket_id: Option<String>` (camelCase `bucketId`, echoed verbatim in grants); `AppOpenRequestParams { pkg: String, label: Option<String>, minutes_requested: Option<u16>, reason: Option<String> }` with the same `MAX_REASON_LEN`/`MAX_EXTEND_MINUTES` bounds; `GrantApps.ask_first: Vec<String>` (`#[serde(default)]`, camelCase `askFirst`, presentation-only — enforcement reads `blocked` exactly as before); STATUS `groups: Option<Vec<GroupSpent>>` where `GroupSpent { id: String, day_secs: u64, week_secs: u64 }` (camelCase, list capped at `MAX_BUCKETS` on parse).

- [ ] **Step 1: Failing tests:** wire-string pins (`"bucket"`, `"app.open"`), `bucketId` round-trip + absent-field back-compat on both extend params, `AppOpenRequestParams` serde round-trip + bounds rejection (reason > 280, minutes > 1440), `askFirst` default-empty on old bytes, STATUS `groups` round-trip + cap. Follow the exact patterns of the neighbouring tests in each file (op.rs pins, params bounds tests, `learning_clause_vectors.rs` style).
- [ ] **Step 2:** `cargo test -p charter-proto` — FAIL.
- [ ] **Step 3: Implement** the four additions. `LimitHit::Bucket` slots into the existing lowercase serde. Note in a doc comment on `askFirst`: "hint only; every listed pkg MUST also be in `blocked` — old wards then still enforce (fail closed, no ask affordance)."
- [ ] **Step 4:** `cargo test -p charter-proto` and the D-Bus lockstep check: extend `charter_ipc::Op` with `app.open` so op.rs's pin test and the ipc crate stay matched (`cd linux && cargo test -p charter-ipc`).
- [ ] **Step 5:** Commit `feat(proto): bucket asks, app.open op, askFirst hint, STATUS group progress`.

### Task 5: Contract freeze

**Files:**
- Modify: `spec/contract.md` (§buckets 306-368, §gift 367-410, §time.extend 801-818, §apps, §STATUS, parity matrix ~787)

- [ ] **Step 1:** Document every Task 1-4 addition with wire examples (a v:2 weekly-only clause; a `bucketId` extend round trip; an `app.open` ask; `askFirst` beside `blocked`; STATUS `groups`), the v:1/v:2 emission rule, the "extra lifts both walls today" semantics, and flip nothing in the parity matrix yet (Android buckets flips in Task 8's row).
- [ ] **Step 2:** `grep -n "weeklyMinutes\|bucketId\|app.open\|askFirst\|groups" spec/contract.md` — every addition present.
- [ ] **Step 3:** Commit `docs(contract): named-times wire additions`.

---

## Phase 2 — Linux warden

### Task 6: Enforce weekly + group pools; report groups in STATUS

**Files:**
- Modify: `linux/crates/charterd/src/runtime.rs` (~1423-1431 gift routing, ~1476-1560 bucket tick/views, ~1674-1685 spent extend)
- Modify: `linux/crates/charter-ipc/src/dto.rs:79-94` (`BucketView`)
- Modify: `linux/crates/charterd/src/state_file.rs:40` (carries the new fields)

**Interfaces:**
- Consumes: Task 1 `BucketSpent`/`spent_bucket_apps`, Task 2 `bucket_week_secs`, Task 3 `apply_bucket`/`bucket_extra_secs` + `GiftBody.group_id`.
- Produces: `BucketView` gains `week_limit_seconds: i64` and `week_remaining_seconds: i64` (both `serde(default = "neg_one")` = `-1` unset — NEVER 0); STATUS emission includes `groups` built from the two meters; the spent closure becomes:

```rust
let spent = |id: &str| {
    let extra = ext.bucket_extra_secs(now, id);
    charter_schedule::buckets::BucketSpent {
        day_secs: usage.bucket_today_secs(now, id).saturating_sub(extra),
        week_secs: usage.bucket_week_secs(now, id).saturating_sub(extra),
    }
};
```

- [ ] **Step 1: Failing tests:** charterd has pure-logic tests around the spent/view assembly — add: weekly wall closes the group's apps; a `bucket_extra` grant re-opens both walls today; gift with `groupId` routes to `apply_bucket` (namespaced `gift:{id}`) and WITHOUT `groupId` routes exactly as before (`Dimension` unchanged); paused set still reports `capped: false` views.
- [ ] **Step 2:** `cd linux && cargo test -p charterd` — FAIL.
- [ ] **Step 3: Implement:** thread the closure above through `spent_bucket_apps` at the ~1674 enforcement point (unchanged sweep downstream); extend the ~1541 view builder with the week axis (`-1` when `weekly_minutes` unset) and the extra-adjusted remainders; gift enactment at ~1423: `if let Some(gid) = body.group_id { ledger.apply_bucket(now, &format!("gift:{}", body.id), mins, &gid) } else { existing }`; time.extend grant intake likewise routes `limitHit == bucket` + `bucketId` to `apply_bucket` keyed by reqId; STATUS builder emits `groups` for every bucket id in force.
- [ ] **Step 4:** `cd linux && cargo fmt --check && cargo clippy && cargo test` — PASS. Existing `a_gift_cannot_lift_a_stand_down`-class invariants untouched and green.
- [ ] **Step 5:** Commit `feat(charterd): weekly group walls, group gifts/extends, STATUS group progress`.

### Task 7: Ward surfaces — group asks and Ask-to-open

**Files:**
- Modify: `linux/crates/charter-tray/src/menu.rs`, `model.rs`; `apps/charter-tray-xapp/src/render.rs`; `apps/charter-console/src/main.rs`
- Modify: `linux/crates/charterd/src/state_file.rs` + `linux/crates/charter-ipc/src/dto.rs` (askFirst list into the per-uid state)

**Interfaces:**
- Consumes: Task 6 `BucketView` week fields; Task 4 `askFirst`, `AppOpenRequestParams`, `LimitHit::Bucket`.
- Produces: tray/console render per-group rows "Play — 45m left today · 2h10 this week"; a spent group's row offers **Ask for more** → brokered `time.extend` with `limit_hit: Bucket, bucket_id`; an **Ask to open** list of `askFirst` apps → brokered `app.open { pkg, label }`. Both ride the existing pre-lock-ask broker path and answer-notification plumbing; Decision echo renders allow/deny.

- [ ] **Step 1: Failing tests** for the pure render/model layer (the tray has model tests; the xapp has the pixel-verdict harness for manual verify): menu model shows week remainder only when `!= -1`; spent group exposes the ask action with the right params; askFirst entries render "Ask to open" not "Blocked".
- [ ] **Step 2:** `cd linux && cargo test` — FAIL, then implement, then PASS (fmt/clippy too).
- [ ] **Step 3:** Manual: `dbus-run-session` tray probe + console launch on this laptop's charterd; verify rows and that an ask round-trips to the relay (guardian side arrives in Task 10).
- [ ] **Step 4:** Commit `feat(linux-ward): group rows, group asks, ask-to-open`.

---

## Phase 3 — Android ward (the parity headline)

### Task 8: Meter + enforce buckets on Android

**Files:**
- Modify: `android/jni/src/warden.rs` (clause 10 read, tick attribution, suspend fold; `credit_bucket` seam at ~1903; `gift_now` at ~488-500)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt:310-340` (`tickAndApply`)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt` (suspend-set assembly already generic)

**Interfaces:**
- Consumes: Tasks 1-3 core APIs (same crates, via JNI).
- Produces: per tick — `foregroundPackage()` → `bucket_for_app` → `credit_app_bucket` (day+week); `spent_bucket_apps` (with the same extra-adjusted closure as Task 6) folded into the suspend set beside the `apps` clause packages; `gift_now` honours `group_id`; extend-grant intake routes `bucket` dimension; JNI exposes `bucketViewsJson()` — same shape as Linux `BucketView` incl. `-1` sentinels — for the mirror. Rust-side logic tests live in `warden.rs`'s existing test module; Kotlin stays a thin pipe.

- [ ] **Step 1: Failing Rust tests** in `warden.rs` (runs in plain `cargo test` on host): a tick with a bucketed foreground pkg credits the meter; at the daily cap the pkg joins the suspend list; weekly wall same; a group gift re-opens today; a non-bucketed pkg credits nothing and never suspends; paused/malformed clause suspends nothing (fail open).
- [ ] **Step 2:** `cd android/jni && cargo test` — FAIL, implement, PASS.
- [ ] **Step 3: Kotlin wiring:** in `tickAndApply`, pass the already-probed foreground package into the warden tick call (extend the existing JNI signature rather than adding a second probe); merge the returned spent-set into `appSuspendSet` before `AppGateOps.reconcile`. No new permissions.
- [ ] **Step 4:** `android/scripts/build-jni.sh` (release! — JNI stale-lib trap: verify the staged `.so` is fresh and the APK has 0 `wire_test_relay` strings, ~27 MB) + `./gradlew testDebugUnitTest` locally.
- [ ] **Step 5:** Commit `feat(android): buckets enforced — meter, weekly walls, suspend-on-spend, group gifts`.

### Task 9: Ward mirror + asks on the phone

**Files:**
- Modify: the ward status/mirror screen (D8 mirror; status block + time-left widget sources) + ask entry points used by the existing time-extend flow
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/AppHoldNotices.kt` neighbourhood for on-request rendering

**Interfaces:**
- Consumes: Task 8 `bucketViewsJson()`; Task 4 op/params.
- Produces: mirror rows per group ("Play — 45m of 1h today · 2h of 5h this week"), **Ask for more** on a spent group (brokered `time.extend` bucket ask), **Ask to open** rows for `askFirst` packages (brokered `app.open`), Decision echo surfaced the way ask outcomes already render (render() rebuilds on onResume — the gift-time outcome-staleness lesson: never capture the view).

- [ ] **Step 1:** Unit tests for the row view-model mapping (JSON → rows, `-1` handling, spent flag) — FAIL → implement → PASS.
- [ ] **Step 2:** Build the APK, adb-install on the attached test phone (dev flow), eyeball the mirror.
- [ ] **Step 3:** Commit `feat(android): ward mirror group rows + group/on-request asks`.

---

## Phase 4 — MyCharter PWA

### Task 10: Wire + domain layer

**Files:**
- Modify: `apps/charter-app/src/wire/types.ts:36-48,296-306,368-375,427-446`; `wire/clause.ts:82-103,215-262,338`; `wire/request.ts:66,110-140`; `wire/grant.ts:98-134`; `wire/status.ts:52-53,99-115`; `domain/types.ts:60-66,146-163,303-335`; `domain/wardenSupport.ts:73-74`
- Test: the adjacent `*.test.ts` vector suites (`bucketsClause.test.ts` etc.)

**Interfaces:**
- Consumes: Task 4/5 wire shapes.
- Produces: `AppBucket` TS with optional `dailyMinutes`/`weeklyMinutes` (≥1 required, daily ≤1440, weekly ≤10080); `bucketsToGrant` emits **v:1 iff every bucket has dailyMinutes, else v:2** (unit-pinned both ways, and the Rust `wire_agreement` bytes stay byte-identical for the v:1 case); `GrantGift.groupId?`; extend request/grant `bucketId?` + `limitHit: 'bucket'` in the whitelist; `app.open` parse in `subscribeRequests`/`request.ts` (reqId/nonce rules identical to `time.extend`); `askFirst` emitted alongside `blocked` (invariant test: `askFirst ⊆ blocked`); STATUS `groups` parsed + capped; `wardenSupport.buckets` android flips from `NEVER` to `versionCode >= <Task 8's shipping code>`, plus a `bucketsWeekly` capability keyed on the new Linux/Android versions (weekly-only ⇒ parity note on stale wards).

- [ ] **Step 1:** Failing vitest vectors for every shape above (byte-pin the v:1/v:2 emissions the way `bucketsClause.test.ts` already pins v:1). Run `npx vitest run` in `apps/charter-app` — FAIL.
- [ ] **Step 2:** Implement; PASS; the Rust `wire_agreement` suite in `buckets.rs` gets the v:2 twin bytes added (cross-stack pin both directions).
- [ ] **Step 3:** Commit `feat(mycharter-wire): weekly buckets v-rule, group gifts/asks, app.open, askFirst, group progress`.

### Task 11: The Named times editor

**Files:**
- Modify: `apps/charter-app/src/screens/Limits.tsx` (replace the Learning section + `BucketsEditor` ~1979-2158 + section registry ~2917-2941 with one section `id: "named-times"`)
- Create: `apps/charter-app/src/screens/NamedTimes.tsx` (the section component — Limits.tsx is already unwieldy; new code goes in its own file)
- Create: `apps/charter-app/src/domain/namedTimes.ts` (pure compile/decompile + move logic)
- Test: `apps/charter-app/src/domain/namedTimes.test.ts`

**Interfaces:**
- Consumes: Task 10 wire fns; `mergedApps`/`appsFor` from `domain/deviceApps.ts`; guardian-side persistence used by `deviceOverrides` for Free-group names.
- Produces (pure, fully tested in `namedTimes.ts`):

```ts
type GroupPolicy = 'free' | 'counted' | 'onRequest';
interface NamedGroup { id: string; label: string; apps: string[]; policy: GroupPolicy;
                       dailyMinutes?: number; weeklyMinutes?: number }
// Compile the group list into the three clause payload fragments one save signs:
function groupsToClauses(groups: NamedGroup[], prior: PriorClauseState): {
  learning: LearningFragment;   // union of free groups' apps (+ shared capMinutes passthrough)
  buckets: BucketsFragment;     // counted groups, v-rule per Task 10
  apps: { blockedAdd: string[]; askFirst: string[] };  // on-request ⇒ BOTH lists
  freeGroupNames: Record<string, string>;              // guardian-side only, never wire
}
function decompose(clauses: PriorClauseState, freeGroupNames: Record<string,string>): NamedGroup[];
function moveApp(groups: NamedGroup[], pkg: string, toGroupId: string): { groups: NamedGroup[]; movedFrom?: string };
```

  UI behaviour: policy segment Free | Counted | On request per card; Counted shows two steppers (*per day*, *per week*, 15-min steps, each clearable, ≥1 enforced with the existing `bucketsNameError`-style guard); create flow offers suggestion chips **Play, Learning, Social, Creative** (plain editable text, no apps attached); `moveApp` semantics with an inline "moved from Play" undo; save bar untouched (one save signs every touched dimension — and `dirty` may be true with nothing nameable after a move, the double-tabs lesson); parity notes per device (Android needs new APK; weekly-only needs updated wards; Free names are guardian-side).

- [ ] **Step 1:** Failing tests for `groupsToClauses` (free/counted/on-request compile, askFirst ⊆ blocked, v-rule passthrough, decompose∘compile round-trip, moveApp uniqueness) — `npx vitest run` FAIL → implement → PASS.
- [ ] **Step 2:** Build the section UI in `NamedTimes.tsx`; wire into Limits' section registry; delete the two old sections; keep the collapsed-row + tab conventions (MyCharter IA memory: the save bar must never move inside a tab).
- [ ] **Step 3:** `npx vitest run && npm run build` in `apps/charter-app` — PASS. Playwright smoke via the local dev server: create a Counted group with weekly, a Free group, an On-request app; save; inspect the signed clause payloads in the console/network.
- [ ] **Step 4:** Commit `feat(mycharter): Named times — one editor for free/counted/on-request groups`.

### Task 12: The conversation — asks inbox, GiveTime, progress

**Files:**
- Modify: the asks inbox component (renders `ChildRequest`), `apps/charter-app/src/components/GiveTime.tsx:45`, the ward Activity/status view that shows `learningTodaySecs`
- Test: adjacent component/domain tests

**Interfaces:**
- Consumes: Task 10 parsed `app.open` + bucket-extend requests, STATUS `groups`; Task 11 group labels (for id → name).
- Produces: a bucket extend renders "More **Play** time?" with remaining context and grant buttons that echo `bucketId`; an `app.open` renders "Open **Minecraft**?" with one-tap windows **30m / 1h / Rest of today** that sign an `AppHold { pkg, Allowed, untilUnix }` via the existing hold flow (absolute instants, guardian-clock trap noted) + a Decision echo; deny sends Decision deny; GiveTime gains an optional group picker (default: whole day, exactly today's behaviour); Limits/Activity show "Play: 45m of 60m today · 2h10 of 5h this week" from STATUS `groups` joined to bucket caps.

- [ ] **Step 1:** Failing tests: request→card mapping for both new ask kinds; grant payload echoes `bucketId`; app.open approval produces a hold with `untilUnix` in the child's tz for "Rest of today"; GiveTime with a group emits `groupId`. FAIL → implement → PASS.
- [ ] **Step 2:** `npx vitest run && npm run build` — PASS; Playwright smoke of both cards.
- [ ] **Step 3:** Commit `feat(mycharter): group-aware asks, holds-from-asks, give-to-group, group progress`.

---

## Phase 5 — ship + hardware verify

### Task 13: Releases

- [ ] **Step 1:** Full gates: `cd core && cargo fmt --check && cargo clippy && cargo test`; same in `linux/` + real-build; `android/scripts/build-jni.sh` + unit tests; `apps/charter-app` vitest + build. All green before any publish.
- [ ] **Step 2:** Push main (bot cuts app versions — never hand-bump). PWA deploys on push.
- [ ] **Step 3:** `scripts/publish-deb.sh` (bump `linux/Cargo.toml` first) and `android/scripts/publish-apk.sh`; then `scripts/sync-front-door-downloads.sh` + push `charter-you`; verify `/download.html` serves the new `?v=` stamped files.
- [ ] **Step 4:** Update the roadmap page (per-device-rules lesson: it drifts unless updated with the feature).

### Task 14: Hardware verification (attached phone + this laptop)

- [ ] **Step 1 (Linux, self-serve):** on this laptop's charterd: author "Play 15m/day · 30m/week" over a real desktop app + one On-request app from the PWA; burn the daily wall (tick clock or short cap), watch the sweep close only Play's apps; Ask-for-more from the tray → grant in PWA → re-opens; Ask-to-open → 30m hold → app runs → hold lapses.
- [ ] **Step 2 (Android, adb, self-serve):** install the published APK on the attached phone (published, for signature continuity); same clause; verify suspend-on-spend of the bucketed package, mirror rows, bucket ask round trip, gift-to-group, and that the week meter survives a forced day roll (`adb shell date` is blocked on a DO device — instead set a 2-minute daily cap and a 4-minute weekly cap and burn through both).
- [ ] **Step 3:** Record outcomes in the memo; anything failing goes back to its task. Publish a checks page with copy-paste commands ONLY if a step needs decented's hands (target: none — the phone is attached).

## Self-review notes (resolved inline)

- Spec §2.2 STATUS groups → Task 4+6+10+12. §2.5 grant-is-a-hold → Task 12 (no new grant object — verified against spec). §2.6 free-name persistence → Task 11 `freeGroupNames`. §3.3 invariants → Global Constraints + Task 6 tests. Weekly-only old-ward fail-open → Task 1 v-rule + Task 10 parity note. All spec sections covered; no placeholders remain; interface names checked consistent across tasks (`BucketSpent`, `apply_bucket`, `bucket_extra_secs`, `bucketViewsJson`, `groupsToClauses`).
