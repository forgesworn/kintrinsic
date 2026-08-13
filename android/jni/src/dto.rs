//! JSON DTOs crossing the JNI boundary. All hex is strict lowercase; these
//! mirror the Linux D-Bus surface shapes (port-spec §2.5) so the Kotlin side
//! and a future Linux client speak the same vocabulary.

use serde::Serialize;

/// Result of `charterInit`.
#[derive(Serialize)]
pub struct InitResult {
    pub machine_pubkey: String,
    pub paired: bool,
    pub guardian: Option<String>,
    pub subject: Option<String>,
    pub enforce_mode: String,
}

/// Result of `charterPairingState` / the pairing setters.
#[derive(Debug, Serialize)]
pub struct PairingState {
    pub paired: bool,
    /// 8-char guardian fingerprint, never the full key.
    pub guardian_short: Option<String>,
    pub guardian: Option<String>,
    pub subject: Option<String>,
    /// The pinned wss relays (empty until a `bunker://` pairing lands).
    pub relays: Vec<String>,
}

/// Result of `charterIngestClause`.
#[derive(Serialize)]
pub struct ClauseResult {
    pub accepted: bool,
    pub kind: Option<String>,
    pub subject: Option<String>,
    pub reason: String,
}

/// Result of `charterPollOnce` — one slow-tick relay round.
#[derive(Serialize)]
pub struct PollResult {
    /// False when no relay could even be tried (unpaired / no relays pinned).
    pub polled: bool,
    pub clauses_seen: u32,
    pub clauses_accepted: u32,
    pub status_emitted: bool,
    pub publish_failures: u32,
    /// True iff a guardian RELEASE was applied this round — the device is now
    /// unpaired and inert (the next tick unlocks everything).
    #[serde(default)]
    pub released: bool,
    /// Why nothing was polled / the transport error (offline is routine).
    pub reason: String,
}

/// A side-signal from a tick. Suspend and lock are NOT effects — the Kotlin
/// enforcer drives those level-triggered from `ChildDecision.locked` +
/// `enforce_mode`, so a dropped edge can never strand the ward. Only the
/// notification/audit signals ride here.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Effect {
    /// High-priority notification: 10 minutes / 1 minute left.
    Warn { level: String },
    /// A verified extension just applied — announce the guardian's yes.
    Granted { minutes: u32 },
    /// A guardian answered no. Announced once per request: a ward who asked
    /// deserves the answer wherever they are, not only if they happen to be
    /// looking at the lock screen when it lands. Silence reads as "ignored".
    Denied,
    /// A classification-only audit tag (content is always empty).
    Audit { outcome: String },
}

/// The per-ward decision the Kotlin service applies each tick.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildDecision {
    pub subject: Option<String>,
    /// Level state (not the edge): is the ward locked right now.
    pub locked: bool,
    /// schedule|budget|malformed|none — the *current* reason, not a remembered edge.
    pub reason: String,
    pub effects: Vec<Effect>,
    /// Effective seconds remaining (negative = unbounded).
    pub remaining_secs: i64,
    /// Whether a signed charter (schedule and/or budget) EXISTS for this ward —
    /// the authoritative gate for the install-lockdown (I17/I28). Distinct from
    /// `locked`: a charted ward is `configured=true` even during allowed hours,
    /// and an unreadable tick preserves the last-known value (I22). The Kotlin
    /// enforcer must gate restrictions on THIS, never on lock activity.
    pub configured: bool,
    /// The active enforcement mode (observe|freezeOnly|enforce) so the Kotlin
    /// side honours staged bring-up on the level-triggered path, not just the
    /// Rust-stripped effects.
    pub enforce_mode: String,
}

/// The `charterTimeLeft` view. Unknown is LOCKED, never unlimited (I10).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeLeftView {
    pub known: bool,
    pub locked: bool,
    pub reason: String,
    pub effective_secs: i64,
    pub schedule_secs: i64,
    pub budget_secs: i64,
    pub extension_secs: i64,
    /// When schedule-locked, seconds until the next window opens.
    pub next_open_secs: Option<i64>,
}

impl TimeLeftView {
    /// The fail-closed unknown state: locked, offline, no numbers.
    pub fn unknown() -> Self {
        TimeLeftView {
            known: false,
            locked: true,
            reason: "unknown".into(),
            effective_secs: 0,
            schedule_secs: 0,
            budget_secs: 0,
            extension_secs: 0,
            next_open_secs: None,
        }
    }
}

/// One named app bucket's day/week, for the ward's own view ("Play: 22 of 60
/// used") — same shape as Linux's `BucketView` (`charter-ipc::dto`), so the
/// two mirror surfaces read identically. Built now (Task 8); consumed by the
/// Kotlin UI in Task 9.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketView {
    /// The bucket's stable slug (its day-counter key).
    pub id: String,
    /// The name the family uses for it.
    pub label: String,
    pub used_seconds: u64,
    /// The bucket's daily allowance. `0` while the whole set is paused —
    /// paired with `capped: false`.
    pub limit_seconds: u64,
    pub remaining_seconds: u64,
    /// The bucket's WEEKLY allowance/remainder, `-1` when no weekly cap is
    /// set (or the whole set is paused) — the same "unset, never 0" sentinel
    /// used elsewhere on this surface.
    pub week_limit_seconds: i64,
    pub week_remaining_seconds: i64,
    /// True once the allowance is gone (either axis) and this bucket's apps
    /// stop.
    pub spent: bool,
    /// False while the guardian has the bucket set lifted (`paused`) — the
    /// last-known meter is still shown (whatever it read before the pause),
    /// but nothing is being confiscated. The meter itself stops moving while
    /// paused: `bucket_for_app` never attributes a tick to a paused bucket,
    /// so `used_seconds` freezes rather than keeps counting.
    pub capped: bool,
}

/// One `askFirst` app still gated right now: an app the guardian's `apps`
/// clause blocks but flagged for an "ask to open" affordance rather than a
/// flat wall. `label` is resolved from the device's own installed-app
/// inventory where possible, falling back to `pkg`.
#[derive(Debug, Serialize)]
pub struct AskFirstAppView {
    pub pkg: String,
    pub label: String,
}

/// The `charterBucketViews` payload: every named bucket's picture plus the
/// labelled `askFirst` list, in one JNI round trip.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BucketViewsPayload {
    pub buckets: Vec<BucketView>,
    pub ask_first: Vec<AskFirstAppView>,
}

/// The `charterLockInfo` child-facing copy (port-spec §2.5). Rendered verbatim
/// by the lock activity — no Kotlin tz/DST math, ever.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LockInfo {
    pub locked: bool,
    pub reason: String,
    /// e.g. "Outside allowed hours" / "Time's up for today".
    pub title: String,
    /// e.g. "You can come back at 07:00 tomorrow." Empty when not derivable.
    pub come_back: String,
    /// e.g. "up to 2h a day — 1h 05m used today". Empty when no budget.
    pub used_line: String,
    /// This device is off until a guardian opens it — no allowed hours at all,
    /// as opposed to a closed window on a device that has them. Both lock with
    /// `reason == "schedule"` (the grant routing depends on it), so the shade
    /// needs this to tell them apart: a ward who has had NO time cannot sensibly
    /// be offered "Ask for more time".
    pub dormant: bool,
}
