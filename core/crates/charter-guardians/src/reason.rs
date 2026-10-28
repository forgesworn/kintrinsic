//! Stable reason codes for *Guardian roots and working keys (v1)*.
//!
//! The strings are part of the golden vectors
//! (`charter-testkit/vectors/guardians/README.md`, "All reason codes"): a code
//! is never renamed or reused for a different meaning.

use serde::{Deserialize, Serialize};

/// Every reason a ward gives for a step: a malformed statement, an accepted
/// input, or a dropped one. Serialises to the README's stable string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Reason {
    // Statement validity.
    #[serde(rename = "bad_event")]
    BadEvent,
    #[serde(rename = "bad_signature")]
    BadSignature,
    #[serde(rename = "wrong_kind")]
    WrongKind,
    #[serde(rename = "missing_marker")]
    MissingMarker,
    #[serde(rename = "d_tag_count")]
    DTagCount,
    #[serde(rename = "p_tag_count")]
    PTagCount,
    #[serde(rename = "unknown_d_prefix")]
    UnknownDPrefix,
    #[serde(rename = "bad_key")]
    BadKey,
    #[serde(rename = "d_p_mismatch")]
    DPMismatch,
    #[serde(rename = "bad_cancel_target")]
    BadCancelTarget,
    #[serde(rename = "bad_objection_target")]
    BadObjectionTarget,
    #[serde(rename = "content_not_json")]
    ContentNotJson,
    #[serde(rename = "bad_version")]
    BadVersion,
    #[serde(rename = "bad_issued_at")]
    BadIssuedAt,
    #[serde(rename = "bad_label")]
    BadLabel,
    #[serde(rename = "bad_expires_at")]
    BadExpiresAt,
    // Accepted.
    #[serde(rename = "paired")]
    Paired,
    #[serde(rename = "approval_applied")]
    ApprovalApplied,
    #[serde(rename = "approval_updated")]
    ApprovalUpdated,
    #[serde(rename = "approval_rebound")]
    ApprovalRebound,
    #[serde(rename = "approval_preempted")]
    ApprovalPreempted,
    #[serde(rename = "withdrawal_applied")]
    WithdrawalApplied,
    #[serde(rename = "withdrawal_held")]
    WithdrawalHeld,
    #[serde(rename = "coguardian_pending")]
    CoguardianPending,
    #[serde(rename = "removal_pending")]
    RemovalPending,
    #[serde(rename = "removal_applied")]
    RemovalApplied,
    #[serde(rename = "move_pending")]
    MovePending,
    #[serde(rename = "cancel_applied")]
    CancelApplied,
    #[serde(rename = "cancel_held")]
    CancelHeld,
    #[serde(rename = "objection_applied")]
    ObjectionApplied,
    #[serde(rename = "objection_held")]
    ObjectionHeld,
    #[serde(rename = "token_registered")]
    TokenRegistered,
    #[serde(rename = "coguardian_applied")]
    CoguardianApplied,
    #[serde(rename = "move_applied")]
    MoveApplied,
    #[serde(rename = "removal_confirmed")]
    RemovalConfirmed,
    #[serde(rename = "clause_accepted")]
    ClauseAccepted,
    #[serde(rename = "signed_accepted")]
    SignedAccepted,
    #[serde(rename = "relays_applied")]
    RelaysApplied,
    #[serde(rename = "status_sent")]
    StatusSent,
    #[serde(rename = "ticked")]
    Ticked,
    // Dropped.
    #[serde(rename = "future")]
    Future,
    #[serde(rename = "seal_mismatch")]
    SealMismatch,
    #[serde(rename = "signer_not_pinned")]
    SignerNotPinned,
    #[serde(rename = "not_signer")]
    NotSigner,
    #[serde(rename = "duplicate")]
    Duplicate,
    #[serde(rename = "stale")]
    Stale,
    #[serde(rename = "tie_not_newer")]
    TieNotNewer,
    #[serde(rename = "expires_at_not_allowed")]
    ExpiresAtNotAllowed,
    #[serde(rename = "key_role_conflict")]
    KeyRoleConflict,
    #[serde(rename = "key_departed")]
    KeyDeparted,
    #[serde(rename = "bound_elsewhere")]
    BoundElsewhere,
    #[serde(rename = "root_at_quota")]
    RootAtQuota,
    #[serde(rename = "root_key_quota")]
    RootKeyQuota,
    #[serde(rename = "held_quota")]
    HeldQuota,
    #[serde(rename = "not_binding_root")]
    NotBindingRoot,
    #[serde(rename = "already_withdrawn")]
    AlreadyWithdrawn,
    #[serde(rename = "already_pinned")]
    AlreadyPinned,
    #[serde(rename = "target_not_pinned")]
    TargetNotPinned,
    #[serde(rename = "not_newer_than_departure")]
    NotNewerThanDeparture,
    #[serde(rename = "root_quota")]
    RootQuota,
    #[serde(rename = "last_root")]
    LastRoot,
    #[serde(rename = "expired")]
    Expired,
    #[serde(rename = "cancelled")]
    Cancelled,
    #[serde(rename = "objected")]
    Objected,
    #[serde(rename = "cancel_signer_mismatch")]
    CancelSignerMismatch,
    #[serde(rename = "objection_mismatch")]
    ObjectionMismatch,
    #[serde(rename = "not_pending")]
    NotPending,
    #[serde(rename = "bad_confirm")]
    BadConfirm,
    #[serde(rename = "token_invalid")]
    TokenInvalid,
    #[serde(rename = "confirmer_not_bound")]
    ConfirmerNotBound,
    #[serde(rename = "approval_required")]
    ApprovalRequired,
    #[serde(rename = "bad_approval")]
    BadApproval,
    #[serde(rename = "pairing_floor")]
    PairingFloor,
    #[serde(rename = "not_newer")]
    NotNewer,
    #[serde(rename = "bad_offer")]
    BadOffer,
    #[serde(rename = "pin_not_wrapped")]
    PinNotWrapped,
    #[serde(rename = "bad_pin_record")]
    BadPinRecord,
}

/// Why a statement event (kind 31120) is malformed, in check order.
pub const STATEMENT_REASONS: &[Reason] = &[
    Reason::BadEvent,
    Reason::BadSignature,
    Reason::WrongKind,
    Reason::MissingMarker,
    Reason::DTagCount,
    Reason::PTagCount,
    Reason::UnknownDPrefix,
    Reason::BadKey,
    Reason::DPMismatch,
    Reason::BadCancelTarget,
    Reason::BadObjectionTarget,
    Reason::ContentNotJson,
    Reason::BadVersion,
    Reason::BadIssuedAt,
    Reason::BadLabel,
    Reason::BadExpiresAt,
];

/// Why a ward accepted an input.
pub const ACCEPT_REASONS: &[Reason] = &[
    Reason::Paired,
    Reason::ApprovalApplied,
    Reason::ApprovalUpdated,
    Reason::ApprovalRebound,
    Reason::ApprovalPreempted,
    Reason::WithdrawalApplied,
    Reason::WithdrawalHeld,
    Reason::CoguardianPending,
    Reason::RemovalPending,
    Reason::RemovalApplied,
    Reason::MovePending,
    Reason::CancelApplied,
    Reason::CancelHeld,
    Reason::ObjectionApplied,
    Reason::ObjectionHeld,
    Reason::TokenRegistered,
    Reason::CoguardianApplied,
    Reason::MoveApplied,
    Reason::RemovalConfirmed,
    Reason::ClauseAccepted,
    Reason::SignedAccepted,
    Reason::RelaysApplied,
    Reason::StatusSent,
    Reason::Ticked,
];

/// Why a ward dropped an input (beyond a malformed statement).
pub const DROP_REASONS: &[Reason] = &[
    Reason::Future,
    Reason::SealMismatch,
    Reason::SignerNotPinned,
    Reason::NotSigner,
    Reason::Duplicate,
    Reason::Stale,
    Reason::TieNotNewer,
    Reason::ExpiresAtNotAllowed,
    Reason::KeyRoleConflict,
    Reason::KeyDeparted,
    Reason::BoundElsewhere,
    Reason::RootAtQuota,
    Reason::RootKeyQuota,
    Reason::HeldQuota,
    Reason::NotBindingRoot,
    Reason::AlreadyWithdrawn,
    Reason::AlreadyPinned,
    Reason::TargetNotPinned,
    Reason::NotNewerThanDeparture,
    Reason::RootQuota,
    Reason::LastRoot,
    Reason::Expired,
    Reason::Cancelled,
    Reason::Objected,
    Reason::CancelSignerMismatch,
    Reason::ObjectionMismatch,
    Reason::NotPending,
    Reason::BadConfirm,
    Reason::TokenInvalid,
    Reason::ConfirmerNotBound,
    Reason::ApprovalRequired,
    Reason::BadApproval,
    Reason::PairingFloor,
    Reason::NotNewer,
    Reason::BadOffer,
    Reason::PinNotWrapped,
    Reason::BadPinRecord,
];

impl Reason {
    /// The stable string, as in the vectors.
    pub fn as_str(self) -> &'static str {
        match self {
            Reason::BadEvent => "bad_event",
            Reason::BadSignature => "bad_signature",
            Reason::WrongKind => "wrong_kind",
            Reason::MissingMarker => "missing_marker",
            Reason::DTagCount => "d_tag_count",
            Reason::PTagCount => "p_tag_count",
            Reason::UnknownDPrefix => "unknown_d_prefix",
            Reason::BadKey => "bad_key",
            Reason::DPMismatch => "d_p_mismatch",
            Reason::BadCancelTarget => "bad_cancel_target",
            Reason::BadObjectionTarget => "bad_objection_target",
            Reason::ContentNotJson => "content_not_json",
            Reason::BadVersion => "bad_version",
            Reason::BadIssuedAt => "bad_issued_at",
            Reason::BadLabel => "bad_label",
            Reason::BadExpiresAt => "bad_expires_at",
            Reason::Paired => "paired",
            Reason::ApprovalApplied => "approval_applied",
            Reason::ApprovalUpdated => "approval_updated",
            Reason::ApprovalRebound => "approval_rebound",
            Reason::ApprovalPreempted => "approval_preempted",
            Reason::WithdrawalApplied => "withdrawal_applied",
            Reason::WithdrawalHeld => "withdrawal_held",
            Reason::CoguardianPending => "coguardian_pending",
            Reason::RemovalPending => "removal_pending",
            Reason::RemovalApplied => "removal_applied",
            Reason::MovePending => "move_pending",
            Reason::CancelApplied => "cancel_applied",
            Reason::CancelHeld => "cancel_held",
            Reason::ObjectionApplied => "objection_applied",
            Reason::ObjectionHeld => "objection_held",
            Reason::TokenRegistered => "token_registered",
            Reason::CoguardianApplied => "coguardian_applied",
            Reason::MoveApplied => "move_applied",
            Reason::RemovalConfirmed => "removal_confirmed",
            Reason::ClauseAccepted => "clause_accepted",
            Reason::SignedAccepted => "signed_accepted",
            Reason::RelaysApplied => "relays_applied",
            Reason::StatusSent => "status_sent",
            Reason::Ticked => "ticked",
            Reason::Future => "future",
            Reason::SealMismatch => "seal_mismatch",
            Reason::SignerNotPinned => "signer_not_pinned",
            Reason::NotSigner => "not_signer",
            Reason::Duplicate => "duplicate",
            Reason::Stale => "stale",
            Reason::TieNotNewer => "tie_not_newer",
            Reason::ExpiresAtNotAllowed => "expires_at_not_allowed",
            Reason::KeyRoleConflict => "key_role_conflict",
            Reason::KeyDeparted => "key_departed",
            Reason::BoundElsewhere => "bound_elsewhere",
            Reason::RootAtQuota => "root_at_quota",
            Reason::RootKeyQuota => "root_key_quota",
            Reason::HeldQuota => "held_quota",
            Reason::NotBindingRoot => "not_binding_root",
            Reason::AlreadyWithdrawn => "already_withdrawn",
            Reason::AlreadyPinned => "already_pinned",
            Reason::TargetNotPinned => "target_not_pinned",
            Reason::NotNewerThanDeparture => "not_newer_than_departure",
            Reason::RootQuota => "root_quota",
            Reason::LastRoot => "last_root",
            Reason::Expired => "expired",
            Reason::Cancelled => "cancelled",
            Reason::Objected => "objected",
            Reason::CancelSignerMismatch => "cancel_signer_mismatch",
            Reason::ObjectionMismatch => "objection_mismatch",
            Reason::NotPending => "not_pending",
            Reason::BadConfirm => "bad_confirm",
            Reason::TokenInvalid => "token_invalid",
            Reason::ConfirmerNotBound => "confirmer_not_bound",
            Reason::ApprovalRequired => "approval_required",
            Reason::BadApproval => "bad_approval",
            Reason::PairingFloor => "pairing_floor",
            Reason::NotNewer => "not_newer",
            Reason::BadOffer => "bad_offer",
            Reason::PinNotWrapped => "pin_not_wrapped",
            Reason::BadPinRecord => "bad_pin_record",
        }
    }

    /// Whether this reason means the input was accepted.
    pub fn is_accept(self) -> bool {
        ACCEPT_REASONS.contains(&self)
    }
}

impl std::fmt::Display for Reason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What happened to a pending statement during the timer advance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TickResult {
    Applied,
    Expired,
    Refused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_match_the_reference_lists() {
        let all: Vec<&Reason> = STATEMENT_REASONS
            .iter()
            .chain(ACCEPT_REASONS)
            .chain(DROP_REASONS)
            .collect();
        let want = concat!(
            "bad_event bad_signature wrong_kind missing_marker d_tag_count p_tag_count unknown_d_prefix bad_key d_p_mismatch bad_cancel_target bad_objection_target content_not_json bad_version bad_issued_at bad_label bad_expires_at ",
            "paired approval_applied approval_updated approval_rebound approval_preempted withdrawal_applied withdrawal_held coguardian_pending removal_pending removal_applied move_pending cancel_applied cancel_held objection_applied objection_held token_registered coguardian_applied move_applied removal_confirmed clause_accepted signed_accepted relays_applied status_sent ticked ",
            "future seal_mismatch signer_not_pinned not_signer duplicate stale tie_not_newer expires_at_not_allowed key_role_conflict key_departed bound_elsewhere root_at_quota root_key_quota held_quota not_binding_root already_withdrawn already_pinned target_not_pinned not_newer_than_departure root_quota last_root expired cancelled objected cancel_signer_mismatch objection_mismatch not_pending bad_confirm token_invalid confirmer_not_bound approval_required bad_approval pairing_floor not_newer bad_offer pin_not_wrapped bad_pin_record"
        );
        let want: Vec<&str> = want.split_whitespace().collect();
        assert_eq!(all.len(), want.len());
        for (r, w) in all.iter().zip(want) {
            assert_eq!(r.as_str(), w);
            assert_eq!(serde_json::to_value(r).unwrap(), serde_json::json!(w));
            let back: Reason = serde_json::from_value(serde_json::json!(w)).unwrap();
            assert_eq!(**r, back);
        }
    }
}
