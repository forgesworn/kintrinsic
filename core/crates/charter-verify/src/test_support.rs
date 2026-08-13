//! Deterministic builders for signed GRANT / CLAUSE events + tamper mutators.
//! Available under the `mock` feature; used by this crate's tests and by the
//! `charterd` integration tests. Signing is deterministic (no aux rand) so
//! tests reproduce.

use charter_primitives::kinds;
use charter_primitives::{EventId, Nonce, NostrEvent, PubKey, ReqId, Sig};
use charter_proto::{ClauseKind, Decision, OpType};
use charter_sys::signer::{MachineSigner, SeedSigner};
use serde_json::{json, Value};

use crate::event::event_id;

/// Sign an arbitrary event with `signer`: compute the NIP-01 id, then sign it.
pub fn sign_event(
    signer: &SeedSigner,
    kind: u16,
    created_at: u64,
    tags: Vec<Vec<String>>,
    content: String,
) -> NostrEvent {
    let pubkey = MachineSigner::pubkey(signer);
    let mut ev = NostrEvent {
        id: EventId::from_bytes([0; 32]),
        pubkey,
        created_at,
        kind,
        tags,
        content,
        sig: Sig::from_bytes([0; 64]),
    };
    ev.id = event_id(&ev);
    ev.sig = MachineSigner::sign(signer, ev.id.as_bytes()).expect("mock sign");
    ev
}

/// A deterministic test guardian.
pub struct TestGuardian {
    /// The underlying seed signer (exposed for "forge with another key" tests).
    pub signer: SeedSigner,
}

impl TestGuardian {
    /// The pinned guardian (seed `0x11`, matching the generated vectors).
    pub fn new() -> Self {
        TestGuardian {
            signer: SeedSigner::from_seed(0x11),
        }
    }

    /// A *different* guardian seed (for wrong-authority tests).
    pub fn from_seed(seed: u8) -> Self {
        TestGuardian {
            signer: SeedSigner::from_seed(seed),
        }
    }

    /// The guardian's pinned pubkey.
    pub fn pubkey(&self) -> PubKey {
        MachineSigner::pubkey(&self.signer)
    }
}

impl Default for TestGuardian {
    fn default() -> Self {
        Self::new()
    }
}

const DEFAULT_TS: u64 = 1_700_000_000;
const DEFAULT_EXP: u64 = 1_700_003_600;

/// Builds signed GRANT events with mutable fields for tamper tests.
pub struct GrantBuilder {
    pub op: OpType,
    pub decision: Decision,
    pub req_id: ReqId,
    pub nonce: Nonce,
    pub ts: u64,
    pub exp: u64,
    pub params: Value,
    pub created_at: u64,
}

impl GrantBuilder {
    /// An allow `install.flatpak` grant with default timings.
    pub fn install_allow(req_id: ReqId, nonce: Nonce) -> Self {
        GrantBuilder {
            op: OpType::InstallFlatpak,
            decision: Decision::Allow,
            req_id,
            nonce,
            ts: DEFAULT_TS,
            exp: DEFAULT_EXP,
            params: json!({"ref": "org.test.App", "remote": "flathub"}),
            created_at: DEFAULT_TS,
        }
    }

    /// An allow `app.open` grant — the minimal answer-signal shape
    /// (`{pkg, minutesGranted}`), mirroring `time.extend`'s params. Callers
    /// pass the params value directly (built from
    /// `charter_proto::AppOpenGrantParams`, or hand-shaped for malformed-input
    /// tests) rather than a separate params-builder method, matching how
    /// every other non-default op in this builder works (`.op().params()`).
    pub fn app_open_allow(req_id: ReqId, nonce: Nonce, params: Value) -> Self {
        GrantBuilder {
            op: OpType::AppOpen,
            decision: Decision::Allow,
            req_id,
            nonce,
            ts: DEFAULT_TS,
            exp: DEFAULT_EXP,
            params,
            created_at: DEFAULT_TS,
        }
    }

    /// A deny grant for the same op.
    pub fn deny(mut self) -> Self {
        self.decision = Decision::Deny;
        self
    }

    /// Override the op (e.g. to build an exec.allow or time.extend grant).
    pub fn op(mut self, op: OpType) -> Self {
        self.op = op;
        self
    }

    /// Override expiry.
    pub fn exp(mut self, exp: u64) -> Self {
        self.exp = exp;
        self
    }

    /// Override issued-at.
    pub fn ts(mut self, ts: u64) -> Self {
        self.ts = ts;
        self
    }

    /// Override params (e.g. to test grant-vs-request divergence).
    pub fn params(mut self, params: Value) -> Self {
        self.params = params;
        self
    }

    /// Override the reqId echoed in the payload.
    pub fn req_id(mut self, req_id: ReqId) -> Self {
        self.req_id = req_id;
        self
    }

    /// Override the nonce echoed in the payload.
    pub fn nonce(mut self, nonce: Nonce) -> Self {
        self.nonce = nonce;
        self
    }

    fn payload_json(&self) -> String {
        let decision = match self.decision {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
        };
        json!({
            "v": 1,
            "op": self.op.as_wire(),
            "reqId": self.req_id.to_hex(),
            "nonce": self.nonce.to_hex(),
            "decision": decision,
            "ts": self.ts,
            "exp": self.exp,
            "params": self.params,
        })
        .to_string()
    }

    fn tags(&self) -> Vec<Vec<String>> {
        vec![kinds::marker_tag(), vec!["d".into(), self.req_id.to_hex()]]
    }

    /// Build a grant event signed by the pinned guardian.
    pub fn build(&self, guardian: &TestGuardian) -> NostrEvent {
        self.build_signed_by(&guardian.signer)
    }

    /// Build a grant event signed by an arbitrary key (wrong-authority tests).
    pub fn build_signed_by(&self, signer: &SeedSigner) -> NostrEvent {
        sign_event(
            signer,
            kinds::CHARTER_DEVICE_GRANT,
            self.created_at,
            self.tags(),
            self.payload_json(),
        )
    }
}

/// Builds signed CLAUSE events.
pub struct ClauseBuilder {
    pub kind: ClauseKind,
    pub issued_at: u64,
    pub body: Value,
    pub created_at: u64,
    /// When set, the clause targets this child (multi-child, Signet-first).
    pub subject: Option<PubKey>,
}

impl ClauseBuilder {
    /// A schedule clause.
    pub fn schedule(issued_at: u64) -> Self {
        ClauseBuilder {
            kind: ClauseKind::Schedule,
            issued_at,
            body: json!({"v": 1, "tz": "Europe/London", "weekly": {}, "issuedAt": issued_at}),
            created_at: issued_at,
            subject: None,
        }
    }

    /// A budget clause.
    pub fn budget(issued_at: u64) -> Self {
        ClauseBuilder {
            kind: ClauseKind::Budget,
            issued_at,
            body: json!({"v": 1, "tz": "Europe/London", "dailyMinutes": 120, "issuedAt": issued_at}),
            created_at: issued_at,
            subject: None,
        }
    }

    /// A content clause (machine-wide web filtering).
    pub fn content(issued_at: u64) -> Self {
        ClauseBuilder {
            kind: ClauseKind::Content,
            issued_at,
            body: json!({"v": 1, "issuedAt": issued_at}),
            created_at: issued_at,
            subject: None,
        }
    }

    /// An apps clause (standing per-app block/allow policy).
    pub fn apps(issued_at: u64) -> Self {
        ClauseBuilder {
            kind: ClauseKind::Apps,
            issued_at,
            body: json!({"v": 1, "posture": "blocklist", "issuedAt": issued_at}),
            created_at: issued_at,
            subject: None,
        }
    }

    /// A gift clause: minutes the guardian gave without being asked.
    pub fn gift(issued_at: u64, id: &str, minutes: u16, expires_at: u64) -> Self {
        ClauseBuilder {
            kind: ClauseKind::Gift,
            issued_at,
            body: json!({
                "v": 1,
                "issuedAt": issued_at,
                "id": id,
                "minutes": minutes,
                "expiresAt": expires_at,
            }),
            created_at: issued_at,
            subject: None,
        }
    }

    /// A stand-down clause: "finish up now, then done for today". Lifting it
    /// is the same clause with `expiresAt` at (or before) its own `issuedAt`.
    pub fn stand_down(issued_at: u64, id: &str, expires_at: u64) -> Self {
        ClauseBuilder {
            kind: ClauseKind::StandDown,
            issued_at,
            body: json!({
                "v": 1,
                "issuedAt": issued_at,
                "id": id,
                "expiresAt": expires_at,
                "graceSecs": 60,
            }),
            created_at: issued_at,
            subject: None,
        }
    }

    /// Override the body.
    pub fn body(mut self, body: Value) -> Self {
        self.body = body;
        self
    }

    /// Target a specific child (sets `subject` on the clause payload).
    pub fn subject(mut self, subject: PubKey) -> Self {
        self.subject = Some(subject);
        self
    }

    fn payload_json(&self) -> String {
        let kind = match self.kind {
            ClauseKind::Schedule => "schedule",
            ClauseKind::Budget => "budget",
            ClauseKind::Content => "content",
            ClauseKind::Apps => "apps",
            ClauseKind::Learning => "learning",
            ClauseKind::AppRules => "apprules",
            ClauseKind::Tethering => "tethering",
            ClauseKind::Update => "update",
            ClauseKind::Lifeline => "lifeline",
            ClauseKind::Buckets => "buckets",
            ClauseKind::Maintenance => "maintenance",
            ClauseKind::Gift => "gift",
            ClauseKind::StandDown => "standdown",
            ClauseKind::Listening => "listening",
            ClauseKind::AlwaysAvailable => "alwaysavailable",
        };
        let mut v = json!({"v": 1, "kind": kind, "issuedAt": self.issued_at, "body": self.body});
        if let Some(subject) = &self.subject {
            v["subject"] = json!(subject.to_hex());
        }
        v.to_string()
    }

    fn tags(&self) -> Vec<Vec<String>> {
        let d = match self.kind {
            ClauseKind::Schedule => "schedule",
            ClauseKind::Budget => "budget",
            ClauseKind::Content => "content",
            ClauseKind::Apps => "apps",
            ClauseKind::Learning => "learning",
            ClauseKind::AppRules => "apprules",
            ClauseKind::Tethering => "tethering",
            ClauseKind::Update => "update",
            ClauseKind::Lifeline => "lifeline",
            ClauseKind::Buckets => "buckets",
            ClauseKind::Maintenance => "maintenance",
            ClauseKind::Gift => "gift",
            ClauseKind::StandDown => "standdown",
            ClauseKind::Listening => "listening",
            ClauseKind::AlwaysAvailable => "alwaysavailable",
        };
        vec![kinds::marker_tag(), vec!["d".into(), d.into()]]
    }

    /// Build a clause event signed by the pinned guardian.
    pub fn build(&self, guardian: &TestGuardian) -> NostrEvent {
        self.build_signed_by(&guardian.signer)
    }

    /// Build a clause event signed by an arbitrary key.
    pub fn build_signed_by(&self, signer: &SeedSigner) -> NostrEvent {
        sign_event(
            signer,
            kinds::CHARTER_DEVICE_CLAUSE,
            self.created_at,
            self.tags(),
            self.payload_json(),
        )
    }
}
