//! The ward side of *Guardian roots and working keys (v1)* as a pure state
//! machine.
//!
//! - The state is plain serde data, persisted as is.
//! - Nothing here reads a clock, does IO or draws randomness: every entry
//!   point takes `now` (Unix seconds) and the ward mints presence tokens
//!   itself.
//! - Every entry point first advances timers to `now` (pending expiry, the
//!   72-hour removal notice, token lifetime), exactly as [`tick`] does, so a
//!   departure that is due is applied before anything else is judged.
//! - Every entry point returns a new state; the input is never changed. A
//!   dropped step still returns the state: timers have advanced, and a few
//!   drops (an objected or last-root removal at confirmation) drop the
//!   pending statement.
//!
//! Where the contract is silent this module takes the reference's fail-closed
//! reading, recorded in `docs/guardian-keys-open-questions.md` ("OQ: …").

use std::cmp::Ordering;
use std::collections::{BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{constant_time_eq, is_hex32, is_hex64, is_one, unix_seconds, verified_event};
use crate::event::{has_marker, Event};
use crate::json::parse_strict_json;
use crate::pin::{validate_pin_record, GuardianPinRecord};
use crate::reason::{Reason, TickResult};
use crate::statement::{parse_statement, PendingType, Statement, StatementType};
use crate::*;

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// How a root came to be pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PinnedVia {
    Pair,
    Coguardian,
    Move,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PinnedRoot {
    pub root: String,
    pub pinned_at: i64,
    pub via: PinnedVia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeyStatus {
    Active,
    Withdrawn,
    Departed,
}

/// Why a key or root left the signer set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepartedBy {
    Withdrawal,
    Removal,
    Move,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkingKeyRecord {
    pub key: String,
    /// The binding root (rule 1). For a departed key, the root it was bound to.
    pub root: String,
    pub status: KeyStatus,
    pub label: Option<String>,
    pub approval_id: String,
    pub approval_issued_at: i64,
    /// Why the key left the signer set, if it did.
    pub departed_by: Option<DepartedBy>,
    /// For a key departed by a root move: the new root that may re-bind it.
    pub rebind_root: Option<String>,
}

/// The newest accepted statement per slot (rule 2). The slot is the `d`,
/// except a cancel (`d|signer`) and an objection (`d|p`). Withdrawals live in
/// `held_withdrawals` / `tombstones`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredStatement {
    pub slot: String,
    pub d: String,
    pub id: String,
    #[serde(rename = "type")]
    pub kind: StatementType,
    pub signer: String,
    pub p: String,
    pub issued_at: i64,
    pub label: Option<String>,
    pub expires_at: Option<i64>,
    /// The ward clock when it was accepted.
    pub received_at: i64,
    /// A cancel or objection that had nothing to act on when it arrived.
    pub held: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRecord {
    pub d: String,
    #[serde(rename = "type")]
    pub kind: PendingType,
    /// The pending statement's event id (named by a presence confirmation).
    pub id: String,
    pub signer: String,
    /// R2.
    pub key: String,
    pub issued_at: i64,
    /// The ward clock when the statement was accepted.
    pub received_at: i64,
    pub expires_at: i64,
    pub confirmed_at: Option<i64>,
    pub takes_effect_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldWithdrawal {
    pub d: String,
    pub id: String,
    pub signer: String,
    pub key: String,
    pub issued_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TombstoneType {
    Withdrawal,
    Removal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tombstone {
    #[serde(rename = "type")]
    pub kind: TombstoneType,
    pub d: String,
    pub id: String,
    pub key: String,
    pub signer: String,
    pub issued_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepartedRoot {
    pub root: String,
    pub issued_at: i64,
    /// `Removal` or `Move`.
    pub by: DepartedBy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresenceToken {
    pub token: String,
    /// The pending statement the token was minted for, or `None` for any.
    pub statement_d: Option<String>,
    pub minted_at: i64,
}

/// A held clause: the minimal model of rules 4 and 5.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeldClause {
    pub subject: String,
    pub kind: String,
    pub signer: String,
    /// The signer's root at acceptance, for "set by" display (rule 5).
    pub root: String,
    pub issued_at: i64,
    /// Set when the signer departs: min(`issued_at`, the departure time).
    /// OQ: Clause floor after a departure.
    pub floor: Option<i64>,
}

/// A relay list replaced within the last 7 days (rule 11).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviousRelays {
    pub relays: Vec<String>,
    pub until: i64,
}

/// The whole ward-side guardian-keys state of one pairing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GuardianState {
    pub v: u8,
    pub paired_at: i64,
    pub roots: Vec<PinnedRoot>,
    pub keys: Vec<WorkingKeyRecord>,
    pub statements: Vec<StoredStatement>,
    pub pending: Vec<PendingRecord>,
    pub held_withdrawals: Vec<HeldWithdrawal>,
    pub tombstones: Vec<Tombstone>,
    pub departed_roots: Vec<DepartedRoot>,
    pub tokens: Vec<PresenceToken>,
    pub clauses: Vec<HeldClause>,
    pub relays: Vec<String>,
    pub relays_updated_at: i64,
    /// Replaced relay lists, oldest first, at most [`MAX_PREVIOUS_RELAYS`].
    pub previous_relays: Vec<PreviousRelays>,
    /// The Android guardian PIN record, set by a gift-wrapped RELAYS.
    pub guardian_pin: Option<GuardianPinRecord>,
    /// Departed keys owed one last STATUS showing their departure.
    pub farewell: Vec<String>,
}

/// One pending statement resolved by the timer advance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TickEntry {
    pub d: String,
    pub result: TickResult,
    pub reason: Reason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Accepted,
    Dropped,
}

/// What a step did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Outcome {
    pub result: Verdict,
    pub reason: Reason,
    /// Pending statements resolved by the timer advance that ran first.
    pub ticked: Vec<TickEntry>,
    /// Keys that left the signer set during this step, sorted.
    pub departed: Vec<String>,
}

impl Outcome {
    pub fn accepted(&self) -> bool {
        self.result == Verdict::Accepted
    }
}

/// The new state and what happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub state: GuardianState,
    pub outcome: Outcome,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// String order as the reference sorts: by UTF-16 code unit. Identical to
/// byte order for the hex and ASCII strings that make up nearly all of it.
pub(crate) fn cmp_str(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

fn sorted_unique(v: impl IntoIterator<Item = String>) -> Vec<String> {
    // Hex keys only: byte order equals code-unit order.
    v.into_iter().collect::<BTreeSet<_>>().into_iter().collect()
}

/// Sort every list whose order carries no meaning, so the state is stable.
fn normalise(s: &mut GuardianState) {
    s.keys.sort_by(|a, b| cmp_str(&a.key, &b.key));
    s.statements.sort_by(|a, b| cmp_str(&a.slot, &b.slot));
    s.pending.sort_by(|a, b| cmp_str(&a.d, &b.d));
    s.held_withdrawals
        .sort_by(|a, b| cmp_str(&a.d, &b.d).then_with(|| cmp_str(&a.signer, &b.signer)));
    s.departed_roots.sort_by(|a, b| cmp_str(&a.root, &b.root));
    s.clauses
        .sort_by(|a, b| cmp_str(&a.subject, &b.subject).then_with(|| cmp_str(&a.kind, &b.kind)));
    s.farewell = sorted_unique(std::mem::take(&mut s.farewell));
}

/// `key` is a pinned root.
pub fn is_pinned(s: &GuardianState, key: &str) -> bool {
    s.roots.iter().any(|r| r.root == key)
}

fn key_index(s: &GuardianState, key: &str) -> Option<usize> {
    s.keys.iter().position(|k| k.key == key)
}

fn key_record<'a>(s: &'a GuardianState, key: &str) -> Option<&'a WorkingKeyRecord> {
    s.keys.iter().find(|k| k.key == key)
}

fn active_key<'a>(s: &'a GuardianState, key: &str) -> Option<&'a WorkingKeyRecord> {
    key_record(s, key).filter(|k| k.status == KeyStatus::Active && is_pinned(s, &k.root))
}

/// A root that was removed or moved away and is not pinned again.
fn is_departed_root(s: &GuardianState, key: &str) -> bool {
    !is_pinned(s, key) && s.departed_roots.iter().any(|d| d.root == key)
}

/// The signer set: every pinned root, plus every active working key bound to
/// a pinned root. Sorted.
pub fn signer_set(s: &GuardianState) -> Vec<String> {
    let mut out: Vec<String> = s.roots.iter().map(|r| r.root.clone()).collect();
    for k in &s.keys {
        if k.status == KeyStatus::Active && is_pinned(s, &k.root) {
            out.push(k.key.clone());
        }
    }
    sorted_unique(out)
}

/// The signer-set gate for CLAUSE, GRANT, RELAYS and USAGE_SYNC authors.
pub fn is_signer(s: &GuardianState, pubkey: &str) -> bool {
    is_pinned(s, pubkey) || active_key(s, pubkey).is_some()
}

/// The root a signer speaks for: itself if pinned, its binding root if an
/// active working key, else `None`.
pub fn root_of(s: &GuardianState, pubkey: &str) -> Option<String> {
    if is_pinned(s, pubkey) {
        return Some(pubkey.to_owned());
    }
    active_key(s, pubkey).map(|k| k.root.clone())
}

/// A key not in the signer set that is known as withdrawn, departed or a
/// departed root (rule 4). Includes a key withdrawn the moment it bound.
pub fn is_departed(s: &GuardianState, key: &str) -> bool {
    if is_signer(s, key) {
        return false;
    }
    if key_record(s, key).is_some_and(|k| k.status != KeyStatus::Active) {
        return true;
    }
    is_departed_root(s, key)
}

fn active_count(s: &GuardianState, root: &str) -> usize {
    s.keys
        .iter()
        .filter(|k| k.status == KeyStatus::Active && k.root == root)
        .count()
}

fn held_count(s: &GuardianState, signer: &str) -> usize {
    s.held_withdrawals
        .iter()
        .filter(|h| h.signer == signer)
        .count()
        + s.statements
            .iter()
            .filter(|st| st.held && st.signer == signer)
            .count()
}

/// Rule 10: a root at its tombstone or held quota can't approve new working
/// keys. OQ: Held-statement quota.
fn root_at_quota(s: &GuardianState, root: &str) -> bool {
    s.tombstones.iter().filter(|t| t.signer == root).count() >= MAX_TOMBSTONES_PER_ROOT
        || held_count(s, root) >= MAX_HELD_PER_ROOT
}

/// Rule 9: every active working key, plus each pinned root with none. Sorted.
pub fn recipients(s: &GuardianState) -> Vec<String> {
    let mut out = Vec::new();
    for r in &s.roots {
        let ws: Vec<String> = s
            .keys
            .iter()
            .filter(|k| k.status == KeyStatus::Active && k.root == r.root)
            .map(|k| k.key.clone())
            .collect();
        if ws.is_empty() {
            out.push(r.root.clone());
        } else {
            out.extend(ws);
        }
    }
    sorted_unique(out)
}

/// STATUS recipients: the live recipients plus departed keys owed one last
/// STATUS. Sorted.
pub fn status_recipients(s: &GuardianState) -> Vec<String> {
    sorted_unique(recipients(s).into_iter().chain(s.farewell.iter().cloned()))
}

/// Rule 11: the relays STATUS goes out on: the current list, then (on a
/// multi-root ward) every list replaced within the last 7 days, newest first,
/// without repeats.
pub fn status_relays(s: &GuardianState, now: i64) -> Vec<String> {
    let mut out = s.relays.clone();
    if s.roots.len() > 1 {
        for prev in s.previous_relays.iter().rev() {
            if now >= prev.until {
                continue;
            }
            for r in &prev.relays {
                if !out.contains(r) {
                    out.push(r.clone());
                }
            }
        }
    }
    out
}

/// Pending expiry: `expires_at` clamped to 7 days after `issued_at`; 7 days
/// without one.
pub fn pending_expiry(issued_at: i64, expires_at: Option<i64>) -> i64 {
    let cap = issued_at + PENDING_MAX_SECONDS;
    expires_at.map_or(cap, |e| e.min(cap))
}

/// The storage slot of a statement. OQ: Cancel or objection before its
/// statement.
pub fn statement_slot(kind: StatementType, d: &str, signer: &str, p: &str) -> String {
    match kind {
        StatementType::Cancel => format!("{d}|{signer}"),
        StatementType::Objection => format!("{d}|{p}"),
        _ => d.to_owned(),
    }
}

fn slot_of(st: &Statement) -> String {
    statement_slot(st.kind, &st.d, &st.signer, &st.p)
}

fn stored_for<'a>(s: &'a GuardianState, slot: &str) -> Option<&'a StoredStatement> {
    s.statements.iter().find(|st| st.slot == slot)
}

fn put_statement(s: &mut GuardianState, st: &Statement, now: i64, held: bool) {
    if st.kind == StatementType::Withdrawal {
        return;
    }
    let slot = slot_of(st);
    s.statements.retain(|x| x.slot != slot);
    s.statements.push(StoredStatement {
        slot,
        d: st.d.clone(),
        id: st.id.clone(),
        kind: st.kind,
        signer: st.signer.clone(),
        p: st.p.clone(),
        issued_at: st.issued_at,
        label: st.label.clone(),
        expires_at: st.expires_at,
        received_at: now,
        held,
    });
}

/// Rule 2 within one slot: re-delivery is `duplicate`, an older one `stale`,
/// and at the same `issuedAt` the lower event id wins (`tie_not_newer`).
/// OQ: Same d, same issuedAt, different content.
fn order_against(stored: Option<&StoredStatement>, st: &Statement) -> Option<Reason> {
    let stored = stored?;
    if stored.id == st.id {
        return Some(Reason::Duplicate);
    }
    if st.issued_at < stored.issued_at {
        return Some(Reason::Stale);
    }
    if st.issued_at == stored.issued_at && cmp_str(&st.id, &stored.id) == Ordering::Greater {
        return Some(Reason::TieNotNewer);
    }
    None
}

/// Drop every pending statement matching `pred`, and every presence token
/// bound to one of them.
fn drop_pending(s: &mut GuardianState, pred: impl Fn(&PendingRecord) -> bool) {
    let dropped: HashSet<String> = s
        .pending
        .iter()
        .filter(|p| pred(p))
        .map(|p| p.d.clone())
        .collect();
    if dropped.is_empty() {
        return;
    }
    s.pending.retain(|p| !dropped.contains(&p.d));
    s.tokens
        .retain(|t| t.statement_d.as_ref().is_none_or(|d| !dropped.contains(d)));
}

fn drop_pending_d(s: &mut GuardianState, d: &str) {
    drop_pending(s, |p| p.d == d);
}

fn upsert_departed_root(s: &mut GuardianState, root: &str, issued_at: i64, by: DepartedBy) {
    s.departed_roots.retain(|d| d.root != root);
    s.departed_roots.push(DepartedRoot {
        root: root.to_owned(),
        issued_at,
        by,
    });
}

/// Rule 4 floor: clamp the held clauses of keys departing at `at`.
/// OQ: Clause floor after a departure.
fn clamp_clauses(s: &mut GuardianState, keys: &[String], at: i64) {
    for c in &mut s.clauses {
        if keys.contains(&c.signer) {
            c.floor = Some(c.floor.unwrap_or(c.issued_at).min(c.issued_at).min(at));
        }
    }
}

/// The parts of a withdrawal recorded in its tombstone.
struct WithdrawalRef {
    d: String,
    id: String,
    signer: String,
    issued_at: i64,
}

fn withdraw(s: &mut GuardianState, idx: usize, w: &WithdrawalRef, at: i64) {
    let key = {
        let rec = &mut s.keys[idx];
        rec.status = KeyStatus::Withdrawn;
        rec.departed_by = Some(DepartedBy::Withdrawal);
        rec.rebind_root = None;
        rec.key.clone()
    };
    s.tombstones.push(Tombstone {
        kind: TombstoneType::Withdrawal,
        d: w.d.clone(),
        id: w.id.clone(),
        key: key.clone(),
        signer: w.signer.clone(),
        issued_at: w.issued_at,
    });
    s.held_withdrawals.retain(|h| h.key != key);
    clamp_clauses(s, &[key], at);
}

/// The parts of a removal needed to apply it.
struct RemovalRef {
    d: String,
    id: String,
    signer: String,
    key: String,
    issued_at: i64,
}

impl From<&PendingRecord> for RemovalRef {
    fn from(p: &PendingRecord) -> Self {
        RemovalRef {
            d: p.d.clone(),
            id: p.id.clone(),
            signer: p.signer.clone(),
            key: p.key.clone(),
            issued_at: p.issued_at,
        }
    }
}

/// Apply a root removal (rule 7) at time `at`. Refused (`false`) unless
/// another, distinct root remains: removal never leaves a ward with no roots.
fn apply_removal(s: &mut GuardianState, r: &RemovalRef, at: i64) -> bool {
    let r2 = r.key.as_str();
    if !is_pinned(s, r2) || !s.roots.iter().any(|x| x.root != r2) {
        return false;
    }
    s.roots.retain(|x| x.root != r2);
    upsert_departed_root(s, r2, r.issued_at, DepartedBy::Removal);
    s.tombstones.push(Tombstone {
        kind: TombstoneType::Removal,
        d: r.d.clone(),
        id: r.id.clone(),
        key: r2.to_owned(),
        signer: r.signer.clone(),
        issued_at: r.issued_at,
    });
    let mut gone = vec![r2.to_owned()];
    for k in &mut s.keys {
        if k.status == KeyStatus::Active && k.root == r2 {
            k.status = KeyStatus::Departed;
            k.departed_by = Some(DepartedBy::Removal);
            k.rebind_root = None;
            gone.push(k.key.clone());
        }
    }
    clamp_clauses(s, &gone, at);
    // "Once a removal applies, every pending statement signed by R2 is dropped."
    drop_pending(s, |p| p.signer == r2 || p.key == r2);
    s.held_withdrawals.retain(|h| h.signer != r2);
    true
}

/// Bind W to the approval's root. Early withdrawals for W can never apply
/// after this (rule 3). Returns the record's index.
fn bind_key(s: &mut GuardianState, approval: &Statement, now: i64) -> usize {
    let w = approval.key.clone();
    s.keys.retain(|k| k.key != w);
    s.keys.push(WorkingKeyRecord {
        key: w.clone(),
        root: approval.signer.clone(),
        status: KeyStatus::Active,
        label: approval.label.clone(),
        approval_id: approval.id.clone(),
        approval_issued_at: approval.issued_at,
        departed_by: None,
        rebind_root: None,
    });
    s.held_withdrawals.retain(|h| h.key != w);
    put_statement(s, approval, now, false);
    s.keys.len() - 1
}

/// An objection by `removed` against removals of it signed by `remover`: it
/// drops a removal whose `issuedAt` is at or below its own, and any removal
/// judged within 7 days of the objection reaching the ward.
/// OQ: Cancel or objection before its statement; OQ: Removal re-issued after
/// an objection.
fn objected_by(
    s: &GuardianState,
    d: &str,
    removed: &str,
    remover: &str,
    issued_at: i64,
    now: i64,
) -> bool {
    let slot = format!("{}{d}|{remover}", StatementType::Objection.prefix());
    stored_for(s, &slot).is_some_and(|o| {
        o.signer == removed
            && (o.issued_at >= issued_at || now < o.received_at + OBJECTION_BLOCK_SECONDS)
    })
}

/// A stored cancel for `d` from `signer` at or after `issued_at`.
/// OQ: Cancel or objection before its statement.
fn cancelled_by(s: &GuardianState, d: &str, signer: &str, issued_at: i64) -> bool {
    let slot = format!("{}{d}|{signer}", StatementType::Cancel.prefix());
    stored_for(s, &slot).is_some_and(|c| c.issued_at >= issued_at)
}

// ---------------------------------------------------------------------------
// Timers
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Due {
    Apply,
    Expire,
}

/// Advance timers to `now`, in time order (ties by `d`): a confirmed removal
/// takes effect at `takes_effect_at` if that is before its `expires_at`;
/// otherwise a pending statement expires at `expires_at`.
/// OQ: Removal that expires before it can take effect.
fn advance(s: &mut GuardianState, now: i64) -> Vec<TickEntry> {
    let mut out = Vec::new();
    s.tokens
        .retain(|t| now < t.minted_at + TOKEN_LIFETIME_SECONDS);
    s.previous_relays.retain(|p| now < p.until);
    loop {
        let mut best: Option<(i64, usize, Due)> = None;
        for (i, p) in s.pending.iter().enumerate() {
            let due = match p.takes_effect_at {
                Some(t) if t < p.expires_at && t <= now => Some((t, Due::Apply)),
                _ if p.expires_at <= now => Some((p.expires_at, Due::Expire)),
                _ => None,
            };
            if let Some((at, act)) = due {
                let better = match best {
                    None => true,
                    Some((bat, bi, _)) => {
                        at < bat || (at == bat && cmp_str(&p.d, &s.pending[bi].d) == Ordering::Less)
                    }
                };
                if better {
                    best = Some((at, i, act));
                }
            }
        }
        let Some((at, i, act)) = best else { break };
        let p = s.pending[i].clone();
        let entry = |result, reason| TickEntry {
            d: p.d.clone(),
            result,
            reason,
        };
        if act == Due::Expire {
            drop_pending_d(s, &p.d);
            out.push(entry(TickResult::Expired, Reason::Expired));
        } else if objected_by(s, &p.d, &p.key, &p.signer, p.issued_at, at) {
            drop_pending_d(s, &p.d);
            out.push(entry(TickResult::Refused, Reason::Objected));
        } else if apply_removal(s, &RemovalRef::from(&p), at) {
            out.push(entry(TickResult::Applied, Reason::RemovalApplied));
        } else {
            drop_pending_d(s, &p.d);
            out.push(entry(TickResult::Refused, Reason::LastRoot));
        }
    }
    out
}

type Judged = (Verdict, Reason);

fn drop(r: Reason) -> Judged {
    (Verdict::Dropped, r)
}

fn accept(r: Reason) -> Judged {
    (Verdict::Accepted, r)
}

/// Run `body` against a timer-advanced copy and wrap the result.
fn step(state: &GuardianState, now: i64, body: impl FnOnce(&mut GuardianState) -> Judged) -> Step {
    let before = signer_set(state);
    let recipients_before: HashSet<String> = recipients(state).into_iter().collect();
    let mut s = state.clone();
    let ticked = advance(&mut s, now);
    let (result, reason) = body(&mut s);
    let after: HashSet<String> = signer_set(&s).into_iter().collect();
    let departed: Vec<String> = before.into_iter().filter(|k| !after.contains(k)).collect();
    for k in &departed {
        if recipients_before.contains(k) {
            s.farewell.push(k.clone());
        }
    }
    normalise(&mut s);
    Step {
        state: s,
        outcome: Outcome {
            result,
            reason,
            ticked,
            departed,
        },
    }
}

/// Advance pending timers to `now` (expiry, the 72-hour removal notice, token
/// lifetime, the previous-relays window).
pub fn tick(state: &GuardianState, now: i64) -> Step {
    step(state, now, |_| accept(Reason::Ticked))
}

// ---------------------------------------------------------------------------
// Statements
// ---------------------------------------------------------------------------

/// Who may seal a statement.
/// - An approval only by the W it names (exception 1).
/// - A withdrawal, co-guardian, removal, move or cancel signed by R by R, or
///   by a W bound to R and in the signer set (exception 2).
/// - An objection only by its signer. OQ: Sealing an objection.
fn seal_allowed(s: &GuardianState, st: &Statement, seal_author: &str) -> bool {
    if st.kind == StatementType::Approval {
        return seal_author == st.key;
    }
    if seal_author == st.signer {
        return true;
    }
    if st.kind == StatementType::Objection {
        return false;
    }
    active_key(s, seal_author).is_some_and(|w| w.root == st.signer)
}

fn process_approval(s: &mut GuardianState, st: &Statement, now: i64) -> Judged {
    // OQ: expiresAt on non-pending statements.
    if st.expires_at.is_some() {
        return drop(Reason::ExpiresAtNotAllowed);
    }
    let w = st.key.as_str();
    let r = st.signer.as_str();
    if is_pinned(s, w) || w == r {
        return drop(Reason::KeyRoleConflict);
    }
    // Rule 4: a removed or moved-away root never returns as a working key.
    if is_departed_root(s, w) {
        return drop(Reason::KeyDeparted);
    }
    #[derive(PartialEq)]
    enum Mode {
        New,
        Update,
        Rebind,
    }
    let idx = key_index(s, w);
    let mut mode = Mode::New;
    if let Some(i) = idx {
        let rec = &s.keys[i];
        match rec.status {
            KeyStatus::Withdrawn => return drop(Reason::KeyDeparted),
            KeyStatus::Departed => {
                // OQ: Working keys after a root move or removal — only the
                // root that replaced the key's root by a confirmed move may
                // re-bind it.
                if rec.departed_by == Some(DepartedBy::Move)
                    && rec.rebind_root.as_deref() == Some(r)
                {
                    mode = Mode::Rebind;
                } else {
                    return drop(Reason::KeyDeparted);
                }
            }
            KeyStatus::Active if rec.root != r => return drop(Reason::BoundElsewhere),
            KeyStatus::Active => mode = Mode::Update,
        }
    }
    if let Some(reason) = order_against(stored_for(s, &st.d), st) {
        return drop(reason);
    }
    if mode != Mode::Update {
        if root_at_quota(s, r) {
            return drop(Reason::RootAtQuota);
        }
        if active_count(s, r) >= MAX_ACTIVE_KEYS_PER_ROOT {
            return drop(Reason::RootKeyQuota);
        }
    }
    if mode == Mode::Update {
        if let Some(i) = idx {
            let rec = &mut s.keys[i];
            rec.label = st.label.clone();
            rec.approval_id = st.id.clone();
            rec.approval_issued_at = st.issued_at;
            put_statement(s, st, now, false);
            return accept(Reason::ApprovalUpdated);
        }
    }
    let held = s
        .held_withdrawals
        .iter()
        .find(|h| h.key == w && h.signer == r)
        .map(|h| WithdrawalRef {
            d: h.d.clone(),
            id: h.id.clone(),
            signer: h.signer.clone(),
            issued_at: h.issued_at,
        });
    let bound = bind_key(s, st, now);
    if let Some(h) = held {
        withdraw(s, bound, &h, now);
        return accept(Reason::ApprovalPreempted);
    }
    accept(if mode == Mode::Rebind {
        Reason::ApprovalRebound
    } else {
        Reason::ApprovalApplied
    })
}

fn process_withdrawal(s: &mut GuardianState, st: &Statement, now: i64) -> Judged {
    let w = st.key.as_str();
    let signer = st.signer.as_str();
    if is_pinned(s, w) {
        return drop(Reason::KeyRoleConflict);
    }
    let wref = WithdrawalRef {
        d: st.d.clone(),
        id: st.id.clone(),
        signer: st.signer.clone(),
        issued_at: st.issued_at,
    };
    if let Some(i) = key_index(s, w) {
        let rec = &s.keys[i];
        return match rec.status {
            KeyStatus::Withdrawn => drop(Reason::AlreadyWithdrawn),
            KeyStatus::Active if rec.root != signer => drop(Reason::NotBindingRoot),
            KeyStatus::Active => {
                withdraw(s, i, &wref, now);
                accept(Reason::WithdrawalApplied)
            }
            // Departed: its old root (re-pinned since) or the root that may
            // re-bind it can still make the departure permanent.
            // OQ: Working keys after a root move or removal.
            KeyStatus::Departed => {
                if rec.root == signer
                    || (rec.departed_by == Some(DepartedBy::Move)
                        && rec.rebind_root.as_deref() == Some(signer))
                {
                    withdraw(s, i, &wref, now);
                    accept(Reason::WithdrawalApplied)
                } else {
                    drop(Reason::NotBindingRoot)
                }
            }
        };
    }
    // Rule 3: not yet bound — keep it, since a pinned root signed it.
    if s.held_withdrawals
        .iter()
        .any(|h| h.key == w && h.signer == signer)
    {
        return drop(Reason::Duplicate);
    }
    if held_count(s, signer) >= MAX_HELD_PER_ROOT {
        return drop(Reason::HeldQuota);
    }
    s.held_withdrawals.push(HeldWithdrawal {
        d: st.d.clone(),
        id: st.id.clone(),
        signer: st.signer.clone(),
        key: st.key.clone(),
        issued_at: st.issued_at,
    });
    accept(Reason::WithdrawalHeld)
}

fn add_pending(
    s: &mut GuardianState,
    st: &Statement,
    kind: PendingType,
    expires_at: i64,
    now: i64,
) {
    drop_pending_d(s, &st.d);
    s.pending.push(PendingRecord {
        d: st.d.clone(),
        kind,
        id: st.id.clone(),
        signer: st.signer.clone(),
        key: st.key.clone(),
        issued_at: st.issued_at,
        received_at: now,
        expires_at,
        confirmed_at: None,
        takes_effect_at: None,
    });
    put_statement(s, st, now, false);
}

/// Rule 7: re-pinning a departed root needs a statement strictly newer than
/// its departure. A same-second removal wins. OQ: Same-second ties across
/// different d.
fn departed_at_or_after(s: &GuardianState, key: &str, issued_at: i64) -> bool {
    s.departed_roots
        .iter()
        .find(|d| d.root == key)
        .is_some_and(|d| issued_at <= d.issued_at)
}

fn process_coguardian_or_move(s: &mut GuardianState, st: &Statement, now: i64) -> Judged {
    let r2 = st.key.as_str();
    if is_pinned(s, r2) {
        return drop(Reason::AlreadyPinned);
    }
    if key_record(s, r2).is_some() {
        return drop(Reason::KeyRoleConflict);
    }
    if departed_at_or_after(s, r2, st.issued_at) {
        return drop(Reason::NotNewerThanDeparture);
    }
    if let Some(reason) = order_against(stored_for(s, &st.d), st) {
        return drop(reason);
    }
    let expires_at = pending_expiry(st.issued_at, st.expires_at);
    if now >= expires_at {
        return drop(Reason::Expired);
    }
    if cancelled_by(s, &st.d, &st.signer, st.issued_at) {
        return drop(Reason::Cancelled);
    }
    let is_cog = st.kind == StatementType::Coguardian;
    if is_cog && s.roots.len() >= MAX_ROOTS {
        return drop(Reason::RootQuota);
    }
    let pt = if is_cog {
        PendingType::Coguardian
    } else {
        PendingType::Move
    };
    add_pending(s, st, pt, expires_at, now);
    accept(if is_cog {
        Reason::CoguardianPending
    } else {
        Reason::MovePending
    })
}

fn process_removal(s: &mut GuardianState, st: &Statement, now: i64) -> Judged {
    let r2 = st.key.as_str();
    if !is_pinned(s, r2) {
        return drop(Reason::TargetNotPinned);
    }
    if let Some(reason) = order_against(stored_for(s, &st.d), st) {
        return drop(reason);
    }
    if st.signer == r2 {
        if !s.roots.iter().any(|x| x.root != r2) {
            return drop(Reason::LastRoot);
        }
        put_statement(s, st, now, false);
        let r = RemovalRef {
            d: st.d.clone(),
            id: st.id.clone(),
            signer: st.signer.clone(),
            key: st.key.clone(),
            issued_at: st.issued_at,
        };
        apply_removal(s, &r, now);
        return accept(Reason::RemovalApplied);
    }
    let expires_at = pending_expiry(st.issued_at, st.expires_at);
    if now >= expires_at {
        return drop(Reason::Expired);
    }
    if objected_by(s, &st.d, r2, &st.signer, st.issued_at, now) {
        return drop(Reason::Objected);
    }
    if cancelled_by(s, &st.d, &st.signer, st.issued_at) {
        return drop(Reason::Cancelled);
    }
    add_pending(s, st, PendingType::Removal, expires_at, now);
    accept(Reason::RemovalPending)
}

fn hold_or_drop(s: &mut GuardianState, st: &Statement, now: i64, reason: Reason) -> Judged {
    let reclaim = stored_for(s, &slot_of(st))
        .is_some_and(|prev| prev.held && prev.signer == st.signer) as usize;
    if held_count(s, &st.signer) - reclaim >= MAX_HELD_PER_ROOT {
        return drop(Reason::HeldQuota);
    }
    put_statement(s, st, now, true);
    accept(reason)
}

fn process_cancel(s: &mut GuardianState, st: &Statement, now: i64) -> Judged {
    if let Some(reason) = order_against(stored_for(s, &slot_of(st)), st) {
        return drop(reason);
    }
    let target = st.target.as_deref().unwrap_or_default();
    if let Some(pend) = s.pending.iter().find(|p| p.d == target) {
        if pend.signer != st.signer {
            return drop(Reason::CancelSignerMismatch);
        }
        // At or below: a same-second cancel wins. OQ: Same-second ties
        // across different d.
        if pend.issued_at <= st.issued_at {
            drop_pending_d(s, target);
            put_statement(s, st, now, false);
            return accept(Reason::CancelApplied);
        }
    }
    hold_or_drop(s, st, now, Reason::CancelHeld)
}

fn process_objection(s: &mut GuardianState, st: &Statement, now: i64) -> Judged {
    if st.signer != st.key {
        return drop(Reason::ObjectionMismatch);
    }
    if let Some(reason) = order_against(stored_for(s, &slot_of(st)), st) {
        return drop(reason);
    }
    let target = st.target.as_deref().unwrap_or_default();
    // Rule 7: an objection drops the pending removal by its `p`, whatever
    // the two issuedAt values.
    if s.pending.iter().any(|p| p.d == target && p.signer == st.p) {
        drop_pending_d(s, target);
        put_statement(s, st, now, false);
        return accept(Reason::ObjectionApplied);
    }
    hold_or_drop(s, st, now, Reason::ObjectionHeld)
}

/// Apply one gift-wrapped statement (kind 31120) at the ward.
///
/// `seal_author` is the authenticated seal author from the NIP-59 unwrap;
/// `inner` is the signed statement as JSON. Checks run in this order:
/// statement validity, the 300-second future refusal, the seal rule, the
/// signer being a pinned root, then the type's own checks.
///
/// Presence confirmations (kind 31121) go to [`apply_presence_confirm`].
pub fn apply_inbound(state: &GuardianState, seal_author: &str, inner: &Value, now: i64) -> Step {
    step(state, now, |s| {
        let st = match parse_statement(inner) {
            Ok(st) => st,
            Err(reason) => return drop(reason),
        };
        if st.issued_at > now + FUTURE_SKEW_SECONDS {
            return drop(Reason::Future);
        }
        if !seal_allowed(s, &st, seal_author) {
            return drop(Reason::SealMismatch);
        }
        if !is_pinned(s, &st.signer) {
            return drop(Reason::SignerNotPinned);
        }
        match st.kind {
            StatementType::Approval => process_approval(s, &st, now),
            StatementType::Withdrawal => process_withdrawal(s, &st, now),
            StatementType::Coguardian | StatementType::Move => {
                process_coguardian_or_move(s, &st, now)
            }
            StatementType::Removal => process_removal(s, &st, now),
            StatementType::Cancel => process_cancel(s, &st, now),
            StatementType::Objection => process_objection(s, &st, now),
        }
    })
}

// ---------------------------------------------------------------------------
// Presence confirmation
// ---------------------------------------------------------------------------

/// Record a presence token the ward minted on a privileged request (polkit on
/// Linux, the guardian PIN on Android). `statement_d` binds the token to one
/// pending statement; `None` lets it confirm any.
/// OQ: Presence confirmation details.
pub fn register_presence_token(
    state: &GuardianState,
    token: &str,
    statement_d: Option<&str>,
    now: i64,
) -> Step {
    step(state, now, |s| {
        if !is_hex32(token) || s.tokens.iter().any(|t| t.token == token) {
            return drop(Reason::TokenInvalid);
        }
        if let Some(d) = statement_d {
            if !s.pending.iter().any(|p| p.d == d) {
                return drop(Reason::NotPending);
            }
        }
        s.tokens.push(PresenceToken {
            token: token.to_owned(),
            statement_d: statement_d.map(str::to_owned),
            minted_at: now,
        });
        accept(Reason::TokenRegistered)
    })
}

struct ConfirmContent {
    author: String,
    token: String,
    statement: String,
    /// `None` when absent; `Some(Value::Null)` when present as `null`.
    approval: Option<Value>,
}

fn parse_confirm(inner: &Value) -> Option<ConfirmContent> {
    let ev = verified_event(inner)?;
    if ev.kind != i64::from(CHARTER_PRESENCE_CONFIRM) || !has_marker(&ev) {
        return None;
    }
    let c = parse_strict_json(&ev.content).ok()?;
    let mut o = match c {
        Value::Object(o) => o,
        _ => return None,
    };
    if !o.get("v").is_some_and(is_one) {
        return None;
    }
    let token = o.get("token")?.as_str().filter(|t| is_hex32(t))?.to_owned();
    let statement = o
        .get("statement")?
        .as_str()
        .filter(|t| is_hex64(t))?
        .to_owned();
    Some(ConfirmContent {
        author: ev.pubkey,
        token,
        statement,
        approval: o.remove("approval"),
    })
}

/// Apply a presence confirmation (kind 31121), signed and sealed by the
/// confirming working key.
pub fn apply_presence_confirm(
    state: &GuardianState,
    seal_author: &str,
    inner: &Value,
    now: i64,
) -> Step {
    step(state, now, |s| {
        let Some(c) = parse_confirm(inner) else {
            return drop(Reason::BadConfirm);
        };
        let w = c.author.as_str();
        if seal_author != w {
            return drop(Reason::SealMismatch);
        }
        let Some(pi) = s.pending.iter().position(|p| p.id == c.statement) else {
            return drop(Reason::NotPending);
        };
        let pend = s.pending[pi].clone();
        let Some(tok) = s
            .tokens
            .iter()
            .find(|t| {
                constant_time_eq(&t.token, &c.token)
                    && t.statement_d.as_ref().is_none_or(|d| *d == pend.d)
            })
            .map(|t| t.token.clone())
        else {
            return drop(Reason::TokenInvalid);
        };
        let spend = |s: &mut GuardianState| s.tokens.retain(|t| t.token != tok);

        if pend.kind == PendingType::Removal {
            if pend.confirmed_at.is_some() {
                return drop(Reason::Duplicate);
            }
            let bound_to_signer = active_key(s, w).is_some_and(|rec| rec.root == pend.signer);
            if !bound_to_signer {
                return drop(Reason::ConfirmerNotBound);
            }
            if objected_by(s, &pend.d, &pend.key, &pend.signer, pend.issued_at, now) {
                drop_pending_d(s, &pend.d);
                return drop(Reason::Objected);
            }
            spend(s);
            // Notice runs from the later of issuedAt and the ward's own
            // receipt. OQ: Removal notice measured from receipt.
            let takes_effect_at =
                now.max(pend.issued_at.max(pend.received_at) + REMOVAL_NOTICE_SECONDS);
            {
                let p = &mut s.pending[pi];
                p.confirmed_at = Some(now);
                p.takes_effect_at = Some(takes_effect_at);
            }
            if takes_effect_at <= now {
                if !apply_removal(s, &RemovalRef::from(&pend), now) {
                    drop_pending_d(s, &pend.d);
                    return drop(Reason::LastRoot);
                }
                return accept(Reason::RemovalApplied);
            }
            return accept(Reason::RemovalConfirmed);
        }

        // Co-guardian or move: the confirming W carries R2's approval of
        // itself. R2's role is checked again: it may have changed since the
        // statement arrived.
        let r2 = pend.key.as_str();
        let Some(approval) = c.approval.as_ref() else {
            return drop(Reason::ApprovalRequired);
        };
        let Ok(ap) = parse_statement(approval) else {
            return drop(Reason::BadApproval);
        };
        if ap.kind != StatementType::Approval || ap.signer != r2 || ap.key != w {
            return drop(Reason::BadApproval);
        }
        if ap.expires_at.is_some() {
            return drop(Reason::ExpiresAtNotAllowed);
        }
        if ap.issued_at > now + FUTURE_SKEW_SECONDS {
            return drop(Reason::Future);
        }
        if is_pinned(s, r2) {
            return drop(Reason::AlreadyPinned);
        }
        if key_record(s, r2).is_some() || is_pinned(s, w) || w == r2 {
            return drop(Reason::KeyRoleConflict);
        }
        if departed_at_or_after(s, r2, pend.issued_at) {
            return drop(Reason::NotNewerThanDeparture);
        }
        if is_departed_root(s, w) {
            return drop(Reason::KeyDeparted);
        }
        if let Some(rec) = key_record(s, w) {
            if rec.status != KeyStatus::Active {
                return drop(Reason::KeyDeparted);
            }
            if pend.kind == PendingType::Coguardian || rec.root != pend.signer {
                return drop(Reason::BoundElsewhere);
            }
        }
        if let Some(reason) = order_against(stored_for(s, &ap.d), &ap) {
            return drop(reason);
        }
        if root_at_quota(s, r2) {
            return drop(Reason::RootAtQuota);
        }
        if pend.kind == PendingType::Coguardian && s.roots.len() >= MAX_ROOTS {
            return drop(Reason::RootQuota);
        }
        spend(s);
        // Whichever statement pins R2 first, every other pending one naming
        // R2 goes.
        drop_pending(s, |p| p.key == r2);

        if pend.kind == PendingType::Coguardian {
            s.roots.push(PinnedRoot {
                root: r2.to_owned(),
                pinned_at: now,
                via: PinnedVia::Coguardian,
            });
            bind_key(s, &ap, now);
            return accept(Reason::CoguardianApplied);
        }

        // Rule 8: R2 replaces R. The confirming W re-binds to R2; R's other
        // working keys depart until R2 approves them. Pending and held
        // statements signed by R are dropped. OQ: Working keys after a root
        // move or removal.
        let r = pend.signer.as_str();
        let new_root = PinnedRoot {
            root: r2.to_owned(),
            pinned_at: now,
            via: PinnedVia::Move,
        };
        // R is always pinned here: a removal or move of R drops every pending
        // statement R signed.
        match s.roots.iter().position(|x| x.root == r) {
            Some(idx) => s.roots[idx] = new_root,
            None => s.roots.push(new_root),
        }
        upsert_departed_root(s, r, pend.issued_at, DepartedBy::Move);
        let mut gone = vec![r.to_owned()];
        for k in &mut s.keys {
            if k.status == KeyStatus::Active && k.root == r && k.key != w {
                k.status = KeyStatus::Departed;
                k.departed_by = Some(DepartedBy::Move);
                k.rebind_root = Some(r2.to_owned());
                gone.push(k.key.clone());
            }
        }
        clamp_clauses(s, &gone, now);
        bind_key(s, &ap, now);
        drop_pending(s, |p| p.signer == r || p.key == r);
        s.held_withdrawals.retain(|h| h.signer != r);
        accept(Reason::MoveApplied)
    })
}

// ---------------------------------------------------------------------------
// Clauses and other signed events
// ---------------------------------------------------------------------------

/// A CLAUSE's signer, seal and identity; the clause body is the caller's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClauseInput<'a> {
    pub author: &'a str,
    pub seal_author: &'a str,
    pub subject: &'a str,
    pub kind: &'a str,
    pub issued_at: i64,
}

/// Rules 4 and 5: accept a CLAUSE if its author is in the signer set and it
/// is newer than the held clause of the same `(subject, kind)`: newer than
/// its `issuedAt` while its signer is current, newer than its departure floor
/// once the signer has departed. Checks: future, seal, signer, pairing
/// floor, newer.
pub fn accept_clause(state: &GuardianState, clause: &ClauseInput<'_>, now: i64) -> Step {
    step(state, now, |s| {
        if clause.issued_at > now + FUTURE_SKEW_SECONDS {
            return drop(Reason::Future);
        }
        if clause.seal_author != clause.author {
            return drop(Reason::SealMismatch);
        }
        if !is_signer(s, clause.author) {
            return drop(Reason::NotSigner);
        }
        if clause.issued_at < s.paired_at {
            return drop(Reason::PairingFloor);
        }
        let same = |c: &HeldClause| c.subject == clause.subject && c.kind == clause.kind;
        if let Some(held) = s.clauses.iter().find(|c| same(c)) {
            let bar = if is_signer(s, &held.signer) {
                held.issued_at
            } else {
                held.floor.unwrap_or(held.issued_at)
            };
            if clause.issued_at <= bar {
                return drop(Reason::NotNewer);
            }
        }
        let root = root_of(s, clause.author).unwrap_or_default();
        s.clauses.retain(|c| !same(c));
        s.clauses.push(HeldClause {
            subject: clause.subject.to_owned(),
            kind: clause.kind.to_owned(),
            signer: clause.author.to_owned(),
            root,
            issued_at: clause.issued_at,
            floor: None,
        });
        accept(Reason::ClauseAccepted)
    })
}

/// The clause a ward enforces for `(subject, kind)`, with who set it.
pub fn held_clause<'a>(
    state: &'a GuardianState,
    subject: &str,
    kind: &str,
) -> Option<&'a HeldClause> {
    state
        .clauses
        .iter()
        .find(|c| c.subject == subject && c.kind == kind)
}

/// The signer-set gate for a GRANT or USAGE_SYNC: the seal author is the
/// inner author, the author is in the signer set, and `ts` is not more than
/// 300 seconds ahead. Their own existing checks still apply on top.
pub fn accept_signed_event(
    state: &GuardianState,
    author: &str,
    seal_author: &str,
    ts: i64,
    now: i64,
) -> Step {
    step(state, now, |s| {
        if ts > now + FUTURE_SKEW_SECONDS {
            return drop(Reason::Future);
        }
        if seal_author != author {
            return drop(Reason::SealMismatch);
        }
        if !is_signer(s, author) {
            return drop(Reason::NotSigner);
        }
        accept(Reason::SignedAccepted)
    })
}

/// How a RELAYS change reached the ward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaysChannel<'a> {
    /// Gift-wrapped over relays or the local link, with the seal's author.
    Wrapped { seal_author: &'a str },
    /// The offline relay code typed or pasted at the ward (no seal).
    Pasted,
}

/// A RELAYS change, already validated as a relay list (2–10 distinct secure
/// endpoints) by the ward's existing check.
#[derive(Debug, Clone, PartialEq)]
pub struct RelaysInput<'a> {
    pub author: &'a str,
    pub channel: RelaysChannel<'a>,
    /// The event's `created_at`.
    pub ts: i64,
    pub relays: &'a [String],
    /// The body's `guardianPin` member: `None` when absent, the raw JSON
    /// (`null` included) when present.
    pub guardian_pin: Option<&'a Value>,
}

/// A RELAYS change from any signer (rule 11), with the existing RELAYS
/// freshness checks: not in the future, at most 600 s old, not before
/// pairing, newer than the last applied. A `guardianPin` is accepted only
/// over the wrapped channel. OQ: How a guardian PIN reaches an Android ward.
pub fn apply_relays(state: &GuardianState, ev: &RelaysInput<'_>, now: i64) -> Step {
    step(state, now, |s| {
        if let RelaysChannel::Wrapped { seal_author } = ev.channel {
            if seal_author != ev.author {
                return drop(Reason::SealMismatch);
            }
        }
        if !is_signer(s, ev.author) {
            return drop(Reason::NotSigner);
        }
        if ev.ts > now {
            return drop(Reason::Future);
        }
        if now - ev.ts > RELAYS_TTL_SECONDS {
            return drop(Reason::Stale);
        }
        if ev.ts < s.paired_at {
            return drop(Reason::PairingFloor);
        }
        if ev.ts <= s.relays_updated_at {
            return drop(Reason::NotNewer);
        }
        let mut pin = None;
        if let Some(raw) = ev.guardian_pin {
            if ev.channel == RelaysChannel::Pasted {
                return drop(Reason::PinNotWrapped);
            }
            match validate_pin_record(raw) {
                Some(rec) => pin = Some(rec),
                None => return drop(Reason::BadPinRecord),
            }
        }
        let old = std::mem::replace(&mut s.relays, ev.relays.to_vec());
        s.previous_relays.push(PreviousRelays {
            relays: old,
            until: now + PREVIOUS_RELAYS_SECONDS,
        });
        if s.previous_relays.len() > MAX_PREVIOUS_RELAYS {
            let excess = s.previous_relays.len() - MAX_PREVIOUS_RELAYS;
            s.previous_relays.drain(..excess);
        }
        s.relays_updated_at = ev.ts;
        if pin.is_some() {
            s.guardian_pin = pin;
        }
        accept(Reason::RelaysApplied)
    })
}

/// The ward has sent STATUS: departed keys have had their last one.
pub fn mark_status_sent(state: &GuardianState, now: i64) -> Step {
    step(state, now, |s| {
        s.farewell.clear();
        accept(Reason::StatusSent)
    })
}

// ---------------------------------------------------------------------------
// Scan-to-pair with a working key
// ---------------------------------------------------------------------------

/// The token the ward minted for scan-to-pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairToken<'a> {
    pub token: &'a str,
    pub minted_at: i64,
}

/// A successful pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paired {
    pub state: GuardianState,
    /// The pinned root.
    pub root: String,
    /// The bound working key, when the offer carried an approval.
    pub working_key: Option<String>,
}

fn offer_content(ev: &Event) -> Option<serde_json::Map<String, Value>> {
    let c = parse_strict_json(&ev.content).ok()?;
    let o = match c {
        Value::Object(o) => o,
        _ => return None,
    };
    o.get("token")?.as_str().filter(|t| is_hex32(t))?;
    if !o.get("relays")?.as_array()?.iter().all(Value::is_string) {
        return None;
    }
    unix_seconds(o.get("ts")?)?;
    Some(o)
}

/// Pair an unpaired ward from a PAIR_OFFER (kind 31117).
///
/// - An offer carrying `approval` must be sealed by the W the approval names,
///   and the approval signed by R: the ward pins R and binds W.
/// - An offer with no approval pins its seal author as the root (a ward
///   cannot tell a working key from a root by its key).
/// - Nothing is ever pinned from a key read out of the payload.
///
/// Token rules are those of scan-to-pair: constant-time compare, 600-second
/// lifetime against the mint time and the offer's `ts`, a future `ts`
/// refused. A contested token (two keys, one token) is the caller's to
/// detect.
pub fn pair_from_offer(
    seal_author: &str,
    inner: &Value,
    token: &PairToken<'_>,
    now: i64,
) -> Result<Paired, Reason> {
    let ev = verified_event(inner).ok_or(Reason::BadOffer)?;
    if ev.kind != i64::from(CHARTER_DEVICE_PAIR_OFFER) || !has_marker(&ev) {
        return Err(Reason::BadOffer);
    }
    if seal_author != ev.pubkey {
        return Err(Reason::SealMismatch);
    }
    let o = offer_content(&ev).ok_or(Reason::BadOffer)?;
    let offer_token = o.get("token").and_then(Value::as_str).unwrap_or_default();
    let ts = o.get("ts").and_then(unix_seconds).unwrap_or_default();
    if !constant_time_eq(token.token, offer_token)
        || now >= token.minted_at + TOKEN_LIFETIME_SECONDS
        || ts > now
        || now - ts >= TOKEN_LIFETIME_SECONDS
    {
        return Err(Reason::TokenInvalid);
    }
    let relays: Vec<String> = o
        .get("relays")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|r| r.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    pair_from_seal(seal_author, o.get("approval"), relays, now)
}

/// The pinning decision of [`pair_from_offer`], once the offer itself (its
/// signature, kind, marker, seal and token) has been checked: an offer with
/// no `approval` pins its seal author; one with an approval pins the root
/// that signed it, which must name the seal author, and binds that working
/// key. `approval` is the offer's `approval` member as delivered (`null`
/// included). For a ward whose PAIR_OFFER inner event is not itself signed
/// (the seal authenticates it), the caller checks the token and calls this.
pub fn pair_from_seal(
    seal_author: &str,
    approval: Option<&Value>,
    relays: Vec<String>,
    now: i64,
) -> Result<Paired, Reason> {
    let mut state = GuardianState {
        v: 1,
        paired_at: now,
        roots: Vec::new(),
        keys: Vec::new(),
        statements: Vec::new(),
        pending: Vec::new(),
        held_withdrawals: Vec::new(),
        tombstones: Vec::new(),
        departed_roots: Vec::new(),
        tokens: Vec::new(),
        clauses: Vec::new(),
        relays,
        relays_updated_at: 0,
        previous_relays: Vec::new(),
        guardian_pin: None,
        farewell: Vec::new(),
    };

    let Some(approval) = approval else {
        state.roots.push(PinnedRoot {
            root: seal_author.to_owned(),
            pinned_at: now,
            via: PinnedVia::Pair,
        });
        return Ok(Paired {
            state,
            root: seal_author.to_owned(),
            working_key: None,
        });
    };
    let ap = parse_statement(approval)
        .ok()
        .filter(|ap| ap.kind == StatementType::Approval)
        .ok_or(Reason::BadApproval)?;
    if ap.key != seal_author {
        return Err(Reason::SealMismatch);
    }
    if ap.signer == ap.key {
        return Err(Reason::KeyRoleConflict);
    }
    if ap.expires_at.is_some() {
        return Err(Reason::ExpiresAtNotAllowed);
    }
    if ap.issued_at > now + FUTURE_SKEW_SECONDS {
        return Err(Reason::Future);
    }
    state.roots.push(PinnedRoot {
        root: ap.signer.clone(),
        pinned_at: now,
        via: PinnedVia::Pair,
    });
    bind_key(&mut state, &ap, now);
    normalise(&mut state);
    Ok(Paired {
        state,
        root: ap.signer,
        working_key: Some(ap.key),
    })
}

// ---------------------------------------------------------------------------
// Projections
// ---------------------------------------------------------------------------

/// One pinned root and its active working keys (sorted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootKeys {
    pub root: String,
    pub keys: Vec<String>,
}

/// A pending statement as STATUS reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPending {
    pub d: String,
    pub kind: PendingType,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub takes_effect_at: Option<i64>,
    pub expires_at: i64,
}

/// The `guardians` object a ward adds to STATUS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusGuardians {
    pub roots: Vec<RootKeys>,
    pub pending: Vec<StatusPending>,
    pub tombstones: usize,
    pub relays: Vec<String>,
}

fn root_keys(s: &GuardianState) -> Vec<RootKeys> {
    s.roots
        .iter()
        .map(|r| {
            let mut keys: Vec<String> = s
                .keys
                .iter()
                .filter(|k| k.status == KeyStatus::Active && k.root == r.root)
                .map(|k| k.key.clone())
                .collect();
            keys.sort_by(|a, b| cmp_str(a, b));
            RootKeys {
                root: r.root.clone(),
                keys,
            }
        })
        .collect()
}

/// The STATUS `guardians` projection.
pub fn status_guardians(s: &GuardianState) -> StatusGuardians {
    StatusGuardians {
        roots: root_keys(s),
        pending: s
            .pending
            .iter()
            .map(|p| StatusPending {
                d: p.d.clone(),
                kind: p.kind,
                takes_effect_at: p.takes_effect_at,
                expires_at: p.expires_at,
            })
            .collect(),
        tombstones: s.tombstones.len(),
        relays: s.relays.clone(),
    }
}

/// A signer and the root it speaks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignerRoot {
    pub key: String,
    pub root: String,
}

/// A pending statement as the golden vectors project it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedPending {
    pub d: String,
    pub kind: PendingType,
    pub signer: String,
    pub statement: String,
    pub issued_at: i64,
    pub received_at: i64,
    pub expires_at: i64,
    pub confirmed_at: Option<i64>,
    pub takes_effect_at: Option<i64>,
}

/// The projected state recorded after every step of the golden scenarios.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedState {
    pub roots: Vec<RootKeys>,
    pub signers: Vec<SignerRoot>,
    pub pending: Vec<ProjectedPending>,
    pub tombstones: usize,
    pub held: usize,
    pub departed: Vec<String>,
    pub recipients: Vec<String>,
    pub farewell: Vec<String>,
    pub clauses: Vec<HeldClause>,
    pub relays: Vec<String>,
    pub status_relays: Vec<String>,
    pub guardian_pin: Option<GuardianPinRecord>,
}

/// Project the state as the golden vectors record it, at `now`.
pub fn project_state(s: &GuardianState, now: i64) -> ProjectedState {
    let departed = sorted_unique(
        s.keys
            .iter()
            .map(|k| &k.key)
            .chain(s.departed_roots.iter().map(|d| &d.root))
            .filter(|k| is_departed(s, k))
            .cloned(),
    );
    ProjectedState {
        roots: root_keys(s),
        signers: signer_set(s)
            .into_iter()
            .map(|key| SignerRoot {
                root: root_of(s, &key).unwrap_or_default(),
                key,
            })
            .collect(),
        pending: s
            .pending
            .iter()
            .map(|p| ProjectedPending {
                d: p.d.clone(),
                kind: p.kind,
                signer: p.signer.clone(),
                statement: p.id.clone(),
                issued_at: p.issued_at,
                received_at: p.received_at,
                expires_at: p.expires_at,
                confirmed_at: p.confirmed_at,
                takes_effect_at: p.takes_effect_at,
            })
            .collect(),
        tombstones: s.tombstones.len(),
        held: s.held_withdrawals.len() + s.statements.iter().filter(|x| x.held).count(),
        departed,
        recipients: recipients(s),
        farewell: s.farewell.clone(),
        clauses: s.clauses.clone(),
        relays: s.relays.clone(),
        status_relays: status_relays(s, now),
        guardian_pin: s.guardian_pin.clone(),
    }
}
