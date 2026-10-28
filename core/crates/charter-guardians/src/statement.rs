//! Guardian statements (kind 31120): parsing and validation on their own
//! terms, before any ward state is consulted.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::event::{
    event_shape, has_marker, is_hex64, is_one, tag_values, unix_seconds, verify_event,
};
use crate::json::parse_strict_json;
use crate::reason::Reason;
use crate::{CHARTER_GUARDIAN_STATEMENT, MAX_LABEL_BYTES};

/// The seven statement types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatementType {
    Approval,
    Withdrawal,
    Coguardian,
    Removal,
    Move,
    Cancel,
    Objection,
}

/// Statement types that can be pending (and so cancelled).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PendingType {
    Coguardian,
    Removal,
    Move,
}

impl StatementType {
    /// The `d` prefix of this type.
    pub fn prefix(self) -> &'static str {
        match self {
            StatementType::Approval => "kintrinsic/device/",
            StatementType::Withdrawal => "kintrinsic/revoke/",
            StatementType::Coguardian => "kintrinsic/guardian/",
            StatementType::Removal => "kintrinsic/unguardian/",
            StatementType::Move => "kintrinsic/root/",
            StatementType::Cancel => "kintrinsic/cancel/",
            StatementType::Objection => "kintrinsic/object/",
        }
    }

    const ALL: [StatementType; 7] = [
        StatementType::Approval,
        StatementType::Withdrawal,
        StatementType::Coguardian,
        StatementType::Removal,
        StatementType::Move,
        StatementType::Cancel,
        StatementType::Objection,
    ];

    fn pending(self) -> Option<PendingType> {
        match self {
            StatementType::Coguardian => Some(PendingType::Coguardian),
            StatementType::Removal => Some(PendingType::Removal),
            StatementType::Move => Some(PendingType::Move),
            _ => None,
        }
    }
}

/// A statement that passed [`parse_statement`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    pub kind: StatementType,
    /// The event's single `d` tag.
    pub d: String,
    /// The event's single `p` tag.
    pub p: String,
    /// The key the statement is about: W for an approval or withdrawal, R2
    /// for a co-guardian, removal or move, the pending statement's R2 for a
    /// cancel, the root being removed for an objection.
    pub key: String,
    /// For a cancel or objection: the `d` of the statement it targets.
    pub target: Option<String>,
    /// For a cancel or objection: the type of the statement it targets.
    pub target_type: Option<PendingType>,
    /// The inner event's author.
    pub signer: String,
    /// The inner event's id.
    pub id: String,
    pub issued_at: i64,
    pub label: Option<String>,
    pub expires_at: Option<i64>,
}

fn split_d(d: &str) -> Option<(StatementType, &str)> {
    StatementType::ALL
        .iter()
        .find_map(|t| d.strip_prefix(t.prefix()).map(|rest| (*t, rest)))
}

/// Split a pending statement's `d` (co-guardian, removal or move) into its
/// type and key.
pub fn split_pending_d(d: &str) -> Option<(PendingType, &str)> {
    let (t, rest) = split_d(d)?;
    let pt = t.pending()?;
    is_hex64(rest).then_some((pt, rest))
}

/// Parse and validate a guardian statement. Checks run in the README's order
/// and the first failure is the reason.
pub fn parse_statement(event: &Value) -> Result<Statement, Reason> {
    let ev = event_shape(event).ok_or(Reason::BadEvent)?;
    if !verify_event(&ev) {
        return Err(Reason::BadSignature);
    }
    if ev.kind != i64::from(CHARTER_GUARDIAN_STATEMENT) {
        return Err(Reason::WrongKind);
    }
    if !has_marker(&ev) {
        return Err(Reason::MissingMarker);
    }
    let ds = tag_values(&ev, "d");
    if ds.len() != 1 {
        return Err(Reason::DTagCount);
    }
    let ps = tag_values(&ev, "p");
    if ps.len() != 1 {
        return Err(Reason::PTagCount);
    }
    let (d, p) = (ds[0], ps[0]);
    let (kind, rest) = split_d(d).ok_or(Reason::UnknownDPrefix)?;
    if !is_hex64(p) {
        return Err(Reason::BadKey);
    }

    let (key, target, target_type) = match kind {
        StatementType::Cancel => {
            // A cancel names a pending co-guardian, removal or move.
            let (tt, key) = split_pending_d(rest).ok_or(Reason::BadCancelTarget)?;
            if key != p {
                return Err(Reason::DPMismatch);
            }
            (key, Some(rest), Some(tt))
        }
        StatementType::Objection => {
            // d = kintrinsic/object/<removal d>, p = the removal's signer.
            let (tt, key) = split_pending_d(rest)
                .filter(|(t, _)| *t == PendingType::Removal)
                .ok_or(Reason::BadObjectionTarget)?;
            (key, Some(rest), Some(tt))
        }
        _ => {
            if !is_hex64(rest) {
                return Err(Reason::BadKey);
            }
            if rest != p {
                return Err(Reason::DPMismatch);
            }
            (rest, None, None)
        }
    };

    let content = parse_strict_json(&ev.content).map_err(|_| Reason::ContentNotJson)?;
    let c = content.as_object().ok_or(Reason::ContentNotJson)?;
    if !c.get("v").is_some_and(is_one) {
        return Err(Reason::BadVersion);
    }
    let issued_at = c
        .get("issuedAt")
        .and_then(unix_seconds)
        .ok_or(Reason::BadIssuedAt)?;
    let label = match c.get("label") {
        None => None,
        Some(Value::String(s)) if s.len() <= MAX_LABEL_BYTES => Some(s.clone()),
        Some(_) => return Err(Reason::BadLabel),
    };
    let expires_at = match c.get("expiresAt") {
        None => None,
        Some(v) => Some(unix_seconds(v).ok_or(Reason::BadExpiresAt)?),
    };

    Ok(Statement {
        kind,
        d: d.to_owned(),
        p: p.to_owned(),
        key: key.to_owned(),
        target: target.map(str::to_owned),
        target_type,
        signer: ev.pubkey.clone(),
        id: ev.id.clone(),
        issued_at,
        label,
        expires_at,
    })
}
