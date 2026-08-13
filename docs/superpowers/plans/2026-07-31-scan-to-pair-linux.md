# Scan-to-Pair for Linux Wards — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a guardian pair a Linux ward by scanning the laptop's QR with their phone — nothing typed, nothing emailed — while keeping a typed code as the offline fallback.

**Architecture:** The laptop's QR gains a **one-time token** alongside its device code. Knowing that token proves the scanner physically saw the laptop screen. An **unpaired** `charterd` opens a narrow relay listener for a new `PAIR_OFFER` (kind `31117`) gift-wrapped to its machine key; MyCharter sends one after scanning. The daemon pins **the authenticated sender of the seal** (`seal_author`) — never a key claimed in the payload — then writes `pairing.json`, binds a locally-minted subject, and restarts into paired mode.

**Tech Stack:** Rust (charter-primitives, charter-transport, charterd), TypeScript/React (MyCharter PWA), wry/GTK + vanilla JS (charter-console), NIP-59 gift-wrap.

## Global Constraints

- Kind numbers live **only** in `core/crates/charter-primitives/src/kinds.rs`. No divergent constants.
- `PAIR_OFFER = 31117`. Add to the frozen-kinds test.
- The pinned guardian key MUST be `seal_author` from `nip59::unwrap_with_author` — the payload never carries a guardian pubkey to trust.
- Token comparison MUST be constant-time. Token is single-use and expires **600 seconds** after mint.
- Default relay is `wss://relay.trotters.cc` (`apps/charter-app/src/signer/config.ts:6`).
- The typed/paste path (`charter-pair --link`) MUST keep working unchanged — it is the offline fallback.
- Existing raw-hex QR/typed device codes MUST still parse in MyCharter (backwards compatibility).
- Copy uses the wardship lexicon: guardian / ward / charter / clause.
- Rust gates run from `linux/`: `cargo fmt`, `cargo clippy`, `cargo test`, real-build. PWA gates: `npm test`.

---

### Task 1: Reserve the PAIR_OFFER kind

**Files:**
- Modify: `core/crates/charter-primitives/src/kinds.rs`

**Interfaces:**
- Produces: `kinds::CHARTER_DEVICE_PAIR_OFFER: u16 = 31117`

- [ ] **Step 1: Write the failing test**

In `kinds.rs`, add to `mod tests`, inside `kinds_are_frozen()`:

```rust
        assert_eq!(CHARTER_DEVICE_PAIR_OFFER, 31117);
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charter-primitives kinds_are_frozen`
Expected: FAIL — `cannot find value CHARTER_DEVICE_PAIR_OFFER`

- [ ] **Step 3: Write minimal implementation**

Add above the curator constants in `kinds.rs`:

```rust
/// Charter device PAIR_OFFER (guardian -> machine, inner rumor): an offer to
/// pin this guardian, sent after the guardian's phone scans the ward's
/// on-screen pairing QR. Carries the one-time token from that QR — proof the
/// sender physically saw the screen. The device pins the SEAL AUTHOR, never a
/// key named in the payload.
pub const CHARTER_DEVICE_PAIR_OFFER: u16 = 31117;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charter-primitives kinds_are_frozen`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add core/crates/charter-primitives/src/kinds.rs
git commit -m "feat(wire): reserve kind 31117 for PAIR_OFFER"
```

---

### Task 2: PAIR_OFFER payload + constant-time token check

**Files:**
- Create: `core/crates/charter-transport/src/pair_offer.rs`
- Modify: `core/crates/charter-transport/src/lib.rs`

**Interfaces:**
- Consumes: `kinds::CHARTER_DEVICE_PAIR_OFFER` (Task 1)
- Produces:
  - `pub struct PairOffer { pub token: String, pub relays: Vec<RelayUrl>, pub ts: u64 }`
  - `pub fn parse_pair_offer(json: &str) -> Option<PairOffer>`
  - `pub fn build_pair_offer(token: &str, relays: &[RelayUrl], ts: u64) -> String`
  - `pub fn token_matches(expected: &str, got: &str) -> bool` (constant-time)

- [ ] **Step 1: Write the failing test**

Create `core/crates/charter-transport/src/pair_offer.rs` with only the tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_an_offer() {
        let relays = vec![RelayUrl::new("wss://relay.trotters.cc").unwrap()];
        let json = build_pair_offer("ab".repeat(16).as_str(), &relays, 1234);
        let got = parse_pair_offer(&json).unwrap();
        assert_eq!(got.token, "ab".repeat(16));
        assert_eq!(got.relays, relays);
        assert_eq!(got.ts, 1234);
    }

    #[test]
    fn rejects_a_token_that_is_not_32_hex() {
        let relays = vec![RelayUrl::new("wss://relay.trotters.cc").unwrap()];
        // too short, non-hex, and empty are all refused at the parse boundary
        for bad in ["abc", "zz".repeat(16).as_str(), ""] {
            let json = format!(
                r#"{{"token":"{bad}","relays":["wss://relay.trotters.cc"],"ts":1}}"#
            );
            assert!(parse_pair_offer(&json).is_none(), "accepted {bad}");
        }
        let _ = relays;
    }

    #[test]
    fn rejects_an_offer_with_no_secure_relay() {
        let json = r#"{"token":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","relays":["ws://nope"],"ts":1}"#;
        assert!(parse_pair_offer(json).is_none());
    }

    #[test]
    fn token_compare_is_exact() {
        let t = "a".repeat(32);
        assert!(token_matches(&t, &t));
        assert!(!token_matches(&t, &"b".repeat(32)));
        assert!(!token_matches(&t, &"a".repeat(31)));
        assert!(!token_matches(&t, ""));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charter-transport pair_offer`
Expected: FAIL — module not declared / items not found

- [ ] **Step 3: Write minimal implementation**

Prepend to `pair_offer.rs`:

```rust
//! The PAIR_OFFER payload (kind 31117): a guardian's offer to be pinned by an
//! unpaired ward, carrying the one-time token from the ward's on-screen QR.
//!
//! SECURITY: this payload deliberately carries **no guardian pubkey**. The
//! device pins the authenticated seal author instead, so a forged payload can
//! only ever nominate the sender themselves — which is the whole point.

use serde::{Deserialize, Serialize};

use charter_sys::relay::RelayUrl;

/// The one-time pairing token: 16 random bytes, lowercase hex.
pub const TOKEN_HEX_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairOffer {
    pub token: String,
    pub relays: Vec<RelayUrl>,
    pub ts: u64,
}

#[derive(Deserialize)]
struct RawOffer {
    token: String,
    relays: Vec<String>,
    ts: u64,
}

fn valid_token(t: &str) -> bool {
    t.len() == TOKEN_HEX_LEN && t.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Parse + validate an offer. Refuses a malformed token or an offer with no
/// `wss://` relay — the same secure-transport floor the `bunker://` pin uses.
pub fn parse_pair_offer(json: &str) -> Option<PairOffer> {
    let raw: RawOffer = serde_json::from_str(json).ok()?;
    if !valid_token(&raw.token) {
        return None;
    }
    let relays: Vec<RelayUrl> = raw
        .relays
        .iter()
        .filter(|r| r.starts_with("wss://"))
        .filter_map(|r| RelayUrl::new(r).ok())
        .collect();
    if relays.is_empty() {
        return None;
    }
    Some(PairOffer {
        token: raw.token.to_ascii_lowercase(),
        relays,
        ts: raw.ts,
    })
}

/// Serialize an offer for the inner rumor's `content`.
pub fn build_pair_offer(token: &str, relays: &[RelayUrl], ts: u64) -> String {
    let offer = PairOffer {
        token: token.to_string(),
        relays: relays.to_vec(),
        ts,
    };
    serde_json::to_string(&offer).expect("offer serializes")
}

/// Constant-time token comparison — never short-circuits on the first
/// differing byte, so a network attacker cannot walk the token out byte by
/// byte with timing.
pub fn token_matches(expected: &str, got: &str) -> bool {
    let (a, b) = (expected.as_bytes(), got.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}
```

Add to `core/crates/charter-transport/src/lib.rs` beside the other `pub mod` lines:

```rust
pub mod pair_offer;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charter-transport pair_offer`
Expected: PASS (4 tests)

- [ ] **Step 5: Commit**

```bash
git add core/crates/charter-transport/src/pair_offer.rs core/crates/charter-transport/src/lib.rs
git commit -m "feat(wire): PAIR_OFFER payload with constant-time token compare"
```

---

### Task 3: Transport — publish and poll PAIR_OFFERs

**Files:**
- Modify: `core/crates/charter-transport/src/transport.rs`

**Interfaces:**
- Consumes: `pair_offer::{build_pair_offer, parse_pair_offer, PairOffer}` (Task 2)
- Produces:
  - `pub struct ReceivedPairOffer { pub offer: PairOffer, pub seal_author: PubKey }`
  - `pub async fn poll_pair_offers(&self, since: u64, now: u64) -> Result<Vec<ReceivedPairOffer>, RelayIoError>`
  - `pub async fn send_pair_offer(&self, token: &str, now: u64) -> Vec<(RelayUrl, PublishOutcome)>`

- [ ] **Step 1: Write the failing test**

Append to `transport.rs`'s existing `mod tests` (mirror the fixture helpers already used by the grant/release tests in that module):

```rust
    #[test]
    fn pair_offer_round_trips_and_reports_the_seal_author() {
        // Guardian sends an offer to the machine; the machine polls it back and
        // learns WHO sealed it — the key it will pin.
        let (guardian_sk, guardian_pk) = test_keypair(7);
        let (machine_sk, machine_pk) = test_keypair(9);
        let relays = vec![RelayUrl::new("wss://relay.trotters.cc").unwrap()];

        let guardian = CharterTransport::new(
            MemoryRelay::default(),
            ScriptedEntropy::new(1),
            guardian_sk,
            machine_pk,
            relays.clone(),
        );
        let machine = CharterTransport::new(
            guardian.relay().clone(),
            ScriptedEntropy::new(2),
            machine_sk,
            guardian_pk,
            relays.clone(),
        );

        let token = "ab".repeat(16);
        block_on(guardian.send_pair_offer(&token, 1000));
        let got = block_on(machine.poll_pair_offers(0, 1000)).unwrap();

        assert_eq!(got.len(), 1);
        assert_eq!(got[0].offer.token, token);
        assert_eq!(got[0].seal_author, guardian_pk);
    }
```

> If `test_keypair` / `MemoryRelay` / `block_on` are named differently in this module, reuse whatever the neighbouring `poll_grants` test uses — do not introduce new helpers.

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charter-transport pair_offer_round_trips`
Expected: FAIL — no method `send_pair_offer`

- [ ] **Step 3: Write minimal implementation**

Add near `ReceivedGrant`:

```rust
/// A PAIR_OFFER delivered to this machine, with the authenticated key that
/// sealed it. `seal_author` is the ONLY key safe to pin.
#[derive(Debug, Clone)]
pub struct ReceivedPairOffer {
    pub offer: crate::pair_offer::PairOffer,
    pub seal_author: PubKey,
}
```

Add to the `impl` block:

```rust
    /// Guardian side: offer to be pinned by the ward whose machine key this
    /// transport addresses, proving physical sight of its screen via `token`.
    pub async fn send_pair_offer(
        &self,
        token: &str,
        now: u64,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        let content = crate::pair_offer::build_pair_offer(token, &self.relays, now);
        let rumor = Rumor {
            id: None,
            pubkey: self.machine_pk,
            created_at: now,
            kind: kinds::CHARTER_DEVICE_PAIR_OFFER,
            tags: vec![kinds::marker_tag()],
            content,
            sig: None,
        };
        self.publish_rumor(&rumor, now).await
    }

    /// Ward side: offers addressed to this machine. Malformed payloads are
    /// dropped silently — an unpaired device is an open mailbox and must not
    /// be knocked over by junk.
    pub async fn poll_pair_offers(
        &self,
        since: u64,
        now: u64,
    ) -> Result<Vec<ReceivedPairOffer>, RelayIoError> {
        let filter = Filter {
            kinds: vec![kinds::GIFT_WRAP],
            p_tags: vec![self.machine_pk],
            since: Some(since),
            ..Default::default()
        };
        let wraps = self.relay.query(&self.relays, filter).await?;
        let mut out = Vec::new();
        for wrap in wraps.into_iter().take(self.max_inbound) {
            if let Ok((rumor, seal_author)) =
                nip59::unwrap_with_author(&wrap, &self.machine_sk, now, nip59::MAX_JITTER_SECS)
            {
                if rumor.kind == kinds::CHARTER_DEVICE_PAIR_OFFER {
                    if let Some(offer) = crate::pair_offer::parse_pair_offer(&rumor.content) {
                        out.push(ReceivedPairOffer { offer, seal_author });
                    }
                }
            }
        }
        Ok(out)
    }
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charter-transport`
Expected: PASS, no regressions

- [ ] **Step 5: Commit**

```bash
git add core/crates/charter-transport/src/transport.rs
git commit -m "feat(wire): publish + poll PAIR_OFFER, surfacing the seal author"
```

---

### Task 4: The one-time pairing token store

**Files:**
- Create: `linux/crates/charterd/src/pair_token.rs`
- Modify: `linux/crates/charterd/src/lib.rs`

**Interfaces:**
- Consumes: `pair_offer::TOKEN_HEX_LEN` (Task 2)
- Produces:
  - `pub const TOKEN_TTL_SECS: u64 = 600;`
  - `pub fn mint(path: &str, now: u64, random: [u8; 16]) -> std::io::Result<String>`
  - `pub fn current(path: &str, now: u64) -> Option<String>` (None when absent or expired)
  - `pub fn consume(path: &str) -> std::io::Result<()>`

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charterd/src/pair_token.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("charter-token-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("pair-token.json").to_string_lossy().into_owned()
    }

    #[test]
    fn a_minted_token_reads_back_until_it_expires() {
        let p = tmp("mint");
        let t = mint(&p, 1_000, [0xAB; 16]).unwrap();
        assert_eq!(t.len(), 32);
        assert_eq!(current(&p, 1_000).as_deref(), Some(t.as_str()));
        assert_eq!(current(&p, 1_000 + TOKEN_TTL_SECS - 1).as_deref(), Some(t.as_str()));
        // At and past the TTL it is gone.
        assert!(current(&p, 1_000 + TOKEN_TTL_SECS).is_none());
    }

    #[test]
    fn consume_makes_it_unusable() {
        let p = tmp("consume");
        mint(&p, 1_000, [0x11; 16]).unwrap();
        consume(&p).unwrap();
        assert!(current(&p, 1_000).is_none());
    }

    #[test]
    fn absent_or_junk_token_file_is_simply_no_token() {
        let p = tmp("junk");
        assert!(current(&p, 1_000).is_none());
        std::fs::write(&p, b"not json").unwrap();
        assert!(current(&p, 1_000).is_none());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charterd pair_token`
Expected: FAIL — module not found

- [ ] **Step 3: Write minimal implementation**

Prepend to `pair_token.rs`:

```rust
//! The ward's one-time pairing token: minted while an unpaired ward shows its
//! pairing QR, spent the moment a guardian's offer proves knowledge of it.
//!
//! Knowing this token means having physically looked at the ward's screen —
//! the same trust basis as typing on the machine, which is why it is enough to
//! authorise a pin.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// How long a shown token stays valid. Long enough to fetch a phone, short
/// enough that a screen glimpsed in passing goes stale.
pub const TOKEN_TTL_SECS: u64 = 600;

#[derive(Serialize, Deserialize)]
struct Stored {
    token: String,
    minted_at: u64,
}

/// Mint + persist a fresh token, returning its hex. Overwrites any previous
/// one, so re-opening the pairing screen always invalidates the old QR.
pub fn mint(path: &str, now: u64, random: [u8; 16]) -> std::io::Result<String> {
    let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
    if let Some(dir) = Path::new(path).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = serde_json::to_string(&Stored {
        token: token.clone(),
        minted_at: now,
    })
    .expect("token serializes");
    let tmp = format!("{path}.tmp");
    std::fs::write(&tmp, body)?;
    // 0600 — unlike the pairing pin this IS a secret; only root reads it.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, path)?;
    Ok(token)
}

/// The live token, or `None` when absent, unreadable, junk, or expired.
pub fn current(path: &str, now: u64) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let stored: Stored = serde_json::from_str(&text).ok()?;
    if now.saturating_sub(stored.minted_at) >= TOKEN_TTL_SECS {
        return None;
    }
    Some(stored.token)
}

/// Spend the token — single use, so a replayed offer finds nothing.
pub fn consume(path: &str) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}
```

Add to `linux/crates/charterd/src/lib.rs`:

```rust
pub mod pair_token;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charterd pair_token`
Expected: PASS (3 tests)

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charterd/src/pair_token.rs linux/crates/charterd/src/lib.rs
git commit -m "feat(charterd): one-time pairing token with TTL and single use"
```

---

### Task 5: Accept-offer decision logic (pure)

**Files:**
- Create: `linux/crates/charterd/src/pair_accept.rs`
- Modify: `linux/crates/charterd/src/lib.rs`

**Interfaces:**
- Consumes: `ReceivedPairOffer` (Task 3), `token_matches` (Task 2)
- Produces: `pub fn choose_offer(offers: &[ReceivedPairOffer], expected_token: &str, now: u64) -> Option<PubKey>`

The pure decision is separated from I/O so every rejection path is unit-tested.

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charterd/src/pair_accept.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use charter_transport::pair_offer::PairOffer;
    use charter_sys::relay::RelayUrl;

    fn offer(token: &str, ts: u64, who: u8) -> ReceivedPairOffer {
        ReceivedPairOffer {
            offer: PairOffer {
                token: token.to_string(),
                relays: vec![RelayUrl::new("wss://relay.trotters.cc").unwrap()],
                ts,
            },
            seal_author: PubKey::from_bytes([who; 32]),
        }
    }

    const GOOD: &str = "abababababababababababababababab";

    #[test]
    fn accepts_the_offer_that_proves_the_token() {
        let got = choose_offer(&[offer(GOOD, 100, 0xAA)], GOOD, 100);
        assert_eq!(got, Some(PubKey::from_bytes([0xAA; 32])));
    }

    #[test]
    fn refuses_a_wrong_token() {
        let bad = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        assert!(choose_offer(&[offer(bad, 100, 0xAA)], GOOD, 100).is_none());
    }

    #[test]
    fn refuses_a_stale_offer() {
        // Offered more than a TTL ago — a replayed capture must not pin.
        let stale = 100;
        let now = stale + charterd_ttl() + 1;
        assert!(choose_offer(&[offer(GOOD, stale, 0xAA)], GOOD, now).is_none());
    }

    #[test]
    fn refuses_a_future_dated_offer() {
        assert!(choose_offer(&[offer(GOOD, 10_000, 0xAA)], GOOD, 100).is_none());
    }

    #[test]
    fn refuses_outright_when_two_offers_race_the_same_token() {
        // Two different keys both presenting the token means the screen was
        // seen by someone unexpected. Pin NEITHER and make the parent retry.
        let both = [offer(GOOD, 100, 0xAA), offer(GOOD, 100, 0xBB)];
        assert!(choose_offer(&both, GOOD, 100).is_none());
    }

    fn charterd_ttl() -> u64 {
        crate::pair_token::TOKEN_TTL_SECS
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charterd pair_accept`
Expected: FAIL — module not found

- [ ] **Step 3: Write minimal implementation**

Prepend to `pair_accept.rs`:

```rust
//! Who — if anyone — may pin this ward. Pure decision, no I/O, so every
//! refusal path is covered by a unit test.

use charter_primitives::PubKey;
use charter_transport::pair_offer::token_matches;
use charter_transport::transport::ReceivedPairOffer;

use crate::pair_token::TOKEN_TTL_SECS;

/// The guardian key to pin, or `None` to keep waiting.
///
/// Refuses: a mismatched token, an offer older than the token's own TTL, an
/// offer dated in the future (a clock-skew forgery), and — deliberately — a
/// tie, where two distinct keys both present the token. A tie means the token
/// leaked; pinning either would be a coin flip on who owns the ward.
pub fn choose_offer(
    offers: &[ReceivedPairOffer],
    expected_token: &str,
    now: u64,
) -> Option<PubKey> {
    let mut winner: Option<PubKey> = None;
    for o in offers {
        if !token_matches(expected_token, &o.offer.token) {
            continue;
        }
        if o.offer.ts > now || now.saturating_sub(o.offer.ts) >= TOKEN_TTL_SECS {
            continue;
        }
        match winner {
            None => winner = Some(o.seal_author),
            Some(w) if w == o.seal_author => {}
            Some(_) => return None, // contested token — pin nobody
        }
    }
    winner
}
```

Add to `linux/crates/charterd/src/lib.rs`:

```rust
pub mod pair_accept;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charterd pair_accept`
Expected: PASS (5 tests)

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charterd/src/pair_accept.rs linux/crates/charterd/src/lib.rs
git commit -m "feat(charterd): pair-offer acceptance rules, incl. contested-token refusal"
```

---

### Task 6: Commit a pin from an accepted offer

**Files:**
- Create: `linux/crates/charterd/src/pair_commit.rs`
- Modify: `linux/crates/charterd/src/lib.rs`

**Interfaces:**
- Consumes: `build_pairing_json` (`pairing_setup.rs`), `set_child_subject` + `load_child_configs` (`device_limits.rs`), `pair_token::consume` (Task 4)
- Produces: `pub fn commit_pin(paths: &PinPaths, guardian: PubKey, relays: &[RelayUrl], machine: PubKey, subject_random: [u8; 32], now: u64) -> Result<(), String>` and `pub struct PinPaths { pub pairing: String, pub limits_dir: String, pub token: String }`

The subject is **minted locally**: MyCharter has no dependant pubkey (`store.tsx:571` sets it `null` and nothing ever assigns it), and the broker routes a subject-less clause to the pairing's sole subject (`core/crates/charter-spine/src/broker.rs:367-406`). So a locally random subject is correct and removes the child-ID field entirely.

- [ ] **Step 1: Write the failing test**

Create `linux/crates/charterd/src/pair_commit.rs` with tests only:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> (PinPaths, String) {
        let d = std::env::temp_dir().join(format!("charter-pin-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("limits.d")).unwrap();
        std::fs::write(
            d.join("limits.d/axel.json"),
            r#"{"limits":{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}"#,
        )
        .unwrap();
        let paths = PinPaths {
            pairing: d.join("pairing.json").to_string_lossy().into_owned(),
            limits_dir: d.join("limits.d").to_string_lossy().into_owned(),
            token: d.join("pair-token.json").to_string_lossy().into_owned(),
        };
        let child = d.join("limits.d/axel.json").to_string_lossy().into_owned();
        (paths, child)
    }

    #[test]
    fn writes_the_pin_binds_the_subject_and_spends_the_token() {
        let (paths, child) = fixture("ok");
        crate::pair_token::mint(&paths.token, 10, [0x22; 16]).unwrap();
        let relays = vec![RelayUrl::new("wss://relay.trotters.cc").unwrap()];

        commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays,
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap();

        let pinned = std::fs::read_to_string(&paths.pairing).unwrap();
        assert!(pinned.contains(&"aa".repeat(32)), "guardian pinned");
        // The subject is bound to the sole child, so clauses RESOLVE.
        let kid = std::fs::read_to_string(&child).unwrap();
        assert!(kid.contains(&"cc".repeat(32)), "subject bound: {kid}");
        // Token spent — a replayed offer finds nothing.
        assert!(crate::pair_token::current(&paths.token, 100).is_none());
    }

    #[test]
    fn refuses_when_there_is_no_child_to_bind() {
        let (paths, child) = fixture("nokid");
        std::fs::remove_file(&child).unwrap();
        let relays = vec![RelayUrl::new("wss://relay.trotters.cc").unwrap()];
        let err = commit_pin(
            &paths,
            PubKey::from_bytes([0xAA; 32]),
            &relays,
            PubKey::from_bytes([0xBB; 32]),
            [0xCC; 32],
            100,
        )
        .unwrap_err();
        assert!(err.contains("child"), "{err}");
        // Nothing half-written.
        assert!(!std::path::Path::new(&paths.pairing).exists());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charterd pair_commit`
Expected: FAIL — module not found

- [ ] **Step 3: Write minimal implementation**

Prepend to `pair_commit.rs`:

```rust
//! Turn an accepted offer into a pinned pairing on disk — the same end state
//! `charter-pair` reaches from a pasted `bunker://` link, so the two paths
//! converge and the daemon below them is unchanged.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use charter_primitives::PubKey;
use charter_sys::relay::RelayUrl;

use crate::device_limits::{load_child_configs, set_child_subject};
use crate::pairing_setup::build_pairing_json;

pub struct PinPaths {
    pub pairing: String,
    pub limits_dir: String,
    pub token: String,
}

/// Write the pin, bind the subject, spend the token.
///
/// Order matters: bind the child FIRST. A pairing with no bound subject leaves
/// the guardian's clauses inert while the UI claims success — the exact trap
/// `charter-pair` documents at its step 6.
pub fn commit_pin(
    paths: &PinPaths,
    guardian: PubKey,
    relays: &[RelayUrl],
    machine: PubKey,
    subject_random: [u8; 32],
    now: u64,
) -> Result<(), String> {
    let children = load_child_configs(&paths.limits_dir);
    let child = match children.as_slice() {
        [] => return Err("no child is set up on this computer yet".into()),
        [(user, _)] => user.clone(),
        _ => return Err("more than one child on this computer — pair from the app".into()),
    };

    let subject_hex: String = subject_random.iter().map(|b| format!("{b:02x}")).collect();
    let subject = PubKey::from_hex(&subject_hex).map_err(|_| "bad subject".to_string())?;

    let relay_params = relays
        .iter()
        .map(|r| format!("relay={r}"))
        .collect::<Vec<_>>()
        .join("&");
    let uri = format!("bunker://{}?{relay_params}&kind=charter", guardian.to_hex());
    let json = build_pairing_json(&uri, machine, subject, now)
        .map_err(|_| "could not build the pairing".to_string())?;

    set_child_subject(&paths.limits_dir, &child, Some(&subject_hex))
        .map_err(|e| format!("could not link {child}: {e}"))?;

    if let Some(dir) = Path::new(&paths.pairing).parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = format!("{}.tmp", paths.pairing);
    std::fs::write(&tmp, &json).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
        .map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &paths.pairing).map_err(|e| e.to_string())?;

    crate::pair_token::consume(&paths.token).map_err(|e| e.to_string())?;
    Ok(())
}
```

Add to `linux/crates/charterd/src/lib.rs`:

```rust
pub mod pair_commit;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charterd pair_commit`
Expected: PASS (2 tests)

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charterd/src/pair_commit.rs linux/crates/charterd/src/lib.rs
git commit -m "feat(charterd): commit a pin from an accepted pair offer"
```

---

### Task 7: The unpaired daemon listens for offers

**Files:**
- Modify: `linux/crates/charterd/src/runtime.rs:1053-1061` (the `None` arm)

**Interfaces:**
- Consumes: `poll_pair_offers` (Task 3), `pair_token::current` (Task 4), `choose_offer` (Task 5), `commit_pin` (Task 6)

Today the `None` arm builds no transport at all, so an unpaired ward cannot hear anything. It gains a **narrow** listener: PAIR_OFFERs only, and only while a live token exists.

- [ ] **Step 1: Write the failing test**

Add to `linux/crates/charterd/tests/real_runtime.rs`:

```rust
#[test]
fn an_unpaired_ward_pins_the_guardian_that_proves_the_token() {
    // Full loop over the in-memory relay: token minted -> guardian offers ->
    // ward pins that guardian and binds a subject.
    let dir = tempfile::tempdir().unwrap();
    let limits = dir.path().join("limits.d");
    std::fs::create_dir_all(&limits).unwrap();
    std::fs::write(
        limits.join("axel.json"),
        r#"{"limits":{"tz":"Europe/London","wake":"07:00","bedtime":"19:00","dailyMinutes":120}}"#,
    )
    .unwrap();

    let paths = charterd::pair_commit::PinPaths {
        pairing: dir.path().join("pairing.json").to_string_lossy().into_owned(),
        limits_dir: limits.to_string_lossy().into_owned(),
        token: dir.path().join("pair-token.json").to_string_lossy().into_owned(),
    };
    let token = charterd::pair_token::mint(&paths.token, 100, [0x33; 16]).unwrap();

    let accepted = charterd::pair_listener::try_pair_once(&paths, &offers_from_guardian(&token), 100);
    assert!(accepted.is_some(), "should have pinned");
    assert!(std::path::Path::new(&paths.pairing).exists());
}
```

> `offers_from_guardian(&token)` builds a `Vec<ReceivedPairOffer>` with a known `seal_author`, exactly as in Task 5's `offer()` helper. Copy that helper into this test file rather than sharing it across crates.

- [ ] **Step 2: Run test to verify it fails**

Run: `cd linux && cargo test -p charterd an_unpaired_ward_pins`
Expected: FAIL — no `pair_listener`

- [ ] **Step 3: Write minimal implementation**

Create `linux/crates/charterd/src/pair_listener.rs`:

```rust
//! The unpaired ward's narrow inbound door: PAIR_OFFERs only, open only while
//! a freshly-shown QR's token is live. Everything else an unpaired device
//! might be sent is ignored.

use charter_primitives::PubKey;
use charter_sys::relay::RelayUrl;
use charter_transport::transport::ReceivedPairOffer;

use crate::pair_accept::choose_offer;
use crate::pair_commit::{commit_pin, PinPaths};

/// One pass: given the offers polled this tick, pin a guardian if exactly one
/// proves the live token. Returns the pinned key. Pure of I/O except the
/// commit itself, so the loop above stays trivial.
pub fn try_pair_once(
    paths: &PinPaths,
    offers: &[ReceivedPairOffer],
    now: u64,
) -> Option<PubKey> {
    let token = crate::pair_token::current(&paths.token, now)?;
    let guardian = choose_offer(offers, &token, now)?;
    let relays: Vec<RelayUrl> = offers
        .iter()
        .find(|o| o.seal_author == guardian)
        .map(|o| o.offer.relays.clone())
        .unwrap_or_default();
    let machine = crate::pairing_setup::read_device_pub("/var/lib/charter/device.pub")?;
    let mut subject = [0u8; 32];
    getrandom::getrandom(&mut subject).ok()?;
    match commit_pin(paths, guardian, &relays, machine, subject, now) {
        Ok(()) => Some(guardian),
        Err(e) => {
            eprintln!("charterd: pairing offer refused — {e}");
            None
        }
    }
}
```

Register it in `lib.rs` (`pub mod pair_listener;`) and wire the `None` arm of `runtime.rs` to poll every 5s while a token is live, calling `try_pair_once`, then `systemctl try-restart charterd.service` on success.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd linux && cargo test -p charterd`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add linux/crates/charterd/src/pair_listener.rs linux/crates/charterd/src/lib.rs linux/crates/charterd/src/runtime.rs linux/crates/charterd/tests/real_runtime.rs
git commit -m "feat(charterd): unpaired wards listen for a pair offer and pin it"
```

---

### Task 8: QR carries the token; console waits for the phone

**Files:**
- Modify: `apps/charter-console/src/main.rs:336-346` (status), `:452-464` (code/QR), `:475-490` (`qr_svg`)
- Modify: `apps/charter-console/ui/app.html:360-376` (Connect page)

**Interfaces:**
- Produces: QR content `charter://pair?m=<64hex>&t=<32hex>`; status gains `"pairToken": <hex|null>`

The **typed** 8-group code stays the bare device code — the offline fallback is untouched.

- [ ] **Step 1: Write the failing test**

Add to `apps/charter-console/src/main.rs` tests:

```rust
    #[test]
    fn the_qr_payload_carries_both_the_machine_and_the_token() {
        let got = pair_qr_payload("ab".repeat(32).as_str(), Some("cd".repeat(16).as_str()));
        assert_eq!(got, format!("charter://pair?m={}&t={}", "ab".repeat(32), "cd".repeat(16)));
    }

    #[test]
    fn without_a_token_the_qr_stays_the_bare_device_code() {
        // Backwards compatible: older MyCharter builds scan raw hex.
        let got = pair_qr_payload("ab".repeat(32).as_str(), None);
        assert_eq!(got, "ab".repeat(32));
    }
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd apps/charter-console && cargo test the_qr_payload`
Expected: FAIL — `pair_qr_payload` not found

- [ ] **Step 3: Write minimal implementation**

```rust
/// What the pairing QR encodes. With a live token this is the scan-to-pair
/// URI; without one it degrades to the bare device code so older MyCharter
/// builds (and the typed fallback) keep working.
fn pair_qr_payload(machine_hex: &str, token: Option<&str>) -> String {
    match token {
        Some(t) => format!("charter://pair?m={machine_hex}&t={t}"),
        None => machine_hex.to_string(),
    }
}
```

Feed it into `qr_svg`, add `"pairToken"` to `collect_status`, and change the Connect page's three steps to:

```
1. On your phone, open MyCharter and add your child.
2. Choose "Set up a computer" and point the camera at this code.
3. That's it — this page will say "Phone connected" on its own.
```

Keep the link+ID form behind a "No internet? Enter a code by hand" disclosure.

- [ ] **Step 4: Run test to verify it passes**

Run: `cd apps/charter-console && cargo test`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/charter-console/src/main.rs apps/charter-console/ui/app.html
git commit -m "feat(console): scan-to-pair QR + wait-for-phone Connect page"
```

---

### Task 9: MyCharter sends the offer after scanning

**Files:**
- Modify: `apps/charter-app/src/wire/deviceCode.ts` (parse `charter://pair?…`)
- Create: `apps/charter-app/src/wire/pairOffer.ts`
- Modify: `apps/charter-app/src/screens/Family.tsx:880-930` (scan handler)
- Modify: `apps/charter-app/src/signer/realSigner.ts` (add `sendPairOffer`)

**Interfaces:**
- Consumes: kind `31117`
- Produces: `parseDevicePairingCode` returns `{ machine: string; token?: string }`; `RealSigner.sendPairOffer(machine: string, token: string): Promise<boolean>`

- [ ] **Step 1: Write the failing test**

Add to `apps/charter-app/src/wire/deviceCode.test.ts`:

```ts
it("parses a scan-to-pair URI into machine + token", () => {
  const m = "ab".repeat(32), t = "cd".repeat(16);
  expect(parseDevicePairingCode(`charter://pair?m=${m}&t=${t}`)).toEqual({ machine: m, token: t });
});

it("still parses a bare device code, with no token", () => {
  const m = "ab".repeat(32);
  expect(parseDevicePairingCode(m)).toEqual({ machine: m });
});

it("rejects a scan-to-pair URI whose token is not 32 hex", () => {
  const m = "ab".repeat(32);
  expect(parseDevicePairingCode(`charter://pair?m=${m}&t=nope`)).toBeNull();
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `npm test -- deviceCode`
Expected: FAIL

- [ ] **Step 3: Write minimal implementation**

Extend `parseDevicePairingCode` to recognise the `charter://pair` form (validating `m` as 64 hex and `t` as 32 hex) while leaving every existing hex/npub/nprofile branch intact and returning `{ machine }` for them. Then, in the `SetupComputer` scan handler, when a `token` is present and the signer is local, call `sendPairOffer(machine, token)` and show "Connecting…" until the device's STATUS appears.

- [ ] **Step 4: Run test to verify it passes**

Run: `npm test -- deviceCode`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/charter-app/src
git commit -m "feat(mycharter): send a pair offer after scanning a computer"
```

---

### Task 10: Contract + docs

**Files:**
- Modify: `spec/contract.md` (kind table + a PAIR_OFFER section)
- Modify: `apps/charter-app/src/screens/Guide.tsx:78-82,244-249`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Document the kind**

Add `31117` to the kinds table (`contract.md:1090`) and a section specifying: payload `{token, relays, ts}`; **pin the seal author, never a payload key**; token 32 hex, single-use, 600s TTL; contested token pins nobody; the offline `bunker://` paste remains normative.

- [ ] **Step 2: Update the guide copy**

Replace the "type the code" instructions with the scan-only flow, keeping the typed code documented as the no-internet fallback.

- [ ] **Step 3: Run the full gates**

```bash
cd linux && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
cd .. && npm test
```

- [ ] **Step 4: Commit**

```bash
git add spec/contract.md apps/charter-app/src/screens/Guide.tsx CHANGELOG.md
git commit -m "docs: specify PAIR_OFFER (31117) and the scan-to-pair flow"
```

---

## Hardware gate (decented)

Code-ready ends at Task 10. On Rob's laptop:

1. `sudo apt install ./charter-latest.deb` (or the built `.deb`)
2. Open Charter → **Connect a phone**
3. On the phone: MyCharter → **Set up a computer** → scan
4. Expect: the laptop flips to **"Phone connected"** within ~10s, unaided
5. Verify: `grep -o '"guardian_pubkey":"[^"]*"' /var/lib/charter/pairing.json` matches the phone, and `/etc/charter/limits.d/examplegame.json` now has a `subject`
6. Change a limit from the phone and confirm it lands
