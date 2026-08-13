# Charter Web Content — Policy Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the pure, headless, zero-privilege policy core for Charter's web-content control: the `GrantContent` clause type, a new `charter-content` crate with a deterministic evaluator (allowlist/blocklist/quorum/parent-override/fail-closed), plus the `ClauseKind::Content` variant and the curator-web-list kind constant.

**Architecture:** Mirrors the existing `charter-schedule` device-enforced-clause pattern exactly. `GrantContent` lives in a new pure crate `charter-content` (just as `GrantSchedule`/`GrantBudget` live in `charter-schedule`); a pure `evaluate_content(&GrantContent, &[CuratorList]) -> EffectiveWebPolicy` function computes the effective decision with no I/O and no clock. Curator lists are an in-memory, already-verified type here (Nostr wire parsing is a later plan). `ClauseKind::Content` is added to `charter-proto`; the curator-list kind constant is added to `charter-primitives`.

**Tech Stack:** Rust (edition 2021, rustc 1.94), `serde` + `serde_json`, `charter-testkit` golden vectors. No `chrono` (v1 has no time-based content logic). No `mock`/`real` features (pure crate).

## Global Constraints

- Rust **edition 2021**, toolchain **1.94**. Build/test from the `linux/` workspace root.
- This crate is **pure**: no OS effects, no privilege, no clock, no `mock`/`real` features. v1 has **no time-based** content rules — the evaluator takes **no timestamp**.
- Wire structs use `#[serde(rename_all = "camelCase")]`; field enums use `#[serde(rename_all = "lowercase")]` — matching `charter-schedule/src/clause.rs` and `charter-proto`'s `ClauseKind`.
- **Determinism:** all set/map outputs use `BTreeSet` / `BTreeMap` (stable ordering for golden vectors and config materialization).
- **Fail-closed:** an unparseable clause **or** `paused: true` ⇒ `EffectiveWebPolicy::locked()` (all web blocked). `revoked: true` ⇒ `EffectiveWebPolicy::unrestricted()` (no constraint). These mirror `GrantBudget`'s `paused ⇒ 0`, `revoked ⇒ no constraint`.
- **Override precedence (both postures):** `parentDeny` > `parentAllow` > curators.
- **Quorum** (`quorumN`, default **2**, floored at **1**) applies **only** to admitting a domain into an **allowlist** via curators; a single curator `block` is sufficient in blocklist posture.
- **No on-device DOB:** `ageTier` is a coarse parent-set policy enum (`young|older`), never a birthdate — consistent with the workspace's `privacy_birthdate_guard`.
- Gate commands (must pass before every commit):
  - `cargo test -p charter-content`
  - `cargo fmt --all --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`

---

## File Structure

| File | Responsibility |
|---|---|
| `linux/crates/charter-content/Cargo.toml` | New pure crate manifest |
| `linux/crates/charter-content/src/lib.rs` | Module wiring + public re-exports |
| `linux/crates/charter-content/src/clause.rs` | `GrantContent` + `Posture`/`AgeTier`/`YoutubeRestrict` enums + helper methods |
| `linux/crates/charter-content/src/domain.rs` | `normalize_domain` |
| `linux/crates/charter-content/src/curator.rs` | In-memory `CuratorList`/`CuratorEntry`/`Rating` (verified-input types) |
| `linux/crates/charter-content/src/evaluate.rs` | `EffectiveWebPolicy` + `evaluate_content` + `evaluate_content_json` |
| `linux/crates/charter-content/tests/content_vectors.rs` | Golden-vector-driven contract test |
| `linux/crates/charter-testkit/vectors/content/content_vectors.json` | Golden vectors (the cross-repo content arithmetic) |
| `linux/Cargo.toml` | Register `charter-content` in the workspace |
| `linux/crates/charter-proto/src/clause.rs` | Add `ClauseKind::Content` variant + `store_key` |
| `linux/crates/charter-primitives/src/kinds.rs` | Add `CHARTER_CURATOR_WEB_LIST` constant |

---

## Task 1: Scaffold `charter-content` crate + `GrantContent` clause types

**Files:**
- Create: `linux/crates/charter-content/Cargo.toml`
- Create: `linux/crates/charter-content/src/lib.rs`
- Create: `linux/crates/charter-content/src/clause.rs`
- Modify: `linux/Cargo.toml` (workspace registration)

**Interfaces:**
- Produces: `charter_content::{GrantContent, Posture, AgeTier, YoutubeRestrict}`. `GrantContent` methods: `effective_quorum() -> u32`, `safe_search_on() -> bool`, `is_paused() -> bool`, `is_revoked() -> bool`.

- [ ] **Step 1: Register the crate in the workspace**

In `linux/Cargo.toml`, add `"crates/charter-content"` to the `[workspace] members` list (and to `default-members` if that key is present), and under `[workspace.dependencies]` add:

```toml
charter-content = { path = "crates/charter-content" }
```

- [ ] **Step 2: Create the crate manifest**

Create `linux/crates/charter-content/Cargo.toml`:

```toml
[package]
name = "charter-content"
version = "0.1.0"
edition = "2021"

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }

[dev-dependencies]
charter-testkit = { workspace = true }
```

- [ ] **Step 3: Create `src/lib.rs` with module wiring**

Create `linux/crates/charter-content/src/lib.rs`:

```rust
//! Pure policy core for Charter web-content control. No I/O, no clock, no
//! privilege. Mirrors the `charter-schedule` device-enforced-clause pattern.

pub mod clause;

pub use clause::{AgeTier, GrantContent, Posture, YoutubeRestrict};
```

- [ ] **Step 4: Write the failing round-trip test**

Append to `linux/crates/charter-content/src/clause.rs` (create the file with this test at the bottom for now):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grant_content_roundtrips_canonical_json() {
        let json = r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"parentAllow":["https://example.com/"],"safeSearch":true,"youtubeRestrict":"moderate","issuedAt":1700000000}"#;
        let c: GrantContent = serde_json::from_str(json).unwrap();
        assert_eq!(c.posture, Posture::Allowlist);
        assert_eq!(c.age_tier, AgeTier::Young);
        assert_eq!(c.curators, vec!["aa".to_string(), "bb".to_string()]);
        assert_eq!(c.effective_quorum(), 2);
        assert!(c.safe_search_on());
        assert_eq!(c.youtube_restrict, Some(YoutubeRestrict::Moderate));
        assert!(!c.is_paused());
        assert!(!c.is_revoked());
        // Round-trips.
        let back: GrantContent =
            serde_json::from_str(&serde_json::to_string(&c).unwrap()).unwrap();
        assert_eq!(c, back);
    }

    #[test]
    fn defaults_quorum_two_and_safesearch_on() {
        let json = r#"{"v":1,"posture":"blocklist","ageTier":"older","issuedAt":1}"#;
        let c: GrantContent = serde_json::from_str(json).unwrap();
        assert_eq!(c.effective_quorum(), 2);
        assert!(c.safe_search_on());
        assert!(c.curators.is_empty());
    }
}
```

- [ ] **Step 5: Run the test to verify it fails**

Run (from `linux/`): `cargo test -p charter-content`
Expected: FAIL — `GrantContent`, `Posture`, etc. not defined / file does not compile.

- [ ] **Step 6: Implement the clause types**

Put this **above** the `#[cfg(test)]` block in `linux/crates/charter-content/src/clause.rs`:

```rust
//! The `content` clause data shape (device-enforced, frozen alongside
//! `spec/contract.md`) and its enums.

use serde::{Deserialize, Serialize};

/// Enforcement posture for a child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Posture {
    Allowlist,
    Blocklist,
}

/// Coarse, parent-set policy selector. NOT derived from a date of birth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgeTier {
    Young,
    Older,
}

/// YouTube restricted-mode level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum YoutubeRestrict {
    #[default]
    Off,
    Moderate,
    Strict,
}

/// The runtime-canonical, **device-enforced** content clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantContent {
    pub v: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    pub posture: Posture,
    pub age_tier: AgeTier,
    #[serde(default)]
    pub curators: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quorum_n: Option<u32>,
    #[serde(default)]
    pub block_categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safe_search: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub youtube_restrict: Option<YoutubeRestrict>,
    #[serde(default)]
    pub parent_allow: Vec<String>,
    #[serde(default)]
    pub parent_deny: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<bool>,
    pub issued_at: u64,
}

impl GrantContent {
    /// Curators required to admit a domain into an allowlist. Default 2, floored at 1.
    pub fn effective_quorum(&self) -> u32 {
        self.quorum_n.unwrap_or(2).max(1)
    }

    /// SafeSearch defaults ON when unspecified.
    pub fn safe_search_on(&self) -> bool {
        self.safe_search.unwrap_or(true)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.unwrap_or(false)
    }

    pub fn is_revoked(&self) -> bool {
        self.revoked.unwrap_or(false)
    }
}
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `cargo test -p charter-content`
Expected: PASS (2 tests). Then `cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] **Step 8: Commit**

```bash
git add linux/Cargo.toml linux/crates/charter-content
git commit -m "feat(content): charter-content crate scaffold + GrantContent clause type"
```

---

## Task 2: Domain normalization

**Files:**
- Create: `linux/crates/charter-content/src/domain.rs`
- Modify: `linux/crates/charter-content/src/lib.rs`

**Interfaces:**
- Produces: `charter_content::domain::normalize_domain(raw: &str) -> String`.

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charter-content/src/domain.rs`:

```rust
//! Domain normalization for set comparison. v1 is intentionally simple
//! (lowercase, strip scheme / `www.` / path / port) — full Public-Suffix-List
//! handling is later work.

/// Normalize a domain or URL to a lowercase bare host.
pub fn normalize_domain(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
        }
    }
    // Cut the host off at the first path / query / port separator.
    let host_end = s.find(['/', '?', ':']).unwrap_or(s.len());
    let host = &s[..host_end];
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scheme_www_path_port_and_lowercases() {
        assert_eq!(normalize_domain("https://www.Example.com/foo?x=1"), "example.com");
        assert_eq!(normalize_domain("http://Example.COM:8080"), "example.com");
        assert_eq!(normalize_domain("  example.com  "), "example.com");
        assert_eq!(normalize_domain("sub.example.com"), "sub.example.com");
        assert_eq!(normalize_domain("www.example.com"), "example.com");
    }
}
```

- [ ] **Step 2: Wire the module**

In `linux/crates/charter-content/src/lib.rs`, add under the existing `pub mod clause;`:

```rust
pub mod domain;
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p charter-content domain`
Expected: PASS (the implementation is included with the test above). If it fails, fix `normalize_domain` until green.

- [ ] **Step 4: Commit**

```bash
git add linux/crates/charter-content/src
git commit -m "feat(content): domain normalization helper"
```

---

## Task 3: Evaluator core — curator types, `EffectiveWebPolicy`, fail-closed paths

**Files:**
- Create: `linux/crates/charter-content/src/curator.rs`
- Create: `linux/crates/charter-content/src/evaluate.rs`
- Modify: `linux/crates/charter-content/src/lib.rs`

**Interfaces:**
- Consumes: `GrantContent`, `normalize_domain`.
- Produces:
  - `charter_content::curator::{CuratorList, CuratorEntry, Rating}` — `CuratorList { curator: String, entries: Vec<CuratorEntry> }`, `CuratorEntry { domain: String, rating: Rating }`, `Rating::{KidSafe, Block, Category(String)}`.
  - `charter_content::evaluate::EffectiveWebPolicy` with fields `posture: Option<Posture>`, `locked: bool`, `allow_domains: BTreeSet<String>`, `block_domains: BTreeSet<String>`, `block_categories: BTreeSet<String>`, `safe_search: bool`, `youtube_restrict: YoutubeRestrict`; constructors `EffectiveWebPolicy::locked()` and `EffectiveWebPolicy::unrestricted()`.
  - `evaluate_content(clause: &GrantContent, lists: &[CuratorList]) -> EffectiveWebPolicy`.
  - `evaluate_content_json(clause_json: &str, lists: &[CuratorList]) -> EffectiveWebPolicy`.

- [ ] **Step 1: Define the curator input types**

Create `linux/crates/charter-content/src/curator.rs`:

```rust
//! In-memory, already-signature-verified curator list. The Nostr wire parsing
//! that produces these lives in a later plan (`charter-transport`).

/// A curator's rating of a domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rating {
    KidSafe,
    Block,
    Category(String),
}

/// One `(domain, rating)` entry in a curator's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorEntry {
    pub domain: String,
    pub rating: Rating,
}

/// A signed curator list, keyed by the curator's pubkey (64-hex).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CuratorList {
    pub curator: String,
    pub entries: Vec<CuratorEntry>,
}
```

- [ ] **Step 2: Wire the modules**

In `linux/crates/charter-content/src/lib.rs`, add and extend the re-exports:

```rust
pub mod curator;
pub mod evaluate;

pub use curator::{CuratorEntry, CuratorList, Rating};
pub use evaluate::{evaluate_content, evaluate_content_json, EffectiveWebPolicy};
```

- [ ] **Step 3: Write the failing fail-closed tests**

Create `linux/crates/charter-content/src/evaluate.rs` with this test module at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::clause::{AgeTier, Posture};

    fn base_clause() -> GrantContent {
        GrantContent {
            v: 1,
            tz: None,
            posture: Posture::Allowlist,
            age_tier: AgeTier::Young,
            curators: vec![],
            quorum_n: None,
            block_categories: vec![],
            safe_search: None,
            youtube_restrict: None,
            parent_allow: vec![],
            parent_deny: vec![],
            paused: None,
            revoked: None,
            issued_at: 1,
        }
    }

    #[test]
    fn paused_locks_all_web() {
        let mut c = base_clause();
        c.paused = Some(true);
        let p = evaluate_content(&c, &[]);
        assert!(p.locked);
        assert_eq!(p.posture, None);
    }

    #[test]
    fn revoked_is_unrestricted() {
        let mut c = base_clause();
        c.revoked = Some(true);
        let p = evaluate_content(&c, &[]);
        assert!(!p.locked);
        assert_eq!(p.posture, None);
        assert!(p.allow_domains.is_empty());
    }

    #[test]
    fn unparseable_json_fails_closed() {
        let p = evaluate_content_json("{ not json", &[]);
        assert!(p.locked);
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p charter-content evaluate`
Expected: FAIL — `EffectiveWebPolicy`, `evaluate_content`, `evaluate_content_json` not defined.

- [ ] **Step 5: Implement the evaluator core**

Put this **above** the `#[cfg(test)]` block in `linux/crates/charter-content/src/evaluate.rs`:

```rust
//! The pure content evaluator: `GrantContent` + verified curator lists →
//! `EffectiveWebPolicy`. No I/O, no clock. Deterministic (`BTreeSet`).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::clause::{GrantContent, Posture, YoutubeRestrict};
use crate::curator::CuratorList;

/// The materializable effective decision for a child's web access.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectiveWebPolicy {
    /// `None` when revoked / no constraint.
    pub posture: Option<Posture>,
    /// Paused or fail-closed → block all web.
    pub locked: bool,
    pub allow_domains: BTreeSet<String>,
    pub block_domains: BTreeSet<String>,
    pub block_categories: BTreeSet<String>,
    pub safe_search: bool,
    pub youtube_restrict: YoutubeRestrict,
}

impl EffectiveWebPolicy {
    /// Fail-closed: web fully blocked (unparseable clause or `paused`).
    pub fn locked() -> Self {
        EffectiveWebPolicy {
            posture: None,
            locked: true,
            allow_domains: BTreeSet::new(),
            block_domains: BTreeSet::new(),
            block_categories: BTreeSet::new(),
            safe_search: true,
            youtube_restrict: YoutubeRestrict::Off,
        }
    }

    /// No constraint (revoked): web unrestricted.
    pub fn unrestricted() -> Self {
        EffectiveWebPolicy {
            posture: None,
            locked: false,
            allow_domains: BTreeSet::new(),
            block_domains: BTreeSet::new(),
            block_categories: BTreeSet::new(),
            safe_search: false,
            youtube_restrict: YoutubeRestrict::Off,
        }
    }
}

/// Parse + evaluate, failing **closed** (locked) on any deserialize error.
pub fn evaluate_content_json(clause_json: &str, lists: &[CuratorList]) -> EffectiveWebPolicy {
    match serde_json::from_str::<GrantContent>(clause_json) {
        Ok(c) => evaluate_content(&c, lists),
        Err(_) => EffectiveWebPolicy::locked(),
    }
}

/// Evaluate a parsed clause against verified curator lists.
pub fn evaluate_content(clause: &GrantContent, lists: &[CuratorList]) -> EffectiveWebPolicy {
    if clause.is_revoked() {
        return EffectiveWebPolicy::unrestricted();
    }
    if clause.is_paused() {
        return EffectiveWebPolicy::locked();
    }

    let safe_search = clause.safe_search_on();
    let youtube_restrict = clause.youtube_restrict.unwrap_or(YoutubeRestrict::Off);

    // Posture-specific domain sets are filled in by Tasks 4 and 5. For now,
    // return the posture with empty sets so the fail-closed paths compile/pass.
    EffectiveWebPolicy {
        posture: Some(clause.posture),
        locked: false,
        allow_domains: BTreeSet::new(),
        block_domains: BTreeSet::new(),
        block_categories: BTreeSet::new(),
        safe_search,
        youtube_restrict,
    }
}
```

> Note: this stub returns the posture with **empty** domain sets — the fail-closed
> tests pass now; the allowlist/blocklist branches (and their `normalize_domain` /
> `Rating` / `BTreeMap` imports) arrive in Tasks 4–5. No unused imports here, so
> clippy stays green.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p charter-content`
Expected: PASS (all tests so far). Then `cargo fmt --all --check` and `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] **Step 7: Commit**

```bash
git add linux/crates/charter-content/src
git commit -m "feat(content): EffectiveWebPolicy + evaluator core with fail-closed paths"
```

---

## Task 4: Allowlist posture — quorum + parent override

**Files:**
- Modify: `linux/crates/charter-content/src/evaluate.rs`

**Interfaces:**
- Consumes: `evaluate_content`, `CuratorList`, `Rating::KidSafe`.
- Produces: allowlist branch of `evaluate_content` filling `allow_domains`.

- [ ] **Step 1: Write the failing allowlist tests**

Add these tests inside the `mod tests` block in `linux/crates/charter-content/src/evaluate.rs`:

```rust
    use crate::curator::{CuratorEntry, CuratorList, Rating};

    fn list(curator: &str, entries: &[(&str, Rating)]) -> CuratorList {
        CuratorList {
            curator: curator.to_string(),
            entries: entries
                .iter()
                .map(|(d, r)| CuratorEntry { domain: d.to_string(), rating: r.clone() })
                .collect(),
        }
    }

    #[test]
    fn allowlist_admits_domain_only_at_quorum() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into(), "bb".into()];
        c.quorum_n = Some(2);
        let lists = vec![
            list("aa", &[("kids.example", Rating::KidSafe), ("solo.example", Rating::KidSafe)]),
            list("bb", &[("kids.example", Rating::KidSafe)]),
        ];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("kids.example")); // 2 curators → admitted
        assert!(!p.allow_domains.contains("solo.example")); // 1 curator → below quorum
    }

    #[test]
    fn allowlist_ignores_unsubscribed_curators() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into()];
        c.quorum_n = Some(1);
        let lists = vec![list("zz", &[("evil.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.is_empty()); // zz not subscribed
    }

    #[test]
    fn allowlist_parent_overrides_win() {
        let mut c = base_clause();
        c.posture = Posture::Allowlist;
        c.curators = vec!["aa".into()];
        c.quorum_n = Some(1);
        c.parent_allow = vec!["https://parent.example/".into()];
        c.parent_deny = vec!["kids.example".into()];
        let lists = vec![list("aa", &[("kids.example", Rating::KidSafe)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.allow_domains.contains("parent.example")); // parentAllow added
        assert!(!p.allow_domains.contains("kids.example")); // parentDeny beats curator
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p charter-content allowlist`
Expected: FAIL — `allow_domains` is empty (Task 3 stub).

- [ ] **Step 3: Implement the allowlist branch**

In `evaluate.rs`, extend the imports — add `use std::collections::BTreeMap;`, add `use crate::domain::normalize_domain;`, and change `use crate::curator::CuratorList;` to `use crate::curator::{CuratorList, Rating};` — then replace the stub `EffectiveWebPolicy { posture: Some(clause.posture), .. }` tail of `evaluate_content` with a `match clause.posture` whose `Allowlist` arm is implemented (leave a `Blocklist` arm stub returning empty sets for Task 5):

```rust
    let parent_allow: BTreeSet<String> =
        clause.parent_allow.iter().map(|d| normalize_domain(d)).collect();
    let parent_deny: BTreeSet<String> =
        clause.parent_deny.iter().map(|d| normalize_domain(d)).collect();
    let subscribed: BTreeSet<&str> = clause.curators.iter().map(|s| s.as_str()).collect();

    match clause.posture {
        Posture::Allowlist => {
            let quorum = clause.effective_quorum() as usize;
            // domain -> distinct curators that rated it kid-safe
            let mut votes: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            for l in lists {
                if !subscribed.contains(l.curator.as_str()) {
                    continue;
                }
                for e in &l.entries {
                    if matches!(e.rating, Rating::KidSafe) {
                        votes
                            .entry(normalize_domain(&e.domain))
                            .or_default()
                            .insert(l.curator.clone());
                    }
                }
            }
            let mut allow: BTreeSet<String> = votes
                .into_iter()
                .filter(|(_, curators)| curators.len() >= quorum)
                .map(|(d, _)| d)
                .collect();
            allow.extend(parent_allow.iter().cloned());
            for d in &parent_deny {
                allow.remove(d);
            }
            EffectiveWebPolicy {
                posture: Some(Posture::Allowlist),
                locked: false,
                allow_domains: allow,
                block_domains: BTreeSet::new(),
                block_categories: BTreeSet::new(),
                safe_search,
                youtube_restrict,
            }
        }
        Posture::Blocklist => EffectiveWebPolicy {
            posture: Some(Posture::Blocklist),
            locked: false,
            allow_domains: BTreeSet::new(),
            block_domains: BTreeSet::new(),
            block_categories: BTreeSet::new(),
            safe_search,
            youtube_restrict,
        },
    }
```

Make sure the imports line reads `use crate::curator::{CuratorList, Rating};` (add `Rating`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p charter-content`
Expected: PASS (all tests). `cargo fmt --all --check` + `cargo clippy --all-targets --all-features -- -D warnings` clean.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charter-content/src
git commit -m "feat(content): allowlist posture — curator quorum + parent override"
```

---

## Task 5: Blocklist posture — curator blocks, categories, parent override

**Files:**
- Modify: `linux/crates/charter-content/src/evaluate.rs`

**Interfaces:**
- Consumes: `evaluate_content`, `Rating::Block`, `clause.block_categories`.
- Produces: blocklist branch filling `block_domains` + `block_categories`.

- [ ] **Step 1: Write the failing blocklist tests**

Add inside `mod tests`:

```rust
    #[test]
    fn blocklist_aggregates_curator_blocks_and_categories() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.curators = vec!["aa".into()];
        c.block_categories = vec!["Gambling".into(), "porn".into()];
        let lists = vec![list("aa", &[("bad.example", Rating::Block)])];
        let p = evaluate_content(&c, &lists);
        assert!(p.block_domains.contains("bad.example"));
        // categories are lowercased, passed through (DNS layer expands them)
        assert!(p.block_categories.contains("gambling"));
        assert!(p.block_categories.contains("porn"));
    }

    #[test]
    fn blocklist_parent_overrides_win() {
        let mut c = base_clause();
        c.posture = Posture::Blocklist;
        c.curators = vec!["aa".into()];
        c.parent_allow = vec!["bad.example".into()]; // unblock a curator block
        c.parent_deny = vec!["https://extra.example/".into()]; // add a block
        let lists = vec![list("aa", &[("bad.example", Rating::Block)])];
        let p = evaluate_content(&c, &lists);
        assert!(!p.block_domains.contains("bad.example")); // parentAllow removed it
        assert!(p.block_domains.contains("extra.example")); // parentDeny added it
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p charter-content blocklist`
Expected: FAIL — `block_domains` / `block_categories` empty (Task 4 stub arm).

- [ ] **Step 3: Implement the blocklist branch**

Replace the stub `Posture::Blocklist => EffectiveWebPolicy { .. }` arm with:

```rust
        Posture::Blocklist => {
            let mut block: BTreeSet<String> = BTreeSet::new();
            for l in lists {
                if !subscribed.contains(l.curator.as_str()) {
                    continue;
                }
                for e in &l.entries {
                    if matches!(e.rating, Rating::Block) {
                        block.insert(normalize_domain(&e.domain));
                    }
                }
            }
            let categories: BTreeSet<String> = clause
                .block_categories
                .iter()
                .map(|c| c.to_ascii_lowercase())
                .collect();
            // parentAllow unblocks; parentDeny adds (parentDeny beats parentAllow).
            for d in &parent_allow {
                block.remove(d);
            }
            block.extend(parent_deny.iter().cloned());
            EffectiveWebPolicy {
                posture: Some(Posture::Blocklist),
                locked: false,
                allow_domains: BTreeSet::new(),
                block_domains: block,
                block_categories: categories,
                safe_search,
                youtube_restrict,
            }
        }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p charter-content`
Expected: PASS (all tests). `cargo fmt --all --check` + clippy clean.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charter-content/src
git commit -m "feat(content): blocklist posture — curator blocks + categories + parent override"
```

---

## Task 6: Golden-vector contract suite

**Files:**
- Create: `linux/crates/charter-testkit/vectors/content/content_vectors.json`
- Create: `linux/crates/charter-content/tests/content_vectors.rs`

**Interfaces:**
- Consumes: `evaluate_content`, `GrantContent`, `CuratorList`, `Rating`, `EffectiveWebPolicy`, `Posture`, `charter_testkit::golden::load_json`.

- [ ] **Step 1: Create the vector file**

Create `linux/crates/charter-testkit/vectors/content/content_vectors.json`:

```json
{
  "vectors": [
    {
      "name": "paused_locks",
      "clause": { "v": 1, "posture": "allowlist", "ageTier": "young", "paused": true, "issuedAt": 1 },
      "lists": [],
      "expect": { "locked": true }
    },
    {
      "name": "revoked_unrestricted",
      "clause": { "v": 1, "posture": "blocklist", "ageTier": "older", "revoked": true, "issuedAt": 1 },
      "lists": [],
      "expect": { "locked": false }
    },
    {
      "name": "allowlist_quorum_two",
      "clause": { "v": 1, "posture": "allowlist", "ageTier": "young", "curators": ["aa", "bb"], "quorumN": 2, "issuedAt": 1 },
      "lists": [
        { "curator": "aa", "entries": [ { "domain": "kids.example", "rating": "kid-safe" }, { "domain": "solo.example", "rating": "kid-safe" } ] },
        { "curator": "bb", "entries": [ { "domain": "kids.example", "rating": "kid-safe" } ] }
      ],
      "expect": { "locked": false, "posture": "allowlist", "allow": ["kids.example"] }
    },
    {
      "name": "allowlist_parent_deny_beats_curator",
      "clause": { "v": 1, "posture": "allowlist", "ageTier": "young", "curators": ["aa"], "quorumN": 1, "parentDeny": ["kids.example"], "parentAllow": ["parent.example"], "issuedAt": 1 },
      "lists": [ { "curator": "aa", "entries": [ { "domain": "kids.example", "rating": "kid-safe" } ] } ],
      "expect": { "locked": false, "posture": "allowlist", "allow": ["parent.example"] }
    },
    {
      "name": "blocklist_curator_and_category",
      "clause": { "v": 1, "posture": "blocklist", "ageTier": "older", "curators": ["aa"], "blockCategories": ["gambling"], "issuedAt": 1 },
      "lists": [ { "curator": "aa", "entries": [ { "domain": "bad.example", "rating": "block" } ] } ],
      "expect": { "locked": false, "posture": "blocklist", "block": ["bad.example"], "categories": ["gambling"] }
    }
  ]
}
```

- [ ] **Step 2: Write the failing vector-driven test**

Create `linux/crates/charter-content/tests/content_vectors.rs`:

```rust
//! The shared `content_vectors.json` — the device-enforced content arithmetic.

use charter_content::{
    evaluate_content, CuratorEntry, CuratorList, GrantContent, Rating,
};

#[derive(serde::Deserialize)]
struct VecFile {
    vectors: Vec<VecCase>,
}

#[derive(serde::Deserialize)]
struct VecCase {
    name: String,
    clause: GrantContent,
    #[serde(default)]
    lists: Vec<VecList>,
    expect: VecExpect,
}

#[derive(serde::Deserialize)]
struct VecList {
    curator: String,
    entries: Vec<VecEntry>,
}

#[derive(serde::Deserialize)]
struct VecEntry {
    domain: String,
    rating: String,
}

#[derive(serde::Deserialize, Default)]
struct VecExpect {
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    posture: Option<String>,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    block: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
}

fn parse_rating(s: &str) -> Rating {
    match s {
        "kid-safe" => Rating::KidSafe,
        "block" => Rating::Block,
        other => match other.strip_prefix("category:") {
            Some(cat) => Rating::Category(cat.to_string()),
            None => panic!("bad rating {other}"),
        },
    }
}

#[test]
fn content_vectors_match() {
    let v: VecFile = charter_testkit::golden::load_json("content/content_vectors.json");
    for case in &v.vectors {
        let lists: Vec<CuratorList> = case
            .lists
            .iter()
            .map(|l| CuratorList {
                curator: l.curator.clone(),
                entries: l
                    .entries
                    .iter()
                    .map(|e| CuratorEntry {
                        domain: e.domain.clone(),
                        rating: parse_rating(&e.rating),
                    })
                    .collect(),
            })
            .collect();

        let got = evaluate_content(&case.clause, &lists);
        assert_eq!(got.locked, case.expect.locked, "{} locked", case.name);
        if let Some(p) = &case.expect.posture {
            let got_p = format!("{:?}", got.posture.unwrap()).to_lowercase();
            assert_eq!(&got_p, p, "{} posture", case.name);
        }
        for d in &case.expect.allow {
            assert!(got.allow_domains.contains(d), "{} allow {d}", case.name);
        }
        for d in &case.expect.block {
            assert!(got.block_domains.contains(d), "{} block {d}", case.name);
        }
        for cat in &case.expect.categories {
            assert!(got.block_categories.contains(cat), "{} cat {cat}", case.name);
        }
    }
}
```

- [ ] **Step 3: Run the test to verify it passes**

Run: `cargo test -p charter-content --test content_vectors`
Expected: PASS (5 vectors). If a `posture` assert fails, confirm `format!("{:?}", Posture::Allowlist).to_lowercase()` yields `"allowlist"` — it does for these variant names.

- [ ] **Step 4: Commit**

```bash
git add linux/crates/charter-content/tests linux/crates/charter-testkit/vectors/content
git commit -m "test(content): golden-vector contract suite for the content evaluator"
```

---

## Task 7: Add `ClauseKind::Content` to `charter-proto`

**Files:**
- Modify: `linux/crates/charter-proto/src/clause.rs`
- Test: `linux/crates/charter-proto/src/clause.rs` (inline `#[cfg(test)]`)

**Interfaces:**
- Produces: `charter_proto::clause::ClauseKind::Content` with `store_key() == 3` and JSON value `"content"`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)]` module in `linux/crates/charter-proto/src/clause.rs` (create one if absent, matching the file's existing imports):

```rust
    #[test]
    fn content_kind_serializes_and_has_store_key() {
        let k = ClauseKind::Content;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"content\"");
        assert_eq!(k.store_key(), 3);
        let back: ClauseKind = serde_json::from_str("\"content\"").unwrap();
        assert_eq!(back, ClauseKind::Content);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p charter-proto content_kind`
Expected: FAIL — no `Content` variant.

- [ ] **Step 3: Add the variant**

In `ClauseKind`, add `Content` after `Budget`; in `store_key`, add the arm:

```rust
            ClauseKind::Content => 3,
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p charter-proto`
Expected: PASS. `cargo fmt --all --check` + clippy clean.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charter-proto/src/clause.rs
git commit -m "feat(proto): add ClauseKind::Content variant (store_key 3)"
```

---

## Task 8: Add the curator-web-list kind constant to `charter-primitives`

**Files:**
- Modify: `linux/crates/charter-primitives/src/kinds.rs`

**Interfaces:**
- Produces: `charter_primitives::kinds::CHARTER_CURATOR_WEB_LIST: u16 = 30100` (provisional; final value is design open-question §10.1).

- [ ] **Step 1: Write the failing freeze test**

In the existing kind-freeze test in `linux/crates/charter-primitives/src/kinds.rs` (the one asserting `CHARTER_DEVICE_REQUEST == 31111`, etc.), add:

```rust
    assert_eq!(CHARTER_CURATOR_WEB_LIST, 30100);
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p charter-primitives`
Expected: FAIL — `CHARTER_CURATOR_WEB_LIST` not defined.

- [ ] **Step 3: Add the constant**

Add near the other `CHARTER_*` constants:

```rust
/// Addressable (NIP-51-style) curator web-list: a curator's signed allow/deny
/// entries; parents subscribe by pubkey. PROVISIONAL — final value is design
/// open-question §10.1 (`docs/superpowers/specs/2026-06-28-charter-web-content-control-design.md`).
pub const CHARTER_CURATOR_WEB_LIST: u16 = 30100;
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p charter-primitives`
Expected: PASS. `cargo fmt --all --check` + clippy clean.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charter-primitives/src/kinds.rs
git commit -m "feat(primitives): add CHARTER_CURATOR_WEB_LIST kind (30100, provisional)"
```

---

## Final verification

- [ ] Run the full headless gate from `linux/`:

```bash
cargo test --workspace
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: all green. `charter-content` builds, its unit + vector tests pass, and `charter-proto` / `charter-primitives` additions pass.

---

## What this plan deliberately does NOT cover (later plans)

- **Plan 2 — curator-list transport ingest** (`charter-transport`): subscribe to subscribed curators' Nostr events (kind `CHARTER_CURATOR_WEB_LIST` + NIP-32 `1985` labels), verify signatures, cache, and produce `CuratorList` values for this evaluator.
- **Plan 3 — `charter-sys` ports + `charterd` enactors**: `WebPolicyOps` (Firefox `policies.json`) + `DnsFilterOps` (AdGuard Home / resolv.conf / firewall), the materialize loop, fail-closed locking, and exec-lock browser-argument constraints (real OS effects, `mock`/`real`).
- **Plan 4 — parent app** (`apps/charter-app`): posture/ageTier, curator subscribe + override UI, minimal curator-publish.
- The `charter_set_content` / `charter_set_default_content` NIP-46 methods (Signet/bunker side).
