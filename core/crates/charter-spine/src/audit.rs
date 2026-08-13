//! Audit records. Every decision emits a gift-wrapped audit to the guardian —
//! **content is always empty**; only routing/classification tags ride along.
//! Child content, exec source paths, the time.extend reason, and any DOB never
//! appear in tags or logs.

use charter_proto::OpType;

/// A gift-wrapped audit record (content is always empty when emitted).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRecord {
    pub outcome: AuditOutcome,
    pub op: OpType,
}

/// The classification of an audited decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditOutcome {
    Enacted,
    Denied,
    Failed,
    Locked,
    Thawed,
}

impl AuditOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuditOutcome::Enacted => "enacted",
            AuditOutcome::Denied => "denied",
            AuditOutcome::Failed => "failed",
            AuditOutcome::Locked => "locked",
            AuditOutcome::Thawed => "thawed",
        }
    }
}

impl AuditRecord {
    pub fn new(outcome: AuditOutcome, op: OpType) -> Self {
        AuditRecord { outcome, op }
    }

    /// The audit tags (classification only — never content).
    pub fn tags(&self) -> Vec<Vec<String>> {
        vec![
            vec!["outcome".into(), self.outcome.as_str().into()],
            vec!["op".into(), self.op.as_wire().into()],
        ]
    }
}
