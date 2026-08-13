# Honest Attribution Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Attribute the game rather than the launcher (`cmdline:` identities), and make time Charter cannot identify visible instead of silent.

**Architecture:** Spec `docs/superpowers/specs/2026-08-03-honest-attribution-design.md`. A third identity form inside the existing bare-string vocabulary (no wire-shape change), matched identically by the meter and the kill sweep; an aggregate unrecognised-time counter on STATUS; user-installed apps in the inventory, flagged; guardian surfaces + a launch-signature table.

**Tech Stack:** Rust (core crates, charterd, android JNI), Kotlin, TypeScript/React.

## Global Constraints

- **No wire-shape change** for the identity form — `cmdline:<substring>` is a bare string in the existing `apps: Vec<String>` / `AppRule.pkg` vocabulary.
- **Metered == stopped:** every matcher change lands on BOTH `focus`/meter and the `/proc` kill sweep, or neither.
- **Enforcement-grade asymmetry:** a user-owned binary may be CAPPED or BLOCKED, never granted FREE (learning) time. `exe_uid` is the test.
- **Fail directions unchanged:** buckets open, learning closed-to-Screen, apps blocked closed. A `cmdline:` entry on a pre-705 ward fails OPEN on a blocklist → guardian-side version gate `cmdlineIdentity` (linux ≥ 705) + parity note.
- **Privacy boundary:** STATUS carries NO per-app usage. Unrecognised time is ONE aggregate number. Never report which unrecognised app, ever.
- **`cmdline:` rules:** ≥ 8 chars after the prefix, case-sensitive, matched per NUL-split token and against the joined line.
- **Ward sees what the guardian sees** (transparency invariant).
- Bot-cut versions: do NOT hand-bump app versions; charterd bumps to 0.7.5 (705) and the ward to 0.6.8 (39) in the release task only.
- Gates: `cd core && cargo fmt --check && cargo clippy --all-targets && cargo test`; `cd linux && ... && cargo test --workspace --features real` (charterd runtime tests need `--features real`) + `cargo build -p charterd --no-default-features --features real`; `cd android/jni && cargo test` + `cd android && ./gradlew testDebugUnitTest` (Kotlin/JNI are NOT in CI); `cd apps/charter-app && npx vitest run && npm run build && npm run lint`.
- Sequential execution only; commit between tasks.

---

### Task 1: The `cmdline:` identity form (core vocabulary)

**Files:** Modify `core/crates/charter-schedule/src/` (wherever `is_flatpak_id` and the identity-vocabulary docs live — grep it), `spec/contract.md` (identity vocabulary section).

**Interfaces produced:** `pub fn is_cmdline_id(pkg: &str) -> bool`; `pub fn cmdline_needle(pkg: &str) -> Option<&str>` (the substring after the prefix, `None` when malformed/too short); `pub const CMDLINE_ID_PREFIX: &str = "cmdline:"`; `pub const MIN_CMDLINE_NEEDLE: usize = 8`. Pure, no IO.

- [ ] **Step 1: failing tests** — `is_cmdline_id("cmdline:net.minecraft.client.main.Main")` true; `is_cmdline_id("/usr/bin/java")` false; `is_cmdline_id("org.x.App")` false; `cmdline_needle` returns the substring; `cmdline_needle("cmdline:short")` → `None` (< 8); `cmdline_needle("cmdline:")` → `None`; a flatpak-looking string is never a cmdline id and vice versa (the three forms are mutually exclusive — pin it).
- [ ] **Step 2:** run, watch fail. **Step 3:** implement. **Step 4:** `cd core && cargo test` green.
- [ ] **Step 5:** document the third form in `spec/contract.md`'s identity vocabulary with the Minecraft worked example, the ≥8 rule, and the explicit note that a pre-705 ward ignores it (blocklist fails open → guardian gates it).
- [ ] **Step 6:** commit `feat(core): cmdline: identity form — name the game, not the launcher`.

### Task 2: charterd matches it on both paths + the enforcement-grade rule

**Files:** `linux/crates/charterd/src/app_rules.rs` (`pkg_matches_process`), `focus.rs` (meter/classify + `bucket_id_for_with`), `runtime.rs` (kill sweep call sites), `ancestry.rs` if the ancestor matcher needs the same treatment.

**Interfaces:** `pkg_matches_process` gains a `cmdline: &[String]` parameter (full NUL-split argv — both call sites already have it: sweep reads it, `FocusedProcess.cmdline` carries it). A `cmdline:` pkg matches iff `cmdline_needle` is a substring of any token or of the joined line. Path/flatpak forms behave EXACTLY as before (pin with the existing tests untouched).

- [ ] **Step 1: failing tests** in `app_rules.rs`/`focus.rs`: a JVM with `["java","-cp","...","net.minecraft.client.main.Main","--gameDir",...]` matches `cmdline:net.minecraft.client.main.Main`; the same JVM does NOT match `/usr/bin/minecraft-launcher`; an unrelated java process does not match; a short/malformed needle matches nothing (fail open); the meter (`bucket_id_for_with`) attributes such a process to its group with NO launcher ancestor present; the sweep's blocked set includes it.
- [ ] **Step 2-4:** fail → implement (thread cmdline through both call sites) → green.
- [ ] **Step 5: the asymmetry.** Test + implement: a process whose `exe_uid` is not root can be capped (bucket) and blocked (apps) but is NEVER classified as learning/free — `focus::classify` must refuse a user-owned binary the free bucket even if the identity matches. Comment WHY (a ward must not be able to make their own time free).
- [ ] **Step 6:** `cd linux && cargo fmt --check && cargo clippy --all-targets && cargo test --workspace --features real` + the real build. Commit `feat(charterd): attribute and stop a game by its command line`.

### Task 3: Unrecognised time + user-installed inventory

**Files:** `linux/crates/charterd/src/app_inventory.rs` (scan dirs + flag), `runtime.rs` (the tick's unrecognised accrual + STATUS), `core/crates/charter-proto/src/status.rs` (`AppRef.userInstalled`, `unrecognisedTodaySecs`), `core/crates/charter-spine/src/status_emit.rs` if it threads the field, `spec/contract.md`.

**Interfaces:** `AppRef { pkg, label, user_installed: Option<bool> }` (additive, `#[serde(default, skip_serializing_if)]`, camelCase `userInstalled`); STATUS gains `unrecognised_today_secs: Option<u64>` (camelCase `unrecognisedTodaySecs`), day-keyed on the same boundary as the other day meters, absent when zero/unknown.

- [ ] **Step 1: failing tests** — inventory includes a `~/.local/share/applications` entry flagged `userInstalled: true` while root-owned entries are unflagged; a `charter-*` or `NoDisplay` user entry is still skipped; the tick accrues unrecognised seconds ONLY when the focused identity is absent from the inventory (an installed-but-ungrouped app accrues NOTHING); the counter is day-keyed and rolls; STATUS carries the aggregate and NEVER any per-app identifier (assert the serialized payload contains no path/pkg of the unrecognised app).
- [ ] **Step 2-4:** fail → implement → green (core + linux gates).
- [ ] **Step 5:** `spec/contract.md`: document both fields, and state the privacy rule explicitly — aggregate only, never per-app, and why.
- [ ] **Step 6:** commit `feat(charterd): count the time Charter can't identify, and show what the ward installed`.

### Task 4: Android tolerance + the M-4 honest clock (both platforms)

**Files:** `android/jni/src/warden.rs` (identity matching + bucket view), `linux/crates/charterd/src/runtime.rs` + `linux/crates/charter-ipc/src/dto.rs` (bucket view), the ward mirror/tray consumers if the display maths lives there.

**Interfaces:** Android: a `cmdline:` identity never matches an Android package and never panics (Android identities are package names; the JVM problem does not exist there) — and `unrecognisedTodaySecs` is simply absent from Android STATUS. M-4: the ward-facing remaining must be computed against **cap + today's extra**, counting down honestly, instead of subtracting extras from spent (which clamps at the base cap and FREEZES — hardware-proven "15m of 15m left" for 18m24s of play).

- [ ] **Step 1: failing tests** — Android: a bucket carrying a `cmdline:` identity meters/suspends nothing and does not crash; the clause still validates. M-4 (BOTH platforms): a 15m bucket + 30m extra reports `remaining` counting down from 45m and reaching 0 only after 45m — assert it MOVES between two ticks in the surplus region (the freeze is what the old test missed).
- [ ] **Step 2-4:** fail → implement → green: `cd android/jni && cargo test`, `cd android && ./gradlew testDebugUnitTest`, `cd linux && cargo test --workspace --features real`.
- [ ] **Step 5:** commit `fix(ward): a gifted minute must move the child's clock; tolerate cmdline identities on Android`.

### Task 5: MyCharter — unrecognised time, user-installed marks, launch signatures

**Files:** `apps/charter-app/src/wire/status.ts` + `src/domain/types.ts` (parse the two new fields), `src/domain/wardenSupport.ts` (`cmdlineIdentity`: linux ≥ 705, android NEVER — it is meaningless there), `src/domain/deviceApps.ts` (carry `userInstalled` through `appsBySection`), `src/screens/NamedTimes.tsx` + `src/screens/Limits.tsx` (marks + the signature attach), the ward day view (Home/Approvals) for the unrecognised line, and a new `src/domain/launchSignatures.ts`.

**Interfaces:** `launchSignatures.ts` exports `LAUNCH_SIGNATURES: {match: (pkg: string) => boolean, cmdlineId: string, label: string, why: string}[]` — ONE entry at launch (Minecraft Java: matches a `minecraft-launcher` basename or the flatpak id, attaches `cmdline:net.minecraft.client.main.Main`); `signatureFor(pkg)`; `withSignature(apps: string[], pkg: string): string[]` (idempotent; never duplicates; never attaches on a device that lacks the capability).

- [ ] **Step 1: failing vitest** — STATUS parse of both fields (absent → undefined, never 0-as-real); `withSignature` idempotent + attaches once; picking Minecraft into a Counted group yields both identities; a ward whose devices are all pre-705 does NOT get the cmdline identity attached and shows the parity note; `appsBySection` carries `userInstalled` per row; the unrecognised line renders only when > 0 and reads honestly.
- [ ] **Step 2-4:** fail → implement → green.
- [ ] **Step 5: copy.** Warm, wardship voice, no accusation: the unrecognised line is "3h 12m Charter didn't recognise" + a one-line explainer; the signature note is "Minecraft runs through Java — Charter will count it however it's started"; user-installed rows read "installed by <ward>". Playwright smoke: two-device seed, screenshot the picker marks and the unrecognised line.
- [ ] **Step 6:** commit `feat(mycharter): unrecognised time, user-installed marks, Minecraft launch signature`.

### Task 6: Release + hardware verification

- [ ] **Step 1:** full gate sweep, all five stacks, explicit per-stack verdicts (do NOT `grep -c` a chained sweep — it hides failures).
- [ ] **Step 2:** bump `linux/Cargo.toml` → 0.7.5 and `android/app/build.gradle.kts` → 0.6.8 (39); `./scripts/publish-deb.sh`; `android/scripts/publish-apk.sh` (verify ~27 MB, 0 `wire_test_relay` strings); commit + push main; `./scripts/sync-front-door-downloads.sh` + push `charter-you`.
- [ ] **Step 3: hardware (this laptop, self-serve).** Author a Counted group holding `cmdline:` for a JVM-launched app; verify metering with no launcher ancestor and that the sweep stops it; verify the unrecognised counter moves for a binary outside the inventory and stays still for an installed-but-ungrouped one.
- [ ] **Step 4: the real gate (needs decented, Robin's paired laptop).** Launch Minecraft three ways — official launcher, renamed binary, direct `java -jar` — and confirm all three land in the same group and all three stop when it is spent. Hand decented the exact commands.
