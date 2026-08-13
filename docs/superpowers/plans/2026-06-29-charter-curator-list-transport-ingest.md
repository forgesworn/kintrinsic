# Charter Curator-List Transport Ingest Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Subscribe to subscribed curators' signed web lists over Nostr (kind 30100), verify their signatures, cache them last-known-good, and feed them into the content evaluator — which `charterd`'s web-content enforcer currently calls with an empty slice (`evaluate_content_json(&clause_json, &[])`).

**Architecture:** Mirror the existing CLAUSE rail (transport delivers → daemon authenticates → store caches → enforcer reads store). A new pure wire-parser turns a kind-30100 `NostrEvent` into the evaluator's existing `CuratorList`. A new `CharterTransport::poll_curator_lists` fetches + signature-verifies + dedups (NIP-51 replaceable). A new `CuratorListStore` port caches verified lists with monotonic rollback protection (exactly like `ClauseStore`). A free `refresh_curator_lists` function reads the cached `content` clause's `curators[]`, fetches, and writes the cache; the `WebContentEnforcer::reconcile` loads the cache and passes it to the evaluator. Fetch failure writes nothing, so the last-known-good cache stands and an allowlist never widens on failure (fail-closed).

**Tech Stack:** Rust 2021, async via `tokio`, `async-trait` ports, `serde`/`serde_json`, BIP-340 schnorr over the single `charter-crypto` backend, mock/real feature discipline.

## Global Constraints

- Edition 2021, `rust-version = 1.94`, license MIT; all crate versions `0.1.0`. (verbatim from `linux/Cargo.toml`)
- Internal crate deps are declared via `{ workspace = true }`; the path entries already exist in `[workspace.dependencies]` (e.g. `charter-content = { path = "crates/charter-content" }`).
- Mock/real feature discipline: every port gets a `Mock*` impl (used by headless tests) and a compile-only `Real*` stub returning `SysError::NotImplemented`. `default = ["mock"]`.
- All four gates must pass, run from the `linux/` directory:
  1. `cargo fmt --all --check`
  2. `cargo clippy --all-targets --all-features -- -D warnings`
  3. `cargo test --workspace`
  4. `cargo build --workspace --no-default-features --features real`
- Determinism: use `BTreeMap`/`BTreeSet` for any collection whose iteration feeds output; tests use fixed timestamps + `ScriptedEntropy`, never wall-clock.
- Privacy: curator lists carry domains only; no per-URL/child data anywhere; audit content stays empty (unchanged by this plan).
- Fail-closed: a fetch failure must never widen an allowlist; the cache is last-known-good.
- The signature/integrity gate is the single `charter-verify` backend (`signature_is_valid`, `id_is_consistent`) — do not add a second crypto path.

---

## File Structure

**New files:**
- `linux/crates/charter-transport/src/curator.rs` — pure wire parser: kind-30100 `NostrEvent` → `charter_content::CuratorList`.
- `linux/crates/charter-transport/tests/curator_ingest.rs` — async fetch/verify/dedup tests over mock + hostile relays.
- `linux/crates/charterd/src/curator_sync.rs` — `refresh_curator_lists` (read clause curators → fetch → cache).
- `linux/crates/charterd/tests/curator_ingest_flow.rs` — end-to-end pipeline test (refresh → reconcile → ports).

**Modified files:**
- `linux/crates/charter-primitives/src/kinds.rs` — curator web-list tag vocabulary constants + frozen-value test.
- `linux/crates/charter-content/src/curator.rs` — `Serialize`/`Deserialize` on `Rating`/`CuratorEntry`/`CuratorList` (cache round-trip).
- `linux/crates/charter-transport/Cargo.toml` — add `charter-content` dependency.
- `linux/crates/charter-transport/src/lib.rs` — declare `pub mod curator;`, re-export `FetchedCuratorList`.
- `linux/crates/charter-transport/src/transport.rs` — `FetchedCuratorList` type + `CharterTransport::poll_curator_lists`.
- `linux/crates/charter-sys/src/persistence.rs` — `CuratorListStore` trait + mock + real stub.
- `linux/crates/charter-sys/src/layer.rs` — wire `CuratorListStore` into `SystemLayer` + `MockSystem` + `RealSystem`.
- `linux/crates/charterd/src/transport_facade.rs` — `TransportFacade::poll_curator_lists` + `MockTransport` impl.
- `linux/crates/charterd/src/broker.rs` — call `refresh_curator_lists` from `poll_once`.
- `linux/crates/charterd/src/lib.rs` — declare `pub mod curator_sync;`.
- `linux/crates/charterd/src/web_content.rs` — `reconcile` loads cached lists instead of passing `&[]`.

All commands below are run from the `linux/` directory.

---

### Task 1: Curator web-list wire vocabulary (charter-primitives)

The single source of truth for the kind-30100 tag tokens, alongside the existing kind constant `CHARTER_CURATOR_WEB_LIST = 30100`.

**Files:**
- Modify: `linux/crates/charter-primitives/src/kinds.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `pub const CURATOR_WEB_NAMESPACE: &str`, `CURATOR_RATING_KID_SAFE: &str`, `CURATOR_RATING_BLOCK: &str`, `CURATOR_RATING_CATEGORY_PREFIX: &str` — the wire vocabulary the Task-3 parser reads.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` block in `linux/crates/charter-primitives/src/kinds.rs`:

```rust
    #[test]
    fn curator_web_vocabulary_is_frozen() {
        assert_eq!(CURATOR_WEB_NAMESPACE, "app.charter.web");
        assert_eq!(CURATOR_RATING_KID_SAFE, "kid-safe");
        assert_eq!(CURATOR_RATING_BLOCK, "block");
        assert_eq!(CURATOR_RATING_CATEGORY_PREFIX, "category:");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charter-primitives curator_web_vocabulary_is_frozen`
Expected: FAIL — `cannot find value CURATOR_WEB_NAMESPACE in this scope`.

- [ ] **Step 3: Write minimal implementation**

In `linux/crates/charter-primitives/src/kinds.rs`, immediately after the `CHARTER_CURATOR_WEB_LIST` const, add:

```rust
/// NIP-32 namespace label (`["L", ...]` tag value) scoping a curator web-list
/// event. A kind-30100 event lacking this label is **not** a Charter curator
/// web list and is ignored by the parser.
pub const CURATOR_WEB_NAMESPACE: &str = "app.charter.web";
/// Rating token (3rd element of an `["r", <domain>, <token>]` tag) marking a
/// domain kid-safe.
pub const CURATOR_RATING_KID_SAFE: &str = "kid-safe";
/// Rating token marking a domain blocked.
pub const CURATOR_RATING_BLOCK: &str = "block";
/// Rating-token prefix marking a domain blocked under a category, e.g.
/// `category:porn`. The suffix is the (non-empty) category key.
pub const CURATOR_RATING_CATEGORY_PREFIX: &str = "category:";
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p charter-primitives curator_web_vocabulary_is_frozen`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charter-primitives/src/kinds.rs
git commit -m "feat(primitives): curator web-list tag vocabulary (namespace + rating tokens)"
```

---

### Task 2: Serde round-trip on the curator list types (charter-content)

The cache stores each verified `CuratorList` as JSON; this adds the (de)serialization. The internal cache representation is deliberately the serde default (variant names `KidSafe`/`Block`/`{"Category":"..."}`) — distinct from the *wire* tokens in Task 1, which the Task-3 parser handles. Keeping them separate keeps the pure-policy crate ignorant of the wire format.

**Files:**
- Modify: `linux/crates/charter-content/src/curator.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: `impl Serialize + Deserialize for Rating, CuratorEntry, CuratorList`.

- [ ] **Step 1: Write the failing test**

Add to `linux/crates/charter-content/src/curator.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curator_list_json_roundtrip() {
        let list = CuratorList {
            curator: "aa".into(),
            entries: vec![
                CuratorEntry { domain: "kids.example".into(), rating: Rating::KidSafe },
                CuratorEntry { domain: "bad.example".into(), rating: Rating::Block },
                CuratorEntry {
                    domain: "p.example".into(),
                    rating: Rating::Category("porn".into()),
                },
            ],
        };
        let json = serde_json::to_string(&list).expect("serialize");
        let back: CuratorList = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(list, back);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charter-content curator_list_json_roundtrip`
Expected: FAIL — `the trait bound CuratorList: Serialize is not satisfied` (compile error).

- [ ] **Step 3: Write minimal implementation**

In `linux/crates/charter-content/src/curator.rs`, add the import at the top and the derives on all three types:

```rust
//! In-memory, already-signature-verified curator list. The Nostr wire parsing
//! that produces these lives in `charter-transport`. The serde representation
//! here is the *cache* format (variant names), NOT the wire token format.

use serde::{Deserialize, Serialize};

/// A curator's rating of a domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Rating {
    KidSafe,
    Block,
    Category(String),
}

/// One `(domain, rating)` entry in a curator's list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratorEntry {
    pub domain: String,
    pub rating: Rating,
}

/// A signed curator list, keyed by the curator's pubkey (64-hex).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CuratorList {
    pub curator: String,
    pub entries: Vec<CuratorEntry>,
}
```

`serde` (with `derive`) and `serde_json` are already `charter-content` dependencies — no `Cargo.toml` change.

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p charter-content curator_list_json_roundtrip`
Expected: PASS.

- [ ] **Step 5: Run the existing golden vectors to confirm no regression**

Run: `cargo test -p charter-content`
Expected: PASS (the `content_vectors` suite is unaffected — it builds `CuratorList` via its own helper, not serde).

- [ ] **Step 6: Commit**

```bash
git add linux/crates/charter-content/src/curator.rs
git commit -m "feat(content): serde on CuratorList/CuratorEntry/Rating for cache round-trip"
```

---

### Task 3: Curator web-list wire parser (charter-transport)

Pure function turning a kind-30100 `NostrEvent` into a `CuratorList`. No I/O, no signature checks (those live in Task 4).

**Files:**
- Modify: `linux/crates/charter-transport/Cargo.toml`
- Create: `linux/crates/charter-transport/src/curator.rs`
- Modify: `linux/crates/charter-transport/src/lib.rs`

**Interfaces:**
- Consumes: Task 1 constants (`charter_primitives::kinds::CURATOR_WEB_*`); `charter_content::{CuratorList, CuratorEntry, Rating}`; `charter_primitives::NostrEvent` (`.tags: Vec<Vec<String>>`, `.has_tag`, `.tag_value`, `.pubkey.to_hex()`).
- Produces: `pub fn parse_curator_list(ev: &NostrEvent) -> Option<CuratorList>`, `pub fn list_id(ev: &NostrEvent) -> String`.

- [ ] **Step 1: Add the `charter-content` dependency**

In `linux/crates/charter-transport/Cargo.toml`, under `[dependencies]`, add (alphabetically near the other `charter-*` lines):

```toml
charter-content = { workspace = true }
```

- [ ] **Step 2: Write the failing test**

Create `linux/crates/charter-transport/src/curator.rs` with ONLY the test module first (the functions come in Step 4):

```rust
//! Wire parsing for curator web-list events (kind 30100; NIP-51 addressable
//! `d`-tag + NIP-32 `L` namespace label). Turns a `NostrEvent` into the pure
//! `charter_content::CuratorList` the evaluator consumes. Signature
//! verification + relay fetch live in `transport.rs`; this module is pure.

use charter_content::{CuratorEntry, CuratorList, Rating};
use charter_primitives::{kinds, NostrEvent};

#[cfg(test)]
mod tests {
    use super::*;
    use charter_primitives::{EventId, PubKey, Sig};

    fn ev(kind: u16, tags: Vec<Vec<String>>) -> NostrEvent {
        NostrEvent {
            id: EventId::from_bytes([0; 32]),
            pubkey: PubKey::from_bytes([0xAB; 32]),
            created_at: 1,
            kind,
            tags,
            content: String::new(),
            sig: Sig::from_bytes([0; 64]),
        }
    }

    fn tag(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| s.to_string()).collect()
    }

    fn ns() -> Vec<String> {
        tag(&["L", "app.charter.web"])
    }

    #[test]
    fn parses_ratings_and_curator_pubkey() {
        let e = ev(
            kinds::CHARTER_CURATOR_WEB_LIST,
            vec![
                tag(&["d", "main"]),
                ns(),
                tag(&["r", "kids.example", "kid-safe"]),
                tag(&["r", "bad.example", "block"]),
                tag(&["r", "p.example", "category:porn"]),
            ],
        );
        let list = parse_curator_list(&e).expect("parses");
        assert_eq!(list.curator, PubKey::from_bytes([0xAB; 32]).to_hex());
        assert_eq!(list.entries.len(), 3);
        assert_eq!(list.entries[0], CuratorEntry { domain: "kids.example".into(), rating: Rating::KidSafe });
        assert_eq!(list.entries[1].rating, Rating::Block);
        assert_eq!(list.entries[2].rating, Rating::Category("porn".into()));
        assert_eq!(list_id(&e), "main");
    }

    #[test]
    fn rejects_wrong_kind() {
        let e = ev(kinds::CHARTER_DEVICE_CLAUSE, vec![ns()]);
        assert!(parse_curator_list(&e).is_none());
    }

    #[test]
    fn rejects_missing_namespace_label() {
        let e = ev(
            kinds::CHARTER_CURATOR_WEB_LIST,
            vec![tag(&["d", "main"]), tag(&["r", "kids.example", "kid-safe"])],
        );
        assert!(parse_curator_list(&e).is_none());
    }

    #[test]
    fn skips_malformed_entries() {
        let e = ev(
            kinds::CHARTER_CURATOR_WEB_LIST,
            vec![
                ns(),
                tag(&["r", "ok.example", "kid-safe"]),
                tag(&["r", "no-rating.example"]),      // too short — skipped
                tag(&["r", "weird.example", "bogus"]), // unknown token — skipped
                tag(&["r", "empty.example", "category:"]), // empty key — skipped
            ],
        );
        let list = parse_curator_list(&e).expect("parses");
        assert_eq!(list.entries.len(), 1);
        assert_eq!(list.entries[0].domain, "ok.example");
    }

    #[test]
    fn missing_d_tag_yields_empty_list_id() {
        let e = ev(kinds::CHARTER_CURATOR_WEB_LIST, vec![ns()]);
        assert_eq!(list_id(&e), "");
        assert!(parse_curator_list(&e).expect("parses").entries.is_empty());
    }
}
```

Add `pub mod curator;` to `linux/crates/charter-transport/src/lib.rs` (under the existing `pub mod transport;` line) so the test compiles.

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test -p charter-transport --lib curator`
Expected: FAIL — `cannot find function parse_curator_list in this scope`.

- [ ] **Step 4: Write minimal implementation**

Insert the functions into `linux/crates/charter-transport/src/curator.rs`, between the `use` lines and the `#[cfg(test)]` module:

```rust
/// The `d`-tag value (NIP-51 addressable list id), or `""` if absent.
pub fn list_id(ev: &NostrEvent) -> String {
    ev.tag_value("d").unwrap_or("").to_string()
}

/// Parse a kind-30100 event into a `CuratorList`. Returns `None` when the event
/// is not a Charter curator web list (wrong kind, or missing the
/// `app.charter.web` namespace label). Malformed individual `r` entries are
/// skipped; the curator's pubkey is taken from the event author.
pub fn parse_curator_list(ev: &NostrEvent) -> Option<CuratorList> {
    if ev.kind != kinds::CHARTER_CURATOR_WEB_LIST {
        return None;
    }
    if !ev.has_tag("L", kinds::CURATOR_WEB_NAMESPACE) {
        return None;
    }
    let entries = ev
        .tags
        .iter()
        .filter(|t| t.len() >= 3 && t[0] == "r")
        .filter_map(|t| {
            parse_rating(&t[2]).map(|rating| CuratorEntry {
                domain: t[1].clone(),
                rating,
            })
        })
        .collect();
    Some(CuratorList {
        curator: ev.pubkey.to_hex(),
        entries,
    })
}

fn parse_rating(token: &str) -> Option<Rating> {
    if token == kinds::CURATOR_RATING_KID_SAFE {
        Some(Rating::KidSafe)
    } else if token == kinds::CURATOR_RATING_BLOCK {
        Some(Rating::Block)
    } else {
        token
            .strip_prefix(kinds::CURATOR_RATING_CATEGORY_PREFIX)
            .filter(|k| !k.is_empty())
            .map(|k| Rating::Category(k.to_string()))
    }
}
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p charter-transport --lib curator`
Expected: PASS (5 tests).

- [ ] **Step 6: Re-export the parser**

In `linux/crates/charter-transport/src/lib.rs`, add to the `pub use` lines:

```rust
pub use curator::{list_id, parse_curator_list};
```

- [ ] **Step 7: Commit**

```bash
git add linux/crates/charter-transport/Cargo.toml linux/crates/charter-transport/src/curator.rs linux/crates/charter-transport/src/lib.rs
git commit -m "feat(transport): curator web-list wire parser (kind 30100 -> CuratorList)"
```

---

### Task 4: Fetch + verify + dedup curator lists (charter-transport)

`CharterTransport::poll_curator_lists` queries the relay for kind-30100 events by the subscribed curators, rejects any whose author is not subscribed or whose id/signature is invalid (a hostile relay can ignore the filter), parses survivors, and keeps the newest per `(curator, list_id)` — NIP-51 replaceable semantics.

**Files:**
- Modify: `linux/crates/charter-transport/src/transport.rs`
- Modify: `linux/crates/charter-transport/src/lib.rs`
- Create: `linux/crates/charter-transport/tests/curator_ingest.rs`

**Interfaces:**
- Consumes: Task 3 (`crate::curator::{list_id, parse_curator_list}`); `charter_content::CuratorList`; `charter_verify::{id_is_consistent, signature_is_valid}`; `charter_sys::relay::Filter` (fields `kinds`, `authors`, `since`); `self.relay.query`.
- Produces: `pub struct FetchedCuratorList { pub curator: PubKey, pub list_id: String, pub created_at: u64, pub list: CuratorList }` and `pub async fn CharterTransport::poll_curator_lists(&self, curators: &[PubKey], since: u64) -> Result<Vec<FetchedCuratorList>, RelayIoError>`.

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charter-transport/tests/curator_ingest.rs`:

```rust
//! Curator web-list ingest: a subscribed curator's signed kind-30100 list is
//! fetched + verified; a hostile relay that ignores the author filter or serves
//! a bad signature is rejected; replaceable (d-keyed) lists keep the newest.

#![cfg(feature = "mock")]

use async_trait::async_trait;

use charter_primitives::{kinds, NostrEvent, PubKey, Sig};
use charter_sys::relay::{
    Filter, MockRelayTransport, PublishOutcome, RelayIoError, RelayTransport, RelayUrl,
};
use charter_sys::signer::{MachineSigner, SeedSigner};
use charter_transport::{CharterTransport, ScriptedEntropy};
use charter_verify::test_support::sign_event;

fn machine_sk() -> [u8; 32] {
    let mut s = [0u8; 32];
    s[31] = 0x22;
    s
}

fn gpk() -> PubKey {
    PubKey::from_bytes([0x11; 32])
}

/// Build a signed kind-30100 curator list event.
fn curator_event(signer: &SeedSigner, d: &str, created_at: u64, entries: &[(&str, &str)]) -> NostrEvent {
    let mut tags = vec![
        vec!["d".to_string(), d.to_string()],
        vec!["L".to_string(), kinds::CURATOR_WEB_NAMESPACE.to_string()],
    ];
    for (domain, rating) in entries {
        tags.push(vec!["r".to_string(), domain.to_string(), rating.to_string()]);
    }
    sign_event(signer, kinds::CHARTER_CURATOR_WEB_LIST, created_at, tags, String::new())
}

fn transport<R: RelayTransport>(relay: R) -> CharterTransport<R, ScriptedEntropy> {
    CharterTransport::new(relay, ScriptedEntropy::new(7), machine_sk(), gpk(), vec!["wss://r".into()])
}

/// A relay that returns every injected event regardless of the filter —
/// models a hostile relay that ignores the author subscription.
struct HostileRelay {
    events: Vec<NostrEvent>,
}

#[async_trait]
impl RelayTransport for HostileRelay {
    async fn publish(&self, _r: &[RelayUrl], _ev: NostrEvent) -> Vec<(RelayUrl, PublishOutcome)> {
        Vec::new()
    }
    async fn query(&self, _r: &[RelayUrl], _f: Filter) -> Result<Vec<NostrEvent>, RelayIoError> {
        Ok(self.events.clone())
    }
}

#[tokio::test]
async fn fetches_and_verifies_subscribed_curator_list() {
    let curator = SeedSigner::from_seed(0xC1);
    let curator_pk = MachineSigner::pubkey(&curator);
    let relay = MockRelayTransport::new();
    relay.inject(curator_event(&curator, "main", 5, &[("kids.example", "kid-safe")]));

    let t = transport(relay);
    let got = t.poll_curator_lists(&[curator_pk], 0).await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].curator, curator_pk);
    assert_eq!(got[0].list.curator, curator_pk.to_hex());
    assert_eq!(got[0].list.entries[0].domain, "kids.example");
}

#[tokio::test]
async fn empty_subscription_fetches_nothing() {
    let curator = SeedSigner::from_seed(0xC1);
    let relay = MockRelayTransport::new();
    relay.inject(curator_event(&curator, "main", 5, &[("kids.example", "kid-safe")]));
    let t = transport(relay);
    assert!(t.poll_curator_lists(&[], 0).await.unwrap().is_empty());
}

#[tokio::test]
async fn hostile_relay_cannot_inject_foreign_or_forged_lists() {
    let curator = SeedSigner::from_seed(0xC1);
    let curator_pk = MachineSigner::pubkey(&curator);
    let rogue = SeedSigner::from_seed(0xEE); // not subscribed

    let valid = curator_event(&curator, "main", 5, &[("kids.example", "kid-safe")]);
    let foreign = curator_event(&rogue, "main", 5, &[("adult.example", "kid-safe")]);
    let mut forged = curator_event(&curator, "x", 6, &[("evil.example", "kid-safe")]);
    forged.sig = Sig::from_bytes([0u8; 64]); // tampered signature

    let relay = HostileRelay { events: vec![valid, foreign, forged] };
    let t = transport(relay);
    let got = t.poll_curator_lists(&[curator_pk], 0).await.unwrap();

    assert_eq!(got.len(), 1, "only the genuine subscribed+signed list survives");
    assert_eq!(got[0].curator, curator_pk);
    assert_eq!(got[0].list.entries[0].domain, "kids.example");
}

#[tokio::test]
async fn replaceable_keeps_newest_per_list_id() {
    let curator = SeedSigner::from_seed(0xC1);
    let curator_pk = MachineSigner::pubkey(&curator);
    let relay = MockRelayTransport::new();
    relay.inject(curator_event(&curator, "main", 5, &[("old.example", "kid-safe")]));
    relay.inject(curator_event(&curator, "main", 9, &[("new.example", "kid-safe")]));

    let t = transport(relay);
    let got = t.poll_curator_lists(&[curator_pk], 0).await.unwrap();
    assert_eq!(got.len(), 1, "same (curator, d) collapses to the newest");
    assert_eq!(got[0].created_at, 9);
    assert_eq!(got[0].list.entries[0].domain, "new.example");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charter-transport --test curator_ingest`
Expected: FAIL — `no method named poll_curator_lists` / `cannot find type FetchedCuratorList`.

- [ ] **Step 3: Write minimal implementation**

In `linux/crates/charter-transport/src/transport.rs`, add `use std::collections::BTreeMap;` near the top imports, add `use charter_content::CuratorList;` and `use crate::curator::{list_id, parse_curator_list};` to the imports, then add the type (after `ReceivedClause`) and the method (inside the `impl<R, E> CharterTransport<R, E>` block, after `poll_clauses`):

```rust
/// A fetched, signature-verified curator web list. Replaceable: keyed by
/// `(curator, list_id)`, newest `created_at` wins.
#[derive(Debug, Clone)]
pub struct FetchedCuratorList {
    pub curator: PubKey,
    pub list_id: String,
    pub created_at: u64,
    pub list: CuratorList,
}
```

```rust
    /// Fetch subscribed curators' signed web lists (kind 30100). Rejects any
    /// event whose author is not in `curators` (a hostile relay may ignore the
    /// filter), whose id is inconsistent, or whose signature is invalid; parses
    /// survivors; keeps the newest event per `(curator, list_id)`. A hostile
    /// relay can drop or reorder but never inject a list the device accepts.
    pub async fn poll_curator_lists(
        &self,
        curators: &[PubKey],
        since: u64,
    ) -> Result<Vec<FetchedCuratorList>, RelayIoError> {
        if curators.is_empty() {
            return Ok(Vec::new());
        }
        let filter = Filter {
            kinds: vec![kinds::CHARTER_CURATOR_WEB_LIST],
            authors: curators.to_vec(),
            since: Some(since),
            ..Default::default()
        };
        let events = self.relay.query(&self.relays, filter).await?;
        let mut newest: BTreeMap<(String, String), FetchedCuratorList> = BTreeMap::new();
        for ev in events.into_iter().take(self.max_inbound) {
            if !curators.contains(&ev.pubkey) {
                continue;
            }
            if !charter_verify::id_is_consistent(&ev) || !charter_verify::signature_is_valid(&ev) {
                continue;
            }
            let Some(list) = parse_curator_list(&ev) else {
                continue;
            };
            let key = (ev.pubkey.to_hex(), list_id(&ev));
            let keep = match newest.get(&key) {
                Some(existing) => ev.created_at > existing.created_at,
                None => true,
            };
            if keep {
                newest.insert(
                    key.clone(),
                    FetchedCuratorList {
                        curator: ev.pubkey,
                        list_id: key.1,
                        created_at: ev.created_at,
                        list,
                    },
                );
            }
        }
        Ok(newest.into_values().collect())
    }
```

- [ ] **Step 4: Re-export the type**

In `linux/crates/charter-transport/src/lib.rs`, add `FetchedCuratorList` to the `transport` re-export:

```rust
pub use transport::{
    CharterTransport, Entropy, FetchedCuratorList, ReceivedClause, ReceivedGrant, ScriptedEntropy,
};
```

- [ ] **Step 5: Run test to verify it passes**

Run: `cargo test -p charter-transport --test curator_ingest`
Expected: PASS (4 tests).

- [ ] **Step 6: Commit**

```bash
git add linux/crates/charter-transport/src/transport.rs linux/crates/charter-transport/src/lib.rs linux/crates/charter-transport/tests/curator_ingest.rs
git commit -m "feat(transport): poll_curator_lists — fetch, verify (author+sig), dedup replaceable"
```

---

### Task 5: CuratorListStore cache port (charter-sys)

A rollback-protected cache of verified lists, modelled exactly on `ClauseStore` but keyed by an opaque string and holding many entries. Wired into `SystemLayer` so `charterd` reads/writes it generically.

**Files:**
- Modify: `linux/crates/charter-sys/src/persistence.rs`
- Modify: `linux/crates/charter-sys/src/layer.rs`

**Interfaces:**
- Consumes: `crate::error::{SysResult, SysError}`; `MockDisk`.
- Produces: `pub trait CuratorListStore { fn put_list(&self, key: &str, created_at: u64, json: &str) -> SysResult<bool>; fn all_lists(&self) -> SysResult<Vec<String>>; }`, `MockCuratorListStore`, `RealCuratorListStore`, and `SystemLayer::CuratorLists` + `fn curator_lists(&self) -> &Self::CuratorLists`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(all(test, feature = "mock"))] mod tests` block in `linux/crates/charter-sys/src/persistence.rs`:

```rust
    #[test]
    fn curator_list_store_rollback_and_listing() {
        let disk = MockDisk::new();
        let s = MockCuratorListStore::new(disk);
        assert!(s.put_list("aa:main", 10, r#"{"curator":"aa"}"#).unwrap());
        assert!(!s.put_list("aa:main", 10, r#"{"curator":"aa2"}"#).unwrap()); // equal -> rollback
        assert!(!s.put_list("aa:main", 5, r#"{"curator":"aa3"}"#).unwrap()); // lower -> rollback
        assert!(s.put_list("aa:main", 20, r#"{"curator":"aaN"}"#).unwrap()); // higher -> ok
        assert!(s.put_list("bb:main", 1, r#"{"curator":"bb"}"#).unwrap());
        let all = s.all_lists().unwrap();
        assert_eq!(all.len(), 2);
        assert!(all.contains(&r#"{"curator":"aaN"}"#.to_string()));
        assert!(all.contains(&r#"{"curator":"bb"}"#.to_string()));
    }

    #[test]
    fn curator_list_store_durable_across_reopen() {
        let disk = MockDisk::new();
        {
            let a = MockCuratorListStore::new(disk.clone());
            assert!(a.put_list("aa:main", 7, r#"{"curator":"aa"}"#).unwrap());
        }
        let b = MockCuratorListStore::new(disk);
        assert_eq!(b.all_lists().unwrap().len(), 1);
        assert!(!b.put_list("aa:main", 7, r#"{"curator":"x"}"#).unwrap()); // still rollback-protected
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charter-sys curator_list_store`
Expected: FAIL — `cannot find type MockCuratorListStore`.

- [ ] **Step 3: Write the trait**

In `linux/crates/charter-sys/src/persistence.rs`, add the trait after the `ClauseStore` trait (around line 46):

```rust
/// Cache of signature-verified curator web lists, keyed by an opaque string
/// (`"<curator-hex>:<listId>"`). Like [`ClauseStore`], each key is rollback-
/// protected by a monotonic `created_at`: a stale replacement is rejected, so a
/// hostile relay replaying an old list cannot revert the cache. `all_lists`
/// returns the newest JSON for every cached key — the evaluator's input.
pub trait CuratorListStore: Send + Sync {
    /// Upsert `json` under `key`. Returns true if stored, false if rejected as a
    /// rollback (`created_at <= highest seen for key`).
    fn put_list(&self, key: &str, created_at: u64, json: &str) -> SysResult<bool>;
    /// Every cached list's JSON, in deterministic (key) order.
    fn all_lists(&self) -> SysResult<Vec<String>>;
}
```

- [ ] **Step 4: Write the mock impl**

In the `#[cfg(feature = "mock")] mod mock` block of `persistence.rs`:

(a) Add a field to `DiskState`:

```rust
        curator_lists: BTreeMap<String, (u64, String)>,
```

(b) Add the store (place after `MockClauseStore`):

```rust
    /// Mock curator-list store with per-key rollback protection.
    pub struct MockCuratorListStore {
        disk: MockDisk,
    }
    impl MockCuratorListStore {
        pub fn new(disk: MockDisk) -> Self {
            Self { disk }
        }
    }
    impl CuratorListStore for MockCuratorListStore {
        fn put_list(&self, key: &str, created_at: u64, json: &str) -> SysResult<bool> {
            let mut g = self.disk.0.lock().expect("disk lock");
            if let Some((prev, _)) = g.curator_lists.get(key) {
                if created_at <= *prev {
                    return Ok(false);
                }
            }
            g.curator_lists
                .insert(key.to_string(), (created_at, json.to_string()));
            Ok(true)
        }
        fn all_lists(&self) -> SysResult<Vec<String>> {
            Ok(self
                .disk
                .0
                .lock()
                .expect("disk lock")
                .curator_lists
                .values()
                .map(|(_, j)| j.clone())
                .collect())
        }
    }
```

(c) Add `MockCuratorListStore` to the `#[cfg(feature = "mock")] pub use mock::{ ... }` list.

- [ ] **Step 5: Write the real stub**

In the `#[cfg(feature = "real")] mod real` block:

```rust
    real_stub!(RealCuratorListStore);
    impl CuratorListStore for RealCuratorListStore {
        fn put_list(&self, _key: &str, _created_at: u64, _json: &str) -> SysResult<bool> {
            Err(SysError::NotImplemented)
        }
        fn all_lists(&self) -> SysResult<Vec<String>> {
            Err(SysError::NotImplemented)
        }
    }
```

Add `RealCuratorListStore` to the `#[cfg(feature = "real")] pub use real::{ ... }` list.

- [ ] **Step 6: Run test to verify it passes**

Run: `cargo test -p charter-sys curator_list_store`
Expected: PASS (2 tests).

- [ ] **Step 7: Wire into `SystemLayer`**

In `linux/crates/charter-sys/src/layer.rs`:

(a) Add `CuratorListStore` to the `use crate::persistence::{ ... }` import.

(b) In the `SystemLayer` trait: add the associated type (after `type Clauses`) and accessor (after `fn clauses`):

```rust
    type CuratorLists: CuratorListStore;
```
```rust
    fn curator_lists(&self) -> &Self::CuratorLists;
```

(c) In `mod mock`: add `MockCuratorListStore` to the `use crate::persistence::{ ... }` import; add the field to `MockSystem` (after `clauses`):

```rust
        curator_lists: MockCuratorListStore,
```

initialize it in `over_disk` (after `clauses: MockClauseStore::new(disk.clone()),`):

```rust
                curator_lists: MockCuratorListStore::new(disk.clone()),
```

and in `impl SystemLayer for MockSystem` add the type + accessor:

```rust
        type CuratorLists = MockCuratorListStore;
```
```rust
        fn curator_lists(&self) -> &Self::CuratorLists {
            &self.curator_lists
        }
```

(d) In `mod real`: add `RealCuratorListStore` to the `use crate::persistence::{ ... }` import; add the field to `RealSystem` (after `clauses`):

```rust
        curator_lists: RealCuratorListStore,
```

and in `impl SystemLayer for RealSystem` add:

```rust
        type CuratorLists = RealCuratorListStore;
```
```rust
        fn curator_lists(&self) -> &Self::CuratorLists {
            &self.curator_lists
        }
```

(e) In the `mock_system_exposes_every_port` test, add a line:

```rust
        let _ = sys.curator_lists();
```

- [ ] **Step 8: Run the full charter-sys suite**

Run: `cargo test -p charter-sys`
Expected: PASS (including `mock_system_exposes_every_port` and `generic_run_over_system_layer_compiles`).

- [ ] **Step 9: Verify the real build still compiles**

Run: `cargo build -p charter-sys --no-default-features --features real`
Expected: builds clean (the `RealCuratorListStore` stub satisfies the new associated type).

- [ ] **Step 10: Commit**

```bash
git add linux/crates/charter-sys/src/persistence.rs linux/crates/charter-sys/src/layer.rs
git commit -m "feat(sys): CuratorListStore port (rollback-protected cache) wired into SystemLayer"
```

---

### Task 6: TransportFacade curator-list poll (charterd)

Extend the daemon's transport seam so the broker can fetch curator lists through the same mockable facade it uses for grants/clauses.

**Files:**
- Modify: `linux/crates/charterd/src/transport_facade.rs`

**Interfaces:**
- Consumes: `charter_transport::FetchedCuratorList` (Task 4); `charter_primitives::PubKey`.
- Produces: `TransportFacade::poll_curator_lists(&self, curators: &[PubKey], since: u64) -> Vec<FetchedCuratorList>`; `MockTransport::deliver_curator_list(&self, list: FetchedCuratorList)`.

- [ ] **Step 1: Write the failing test**

Add to `linux/crates/charterd/src/transport_facade.rs` (new test module at end of file):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use charter_content::{CuratorEntry, CuratorList, Rating};
    use charter_transport::FetchedCuratorList;

    fn pk(b: u8) -> PubKey {
        PubKey::from_bytes([b; 32])
    }

    #[tokio::test]
    async fn mock_delivers_then_drains_curator_lists() {
        let t = MockTransport::new(pk(0x11), pk(0x22));
        t.deliver_curator_list(FetchedCuratorList {
            curator: pk(0xaa),
            list_id: "main".into(),
            created_at: 5,
            list: CuratorList {
                curator: pk(0xaa).to_hex(),
                entries: vec![CuratorEntry { domain: "kids.example".into(), rating: Rating::KidSafe }],
            },
        });
        let got = t.poll_curator_lists(&[pk(0xaa)], 0).await;
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].list.entries[0].domain, "kids.example");
        assert!(t.poll_curator_lists(&[pk(0xaa)], 0).await.is_empty(), "drained");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charterd --lib transport_facade`
Expected: FAIL — `no method named poll_curator_lists` / `no method named deliver_curator_list`.

- [ ] **Step 3: Write minimal implementation**

In `linux/crates/charterd/src/transport_facade.rs`:

(a) Extend the import:

```rust
use charter_transport::{FetchedCuratorList, ReceivedClause, ReceivedGrant};
```

(b) Add the trait method to `TransportFacade` (after `poll_clauses`):

```rust
    async fn poll_curator_lists(&self, curators: &[PubKey], since: u64) -> Vec<FetchedCuratorList>;
```

(c) Add a field to `Inner`:

```rust
    curator_lists: Vec<FetchedCuratorList>,
```

(d) Add the scripting helper to `impl MockTransport` (after `deliver_clause`):

```rust
    /// Queue a (already verified) curator list for delivery.
    pub fn deliver_curator_list(&self, list: FetchedCuratorList) {
        self.inner.lock().expect("lock").curator_lists.push(list);
    }
```

(e) Implement the method in `impl TransportFacade for MockTransport` (after `poll_clauses`):

```rust
    async fn poll_curator_lists(&self, _curators: &[PubKey], _since: u64) -> Vec<FetchedCuratorList> {
        std::mem::take(&mut self.inner.lock().expect("lock").curator_lists)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p charterd --lib transport_facade`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charterd/src/transport_facade.rs
git commit -m "feat(charterd): TransportFacade::poll_curator_lists + MockTransport scripting"
```

---

### Task 7: refresh_curator_lists — read clause curators, fetch, cache (charterd)

A free function (testable with just `MockSystem` + `MockTransport`, no full `Broker`) that drives the ingest: read the cached `content` clause → parse `curators[]` → fetch over the facade → write each to the cache with rollback protection. Wired into `Broker::poll_once`.

**Files:**
- Create: `linux/crates/charterd/src/curator_sync.rs`
- Modify: `linux/crates/charterd/src/lib.rs`
- Modify: `linux/crates/charterd/src/broker.rs`

**Interfaces:**
- Consumes: `charter_content::GrantContent` (field `curators: Vec<String>`); `charter_proto::ClauseKind` (`.store_key()`); `charter_sys::persistence::{ClauseStore, CuratorListStore}`; `charter_sys::SystemLayer`; `crate::transport_facade::TransportFacade` (Task 6); `charter_primitives::PubKey` (`from_hex`, `to_hex`); Task 2 serde.
- Produces: `pub async fn refresh_curator_lists<S: SystemLayer, T: TransportFacade>(sys: &S, transport: &T)`.

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charterd/src/curator_sync.rs` with imports + the function signature stub + tests (the body comes in Step 3):

```rust
//! Curator-list ingest driver: read the cached `content` clause to learn the
//! subscribed curators, fetch their signed lists over the transport facade, and
//! write each to the rollback-protected cache. Fail-closed: a fetch failure (or
//! a clause without curators) writes nothing, so the last-known-good cache
//! stands and an allowlist never widens on failure.

use charter_content::GrantContent;
use charter_primitives::PubKey;
use charter_proto::ClauseKind;
use charter_sys::persistence::{ClauseStore, CuratorListStore};
use charter_sys::SystemLayer;

use crate::transport_facade::TransportFacade;

pub async fn refresh_curator_lists<S: SystemLayer, T: TransportFacade>(_sys: &S, _transport: &T) {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_content::{CuratorEntry, CuratorList, Rating};
    use charter_sys::MockSystem;
    use charter_transport::FetchedCuratorList;

    use crate::transport_facade::MockTransport;

    fn pk(b: u8) -> PubKey {
        PubKey::from_bytes([b; 32])
    }

    fn fetched(curator: PubKey, created_at: u64, domain: &str) -> FetchedCuratorList {
        FetchedCuratorList {
            curator,
            list_id: "main".into(),
            created_at,
            list: CuratorList {
                curator: curator.to_hex(),
                entries: vec![CuratorEntry { domain: domain.into(), rating: Rating::KidSafe }],
            },
        }
    }

    fn put_content(sys: &MockSystem, json: &str) {
        sys.clauses()
            .put_clause(ClauseKind::Content.store_key(), 1, json)
            .unwrap();
    }

    #[tokio::test]
    async fn caches_lists_for_subscribed_curators() {
        let sys = MockSystem::new(1000);
        let c1 = pk(0xc1);
        let clause = format!(
            r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{}"],"issuedAt":1}}"#,
            c1.to_hex()
        );
        put_content(&sys, &clause);
        let transport = MockTransport::new(pk(0x11), pk(0x22));
        transport.deliver_curator_list(fetched(c1, 5, "kids.example"));

        refresh_curator_lists(&sys, &transport).await;

        let all = sys.curator_lists().all_lists().unwrap();
        assert_eq!(all.len(), 1);
        assert!(all[0].contains("kids.example"));
    }

    #[tokio::test]
    async fn clause_without_curators_is_noop() {
        let sys = MockSystem::new(1000);
        put_content(&sys, r#"{"v":1,"posture":"allowlist","ageTier":"young","issuedAt":1}"#);
        let transport = MockTransport::new(pk(0x11), pk(0x22));
        transport.deliver_curator_list(fetched(pk(0xc1), 5, "kids.example"));

        refresh_curator_lists(&sys, &transport).await;
        assert!(sys.curator_lists().all_lists().unwrap().is_empty());
    }

    #[tokio::test]
    async fn absent_clause_is_noop() {
        let sys = MockSystem::new(1000);
        let transport = MockTransport::new(pk(0x11), pk(0x22));
        refresh_curator_lists(&sys, &transport).await;
        assert!(sys.curator_lists().all_lists().unwrap().is_empty());
    }
}
```

Add `pub mod curator_sync;` to `linux/crates/charterd/src/lib.rs` (after `pub mod broker;`).

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charterd --lib curator_sync`
Expected: FAIL — the three tests panic on `unimplemented!()`.

- [ ] **Step 3: Write minimal implementation**

Replace the `refresh_curator_lists` stub body in `curator_sync.rs`:

```rust
pub async fn refresh_curator_lists<S: SystemLayer, T: TransportFacade>(sys: &S, transport: &T) {
    let clause_json = match sys.clauses().get_clause(ClauseKind::Content.store_key()) {
        Ok(Some(j)) => j,
        _ => return,
    };
    let clause: GrantContent = match serde_json::from_str(&clause_json) {
        Ok(c) => c,
        Err(_) => return,
    };
    let curators: Vec<PubKey> = clause
        .curators
        .iter()
        .filter_map(|hex| PubKey::from_hex(hex).ok())
        .collect();
    if curators.is_empty() {
        return;
    }
    let fetched = transport.poll_curator_lists(&curators, 0).await;
    for f in fetched {
        if let Ok(json) = serde_json::to_string(&f.list) {
            let key = format!("{}:{}", f.curator.to_hex(), f.list_id);
            let _ = sys.curator_lists().put_list(&key, f.created_at, &json);
        }
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p charterd --lib curator_sync`
Expected: PASS (3 tests).

- [ ] **Step 5: Wire into `Broker::poll_once`**

In `linux/crates/charterd/src/broker.rs`, inside `poll_once`, add the refresh call after the clause loop and before the grant loop:

```rust
        for clause in self.transport.poll_clauses(cursor, now).await {
            self.on_clause(clause).await;
        }
        crate::curator_sync::refresh_curator_lists(&self.sys, &self.transport).await;
        for grant in self.transport.poll_grants(cursor, now).await {
            self.on_grant(grant).await;
        }
```

- [ ] **Step 6: Run the existing broker loop tests to confirm no regression**

Run: `cargo test -p charterd --test broker_loop`
Expected: PASS (the added refresh is a no-op when no `content` clause is cached).

- [ ] **Step 7: Commit**

```bash
git add linux/crates/charterd/src/curator_sync.rs linux/crates/charterd/src/lib.rs linux/crates/charterd/src/broker.rs
git commit -m "feat(charterd): refresh_curator_lists ingest driver, wired into poll_once"
```

---

### Task 8: Enforcer reads the cached lists (charterd)

Replace the empty slice at `web_content.rs:99` with the cached, signature-verified lists.

**Files:**
- Modify: `linux/crates/charterd/src/web_content.rs`

**Interfaces:**
- Consumes: `charter_content::CuratorList` + Task 2 serde; `charter_sys::persistence::CuratorListStore` (Task 5).
- Produces: `reconcile` now passes the cached `Vec<CuratorList>` to `evaluate_content_json`.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `linux/crates/charterd/src/web_content.rs` (the `CuratorListStore` import goes alongside the existing `ClauseStore` import in that test module):

```rust
    fn put_list(sys: &MockSystem, key: &str, created_at: u64, json: &str) {
        use charter_sys::persistence::CuratorListStore;
        sys.curator_lists().put_list(key, created_at, json).unwrap();
    }

    #[tokio::test]
    async fn allowlist_admits_quorum_domain_from_cached_lists() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"issuedAt":1}"#,
        );
        put_list(&sys, "aa:main", 5, r#"{"curator":"aa","entries":[{"domain":"kids.example","rating":"KidSafe"}]}"#);
        put_list(&sys, "bb:main", 5, r#"{"curator":"bb","entries":[{"domain":"kids.example","rating":"KidSafe"}]}"#);

        let mut e = WebContentEnforcer::new();
        assert_eq!(e.reconcile(&sys).await, WebReconcile::Enacted { locked: false });
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(pol.contains("kids.example"), "a quorum-admitted domain must reach the Firefox allowlist");
    }

    #[tokio::test]
    async fn allowlist_excludes_sub_quorum_domain() {
        let sys = MockSystem::new(1000);
        put_content(
            &sys,
            r#"{"v":1,"posture":"allowlist","ageTier":"young","curators":["aa","bb"],"quorumN":2,"issuedAt":1}"#,
        );
        // Only one curator rates solo.example — below quorum 2.
        put_list(&sys, "aa:main", 5, r#"{"curator":"aa","entries":[{"domain":"solo.example","rating":"KidSafe"}]}"#);

        let mut e = WebContentEnforcer::new();
        e.reconcile(&sys).await;
        let pol = sys.web_policy().last_policies().unwrap();
        assert!(!pol.contains("solo.example"), "a sub-quorum domain must not be admitted");
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p charterd --lib web_content::tests::allowlist_admits_quorum_domain_from_cached_lists`
Expected: FAIL — `kids.example` is absent because `reconcile` still passes `&[]`.

- [ ] **Step 3: Write minimal implementation**

In `linux/crates/charterd/src/web_content.rs`:

(a) Extend the imports:

```rust
use charter_content::{evaluate_content_json, CuratorList, EffectiveWebPolicy};
use charter_sys::persistence::{ClauseStore, CuratorListStore};
```

(b) Add a private helper (above `impl WebContentEnforcer` or as a free fn in the module):

```rust
/// Load + deserialize the cached curator lists, skipping any that fail to parse.
fn cached_lists<S: SystemLayer>(sys: &S) -> Vec<CuratorList> {
    sys.curator_lists()
        .all_lists()
        .unwrap_or_default()
        .iter()
        .filter_map(|j| serde_json::from_str::<CuratorList>(j).ok())
        .collect()
}
```

(c) Replace the body of `reconcile` (the `evaluate_content_json` line):

```rust
    pub async fn reconcile<S: SystemLayer>(&mut self, sys: &S) -> WebReconcile {
        let clause_json = match sys.clauses().get_clause(ClauseKind::Content.store_key()) {
            Ok(Some(j)) => j,
            Ok(None) | Err(_) => return WebReconcile::Absent,
        };
        let lists = cached_lists(sys);
        let policy = evaluate_content_json(&clause_json, &lists);

        if self.last_applied.as_ref() == Some(&policy) {
            return WebReconcile::Unchanged;
        }
        self.materialize(sys, policy).await
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p charterd --lib web_content`
Expected: PASS — both new tests plus the five pre-existing `web_content` tests (absent/allowlist/unchanged/unparseable/force_lock/audit) still green.

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charterd/src/web_content.rs
git commit -m "feat(charterd): web enforcer feeds cached curator lists into the evaluator"
```

---

### Task 9: End-to-end ingest → reconcile integration test (charterd)

A headless mock test proving the full pipeline: a `content` clause subscribing two curators + scripted signed lists → `refresh_curator_lists` → `reconcile` materializes both ports with the quorum-admitted domain; and an unsubscribed curator can never inject a domain.

**Files:**
- Create: `linux/crates/charterd/tests/curator_ingest_flow.rs`

**Interfaces:**
- Consumes: `charterd::curator_sync::refresh_curator_lists`; `charterd::web_content::{WebContentEnforcer, WebReconcile}`; `charterd::transport_facade::MockTransport`; `charter_transport::FetchedCuratorList`; `charter_sys::MockSystem`; `charter_content::{CuratorList, CuratorEntry, Rating}`; `charter_proto::ClauseKind`.
- Produces: integration coverage (no new library code).

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charterd/tests/curator_ingest_flow.rs`:

```rust
//! End-to-end (headless, mock) curator-list ingest: clause -> fetch -> cache ->
//! evaluate -> materialize. Proves quorum admission reaches the Firefox + DNS
//! artifacts and that an unsubscribed curator can never inject a domain.

#![cfg(feature = "mock")]

use charter_content::{CuratorEntry, CuratorList, Rating};
use charter_primitives::PubKey;
use charter_proto::ClauseKind;
use charter_sys::persistence::ClauseStore;
use charter_sys::{MockSystem, SystemLayer}; // SystemLayer brings the .clauses()/.web_policy()/.dns_filter() accessors into scope
use charter_transport::FetchedCuratorList;

use charterd::curator_sync::refresh_curator_lists;
use charterd::transport_facade::MockTransport;
use charterd::web_content::{WebContentEnforcer, WebReconcile};

fn pk(b: u8) -> PubKey {
    PubKey::from_bytes([b; 32])
}

fn fetched(curator: PubKey, domain: &str, rating: Rating) -> FetchedCuratorList {
    FetchedCuratorList {
        curator,
        list_id: "main".into(),
        created_at: 5,
        list: CuratorList {
            curator: curator.to_hex(),
            entries: vec![CuratorEntry { domain: domain.into(), rating }],
        },
    }
}

fn put_content(sys: &MockSystem, json: &str) {
    sys.clauses()
        .put_clause(ClauseKind::Content.store_key(), 1, json)
        .unwrap();
}

#[tokio::test]
async fn quorum_admits_domain_end_to_end() {
    let sys = MockSystem::new(1000);
    let c1 = pk(0xc1);
    let c2 = pk(0xc2);
    let clause = format!(
        r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{}","{}"],"quorumN":2,"issuedAt":1}}"#,
        c1.to_hex(),
        c2.to_hex()
    );
    put_content(&sys, &clause);

    let transport = MockTransport::new(pk(0x11), pk(0x22));
    transport.deliver_curator_list(fetched(c1, "kids.example", Rating::KidSafe));
    transport.deliver_curator_list(fetched(c2, "kids.example", Rating::KidSafe));

    refresh_curator_lists(&sys, &transport).await;

    let mut enforcer = WebContentEnforcer::new();
    assert_eq!(enforcer.reconcile(&sys).await, WebReconcile::Enacted { locked: false });

    let pol = sys.web_policy().last_policies().unwrap();
    assert!(pol.contains("kids.example"), "quorum domain must be in the Firefox allowlist");
    let plan = sys.dns_filter().last_plan().unwrap();
    assert!(plan.contains("allowlist"));
}

#[tokio::test]
async fn unsubscribed_curator_cannot_inject_domain() {
    let sys = MockSystem::new(1000);
    let c1 = pk(0xc1);
    let rogue = pk(0xee); // not in curators[]
    let clause = format!(
        r#"{{"v":1,"posture":"allowlist","ageTier":"young","curators":["{}"],"quorumN":1,"issuedAt":1}}"#,
        c1.to_hex()
    );
    put_content(&sys, &clause);

    let transport = MockTransport::new(pk(0x11), pk(0x22));
    transport.deliver_curator_list(fetched(c1, "kids.example", Rating::KidSafe));
    transport.deliver_curator_list(fetched(rogue, "adult.example", Rating::KidSafe));

    refresh_curator_lists(&sys, &transport).await;
    let mut enforcer = WebContentEnforcer::new();
    enforcer.reconcile(&sys).await;

    let pol = sys.web_policy().last_policies().unwrap();
    assert!(pol.contains("kids.example"));
    assert!(
        !pol.contains("adult.example"),
        "the evaluator filters by subscribed curators — a rogue list never admits a domain"
    );
}
```

- [ ] **Step 2: Run test to verify it fails (then passes)**

Run: `cargo test -p charterd --test curator_ingest_flow`
Expected: PASS — all the library code already exists (Tasks 1–8). If a name/path is wrong this is where it surfaces; fix imports until green. (If you want to see a real RED first, temporarily assert `!pol.contains("kids.example")` in the first test, watch it fail, then revert.)

- [ ] **Step 3: Commit**

```bash
git add linux/crates/charterd/tests/curator_ingest_flow.rs
git commit -m "test(charterd): end-to-end curator-list ingest -> reconcile -> ports"
```

---

## Final Verification (run all four gates)

- [ ] **fmt:** `cargo fmt --all --check` → no diff.
- [ ] **clippy:** `cargo clippy --all-targets --all-features -- -D warnings` → clean.
- [ ] **test:** `cargo test --workspace` → all green.
- [ ] **real build:** `cargo build --workspace --no-default-features --features real` → compiles.

If clippy flags the new code: common fixes are `&Vec` → `&[..]`, redundant clones, or `needless_return`. The dedup loop in Task 4 and the `filter_map` in Task 3 are written to be clippy-clean; keep that style.

---

## Self-Review

**Spec coverage (design §9 build sequence, item 4 — "subscribe to subscribed curators' lists/labels; verify signatures; cache; feed evaluator"):**
- *Subscribe / fetch by subscribed curators* → Task 4 (`poll_curator_lists` filters by `curators`), Task 7 (reads `curators[]` from the clause).
- *Verify signatures* → Task 4 (`id_is_consistent` + `signature_is_valid` + author-membership; hostile-relay test).
- *Cache* → Task 5 (`CuratorListStore`, rollback-protected, durable-across-reopen), Task 7 (writes it).
- *Feed evaluator* → Task 8 (`reconcile` reads cache → `evaluate_content_json`), Task 9 (e2e).
- *Wire format (§4.2: kind 30100, `d`/`r`/`L` tags, `app.charter.web`, `kid-safe`/`block`/`category:`)* → Task 1 (vocabulary) + Task 3 (parser).
- *Fail-closed / last-known-good (§4.4)* → Task 7 writes nothing on fetch failure (cache stands); Task 5 rollback protection blocks replay; evaluator already never widens (existing). NIP-51 replaceable dedup → Task 4.
- *Per-site NIP-32 labels / NIP-56 reports* → these are the **advisory** flag flow (§4.2), explicitly "never auto-acting"; out of scope for the auto-evaluation path this plan implements (note: not wired into `evaluate_content`). Curator-authoring UI (§4.5) and the daemon reconcile *cadence* (a periodic timer calling `poll_once`/`reconcile`) are separate, later concerns — this plan delivers the ingest pipeline and wires it into the existing `poll_once`.

**Placeholder scan:** No `TBD`/`later`/"add error handling" — every code step shows complete code. The only intentional stub is Task 7 Step 1's `unimplemented!()`, which exists solely to make the test compile-and-fail RED before Step 3 fills it.

**Type consistency check:**
- `CuratorList { curator: String, entries: Vec<CuratorEntry> }`, `CuratorEntry { domain: String, rating: Rating }`, `Rating::{KidSafe,Block,Category(String)}` — used identically in Tasks 2/3/4/6/7/8/9.
- `FetchedCuratorList { curator: PubKey, list_id: String, created_at: u64, list: CuratorList }` — defined Task 4, constructed identically in Tasks 6/7/9.
- `poll_curator_lists(&[PubKey], u64)` — same arity on `CharterTransport` (Task 4, returns `Result<_, RelayIoError>`) and `TransportFacade` (Task 6, returns `Vec<_>`); the facade swallows relay errors into an empty Vec, matching the existing `poll_grants`/`poll_clauses` facade convention.
- `CuratorListStore::{put_list(&str,u64,&str)->SysResult<bool>, all_lists()->SysResult<Vec<String>>}` — defined Task 5, called in Tasks 7/8 and the Task-8 test helper.
- Cache JSON: written via serde (`serde_json::to_string(&CuratorList)`) in Task 7 and read via `serde_json::from_str::<CuratorList>` in Task 8; the Task-8 seed JSON uses the serde-default `"rating":"KidSafe"` form, which matches Task 2's derive (no `rename_all`). The *wire* token `"kid-safe"` (Task 1/3) is a separate representation and never appears in cache JSON — verified consistent.
