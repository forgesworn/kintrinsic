//! JSON DTOs crossing the D-Bus boundary. Payloads pass as JSON strings so
//! wire evolution does not churn D-Bus signatures.

use serde::{Deserialize, Serialize};

/// The brokered operation types. Wire strings are the dotted forms.
/// `install.apk` is Android's op — it never rides this Linux surface, but the
/// DTO stays TOTAL over `charter_proto::OpType` (the lockstep rule) so a
/// listed cross-platform record can always be represented.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    #[serde(rename = "install.flatpak")]
    InstallFlatpak,
    #[serde(rename = "install.apk")]
    InstallApk,
    #[serde(rename = "exec.allow")]
    ExecAllow,
    #[serde(rename = "time.extend")]
    TimeExtend,
    /// A child's ask to open an app gated by `blocked`/`askFirst`. Android's
    /// op — it never rides this Linux D-Bus surface today, but the DTO stays
    /// TOTAL over `charter_proto::OpType` (the lockstep rule).
    #[serde(rename = "app.open")]
    AppOpen,
}

impl Op {
    /// The dotted wire string for this op.
    pub fn as_wire(&self) -> &'static str {
        match self {
            Op::InstallFlatpak => "install.flatpak",
            Op::InstallApk => "install.apk",
            Op::ExecAllow => "exec.allow",
            Op::TimeExtend => "time.extend",
            Op::AppOpen => "app.open",
        }
    }

    /// Parse the dotted wire string.
    pub fn from_wire(s: &str) -> Option<Op> {
        match s {
            "install.flatpak" => Some(Op::InstallFlatpak),
            "install.apk" => Some(Op::InstallApk),
            "exec.allow" => Some(Op::ExecAllow),
            "time.extend" => Some(Op::TimeExtend),
            "app.open" => Some(Op::AppOpen),
            _ => None,
        }
    }
}

/// Lifecycle state of a brokered request, as seen by the user surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReqState {
    Pending,
    Granted,
    Enacting,
    Enacted,
    Denied,
    Failed,
    Rejected,
    Expired,
    Cancelled,
}

/// A request status row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestStatusView {
    pub req_id: String,
    pub op: Op,
    pub state: ReqState,
    /// Human-readable detail (e.g. "waiting for approval", a deny reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    pub created_at: u64,
}

/// One named app bucket's day, for the ward's own view ("Play: 22 of 60 used").
///
/// A bucket is CAPPED, not free: spending it closes that bucket only, so the
/// row has to say enough for the ward to see which of their allowances is
/// nearly gone without having to discover it by an app refusing to open.
///
/// When the guardian has granted today's bucket a `time.extend`/gift
/// surplus, `limit_seconds`/`remaining_seconds` (and their week twins) are
/// built against the cap PLUS that extra, not the base cap alone (M-4,
/// 2026-08-03) — otherwise the display freezes at the base cap for as long
/// as the surplus is unspent, "15m of 15m left" unmoving for the whole time
/// the ward is actually still playing on the gift. `used_seconds` is always
/// the RAW spend either way; only the two builders (charterd's `bucket_view`
/// in `runtime.rs`, Android's twin in `warden.rs`) know about the extra.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketView {
    /// The bucket's stable slug (its day-counter key).
    pub id: String,
    /// The name the family uses for it.
    pub label: String,
    pub used_seconds: u64,
    /// The bucket's daily allowance, PLUS today's granted extra if any. `0`
    /// while the whole set is paused — paired with `capped: false`, which is
    /// what the surface reads.
    pub limit_seconds: u64,
    pub remaining_seconds: u64,
    /// The bucket's WEEKLY allowance/remainder, `-1` when no weekly cap is
    /// set (or the whole set is paused) — the same "unset, never 0" sentinel
    /// `budget_day_seconds`/`budget_week_seconds` use, so an older payload
    /// that predates these fields is never misread as "the week is used up".
    #[serde(default = "unset_seconds")]
    pub week_limit_seconds: i64,
    #[serde(default = "unset_seconds")]
    pub week_remaining_seconds: i64,
    /// True once the allowance is gone (either axis) and this bucket's apps
    /// stop.
    pub spent: bool,
    /// False while the guardian has the bucket set lifted (`paused`) — the
    /// meter still runs and is still shown, but nothing is being confiscated.
    pub capped: bool,
}

/// One `askFirst` app still gated right now: an app the guardian's `apps`
/// clause blocks but flagged for an "ask to open" affordance rather than a
/// flat wall. `label` is resolved by the daemon from its own installed-app
/// inventory where possible (falling back to `pkg`) — so the ward's surface
/// can render "Ask to open Minecraft" without a policy read of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AskFirstAppView {
    /// On-device identity — the same vocabulary as `AppOpenRequestParams::pkg`.
    pub pkg: String,
    pub label: String,
}

/// Time-left breakdown. All `*_seconds` use `-1` to mean unbounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeLeftView {
    pub effective_seconds: i64,
    pub schedule_seconds: i64,
    pub budget_seconds: i64,
    pub extension_seconds: i64,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Unix seconds when the next window opens, if currently locked by schedule.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_open: Option<u64>,
    /// True if the breakdown is being served from cache while offline.
    pub offline: bool,
    /// Learning-bucket seconds today, when a learning clause is in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_today_seconds: Option<u64>,
    /// Ordinary screen time charged today. The counterpart to the remaining
    /// figures: "40 of 120 used" is the sentence a ward can act on, where
    /// "80 minutes left" alone hides how big the day was to begin with.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_today_seconds: Option<u64>,
    /// The named app buckets in force today, each with its own day. Empty when
    /// no `buckets` clause is set — every consumer predates this field, so it
    /// defaults rather than being required.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<BucketView>,
    /// The DAILY and WEEKLY caps separately (`-1` = that cap is not set).
    /// `budget_seconds` is whichever binds, which cannot tell a ward on both
    /// caps which one is about to stop them.
    ///
    /// Defaults to `-1` ("not set"), NOT to `0`: a payload from a daemon that
    /// predates these fields must read as "no weekly cap", never as "your week
    /// is used up".
    #[serde(default = "unset_seconds")]
    pub budget_day_seconds: i64,
    #[serde(default = "unset_seconds")]
    pub budget_week_seconds: i64,
    /// The `askFirst` apps still blocked right now, labelled for the ward's
    /// "Ask to open" list. Empty when no `apps` clause sets any, or when a
    /// held-open ask-first app is not currently blocked at all — there is
    /// nothing to ask for. Absent bytes from a pre-askFirst daemon default to
    /// empty, never an error.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask_first: Vec<AskFirstAppView>,
}

/// The `-1` sentinel meaning "this cap is not set". A free function because
/// serde's `default` needs a path, and defaulting these to `0` would turn an
/// old daemon's silence into "no time left this week".
fn unset_seconds() -> i64 {
    -1
}

impl TimeLeftView {
    /// Which wall a `time.extend` ask should name (M7): an extension routed to
    /// the wrong dimension lands in an isolated pool and never lifts the lock
    /// — "+30m past bedtime" charged to the budget pool unlocks nothing.
    /// Prefer the daemon's authoritative lock `reason`; fall back to the
    /// seconds heuristic only when there is none, and require the schedule
    /// window to be the SOLE zero — a degraded both-zero snapshot (e.g.
    /// [`TimeLeftView::unknown`]) must not be mislabeled as schedule. Shared
    /// by every asker (CLI, tray) so the routing rule cannot fork.
    pub fn limit_hit(&self) -> &'static str {
        match self.reason.as_deref() {
            Some("schedule") => "schedule",
            Some("budget") => "budget",
            _ if self.schedule_seconds == 0 && self.budget_seconds != 0 => "schedule",
            _ => "budget",
        }
    }

    /// An "unknown" snapshot — explicitly NOT "unlimited" (fail-safe).
    pub fn unknown() -> Self {
        TimeLeftView {
            effective_seconds: 0,
            schedule_seconds: 0,
            budget_seconds: 0,
            extension_seconds: 0,
            locked: true,
            reason: Some("unknown".into()),
            next_open: None,
            offline: true,
            learning_today_seconds: None,
            used_today_seconds: None,
            buckets: Vec::new(),
            budget_day_seconds: -1,
            budget_week_seconds: -1,
            ask_first: Vec::new(),
        }
    }
}

/// Pairing state for the user surface. Never carries the full guardian key —
/// only a short fingerprint for confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PairingStateView {
    pub paired: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guardian_short: Option<String>,
}

/// Metadata for an exec candidate surfaced to the user surface.
///
/// **Deliberately has no `sha256` field** — the content hash never leaves
/// `charterd`; only the daemon hashes and only the daemon authorizes by hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecMeta {
    pub name: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// A daemon -> surface event (signal payload).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum DaemonEvent {
    RequestUpdated {
        status: RequestStatusView,
    },
    TimeLeftChanged {
        time_left: TimeLeftView,
    },
    LockStateChanged {
        locked: bool,
        reason: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_wire_strings() {
        assert_eq!(Op::InstallFlatpak.as_wire(), "install.flatpak");
        assert_eq!(Op::ExecAllow.as_wire(), "exec.allow");
        assert_eq!(Op::TimeExtend.as_wire(), "time.extend");
        assert_eq!(Op::AppOpen.as_wire(), "app.open");
        assert_eq!(Op::from_wire("time.extend"), Some(Op::TimeExtend));
        assert_eq!(Op::from_wire("app.open"), Some(Op::AppOpen));
        assert_eq!(Op::from_wire("nope"), None);
    }

    #[test]
    fn op_serializes_to_dotted_string() {
        assert_eq!(
            serde_json::to_string(&Op::ExecAllow).unwrap(),
            "\"exec.allow\""
        );
    }

    #[test]
    fn request_status_roundtrip() {
        let v = RequestStatusView {
            req_id: "abc".into(),
            op: Op::InstallFlatpak,
            state: ReqState::Pending,
            detail: Some("waiting for approval".into()),
            created_at: 100,
        };
        let json = serde_json::to_string(&v).unwrap();
        let back: RequestStatusView = serde_json::from_str(&json).unwrap();
        assert_eq!(v, back);
    }

    #[test]
    fn exec_meta_has_no_sha256() {
        let m = ExecMeta {
            name: "Game".into(),
            size: 42,
            origin: None,
        };
        let v = serde_json::to_value(&m).unwrap();
        assert!(
            v.get("sha256").is_none(),
            "ExecMeta must never carry a hash"
        );
        assert!(v.get("hash").is_none());
    }

    #[test]
    fn daemon_event_roundtrip() {
        let e = DaemonEvent::LockStateChanged {
            locked: true,
            reason: Some("bedtime".into()),
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: DaemonEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn a_time_extend_ask_names_the_wall_that_is_hit() {
        fn t(reason: Option<&str>, schedule: i64, budget: i64, locked: bool) -> TimeLeftView {
            TimeLeftView {
                effective_seconds: schedule.min(budget),
                schedule_seconds: schedule,
                budget_seconds: budget,
                extension_seconds: 0,
                locked,
                reason: reason.map(|r| r.into()),
                next_open: None,
                offline: false,
                learning_today_seconds: None,
                used_today_seconds: None,
                buckets: Vec::new(),
                budget_day_seconds: -1,
                budget_week_seconds: -1,
                ask_first: Vec::new(),
            }
        }
        // The daemon's authoritative reason wins outright.
        assert_eq!(t(Some("schedule"), 0, 100, true).limit_hit(), "schedule");
        assert_eq!(t(Some("budget"), 100, 0, true).limit_hit(), "budget");
        // No reason: schedule only when it is the SOLE zero.
        assert_eq!(t(None, 0, 100, true).limit_hit(), "schedule");
        assert_eq!(t(None, 100, 0, true).limit_hit(), "budget");
        // A degraded both-zero snapshot must not be mislabeled as schedule.
        assert_eq!(t(None, 0, 0, true).limit_hit(), "budget");
    }

    #[test]
    fn time_left_unknown_is_locked_not_unlimited() {
        let t = TimeLeftView::unknown();
        assert!(t.locked);
        assert_ne!(t.effective_seconds, -1);
    }

    #[test]
    fn time_left_bucket_rows_are_backward_compatible() {
        // A pre-buckets daemon's payload still parses, with no rows.
        let old = r#"{"effective_seconds":60,"schedule_seconds":60,"budget_seconds":60,"extension_seconds":0,"locked":false,"offline":false}"#;
        let t: TimeLeftView = serde_json::from_str(old).unwrap();
        assert!(t.buckets.is_empty());
        assert_eq!(t.used_today_seconds, None);
        // Empty is omitted from the wire, so an old client sees the old shape.
        let wire = serde_json::to_string(&t).unwrap();
        assert!(!wire.contains("buckets"), "{wire}");
        assert!(
            !wire.contains("usedToday") && !wire.contains("used_today"),
            "{wire}"
        );
        // Rows roundtrip, camelCase on the wire.
        let mut t2 = t.clone();
        t2.used_today_seconds = Some(2400);
        t2.buckets = vec![BucketView {
            id: "play".into(),
            label: "Play".into(),
            used_seconds: 1320,
            limit_seconds: 3600,
            remaining_seconds: 2280,
            week_limit_seconds: -1,
            week_remaining_seconds: -1,
            spent: false,
            capped: true,
        }];
        let wire2 = serde_json::to_string(&t2).unwrap();
        assert!(wire2.contains("\"usedSeconds\":1320"), "{wire2}");
        let back: TimeLeftView = serde_json::from_str(&wire2).unwrap();
        assert_eq!(back.buckets[0].label, "Play");
        assert_eq!(back.used_today_seconds, Some(2400));
    }

    /// A pre-weekly-bucket payload (day fields only) still parses, and the
    /// new week fields read `-1` ("not set"), never `0` ("the week is
    /// used up") — the same discipline `budget_week_seconds` established.
    #[test]
    fn bucket_week_fields_default_to_unset_not_zero() {
        let old = r#"{"id":"play","label":"Play","usedSeconds":600,"limitSeconds":3600,"remainingSeconds":3000,"spent":false,"capped":true}"#;
        let b: BucketView = serde_json::from_str(old).unwrap();
        assert_eq!(b.week_limit_seconds, -1);
        assert_eq!(b.week_remaining_seconds, -1);
        let wire = serde_json::to_string(&b).unwrap();
        let back: BucketView = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.week_limit_seconds, -1);
        assert_eq!(back.week_remaining_seconds, -1);
    }

    #[test]
    fn time_left_learning_field_is_backward_compatible() {
        // Absent on the wire → None (a pre-learning daemon's payload parses).
        let old = r#"{"effective_seconds":60,"schedule_seconds":60,"budget_seconds":60,"extension_seconds":0,"locked":false,"offline":false}"#;
        let t: TimeLeftView = serde_json::from_str(old).unwrap();
        assert_eq!(t.learning_today_seconds, None);
        // None is omitted from the wire (old clients see the exact old shape).
        assert!(!serde_json::to_string(&t).unwrap().contains("learning"));
        // Some roundtrips.
        let mut t2 = t.clone();
        t2.learning_today_seconds = Some(1920);
        let back: TimeLeftView =
            serde_json::from_str(&serde_json::to_string(&t2).unwrap()).unwrap();
        assert_eq!(back.learning_today_seconds, Some(1920));
    }

    /// A pre-`askFirst` payload (no such key at all) still parses, with an
    /// empty list — never an error, and never rendered as an ask-to-open row
    /// out of nowhere.
    #[test]
    fn time_left_ask_first_is_backward_compatible() {
        let old = r#"{"effective_seconds":60,"schedule_seconds":60,"budget_seconds":60,"extension_seconds":0,"locked":false,"offline":false}"#;
        let t: TimeLeftView = serde_json::from_str(old).unwrap();
        assert!(t.ask_first.is_empty());
        // Empty is omitted from the wire (old clients see the exact old shape).
        assert!(!serde_json::to_string(&t).unwrap().contains("ask_first"));

        // A populated list roundtrips, pkg + label intact.
        let mut t2 = t.clone();
        t2.ask_first = vec![AskFirstAppView {
            pkg: "com.mojang.minecraftpe".into(),
            label: "Minecraft".into(),
        }];
        let wire = serde_json::to_string(&t2).unwrap();
        assert!(wire.contains("\"ask_first\""), "{wire}");
        let back: TimeLeftView = serde_json::from_str(&wire).unwrap();
        assert_eq!(back.ask_first, t2.ask_first);
    }
}
