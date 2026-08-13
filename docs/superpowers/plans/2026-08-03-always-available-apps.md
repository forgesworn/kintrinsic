# Always Available Apps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A guardian can name apps that stay open at any hour — exempt from the schedule and budget locks, never from a stand-down or a malformed charter — with each entry either standing or carrying an absolute expiry.

**Architecture:** A new `alwaysavailable` clause parsed in `charter-proto`, resolved per-tick in the JNI warden into an exempt package set, subtracted from the lock-driven suspend posture in `appSuspendSet`, surfaced as an **Open** row on the LockTask-pinned shade (the path the lifeline dialer already proves), and metered into a new day-keyed `out_of_hours_today_secs` counter that never touches the daily budget. The guardian authors it in a new Limits section; the ward sees the same counter in the mirror.

**Tech Stack:** Rust (`charter-proto`, `charter-spine`, `charter-schedule`, `android/jni`), Kotlin (Android Device Owner warden), TypeScript/Preact (MyCharter PWA), JNI bridge.

## Global Constraints

- **Android only.** Do not touch `linux/`. Linux parity is an explicit follow-on.
- **Wire kind tag is `"alwaysavailable"`** — all-lowercase, matching `#[serde(rename_all = "lowercase")]` on `ClauseKind`. The Rust variant is `ClauseKind::AlwaysAvailable`.
- **Store key is 15.** 14 is `Listening`; 100/101/102 are device-local slots.
- **`v` must equal `CLAUSE_VERSION` (1).** Anything else is invalid.
- **Fail CLOSED.** Absent, unparseable, invalid, expired, or unrecognised ⇒ empty exempt set ⇒ exactly today's behaviour.
- **Exempting lock reasons are `"schedule"` and `"budget"` ONLY.** Never `"standdown"`, never `"malformed"`, never an unknown reason string.
- **A standing block always wins.** An app in the `apps` clause blocklist (or outside its allowlist) stays suspended even when named here.
- **`untilUnix` is absolute unix seconds, never a duration** — mirrors `AppHold`. A duration restarts on every re-read and would never end.
- **Never hardcode package names** in shipped code. Pickers draw from the device's reported inventory. Test fixtures may use `com.book` / `com.chat` as the existing tests do.
- **Out-of-hours time never accrues to the daily budget.**
- **No new dependencies** in any crate or `package.json`.

**Gate commands (run from repo root unless noted):**
- Rust: `cd core && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
- JNI: `cd android/jni && cargo test`
- Kotlin unit: `cd android && ./gradlew :app:testDebugUnitTest`
- Web: `npm test` and `npm run lint`

---

### Task 1: The clause body in `charter-proto`

**Files:**
- Modify: `core/crates/charter-proto/src/clause.rs` (add variant to `ClauseKind` ~line 69, arm to `store_key()` ~line 124, new types after `ListeningBody`'s impl ~line 285)
- Modify: `core/crates/charter-proto/src/lib.rs:27` (re-export)
- Test: `core/crates/charter-proto/src/clause.rs` (new `mod always_available_tests`, alongside `mod listening_tests` ~line 421)

**Interfaces:**
- Consumes: `CLAUSE_VERSION`, existing `ClauseKind`.
- Produces:
  - `ClauseKind::AlwaysAvailable` (tag `"alwaysavailable"`, `store_key() == 15`)
  - `pub struct AlwaysAvailableApp { pub pkg: String, pub until_unix: Option<u64> }`
  - `pub struct AlwaysAvailableBody { pub v: u32, pub issued_at: u64, pub apps: Vec<AlwaysAvailableApp> }`
  - `pub fn AlwaysAvailableBody::exempt_packages(&self, lock_reason: &str, now: u64) -> Vec<String>`

- [ ] **Step 1: Write the failing tests**

Add at the end of `core/crates/charter-proto/src/clause.rs`:

```rust
#[cfg(test)]
mod always_available_tests {
    use super::*;

    fn body(apps: Vec<AlwaysAvailableApp>) -> AlwaysAvailableBody {
        AlwaysAvailableBody { v: CLAUSE_VERSION, issued_at: 1_000, apps }
    }

    fn standing(pkg: &str) -> AlwaysAvailableApp {
        AlwaysAvailableApp { pkg: pkg.to_string(), until_unix: None }
    }

    fn until(pkg: &str, t: u64) -> AlwaysAvailableApp {
        AlwaysAvailableApp { pkg: pkg.to_string(), until_unix: Some(t) }
    }

    #[test]
    fn store_key_is_fifteen() {
        assert_eq!(ClauseKind::AlwaysAvailable.store_key(), 15);
    }

    #[test]
    fn kind_tag_is_all_lowercase() {
        let k: ClauseKind = serde_json::from_str("\"alwaysavailable\"").unwrap();
        assert_eq!(k, ClauseKind::AlwaysAvailable);
        assert_eq!(
            serde_json::to_string(&ClauseKind::AlwaysAvailable).unwrap(),
            "\"alwaysavailable\""
        );
    }

    #[test]
    fn a_standing_app_is_exempt_under_schedule_and_budget() {
        let b = body(vec![standing("com.book")]);
        assert_eq!(b.exempt_packages("schedule", 5_000), vec!["com.book"]);
        assert_eq!(b.exempt_packages("budget", 5_000), vec!["com.book"]);
    }

    /// The guardian's stop-right-now button must mean stop, or it stops
    /// meaning anything. Malformed means we cannot read our own rules, so we
    /// cannot trust the app list inside them either.
    #[test]
    fn no_app_survives_a_standdown_or_a_malformed_charter() {
        let b = body(vec![standing("com.book")]);
        assert!(b.exempt_packages("standdown", 5_000).is_empty());
        assert!(b.exempt_packages("malformed", 5_000).is_empty());
        assert!(b.exempt_packages("unknown", 5_000).is_empty());
        assert!(b.exempt_packages("", 5_000).is_empty());
    }

    #[test]
    fn an_expiry_ends_the_grant_at_the_instant() {
        let b = body(vec![until("com.chat", 5_000)]);
        assert_eq!(b.exempt_packages("schedule", 4_999), vec!["com.chat"]);
        assert!(b.exempt_packages("schedule", 5_000).is_empty());
        assert!(b.exempt_packages("schedule", 5_001).is_empty());
    }

    /// An expired entry is inert, not poison — the rest of the list stands.
    #[test]
    fn an_expired_entry_does_not_void_the_others() {
        let b = body(vec![until("com.chat", 1), standing("com.book")]);
        assert_eq!(b.exempt_packages("schedule", 5_000), vec!["com.book"]);
    }

    #[test]
    fn a_wrong_version_exempts_nothing() {
        let mut b = body(vec![standing("com.book")]);
        b.v = 99;
        assert!(b.exempt_packages("schedule", 5_000).is_empty());
    }

    #[test]
    fn an_empty_list_exempts_nothing() {
        assert!(body(vec![]).exempt_packages("schedule", 5_000).is_empty());
    }

    #[test]
    fn until_unix_is_omitted_when_absent_and_round_trips() {
        let json = serde_json::to_string(&standing("com.book")).unwrap();
        assert_eq!(json, r#"{"pkg":"com.book"}"#);
        let back: AlwaysAvailableApp = serde_json::from_str(&json).unwrap();
        assert_eq!(back.until_unix, None);

        let json = serde_json::to_string(&until("com.chat", 5_000)).unwrap();
        assert_eq!(json, r#"{"pkg":"com.chat","untilUnix":5000}"#);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd core && cargo test -p charter-proto always_available`
Expected: FAIL — `cannot find type AlwaysAvailableBody` / `no variant named AlwaysAvailable`.

- [ ] **Step 3: Add the `ClauseKind` variant**

In `core/crates/charter-proto/src/clause.rs`, after the `Listening` variant (~line 69) and before the closing `}` of the enum:

```rust
    /// Apps open at ANY hour, by name (spec 2026-08-03). Not a category of app
    /// and deliberately not about audio: an audiobook player at 2am and a
    /// messaging app at a sleepover are the same shape. Exempt from the
    /// schedule and budget locks only — never a stand-down (the guardian's
    /// "stop now" must mean stop), never a malformed charter (rules we cannot
    /// read cannot be trusted to name a safe app), never a standing block.
    ///
    /// Distinct from `Listening`, which requires audio already sounding: that
    /// clause lets a story FINISH, this one lets her START something.
    /// Replace-the-set, like `apprules`.
    #[serde(rename = "alwaysavailable")]
    AlwaysAvailable,
```

In `store_key()`, after the `Listening => 14` arm:

```rust
            ClauseKind::AlwaysAvailable => 15,
```

- [ ] **Step 4: Add the body types**

In the same file, after the `impl ListeningBody { … }` block:

```rust
/// One always-available app. `until_unix` absent ⇒ standing, no end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlwaysAvailableApp {
    pub pkg: String,
    /// Unix seconds; the grant ends AT this instant. ABSOLUTE, never a
    /// duration — mirrors [`AppHold`]. A duration restarts every time the
    /// stored clause is re-read, and the grant would never end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_unix: Option<u64>,
}

/// Apps a guardian has named as openable at any hour (spec 2026-08-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlwaysAvailableBody {
    pub v: u32,
    pub issued_at: u64,
    /// Empty ⇒ nothing is exempt.
    #[serde(default)]
    pub apps: Vec<AlwaysAvailableApp>,
}

/// The only lock reasons an always-available app may outlive.
///
/// A stand-down is the guardian saying stop, right now; if an app could sit
/// outside it the button stops meaning anything, and the lifeline still
/// reaches her either way. `malformed` means Charter cannot read its own
/// rules, and a rule set we cannot trust cannot be trusted to name a safe app.
const EXEMPTING_LOCK_REASONS: [&str; 2] = ["schedule", "budget"];

impl AlwaysAvailableBody {
    /// Which packages may be opened right now, given why the device is locked.
    ///
    /// Fails CLOSED at every turn, like [`ListeningBody::verdict`] and unlike
    /// [`BreakGlassCfg::safety_net`]: this clause LOOSENS enforcement, so a
    /// wrong version, an unknown lock reason, or an expired entry all resolve
    /// to "not exempt" — the behaviour before this clause existed.
    ///
    /// Note what is NOT checked here: the standing app policy. The caller
    /// subtracts this set from the LOCK-DRIVEN posture only, so a guardian's
    /// outright block always wins. See `appSuspendSet` on the Kotlin side.
    pub fn exempt_packages(&self, lock_reason: &str, now: u64) -> Vec<String> {
        if self.v != CLAUSE_VERSION || !EXEMPTING_LOCK_REASONS.contains(&lock_reason) {
            return Vec::new();
        }
        self.apps
            .iter()
            // An expired entry is inert, not poison: it drops out and the rest
            // of the list stands.
            .filter(|a| a.until_unix.is_none_or(|t| now < t))
            .map(|a| a.pkg.clone())
            .collect()
    }
}
```

- [ ] **Step 5: Re-export the new types**

In `core/crates/charter-proto/src/lib.rs`, add `AlwaysAvailableApp, AlwaysAvailableBody` to the existing `pub use clause::{…}` list (alphabetically first, before `LifelineNumber`).

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd core && cargo test -p charter-proto always_available`
Expected: PASS, 8 tests.

- [ ] **Step 7: Fix the exhaustive matches the new variant broke**

Two `match` sites are exhaustive over `ClauseKind` and will not compile:

`core/crates/charter-spine/src/broker.rs:406` — add to the per-child routing list, immediately after `| ClauseKind::Listening`:

```rust
                | ClauseKind::AlwaysAvailable => Some(self.subject),
```

(i.e. `ClauseKind::Listening` loses its `=> Some(self.subject)` and becomes another `|` arm.)

`core/crates/charter-verify/src/test_support.rs` — both match arms (~317 and ~341), after each `ClauseKind::Listening => "listening",`:

```rust
            ClauseKind::AlwaysAvailable => "alwaysavailable",
```

- [ ] **Step 8: Run the full Rust gates**

Run: `cd core && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS, no warnings.

- [ ] **Step 9: Commit**

```bash
git add core/crates/charter-proto/src/clause.rs core/crates/charter-proto/src/lib.rs \
        core/crates/charter-spine/src/broker.rs core/crates/charter-verify/src/test_support.rs
git commit -m "feat(proto): alwaysavailable clause — apps open at any hour, by name"
```

---

### Task 2: Close the standing-block hole in `appSuspendSet`, and add the exemption

**Files:**
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt:52-75` (the `appSuspendSet` fn) and `:32-38` (the `reconcile` interface signature)
- Test: `android/app/src/test/kotlin/org/forgesworn/charter/enforce/AppSuspendSetTest.kt`

**Interfaces:**
- Consumes: nothing from Task 1 (pure Kotlin set logic).
- Produces: `appSuspendSet(launchable, locked, appPolicy, ruleSuspensions, listeningExempt, bucketSuspensions, alwaysAvailable)` — new final parameter `alwaysAvailable: Set<String> = emptySet()`. `AppGateOps.reconcile` gains the same trailing parameter with the same default.

**Why this task exists beyond the feature:** `appSuspendSet` documents at `Enforcement.kt:71-73` that a standing block beats an exemption, but implements `union - listeningExempt`, which subtracts from the *whole* union including the blocklist. Its guarding test (`AppSuspendSetTest.kt:66`) runs `locked = false` and never exercises the case. Fix it here because this task adds a second exemption set through the identical seam.

- [ ] **Step 1: Write the failing tests**

Append to `android/app/src/test/kotlin/org/forgesworn/charter/enforce/AppSuspendSetTest.kt`:

```kotlin
    // --- always available (spec 2026-08-03) -----------------------------------

    /**
     * The shipped `listening` exemption subtracted from the WHOLE union, so a
     * guardian-blocked app that happened to be playing came back at the lock —
     * contradicting this function's own documented contract. The existing test
     * for it runs unlocked and never saw this.
     */
    @Test
    fun `an exemption never beats a standing block, even while locked`() {
        val out = appSuspendSet(
            launchable,
            locked = true,
            appPolicy = policy("blocklist", blocked = listOf("com.book")),
            ruleSuspensions = emptySet(),
            listeningExempt = setOf("com.book"),
        )
        assertTrue("a blocked app is blocked, playing or not", out.contains("com.book"))
    }

    @Test
    fun `an allowlist still excludes an app it does not name, even when exempt`() {
        val out = appSuspendSet(
            launchable,
            locked = true,
            appPolicy = policy("allowlist", allowed = listOf("com.school")),
            ruleSuspensions = emptySet(),
            alwaysAvailable = setOf("com.book"),
        )
        assertTrue("outside the allowlist stays out", out.contains("com.book"))
    }

    @Test
    fun `an always-available app survives a lock with no audio playing`() {
        val out = appSuspendSet(
            launchable,
            locked = true,
            appPolicy = null,
            ruleSuspensions = emptySet(),
            alwaysAvailable = setOf("com.book"),
        )
        assertFalse("she must be able to START it", out.contains("com.book"))
        assertTrue("everything else still goes", out.containsAll(listOf("com.game", "com.chat")))
    }

    /**
     * Nothing to exempt while unlocked — the clause is about surviving a lock,
     * never about dodging a per-app rule during allowed hours.
     */
    @Test
    fun `always-available never unblocks during allowed hours`() {
        val out = appSuspendSet(
            launchable,
            locked = false,
            appPolicy = null,
            ruleSuspensions = setOf("com.book"),
            alwaysAvailable = setOf("com.book"),
        )
        assertTrue("a live per-app rule still applies", out.contains("com.book"))
    }

    @Test
    fun `an empty always-available set changes nothing`() {
        assertEquals(
            appSuspendSet(launchable, true, null, emptySet()),
            appSuspendSet(launchable, true, null, emptySet(), emptySet(), emptySet(), emptySet()),
        )
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests '*AppSuspendSetTest*'`
Expected: FAIL — `no parameter named alwaysAvailable`, and `an exemption never beats a standing block` fails its assertion.

- [ ] **Step 3: Rewrite `appSuspendSet`**

Replace `android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt:52-75` with:

```kotlin
fun appSuspendSet(
    launchable: Collection<String>,
    locked: Boolean,
    appPolicy: org.forgesworn.charter.native.CharterCore.AppPolicy?,
    ruleSuspensions: Set<String>,
    listeningExempt: Set<String> = emptySet(),
    bucketSuspensions: Set<String> = emptySet(),
    alwaysAvailable: Set<String> = emptySet(),
): Set<String> {
    // The STANDING posture — what the guardian's app policy says regardless of
    // the clock. Kept separate from the lock posture below because exemptions
    // may never touch it.
    val standing: Set<String> = when {
        appPolicy == null -> emptySet()
        appPolicy.posture == "allowlist" -> launchable.toSet() - appPolicy.allowed.toSet()
        else -> appPolicy.blocked.toSet()
    }
    // The LOCK posture — the whole surface, and the only thing an exemption may
    // ever be subtracted from. Both exemptions are permission to survive a
    // LOCK: a story the family agreed may finish (`listening`, gated on audio
    // actually sounding), and an app they agreed is open at any hour
    // (`alwaysavailable`, gated on the lock reason, resolved by the caller).
    //
    // Subtracting from the lock posture ALONE is the fix for the hole this
    // function always claimed to close but did not: `union - listeningExempt`
    // also cancelled a standing blocklist, so an app the guardian blocked
    // outright came back at the lock if it was making a noise.
    val lockPosture: Set<String> =
        if (locked) launchable.toSet() - listeningExempt - alwaysAvailable else emptySet()
    // Union: a package suspended by the lock, the standing policy, a per-app
    // rule, OR its named-times bucket stays suspended — the dimensions never
    // cancel each other out.
    return lockPosture + standing + ruleSuspensions + bucketSuspensions
}
```

Update the KDoc above it (lines 42-51) — replace the final sentence about `listeningExempt` with:

```kotlin
 * Exemptions ([listeningExempt], [alwaysAvailable]) are subtracted from the
 * LOCK posture only, never from the standing policy or the per-app rules: they
 * are permission to survive a lock, never permission to dodge a block a
 * guardian set outright. An app blocked in the Apps clause stays blocked
 * whether it is making a noise or named as always available.
```

- [ ] **Step 4: Add the parameter to the interface and the DPM impl**

In the `AppGateOps` interface (`:32-38`), add a final parameter to `reconcile`:

```kotlin
        bucketSuspensions: Set<String> = emptySet(),
        alwaysAvailable: Set<String> = emptySet(),
    ): List<String>
```

In `DpmAppGateOps.reconcile` (`:177-209`), mirror the parameter and thread it through:

```kotlin
    override fun reconcile(
        locked: Boolean,
        appPolicy: org.forgesworn.charter.native.CharterCore.AppPolicy?,
        ruleSuspensions: Set<String>,
        listeningExempt: Set<String>,
        bucketSuspensions: Set<String>,
        alwaysAvailable: Set<String>,
    ): List<String> {
```

and in the `appSuspendSet(...)` call inside it, add `alwaysAvailable,` after `bucketSuspensions,`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests '*AppSuspendSetTest*'`
Expected: PASS — including the pre-existing listening and bucket tests, unchanged.

- [ ] **Step 6: Commit**

```bash
git add android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt \
        android/app/src/test/kotlin/org/forgesworn/charter/enforce/AppSuspendSetTest.kt
git commit -m "fix(enforce): an exemption may not beat a standing block; add alwaysAvailable seam

appSuspendSet documented that a guardian's outright block survives an
exemption, then subtracted listeningExempt from the whole union — cancelling
the blocklist too. The guarding test ran unlocked and never saw it.

Split the standing posture from the lock posture; exemptions subtract from the
lock posture alone. Adds the alwaysAvailable exemption through the same seam."
```

---

### Task 3: Resolve the clause in the warden and cross the JNI boundary

**Files:**
- Modify: `android/jni/src/warden.rs` (new `always_available_view` next to `listening_view` ~line 2129)
- Modify: `android/jni/src/lib.rs` (new export next to `charterListeningView` ~line 532)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterNative.kt` (~line 146)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterCore.kt` (~line 399)
- Test: `android/jni/src/warden.rs` (in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `charter_proto::{AlwaysAvailableBody, ClauseKind::AlwaysAvailable}` from Task 1.
- Produces:
  - Rust: `Warden::always_available_view(&self, now: u64, locked: bool, lock_reason: &str) -> String` returning `{"open":[…]}`
  - Kotlin: `CharterCore.alwaysAvailable(nowUnix: Long, locked: Boolean, lockReason: String): Set<String>`

Unlike `listening_view` this takes `&self`, not `&mut self`: there is no first-sight instant to pin, because there is no grace to run down.

- [ ] **Step 1: Write the failing test**

In `android/jni/src/warden.rs`, inside the existing `mod tests` (starts line 2741). Clauses are planted by **signing a real event and ingesting it** — there is no seeding helper, and inventing one would skip the ingest path these tests exist to exercise. The idiom below is copied from `app_rule_suspensions_are_exactly_the_blocked_now_packages` (~line 3792); follow it exactly, including the `temp_base` / `remove_dir_all` bracketing.

Note there are currently **no warden-level tests for `listening_view` at all**, so there is no closer precedent to follow than the appRules one.

```rust
    /// She must be able to START an audiobook at 2am — the whole distinction
    /// from `listening`, which needs audio already sounding.
    #[test]
    fn always_available_opens_named_apps_but_only_for_the_right_lock() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No clause yet → nothing opens (fail-closed).
        assert_eq!(
            w.always_available_view(5_000, true, "schedule"),
            r#"{"open":[]}"#,
            "fail closed before any clause"
        );

        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::AlwaysAvailable,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({
            "v": 1,
            "issuedAt": 1,
            "apps": [
                { "pkg": "app.example.book" },
                { "pkg": "app.example.chat", "untilUnix": 5000 }
            ]
        }))
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1);
        assert!(res.accepted, "alwaysavailable clause rejected: {}", res.reason);

        // A schedule lock: both are open while the temporary one is live.
        let v = w.always_available_view(4_999, true, "schedule");
        assert!(v.contains("app.example.book"), "got {v}");
        assert!(v.contains("app.example.chat"), "got {v}");

        // A budget lock exempts exactly the same way.
        assert!(w
            .always_available_view(4_999, true, "budget")
            .contains("app.example.book"));

        // The expiry ends the temporary grant at the instant, with no restart.
        let v = w.always_available_view(5_000, true, "schedule");
        assert!(v.contains("app.example.book"), "the standing one stands: {v}");
        assert!(!v.contains("app.example.chat"), "the lapsed one is gone: {v}");

        // Unlocked, and the two locks this clause may never outlive.
        assert_eq!(w.always_available_view(4_999, false, "schedule"), r#"{"open":[]}"#);
        assert_eq!(w.always_available_view(4_999, true, "standdown"), r#"{"open":[]}"#);
        assert_eq!(w.always_available_view(4_999, true, "malformed"), r#"{"open":[]}"#);
        assert_eq!(w.always_available_view(4_999, true, "unknown"), r#"{"open":[]}"#);

        let _ = std::fs::remove_dir_all(&base);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd android/jni && cargo test always_available`
Expected: FAIL — `no method named always_available_view`.

- [ ] **Step 3: Implement `always_available_view`**

In `android/jni/src/warden.rs`, immediately after `listening_view`:

```rust
    /// Which packages may be OPENED right now, though the device is locked.
    ///
    /// Returns `{"open":[…]}` — what the app gate subtracts from the lock
    /// posture, and what the shade offers as its Open row.
    ///
    /// Deliberately unlike [`Warden::listening_view`] in two ways. It takes
    /// `&self`: there is no first-sight instant to pin, because there is no
    /// grace running down — an entry's `untilUnix` is absolute and answers for
    /// itself. And it asks no question about audio: this clause is about
    /// STARTING something, not about letting one finish.
    ///
    /// Fails CLOSED at every turn (no clause, unparseable, unlocked, a lock
    /// reason that does not exempt): an empty set is exactly the behaviour
    /// before this clause existed.
    pub fn always_available_view(&self, now: u64, locked: bool, lock_reason: &str) -> String {
        const EMPTY: &str = r#"{"open":[]}"#;
        if !locked {
            return EMPTY.to_string();
        }
        let Some(subject) = self.subject else {
            return EMPTY.to_string();
        };
        let body = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::AlwaysAvailable.store_key(),
            )
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_str::<charter_proto::AlwaysAvailableBody>(&b).ok());
        let Some(body) = body else {
            return EMPTY.to_string();
        };
        serde_json::json!({ "open": body.exempt_packages(lock_reason, now) }).to_string()
    }
```

- [ ] **Step 4: Export it over JNI**

In `android/jni/src/lib.rs`, after the `charterListeningView` export, following that function's exact idiom for string arguments (read it and the nearest `JString`-taking export before writing this — match their error handling and `with_warden` wrapper verbatim):

```rust
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterAlwaysAvailable(
    mut env: JNIEnv,
    _class: JClass,
    now_unix: jlong,
    locked: jboolean,
    lock_reason: JString,
) -> jstring {
    let reason: String = env
        .get_string(&lock_reason)
        .map(|s| s.into())
        .unwrap_or_default();
    with_warden(&mut env, |w| {
        w.always_available_view(now_unix as u64, locked != 0, &reason)
    })
}
```

- [ ] **Step 5: Declare and bind it in Kotlin**

In `CharterNative.kt`, after `charterListeningView`:

```kotlin
    /** Packages that may be OPENED though the device is locked. Takes the lock
     *  REASON because only "schedule" and "budget" exempt anything — a
     *  stand-down or a malformed charter never does. */
    external fun charterAlwaysAvailable(
        nowUnix: Long,
        locked: Boolean,
        lockReason: String,
    ): String
```

In `CharterCore.kt`, after `listeningView`:

```kotlin
    /** Apps the family agreed are open at any hour, live right now. Empty
     *  whenever the clause is absent, unusable, expired, or the lock is one
     *  this clause may not outlive. */
    fun alwaysAvailable(nowUnix: Long, locked: Boolean, lockReason: String): Set<String> {
        val arr = JSONObject(CharterNative.charterAlwaysAvailable(nowUnix, locked, lockReason))
            .optJSONArray("open")
        return buildSet {
            for (i in 0 until (arr?.length() ?: 0)) arr!!.optString(i)?.let { add(it) }
        }
    }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cd android/jni && cargo test always_available`
Expected: PASS, 1 test covering all eight assertions.

- [ ] **Step 7: Run the JNI gates**

Run: `cd android/jni && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS, no warnings.

- [ ] **Step 8: Commit**

```bash
git add android/jni/src/warden.rs android/jni/src/lib.rs \
        android/app/src/main/kotlin/org/forgesworn/charter/native/CharterNative.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/native/CharterCore.kt
git commit -m "feat(jni): always_available_view — which apps may be opened through a lock"
```

---

### Task 4: Wire it into the tick, and make the LockTask allowlist level-triggered

**Files:**
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt:93-110` (the once-at-init `setLockTaskPackages`) and `:397-425` (the reconcile block in `applyDecision`)

**Interfaces:**
- Consumes: `CharterCore.alwaysAvailable(nowUnix, locked, lockReason)` (Task 3); `appGate.reconcile(…, alwaysAvailable)` (Task 2).
- Produces: `WardenController.lockTaskPackages(alwaysAvailable: Set<String>): Array<String>` — the phone stack plus the live always-available set. Task 5 relies on the shade's Open row matching this exact set.

**The bug this task fixes:** `setLockTaskPackages` is called once, inside the `if (initialized && isDeviceOwner)` block at init. The always-available set changes when the guardian edits the clause and when an entry expires, so it must be re-derived every tick like every other Charter posture — including after a reboot.

- [ ] **Step 1: Extract the phone-stack list into a reusable function**

Replace the `val phonePackages = buildList { … }` block and its `setLockTaskPackages` call (`:101-110`) with a call to a new private method, and add that method near `applyDecision`:

```kotlin
    /**
     * The LockTask allowlist: Charter itself, the phone stack, and whatever the
     * `alwaysavailable` clause opens right now.
     *
     * The phone stack is here so a lifeline call has a face — without it a call
     * runs headless behind the lock with no hang-up (the 2026-07-24 on-device
     * trap). The always-available set is here so the shade's Open row can
     * actually launch what it offers.
     *
     * RE-DERIVED EVERY TICK, never once at init: entries expire on their own
     * clock and a guardian edits the clause while the phone is locked. This was
     * a one-shot call until 2026-08-03, which would have pinned whatever the
     * set happened to be at boot.
     */
    private fun lockTaskPackages(alwaysAvailable: Set<String>): Array<String> =
        buildList {
            add(context.packageName)
            runCatching {
                val telecom = context.getSystemService(Context.TELECOM_SERVICE)
                    as android.telecom.TelecomManager
                telecom.defaultDialerPackage?.let { add(it) }
                telecom.systemDialerPackage?.let { add(it) }
            }
            addAll(alwaysAvailable)
        }.distinct().toTypedArray()

    /** The last allowlist actually pushed, so a level-triggered caller doesn't
     *  re-issue an identical DPM call every tick — the same discipline
     *  [DpmRestrictionOps.applyTetherMode] follows after Robin's phone logged
     *  ~68 redundant restriction changes a minute (2026-07-26). */
    @Volatile private var lastLockTaskPackages: List<String>? = null

    private fun syncLockTaskPackages(alwaysAvailable: Set<String>) {
        val next = lockTaskPackages(alwaysAvailable)
        if (next.toList() == lastLockTaskPackages) return
        lastLockTaskPackages = next.toList()
        runCatching { dpm.setLockTaskPackages(admin, next) }
            .onFailure { Log.w(TAG, "could not set LockTask packages", it) }
    }
```

At init (where `setLockTaskPackages` used to be), call `syncLockTaskPackages(emptySet())` so the phone stack is allowed from the first moment, before any decision has run.

- [ ] **Step 2: Resolve and apply the exemption in `applyDecision`**

In the `if (d.enforceMode != CharterCore.Mode.OBSERVE) { … }` block, after the `listening` resolution and before `appGate.reconcile(…)`:

```kotlin
            // Apps the family agreed are open at ANY hour (spec 2026-08-03).
            // Takes the lock REASON, not just `locked`: a stand-down and a
            // malformed charter must take everything, so the clause is asked
            // whether THIS lock is one it may outlive.
            val alwaysAvailable = runCatching {
                CharterCore.alwaysAvailable(nowUnix, d.locked, d.reason)
            }.getOrDefault(emptySet())
            // Level-triggered, exactly like the suspend set below: the clause
            // changes and entries expire, so the allowlist is re-derived here
            // rather than pinned at boot.
            syncLockTaskPackages(alwaysAvailable)
```

and add the argument to the reconcile call:

```kotlin
            appGate.reconcile(
                locked = d.locked,
                appPolicy = appPolicy,
                ruleSuspensions = ruleSuspensions.toSet(),
                listeningExempt = listening?.exempt ?: emptySet(),
                bucketSuspensions = bucketSuspensions.toSet(),
                alwaysAvailable = alwaysAvailable,
            )
```

- [ ] **Step 3: Write the failing test for the allowlist composition**

Create `android/app/src/test/kotlin/org/forgesworn/charter/service/LockTaskPackagesTest.kt`. `lockTaskPackages` touches `Context`/`TelecomManager`, so extract its pure part into a testable top-level function in `Enforcement.kt` and have the private method call it:

```kotlin
/**
 * The LockTask allowlist as pure set logic: Charter, the phone stack, and the
 * live always-available set, deduped and order-stable. The JVM-testable seam
 * behind [WardenController.lockTaskPackages].
 */
fun lockTaskAllowlist(
    self: String,
    dialers: List<String>,
    alwaysAvailable: Set<String>,
): List<String> = (listOf(self) + dialers + alwaysAvailable.sorted()).distinct()
```

```kotlin
package org.forgesworn.charter.service

import org.forgesworn.charter.enforce.lockTaskAllowlist
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class LockTaskPackagesTest {

    @Test
    fun `the phone stack is always allowed, so a lifeline call has a face`() {
        val out = lockTaskAllowlist("org.forgesworn.charter", listOf("com.android.dialer"), emptySet())
        assertEquals(listOf("org.forgesworn.charter", "com.android.dialer"), out)
    }

    @Test
    fun `always-available apps join the allowlist`() {
        val out = lockTaskAllowlist("org.forgesworn.charter", emptyList(), setOf("com.book"))
        assertTrue(out.contains("com.book"))
    }

    @Test
    fun `a duplicate dialer is not listed twice`() {
        val out = lockTaskAllowlist(
            "org.forgesworn.charter",
            listOf("com.android.dialer", "com.android.dialer"),
            setOf("com.android.dialer"),
        )
        assertEquals(1, out.count { it == "com.android.dialer" })
    }

    /** Order-stable, so the level-triggered caller's equality check does not
     *  churn a DPM call every tick over set iteration order. */
    @Test
    fun `the same inputs give a byte-identical list`() {
        val a = lockTaskAllowlist("self", listOf("d"), setOf("com.b", "com.a"))
        val b = lockTaskAllowlist("self", listOf("d"), setOf("com.a", "com.b"))
        assertEquals(a, b)
    }
}
```

- [ ] **Step 4: Run the tests to verify they fail, then pass**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests '*LockTaskPackagesTest*'`
Expected first: FAIL — `unresolved reference: lockTaskAllowlist`. After adding it: PASS, 4 tests.

- [ ] **Step 5: Run the whole Kotlin unit suite**

Run: `cd android && ./gradlew :app:testDebugUnitTest`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt \
        android/app/src/test/kotlin/org/forgesworn/charter/service/LockTaskPackagesTest.kt
git commit -m "feat(warden): resolve alwaysavailable each tick; level-trigger the LockTask allowlist

setLockTaskPackages was called once at init. Entries expire on their own clock
and a guardian edits the clause while the phone is locked, so the allowlist is
now re-derived every tick like every other Charter posture, and only pushed
when it actually changes."
```

---

### Task 5: The Open row on the shade

**Files:**
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/ui/LockActivity.kt` (new `paintOpenRow` helper; call it from `renderViews` after the ask button block ~line 649 and before the lifeline offer ~line 695)
- Test: `android/app/src/test/kotlin/org/forgesworn/charter/ui/OpenRowTest.kt` (create)

**Interfaces:**
- Consumes: `CharterCore.alwaysAvailable(nowUnix, locked, lockReason)` (Task 3).
- Produces: `openRowEntries(alwaysAvailable: Set<String>, inventory: List<Pair<String, String>>): List<Pair<String, String>>` in `Enforcement.kt` — `(pkg, label)` pairs to render, sorted by label. Empty ⇒ render no row at all.

- [ ] **Step 1: Write the failing test**

Create `android/app/src/test/kotlin/org/forgesworn/charter/ui/OpenRowTest.kt`:

```kotlin
package org.forgesworn.charter.ui

import org.forgesworn.charter.enforce.openRowEntries
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class OpenRowTest {

    private val inventory = listOf(
        "com.book" to "Voice",
        "com.pods" to "AntennaPod",
        "com.game" to "Some Game",
    )

    /**
     * A family that never sets this clause must see a shade byte-identical to
     * the one they see today. No empty row, no header, nothing.
     */
    @Test
    fun `nothing named means no row at all`() {
        assertTrue(openRowEntries(emptySet(), inventory).isEmpty())
    }

    @Test
    fun `named apps render with their friendly labels, sorted`() {
        val out = openRowEntries(setOf("com.book", "com.pods"), inventory)
        assertEquals(listOf("com.pods" to "AntennaPod", "com.book" to "Voice"), out)
    }

    /**
     * The clause can name an app that has since been uninstalled. Offering a
     * button that opens nothing is worse than offering no button.
     */
    @Test
    fun `an app missing from the inventory is not offered`() {
        assertTrue(openRowEntries(setOf("com.gone"), inventory).isEmpty())
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd android && ./gradlew :app:testDebugUnitTest --tests '*OpenRowTest*'`
Expected: FAIL — `unresolved reference: openRowEntries`.

- [ ] **Step 3: Implement the pure part**

In `android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt`:

```kotlin
/**
 * What the shade's Open row should offer: the live always-available packages
 * that the device can actually launch, as `(pkg, label)` sorted by label.
 *
 * An app named in the clause but absent from the inventory is dropped — it was
 * uninstalled since the guardian chose it, and a button that opens nothing is
 * worse than no button. An empty result means render NO row: a family that
 * never sets this clause sees the shade exactly as it is today.
 */
fun openRowEntries(
    alwaysAvailable: Set<String>,
    inventory: List<Pair<String, String>>,
): List<Pair<String, String>> =
    inventory.filter { it.first in alwaysAvailable }.sortedBy { it.second.lowercase() }
```

- [ ] **Step 4: Render it on the shade**

In `LockActivity.kt`, add a helper and call it from `renderViews` after the ask-status view is added (`root.addView(askStatus)`, ~line 649):

```kotlin
    /**
     * The Open row: the apps the family agreed are open at any hour.
     *
     * Launching works because these packages are on the LockTask allowlist
     * (WardenController.syncLockTaskPackages) — the same path that lets the
     * in-call UI surface over the pin for a lifeline call. Leaving the app
     * returns her here, because the shade is re-asserted level-triggered from
     * `locked` on the next tick.
     */
    private fun paintOpenRow(root: LinearLayout) {
        val open = runCatching {
            org.forgesworn.charter.enforce.openRowEntries(
                CharterCore.alwaysAvailable(
                    System.currentTimeMillis() / 1000,
                    true,
                    info?.reason ?: "",
                ),
                org.forgesworn.charter.enforce.launchableAppsPairs(this),
            )
        }.getOrDefault(emptyList())
        if (open.isEmpty()) return

        root.addView(
            text("Open at any hour", CharterTheme.FS_SMALL, CharterTheme.INK_TEXT_2, 16),
        )
        for ((pkg, label) in open) {
            root.addView(
                CharterTheme.secondaryButton(this, label, onInk = true).apply {
                    setOnClickListener {
                        val i = packageManager.getLaunchIntentForPackage(pkg)
                        if (i == null) {
                            Log.w(TAG, "no launch intent for $pkg")
                        } else {
                            runCatching { startActivity(i) }
                                .onFailure { Log.w(TAG, "could not open $pkg", it) }
                        }
                    }
                },
                CharterTheme.stackParams(this, 8),
            )
        }
    }
```

Call it: `paintOpenRow(root)` immediately after `root.addView(askStatus)`. `paintOpenRow` must take `info` as a parameter (or read the same local `renderViews` already holds) — do not re-query.

The lock reason needs no new field: `renderViews` already has `info` from `CharterCore.lockInfo(...)`, and `info?.reason` is the same value the ask button routes on at `LockActivity.kt:609` (`info?.reason == "standdown"`) and `:620` (`if (info?.reason == "schedule")`). Use `info?.reason ?: ""` — the empty string is not an exempting reason, so an unreadable `info` fails closed to no row.

Add `launchableAppsPairs` to `Enforcement.kt` next to `launchableAppsJson` (`:154-169`), returning `List<Pair<String, String>>`. Extract the shared query-and-dedupe loop so `launchableAppsJson` calls it and serialises the result, rather than duplicating the loop — the two must not drift on the deny-list or the dedupe.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cd android && ./gradlew :app:testDebugUnitTest`
Expected: PASS.

- [ ] **Step 6: Build the app to verify it compiles**

Run: `cd android && ./gradlew :app:assembleDebug`
Expected: BUILD SUCCESSFUL.

- [ ] **Step 7: Commit**

```bash
git add android/app/src/main/kotlin/org/forgesworn/charter/ui/LockActivity.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt \
        android/app/src/test/kotlin/org/forgesworn/charter/ui/OpenRowTest.kt
git commit -m "feat(shade): an Open row for the apps open at any hour"
```

---

### Task 6: The out-of-hours counter

**Files:**
- Modify: `core/crates/charter-schedule/src/usage.rs` (new field beside `unrecognised_today_secs:55-62`, plus its accessor and the day-roll reset)
- Modify: `android/jni/src/warden.rs` (mark the counter in the tick where usage accrues; expose it on the status/decision surface beside the existing counters)
- Test: `core/crates/charter-schedule/src/usage.rs` (in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `always_available_view` (Task 3) to know whether the foreground package is exempt.
- Produces:
  - `UsageLedger::mark_out_of_hours(&mut self, secs: u64)`
  - `UsageLedger::out_of_hours_today_secs(&self) -> u64`
  - `UsageLedger::out_of_hours_week_secs(&self) -> u64`
  - `UsageLedger::out_of_hours_nights_week(&self) -> u32`

**Read this before writing the field.** `unrecognised_today_secs` (`usage.rs:55-62`) is the right precedent for the *field shape* — a day-keyed counter carrying additional information about time, never a different pool of time, with `#[serde(default)]` so old snapshots restore at zero. It is **not** a precedent for the status wire: `unrecognisedTodaySecs` is Linux-only and a test at `warden.rs:6235` asserts Android never stamps it. Out-of-hours is the opposite — Android is the only platform that stamps it. Do not "follow the precedent" into leaving it off the Android status payload.

**Three counters, not one.** The guardian's line is "3 nights this week, about 40 min", so a single day-keyed seconds counter cannot render it. Mirror the existing `used_today_secs` / `used_week_secs` pair, plus a week-keyed count of distinct days that saw any out-of-hours use — incremented exactly when today's counter crosses 0 → non-zero, so it counts nights rather than sessions.

- [ ] **Step 1: Write the failing tests**

In `core/crates/charter-schedule/src/usage.rs`'s test module:

```rust
    /// Out-of-hours time is a FACT ABOUT the day, never a pool of time. It must
    /// never reach the budget: charging her for a bad night's sleep is the
    /// counting and the enforcing disagreeing.
    #[test]
    fn out_of_hours_time_never_touches_the_budget() {
        let mut u = UsageLedger::new("Europe/London", WeekStart::Monday, 1_700_000_000);
        let before = u.used_today_secs();
        u.mark_out_of_hours(600);
        assert_eq!(u.out_of_hours_today_secs(), 600);
        assert_eq!(u.used_today_secs(), before, "the budget must not move");
    }

    #[test]
    fn out_of_hours_accumulates_and_resets_with_the_day() {
        let mut u = UsageLedger::new("Europe/London", WeekStart::Monday, 1_700_000_000);
        u.mark_out_of_hours(300);
        u.mark_out_of_hours(300);
        assert_eq!(u.out_of_hours_today_secs(), 600);
        // A day later, the counter is a fresh day's counter.
        u.reconcile("Europe/London", WeekStart::Monday, 1_700_000_000 + 86_400 * 2);
        assert_eq!(u.out_of_hours_today_secs(), 0);
    }

    /// Three wakings in one night are ONE night. The line says "3 nights this
    /// week", not "3 times".
    #[test]
    fn several_wakings_in_one_night_count_as_one_night() {
        let mut u = UsageLedger::new("Europe/London", WeekStart::Monday, 1_700_000_000);
        u.mark_out_of_hours(300);
        u.mark_out_of_hours(300);
        u.mark_out_of_hours(300);
        assert_eq!(u.out_of_hours_nights_week(), 1);
        assert_eq!(u.out_of_hours_week_secs(), 900);
    }

    #[test]
    fn a_zero_credit_never_invents_a_night() {
        let mut u = UsageLedger::new("Europe/London", WeekStart::Monday, 1_700_000_000);
        u.mark_out_of_hours(0);
        assert_eq!(u.out_of_hours_nights_week(), 0);
        assert_eq!(u.out_of_hours_today_secs(), 0);
    }

    /// The day counter rolls nightly; the week total and the night count do
    /// not — they roll with the week, beside `used_week_secs`.
    #[test]
    fn a_second_night_adds_to_the_week_after_the_day_rolls() {
        let mut u = UsageLedger::new("Europe/London", WeekStart::Monday, 1_700_000_000);
        u.mark_out_of_hours(600);
        u.reconcile("Europe/London", WeekStart::Monday, 1_700_000_000 + 86_400);
        assert_eq!(u.out_of_hours_today_secs(), 0, "a fresh day");
        u.mark_out_of_hours(300);
        assert_eq!(u.out_of_hours_nights_week(), 2);
        assert_eq!(u.out_of_hours_week_secs(), 900);
    }

    /// Snapshots written before this counter existed must restore cleanly.
    #[test]
    fn a_pre_existing_snapshot_restores_with_a_zero_counter() {
        let json = r#"{"tz":"Europe/London","weekStart":"monday","dayKey":"2026-08-03",
            "weekKey":"2026-W32","usedTodaySecs":120,"usedWeekSecs":120}"#;
        let u = UsageLedger::from_snapshot(json).expect("restores");
        assert_eq!(u.out_of_hours_today_secs(), 0);
        assert_eq!(u.out_of_hours_week_secs(), 0);
        assert_eq!(u.out_of_hours_nights_week(), 0);
    }
```

The snapshot JSON above is illustrative. Before running, dump a real one with `serde_json::to_string(&UsageLedger::new(...))` and copy its exact field naming — `UsageLedger`'s serde shape is the authority, not this plan. Also confirm `from_snapshot`'s real signature (`usage.rs`) before writing the call; the existing restore test in that module shows it.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd core && cargo test -p charter-schedule out_of_hours`
Expected: FAIL — `no method named mark_out_of_hours`.

- [ ] **Step 3: Add the field and its methods**

In `UsageLedger`, after `unrecognised_today_secs`:

```rust
    /// Day-keyed seconds spent in an app the `alwaysavailable` clause opened
    /// while the device was LOCKED (spec 2026-08-03) — the 2am audiobook, the
    /// sleepover message. Like `unrecognised_today_secs` this is additional
    /// information ABOUT the day and never a different pool of time: it is
    /// deliberately absent from `used_today_secs`, because charging a ward for
    /// a bad night's sleep would make the counting and the enforcing disagree.
    ///
    /// One counter, not two: use after the daily limit is spent at 5pm counts
    /// the same as use at 2am. The lock reason is Charter's business; the
    /// pattern is the family's.
    ///
    /// `#[serde(default)]` so pre-2026-08-03 snapshots restore with a zero
    /// counter.
    #[serde(default)]
    out_of_hours_today_secs: u64,
    /// Week-keyed total, for the guardian's weekly line. Rolls with
    /// `used_week_secs`, not with the day.
    #[serde(default)]
    out_of_hours_week_secs: u64,
    /// Week-keyed count of DISTINCT DAYS that saw any out-of-hours use — the
    /// "3 nights" half of the line. Incremented exactly when the day counter
    /// crosses 0 → non-zero, so three wakings in one night are one night.
    #[serde(default)]
    out_of_hours_nights_week: u32,
```

Add the methods beside the equivalent `used_*` accessors:

```rust
    /// Credit out-of-hours seconds. Never touches `used_today_secs` or
    /// `used_week_secs` — this is a fact ABOUT the day, not a pool of time.
    pub fn mark_out_of_hours(&mut self, secs: u64) {
        if secs == 0 {
            return;
        }
        // The 0 → non-zero crossing IS the night. Counted before the add, so
        // a second waking the same night does not count twice.
        if self.out_of_hours_today_secs == 0 {
            self.out_of_hours_nights_week = self.out_of_hours_nights_week.saturating_add(1);
        }
        self.out_of_hours_today_secs = self.out_of_hours_today_secs.saturating_add(secs);
        self.out_of_hours_week_secs = self.out_of_hours_week_secs.saturating_add(secs);
    }

    pub fn out_of_hours_today_secs(&self) -> u64 {
        self.out_of_hours_today_secs
    }

    pub fn out_of_hours_week_secs(&self) -> u64 {
        self.out_of_hours_week_secs
    }

    pub fn out_of_hours_nights_week(&self) -> u32 {
        self.out_of_hours_nights_week
    }
```

In `reconcile`, zero `out_of_hours_today_secs` at the **day**-roll site (beside where `used_today_secs` is zeroed), and zero **both** `out_of_hours_week_secs` and `out_of_hours_nights_week` at the **week**-roll site (beside `used_week_secs`). Put each new line immediately beside its twin so they cannot drift.

Initialise all three to `0` in `UsageLedger::new`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd core && cargo test -p charter-schedule out_of_hours`
Expected: PASS, 6 tests.

- [ ] **Step 5: Accrue it in the warden tick**

In `android/jni/src/warden.rs`, in `tick` (starts :1716), immediately after the named-times bucket credit block (ends ~:1848) and before the extension drain. `elapsed` is already `self.elapsed_since(now)` — the clamped interval from the 2026-08-01 background-power pass. Reuse it; do **not** add a second clamp.

```rust
        // Out-of-hours (spec 2026-08-03): the device is locked, the foreground
        // app is one the family agreed is open at any hour, and she is
        // actually looking at it.
        //
        // `activity` is FrozenByCharter here — a lock credits no screen time —
        // so this counter is genuinely separate from the budget rather than
        // merely excluded from it.
        //
        // `screen_interactive` is load-bearing, not a nicety. A sleep timer
        // stops the audio but leaves the app in the foreground, so counting
        // regardless of screen state would report eight hours for a
        // thirty-minute session and make the guardian's line useless in
        // exactly the case it exists for.
        if self.enforcer.is_locked() && screen_interactive {
            if let Some(fg) = foreground_pkg {
                let open: Vec<String> = serde_json::from_str::<serde_json::Value>(
                    &self.always_available_view(now, true, &lock_reason),
                )
                .ok()
                .and_then(|v| serde_json::from_value(v["open"].clone()).ok())
                .unwrap_or_default();
                if open.iter().any(|p| p == fg) {
                    if let Some(u) = self.usage.as_mut() {
                        u.mark_out_of_hours(elapsed);
                    }
                }
            }
        }
```

`lock_reason` must be the same string the decision reports (`"schedule"` / `"budget"` / `"standdown"` / `"malformed"`) — the `reason_str` helper at `:2440-2443` maps `LockReason` to it. If the tick does not already have it in scope at this point, derive it there from the same source the decision does; do not re-derive the lock independently.

Borrow note: `always_available_view` takes `&self` and `self.usage.as_mut()` takes `&mut self`, so resolve `open` into an owned `Vec<String>` **before** touching `self.usage`, exactly as written above.

- [ ] **Step 6: Write a warden-level test**

Add to `warden.rs`'s test module a test asserting that a tick with the device locked, a schedule lock reason, and an always-available foreground package moves `out_of_hours_today_secs` and leaves `used_today_secs` unchanged. Follow the idiom of the nearest existing accrual test.

- [ ] **Step 7: Run the gates**

Run: `cd core && cargo test && cd ../android/jni && cargo clippy --all-targets -- -D warnings && cargo test`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add core/crates/charter-schedule/src/usage.rs android/jni/src/warden.rs
git commit -m "feat(usage): out-of-hours counter — a fact about the day, not a pool of time"
```

---

### Task 7: The guardian wire

**Files:**
- Modify: `apps/charter-app/src/wire/types.ts` (`ClauseKind` union :50-64; new `GrantAlwaysAvailable`; add to the `GrantBody` union :434)
- Modify: `apps/charter-app/src/domain/types.ts` (new `AlwaysAvailablePolicy`; add to `Policy` :255)
- Modify: `apps/charter-app/src/wire/clause.ts` (import :11,:25; new `alwaysAvailableToGrant`; emit in the clause list :408)
- Test: `apps/charter-app/src/wire/clause.test.ts` (or create `alwaysAvailable.test.ts` beside it, matching the repo's existing convention — check which exists first)

**Interfaces:**
- Consumes: nothing from earlier tasks (TypeScript side is independent until the wire meets the device).
- Produces:
  - `AlwaysAvailableEntry { pkg: string; untilUnix?: number }`
  - `AlwaysAvailablePolicy { apps: AlwaysAvailableEntry[] }`
  - `GrantAlwaysAvailable { v: 1; issuedAt: number; apps: AlwaysAvailableEntry[] }`
  - `alwaysAvailableToGrant(policy: AlwaysAvailablePolicy, issuedAt: number): GrantAlwaysAvailable`

- [ ] **Step 1: Write the failing tests**

```ts
import { describe, expect, it } from "vitest";
import { alwaysAvailableToGrant } from "./clause";

describe("alwaysAvailableToGrant", () => {
  it("carries a standing app with no expiry field at all", () => {
    const g = alwaysAvailableToGrant({ apps: [{ pkg: "com.book" }] }, 1_000);
    expect(g).toEqual({ v: 1, issuedAt: 1_000, apps: [{ pkg: "com.book" }] });
    expect("untilUnix" in g.apps[0]).toBe(false);
  });

  it("carries an absolute expiry through unchanged", () => {
    const g = alwaysAvailableToGrant(
      { apps: [{ pkg: "com.chat", untilUnix: 5_000 }] },
      1_000,
    );
    expect(g.apps[0]).toEqual({ pkg: "com.chat", untilUnix: 5_000 });
  });

  // An entry that expired before it was even signed is noise on the wire and
  // reads as a live grant in the guardian's own summary.
  it("drops an entry that has already expired at signing time", () => {
    const g = alwaysAvailableToGrant(
      { apps: [{ pkg: "com.chat", untilUnix: 900 }, { pkg: "com.book" }] },
      1_000,
    );
    expect(g.apps).toEqual([{ pkg: "com.book" }]);
  });

  it("emits an empty list rather than omitting the field", () => {
    expect(alwaysAvailableToGrant({ apps: [] }, 1_000).apps).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npm test -- alwaysAvailable`
Expected: FAIL — `alwaysAvailableToGrant is not exported`.

- [ ] **Step 3: Add the wire types**

In `apps/charter-app/src/wire/types.ts`, add `| "alwaysavailable"` to `ClauseKind`, and after `GrantListening`:

```ts
/**
 * Contract `AlwaysAvailableApp` — one app open at any hour.
 *
 * `untilUnix` is ABSOLUTE, never a duration, for the same reason `AppHold`'s
 * is: a duration restarts every time the stored clause is re-read and the
 * grant would never end. Absent ⇒ standing, no end.
 */
export interface AlwaysAvailableEntry {
  pkg: string;
  untilUnix?: number;
}

/**
 * Contract `GrantAlwaysAvailable` (spec 2026-08-03) — apps openable at any
 * hour. Mirrors `charter_proto::AlwaysAvailableBody`.
 *
 * SEMANTICS: like `listening` and unlike most clauses here, an absent or
 * unusable one means NOTHING is exempt. It loosens enforcement, so it fails
 * closed.
 *
 * Exempt from the schedule and budget locks ONLY — never a stand-down, never a
 * malformed charter, never a standing block in `apps`.
 */
export interface GrantAlwaysAvailable {
  v: 1;
  issuedAt: number;
  apps: AlwaysAvailableEntry[];
}
```

Add `| GrantAlwaysAvailable` to the `GrantBody` union at :434.

- [ ] **Step 4: Add the domain type**

In `apps/charter-app/src/domain/types.ts`, beside `ListeningPolicy`:

```ts
/** The family's agreed list of apps open at any hour (spec 2026-08-03). */
export interface AlwaysAvailablePolicy {
  apps: AlwaysAvailableEntry[];
}
```

and to `Policy`, beside `listening?: ListeningPolicy;`:

```ts
  alwaysAvailable?: AlwaysAvailablePolicy;
```

Re-export `AlwaysAvailableEntry` from `domain/types.ts` or import it from the wire module, following whichever direction the file already uses for `AppHold`.

- [ ] **Step 5: Add the mapper**

In `apps/charter-app/src/wire/clause.ts`, beside `listeningToGrant`:

```ts
/**
 * Map the family's always-available list to the wire.
 *
 * Entries already past their expiry at signing time are dropped: they would be
 * inert on the device anyway, and leaving them on the wire makes the guardian's
 * own summary read as though a lapsed grant were live.
 */
export function alwaysAvailableToGrant(
  policy: AlwaysAvailablePolicy,
  issuedAt: number,
): GrantAlwaysAvailable {
  return {
    v: 1,
    issuedAt,
    apps: policy.apps
      .filter((a) => a.untilUnix === undefined || a.untilUnix > issuedAt)
      .map((a) =>
        a.untilUnix === undefined ? { pkg: a.pkg } : { pkg: a.pkg, untilUnix: a.untilUnix },
      ),
  };
}
```

and emit it beside the listening line (:408):

```ts
  if (policy.alwaysAvailable)
    out.push(base("alwaysavailable", alwaysAvailableToGrant(policy.alwaysAvailable, issuedAt)));
```

Add `AlwaysAvailablePolicy` to the domain import at :11 and `GrantAlwaysAvailable` to the wire import at :25. Add `GrantAlwaysAvailable` to the union at :391.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `npm test -- alwaysAvailable && npm run lint`
Expected: PASS, 4 tests, no lint errors.

- [ ] **Step 7: Commit**

```bash
git add apps/charter-app/src/wire/types.ts apps/charter-app/src/domain/types.ts \
        apps/charter-app/src/wire/clause.ts apps/charter-app/src/wire/alwaysAvailable.test.ts
git commit -m "feat(wire): GrantAlwaysAvailable — apps open at any hour, standing or expiring"
```

---

### Task 8: The guardian's editor

**Files:**
- Modify: `apps/charter-app/src/screens/Limits.tsx` — `SectionId` union :415-423, `SECTION_TITLE` :425-434, `SECTION_TAB_OF` :436-445, a `buildAlwaysAvailable` normaliser + `alwaysAvailableSummary` beside `buildListening`/`listeningSummary` :311-325, the editor component beside the listening one :1123-1240, and the draft/dirty/save wiring at :2292, :2391, :2523, :2654, :2711, :2882
- Test: `apps/charter-app/src/screens/limitsSections.test.ts`

**Interfaces:**
- Consumes: `AlwaysAvailablePolicy`, `AlwaysAvailableEntry` (Task 7).
- Produces: `buildAlwaysAvailable(src?: AlwaysAvailablePolicy): AlwaysAvailablePolicy`, `alwaysAvailableSummary(a: AlwaysAvailablePolicy, now: number): string`.

The section goes on the **`content`** tab (labelled "Apps & web"), not `time` — it is about which apps, not about how time is counted. Title: **"Always available"**.

- [ ] **Step 1: Write the failing tests**

In `apps/charter-app/src/screens/limitsSections.test.ts`:

```ts
describe("alwaysAvailableSummary", () => {
  const NOW = 1_700_000_000;

  it("says plainly when nothing is named", () => {
    expect(alwaysAvailableSummary({ apps: [] }, NOW)).toBe("Nothing — the lock takes everything");
  });

  it("names the apps when they are all standing", () => {
    expect(
      alwaysAvailableSummary({ apps: [{ pkg: "com.book" }, { pkg: "com.pods" }] }, NOW),
    ).toBe("2 apps, always");
  });

  it("counts a temporary grant separately, because it will lapse", () => {
    expect(
      alwaysAvailableSummary(
        { apps: [{ pkg: "com.book" }, { pkg: "com.chat", untilUnix: NOW + 3600 }] },
        NOW,
      ),
    ).toBe("1 app, always · 1 for now");
  });

  it("ignores an entry that has already lapsed", () => {
    expect(
      alwaysAvailableSummary({ apps: [{ pkg: "com.chat", untilUnix: NOW - 1 }] }, NOW),
    ).toBe("Nothing — the lock takes everything");
  });

  it("uses the singular for one app", () => {
    expect(alwaysAvailableSummary({ apps: [{ pkg: "com.book" }] }, NOW)).toBe("1 app, always");
  });
});

describe("buildAlwaysAvailable", () => {
  it("normalises an absent policy to an empty list", () => {
    expect(buildAlwaysAvailable(undefined)).toEqual({ apps: [] });
  });

  it("is idempotent, so a saved draft compares byte-identical", () => {
    const once = buildAlwaysAvailable({ apps: [{ pkg: "com.book" }] });
    expect(buildAlwaysAvailable(once)).toEqual(once);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npm test -- limitsSections`
Expected: FAIL — `alwaysAvailableSummary is not exported`.

- [ ] **Step 3: Add the normaliser and summary**

In `Limits.tsx`, beside `buildListening`/`listeningSummary`:

```ts
/** Normalize a saved always-available list. Defaults to empty — the behaviour
 *  before this clause existed. */
export function buildAlwaysAvailable(src?: AlwaysAvailablePolicy): AlwaysAvailablePolicy {
  return { apps: (src?.apps ?? []).map((a) => (a.untilUnix === undefined ? { pkg: a.pkg } : { pkg: a.pkg, untilUnix: a.untilUnix })) };
}

/** A lapsed entry is already inert on the device; never let the summary claim
 *  a grant that has ended. */
export function alwaysAvailableSummary(a: AlwaysAvailablePolicy, now: number): string {
  const live = a.apps.filter((x) => x.untilUnix === undefined || x.untilUnix > now);
  const standing = live.filter((x) => x.untilUnix === undefined).length;
  const temporary = live.length - standing;
  if (live.length === 0) return "Nothing — the lock takes everything";
  const parts: string[] = [];
  if (standing > 0) parts.push(`${standing} app${standing === 1 ? "" : "s"}, always`);
  if (temporary > 0) parts.push(`${temporary} for now`);
  return parts.join(" · ");
}
```

- [ ] **Step 4: Register the section**

Add `| "always-available"` to `SectionId`; `"always-available": "Always available"` to `SECTION_TITLE`; `"always-available": "content"` to `SECTION_TAB_OF`.

- [ ] **Step 5: Build the editor component**

Add an `AlwaysAvailableEditor` beside the listening editor. Copy the listening editor's device-grouped app picker verbatim (`sections.map((s) => …)`, the `📱`/`💻` device headers, and the "Nothing to choose yet" empty states) — a merged list mixes a laptop's flatpak ids with a phone's Android packages as though they were one vocabulary, which is why that idiom exists.

Above the picker, an explanatory paragraph:

```tsx
<p className="card-sub">
  These apps open at any hour, even when the phone is otherwise locked, and
  their time is never counted against her limit. An audiobook player at 2am, a
  messaging app at a sleepover.
</p>
<p className="card-sub">
  They stay shut for a &ldquo;Finish now&rdquo;, and an app you have blocked in
  Apps stays blocked. Charter cannot filter inside an app it is letting through
  &mdash; choose apps you would be content with unsupervised.
</p>
```

Each chosen app gets an expiry control: a two-way choice between **Always** and **Until**, writing an absolute unix-seconds `untilUnix`.

The existing absolute-instant picker is `apps/charter-app/src/components/AppHoldSheet.tsx` — `onPick: (untilUnix: number) => void`, driven by presets that convert to an absolute instant at press time (`Math.floor(Date.now() / 1000) + p.minutes * 60`, :126) plus a `bedtime` option (:138). Mirror its shape and its prop contract so the two read as one idiom to a guardian who has used a hold.

**Its presets do not fit, though, and must not simply be reused.** `AppHoldSheet` is built for "allow this for an hour" — same-day spans. A sleepover is two nights out. Add night-spanning presets for this sheet: *tomorrow morning*, *Sunday morning*, *a week*. Compute each from the guardian's local clock, and render the resolved instant back as plain text ("until Sun 3 Aug, 9:00am") so a preset never hides which moment was actually signed.

- [ ] **Step 6: Wire the draft, dirty flag, and save**

Mirror `listening` at each of these sites exactly:
- `:2292` — add `buildAlwaysAvailable(policy.alwaysAvailable)` to the draft-state initialiser
- `:2391-2392` — `savedAlwaysAvailable` memo + `alwaysAvailableDirty` JSON comparison
- `:2523` — add `|| alwaysAvailableDirty` to the aggregate dirty flag
- `:2654` — `alwaysAvailable: alwaysAvailableDirty ? draftAlwaysAvailable : undefined`
- `:2711` — reset the draft on a fresh policy
- `:2882` — the section registry entry: `{ id: "always-available", summary: alwaysAvailableSummary(draftAlwaysAvailable, Math.floor(Date.now() / 1000)), dirty: alwaysAvailableDirty }`

The save bar must stay outside the tabs — one clause signs every dimension, and moving it inside a tab was the 2026-08-01 trap.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `npm test && npm run lint`
Expected: PASS, no lint errors.

- [ ] **Step 8: Commit**

```bash
git add apps/charter-app/src/screens/Limits.tsx apps/charter-app/src/screens/limitsSections.test.ts
git commit -m "feat(mycharter): Always available section — name the apps open at any hour"
```

---

### Task 9: Show the counter to both of them

**Files:**
- Modify: `android/jni/src/warden.rs` (add `outOfHoursTodaySecs` to the status payload beside the existing usage counters)
- Modify: `core/crates/charter-proto/src/lib.rs` or the status payload struct (add the field; `#[serde(default)]` for older wards)
- Modify: `apps/charter-app/src/insights/usageHistory.ts` (new `outOfHoursLine`, beside `weekSummary`:191)
- Modify: `apps/charter-app/src/screens/Activity.tsx` (render the line in the weekly picture)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/ui/GroupMirror.kt` (the ward's own view of the same number)
- Test: `apps/charter-app/src/insights/usageHistory.test.ts` (existing suite)

**Interfaces:**
- Consumes: `UsageLedger::out_of_hours_nights_week()` and `out_of_hours_week_secs()` (Task 6). The *week* counters drive the line; `out_of_hours_today_secs` is the ward mirror's "tonight" figure.
- Produces: `outOfHoursLine(nights: number, totalMins: number): string | null` in the Activity screen — `null` when there is nothing to say.

- [ ] **Step 1: Write the failing test**

`outOfHoursLine` belongs in `apps/charter-app/src/insights/usageHistory.ts`, beside `weekSummary` (:191) — **not** in `Activity.tsx`, which has no formatter of its own. It takes **seconds** and delegates to the existing `humanDuration` (:184), and it uses `weekSummary`'s ` · ` separator and calm register: no scores, no praise, no blame.

Test in `apps/charter-app/src/insights/usageHistory.test.ts` (the existing suite for that module):

```ts
describe("outOfHoursLine", () => {
  // A family that never sets the clause must see no line at all, not a zero.
  it("says nothing when there is nothing to say", () => {
    expect(outOfHoursLine(0, 0)).toBeNull();
  });

  it("reads plainly for a single night", () => {
    expect(outOfHoursLine(1, 12 * 60)).toBe("1 night this week · 12m");
  });

  it("pluralises the nights", () => {
    expect(outOfHoursLine(3, 40 * 60)).toBe("3 nights this week · 40m");
  });

  it("delegates hours to humanDuration rather than rolling its own", () => {
    expect(outOfHoursLine(5, 135 * 60)).toBe("5 nights this week · 2h 15m");
  });
});
```

Implementation:

```ts
/** The week's out-of-hours use, in the same calm register as [weekSummary] —
 *  a fact about the week, never a warning. `null` when there is nothing to
 *  say, so a family that never set the clause sees no line at all. */
export function outOfHoursLine(nights: number, secs: number): string | null {
  if (nights === 0 || secs === 0) return null;
  return `${nights} night${nights > 1 ? "s" : ""} this week · ${humanDuration(secs)}`;
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npm test -- usageHistory`
Expected: FAIL — `outOfHoursLine is not exported`.

- [ ] **Step 3: Carry the counter on STATUS**

Add three fields to the status payload struct — `out_of_hours_today_secs`, `out_of_hours_week_secs`, `out_of_hours_nights_week` — each `Option<…>` with `#[serde(default, skip_serializing_if = "Option::is_none")]`, so an older ward that never sends them does not break a newer MyCharter. Populate them from the ledger at the same site the other usage counters are stamped, and add the camelCase mirrors (`outOfHoursTodaySecs`, `outOfHoursWeekSecs`, `outOfHoursNightsWeek`) to the TypeScript status type.

**Android stamps these.** Do not copy the `unrecognisedTodaySecs` posture — that counter is Linux-only and `warden.rs:6235` asserts Android leaves it unset. Out-of-hours is the reverse: Android is the only platform that produces it, so the existing test stays green untouched and a new assertion should confirm Android *does* stamp `outOfHoursTodaySecs` once the ledger is non-zero.

- [ ] **Step 4: Render the guardian's line**

In `Activity.tsx`, import `outOfHoursLine` from `../insights/usageHistory` and render it in the weekly picture beside the existing `weekSummary` output, styled as a quiet secondary line — not a warning, not a badge, no colour change. It is information, not an alarm. Render nothing at all when it returns `null`.

- [ ] **Step 5: Render the ward's line**

In `GroupMirror.kt`, add the same sentence to the ward's mirror, from the device's own ledger. **This is not optional and has no guardian-only variant** — she must see what is recorded about her. Use identical wording to the guardian's line.

- [ ] **Step 6: Run the gates**

Run: `npm test && npm run lint && cd core && cargo test && cd ../android && ./gradlew :app:testDebugUnitTest`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: surface out-of-hours use to guardian and ward alike

Same sentence on both screens. Transparency is the invariant, so there is no
guardian-only variant of this number."
```

---

### Task 10: Full gates and the hardware handoff

- [ ] **Step 1: Run every gate**

```bash
cd core && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd ../android/jni && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd .. && ./gradlew :app:testDebugUnitTest && ./gradlew :app:assembleDebug
cd .. && npm test && npm run lint
```

Expected: all PASS, no warnings.

- [ ] **Step 2: Verify no Linux code was touched**

Run: `git diff --stat main -- linux/`
Expected: empty. Linux parity is an explicit follow-on.

- [ ] **Step 3: Verify the release APK has no mock seams**

Follow `android/scripts/build-jni.sh` and the JNI stale-lib discipline: a debug build staged before a release publish once shipped the DEBUG lib with mock seams while the no-mock gate passed. Confirm the built APK is ~27 MB with 0 `wire_test_relay` strings before any publish.

- [ ] **Step 4: Hand decented the hardware round**

Do NOT claim this works on a phone. It is code-ready and gate-green; on-device is unconfirmed until decented runs the six checks in the spec's "Hardware gate" section. Give him one step at a time and do the diagnosis yourself.

---

## Open item (not blocking this plan)

Whether the daughter's phone is already provisioned as a Charter ward with Device Owner is unconfirmed. If it is a new device, provisioning is a separate job ahead of the hardware round — it needs decented's hands, and the no-wipe path requires no accounts and a single user on the phone.
