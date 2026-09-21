//! The CLAUSE payload — the content of a `CHARTER_DEVICE_CLAUSE` (kind 31113)
//! NIP-01 event signed by the guardian. In Phase 1 the body is opaque; Phase 6
//! parses it into a `GrantSchedule` / `GrantBudget`. Authentication (signature
//! + author + monotonic `issuedAt`) does not depend on the body shape.

use serde::{Deserialize, Serialize};

use charter_primitives::PubKey;

use crate::error::ProtoError;

/// Current clause schema version.
pub const CLAUSE_VERSION: u32 = 1;

/// Which standing clause this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClauseKind {
    Schedule,
    Budget,
    Content,
    Apps,
    Learning,
    /// Per-app rules (block + allowed-hours) for specific apps/games. One clause
    /// carries the whole rule set (replace-the-set, like `learning`).
    AppRules,
    /// Hotspot/tethering posture: default-blocked, guardian-grantable for a
    /// window (raw or Charter-filtered). Replace-the-state, like `apprules`.
    Tethering,
    /// Guardian-directed Charter self-update: "this device runs the app at
    /// `versionCode` or newer, fetched from `url`". Replace-the-state, like
    /// `tethering`; a device already at or past the version does nothing.
    Update,
    /// The communication lifeline (spec D9): guardian phone numbers the ward
    /// can always call — surfaced ON the lock screen, so a locked phone is
    /// still a phone. Replace-the-set, like `apprules`.
    Lifeline,
    /// Named app buckets with their own daily allowance ("Play is an hour a
    /// day"). Generalises the `learning` bucket: where learning time is FREE,
    /// a bucket here is CAPPED, and spending it closes that bucket only —
    /// never the whole device. Replace-the-set, like `apprules`.
    Buckets,
    /// A one-off GIFT of time, given on the guardian's own initiative — no ask
    /// required, and available after a "no" without rewriting the schedule.
    /// Additive and today-only: it rides the same extension ledger a granted
    /// `time.extend` does, idempotent by the gift's own `id`.
    Gift,
    /// A guardian-opened MAINTENANCE WINDOW: for a short, signed, expiring
    /// span the ward stands down `DISALLOW_INSTALL_APPS` so a cabled phone can
    /// be repaired. Exists because our own install lock, plus a broken
    /// self-updater, once left a phone with no remote recovery at all
    /// (2026-07-26) — the fix must not depend on the thing that broke.
    Maintenance,
    /// A guardian-called STAND-DOWN: "finish up now". Caps the ward's remaining
    /// time at a short grace (the warning she is owed), then holds it at zero
    /// until the guardian lifts it — or until it lapses at end of her day, so a
    /// forgotten stand-down never runs into tomorrow.
    ///
    /// Deliberately NOT a second kind of lock: it moves the ward's remaining
    /// time, so the existing lock, the learning exemption, the lifeline and
    /// break-glass all apply unchanged. Replace-the-state, like `tethering`;
    /// lifting it is a newer clause whose `expires_at` has already passed.
    StandDown,
    /// What happens to audio ALREADY PLAYING when the lock lands — a family's
    /// agreed answer, not a fixed one. Charter counts Active time only, so
    /// screen-off listening already costs a child nothing; killing the
    /// audiobook at the window's edge made the counting and the enforcing
    /// disagree, and cut stories off mid-sentence. Replace-the-state.
    Listening,
    /// Apps open at ANY hour, by name (spec 2026-08-03). Not a category of app
    /// and deliberately not about audio: an audiobook player at 2am and a
    /// messaging app at a sleepover are the same shape. Exempt from the
    /// schedule and budget locks only — never a stand-down (the guardian's
    /// "stop now" must mean stop), never a malformed charter (rules we cannot
    /// read cannot be trusted to name a safe app), never a standing block.
    ///
    /// Distinct from `Listening`, which requires audio already sounding: that
    /// clause lets a story FINISH, this one lets her START something.
    /// Replace-the-set, like `apprules`.
    #[serde(rename = "alwaysavailable")]
    AlwaysAvailable,
}

/// Store key for the latest verified USAGE_SYNC view in the per-(subject, key)
/// rollback-protected store. Not a clause — but the store's monotonic floor is
/// exactly the `ts`-replay protection the contract requires, and `clear_for`
/// on release wipes it with the clauses. Clause kinds own 1..=15; keep this
/// well clear of them.
pub const USAGE_SYNC_STORE_KEY: u16 = 100;

/// Device-local slot recording when this device FIRST saw the standing
/// stand-down: `{"id": …, "startedAt": …}`. Not a clause — nothing signs it and
/// it never crosses the wire.
///
/// It exists so the ward's warning is a real minute. Were the lock instant
/// stamped by the guardian as `issuedAt + grace`, relay transit would eat into
/// it, and a phone that spent an hour offline would come back and lock her out
/// with no warning at all. Persisting first-sight also means a reboot cannot
/// restart the grace, so the countdown can't be dodged either. Lives in the
/// clause store to inherit its durability and its `clear_for` on release.
pub const STANDDOWN_GRACE_STORE_KEY: u16 = 101;

/// Device-local slot recording the install window THIS DEVICE has seen:
/// `{"startedAt": …, "endedAt": … | null}`. Not a clause — nothing signs it.
///
/// It exists because the window's own clause cannot answer "what happened while
/// installs were open". Closing early REPLACES that clause with one whose
/// `issuedAt` is the moment of closing, so the original start is gone the
/// instant a guardian uses the Close button — precisely the case where they
/// most want the account. Recording first-sight locally survives that, and
/// survives a reboot mid-window, exactly as the stand-down grace slot does.
///
/// First-SIGHT, not the clause's `issuedAt`: a phone that was asleep when the
/// clause arrived had its lock come down later, and must never be asked to
/// account for installs from before it was let through.
pub const MAINTENANCE_SPAN_STORE_KEY: u16 = 102;

impl ClauseKind {
    /// A stable per-clause-type key for the rollback-protected clause store.
    /// (Distinct from the Nostr event kind, which is 31113 for both.)
    pub fn store_key(&self) -> u16 {
        match self {
            ClauseKind::Schedule => 1,
            ClauseKind::Budget => 2,
            ClauseKind::Content => 3,
            ClauseKind::Apps => 4,
            ClauseKind::Learning => 5,
            ClauseKind::AppRules => 6,
            ClauseKind::Tethering => 7,
            ClauseKind::Update => 8,
            ClauseKind::Lifeline => 9,
            ClauseKind::Buckets => 10,
            ClauseKind::Maintenance => 11,
            ClauseKind::Gift => 12,
            ClauseKind::StandDown => 13,
            ClauseKind::Listening => 14,
            ClauseKind::AlwaysAvailable => 15,
        }
    }
}

/// One lifeline entry: a label the ward recognises ("Mum") and the number the
/// lock screen dials.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifelineNumber {
    pub label: String,
    pub number: String,
}

/// The `lifeline` clause body: 1–3 guardian numbers, always callable from the
/// ward's lock screen. Fail-closed validation — a malformed lifeline must
/// never render a call button with a garbage number.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifelineBody {
    pub v: u32,
    pub numbers: Vec<LifelineNumber>,
    pub issued_at: u64,
    /// Show the platform's own emergency number on the shade. The DEVICE
    /// resolves the region-correct digits (999/911/112/000 from SIM+network);
    /// a digit string for it never crosses the wire. Absent = off, so
    /// pre-v2 clauses stay byte-identical.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emergency_services: Option<bool>,
    /// Offer the ward a TORCH on the shade. A locked phone is still a light —
    /// a child walking home in the dark should not have to break the glass to
    /// see. Absent = off, so pre-torch clauses stay byte-identical, and an
    /// older ward that has never heard of this field simply ignores it (the
    /// body does not deny unknown fields), so no ship-order guard is needed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub torch: Option<bool>,
    /// The break-glass override (`docs/superpowers/specs/2026-07-24-lifeline-incall-and-break-glass.md`):
    /// the ward can always unlock in an emergency; doing so is loud, not
    /// blocked. Absent = ENABLED ([`BreakGlassCfg::safety_net`]) — so a
    /// guardian turning it off must send an explicit `enabled: false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_glass: Option<BreakGlassCfg>,
}

/// What the ward's break-glass unlock opens, and for how long. There is
/// deliberately NO rate limit or cooldown field: a technical cap on an
/// emergency button rebuilds the cage. Overuse is a conversation, and the
/// transparency (audit + guardian notification) is what starts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakGlassCfg {
    pub enabled: bool,
    pub scope: BreakGlassScope,
    pub duration_minutes: u16,
}

/// How much the override opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BreakGlassScope {
    /// Calls only — the phone is a phone, everything else stays shaded.
    Calls,
    /// The whole phone, for the window.
    Full,
}

/// What happens to audio already playing when the lock lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListeningMode {
    /// The lock takes everything, as it always did.
    Stop,
    /// A story may finish. The screen still shades; nothing new may start.
    Continue,
    /// A story may finish, within a limit.
    Grace,
}

/// The family's agreed answer for audio at the lock (spec 2026-07-29).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListeningBody {
    pub v: u32,
    pub issued_at: u64,
    pub mode: ListeningMode,
    /// `Grace` only. Minutes of listening allowed after the lock.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grace_minutes: Option<u16>,
    /// Packages that count as listening. Empty ⇒ nothing is exempt.
    #[serde(default)]
    pub apps: Vec<String>,
}

/// A grace is a story finishing, not an evening.
pub const MAX_LISTENING_GRACE_MINUTES: u16 = 240;

/// Why a package may (or may not) keep playing through the lock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListeningVerdict {
    /// Suspend it with everything else.
    Stop,
    /// Let it keep playing, indefinitely, while the lock stands.
    Continue,
    /// Let it keep playing until `secs_left` runs out.
    Grace { secs_left: u64 },
}

impl ListeningBody {
    /// May `pkg` keep playing?
    ///
    /// Fail CLOSED, unlike [`BreakGlassCfg::safety_net`]: this clause LOOSENS
    /// enforcement — it keeps an app alive past a lock — so every uncertainty
    /// resolves to `Stop`, the behaviour before this clause existed. (Break-glass
    /// fails the other way because the harm there is a ward stranded with no way
    /// out; the harm here is a story ending early.)
    ///
    /// `audio_playing` is load-bearing and not a courtesy: without it a named app
    /// could survive every lock by playing silence. The guardian's list says
    /// WHICH apps may, actually producing audio says WHEN.
    ///
    /// `lock_started_at` is pinned device-side on first sight of the lock spell
    /// (the stand-down grace discipline), so a reboot cannot restart the clock.
    pub fn verdict(
        &self,
        pkg: &str,
        audio_playing: bool,
        lock_started_at: u64,
        now: u64,
    ) -> ListeningVerdict {
        if self.v != CLAUSE_VERSION || !audio_playing {
            return ListeningVerdict::Stop;
        }
        if !self.apps.iter().any(|a| a == pkg) {
            return ListeningVerdict::Stop;
        }
        match self.mode {
            ListeningMode::Stop => ListeningVerdict::Stop,
            ListeningMode::Continue => ListeningVerdict::Continue,
            ListeningMode::Grace => {
                let mins = self.grace_minutes.unwrap_or(0);
                if mins == 0 || mins > MAX_LISTENING_GRACE_MINUTES {
                    return ListeningVerdict::Stop;
                }
                // Measured as ELAPSED, not as (end − now): a clock that jumped
                // backwards would otherwise mint a grace longer than the
                // guardian ever granted. `saturating_sub` floors elapsed at
                // zero, so the worst a bad clock can do is hand back the full
                // window it was already owed.
                let total = u64::from(mins) * 60;
                let elapsed = now.saturating_sub(lock_started_at);
                if elapsed >= total {
                    ListeningVerdict::Stop
                } else {
                    ListeningVerdict::Grace {
                        secs_left: total - elapsed,
                    }
                }
            }
        }
    }
}

/// One always-available app. `until_unix` absent ⇒ standing, no end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlwaysAvailableApp {
    pub pkg: String,
    /// Unix seconds; the grant ends AT this instant. ABSOLUTE, never a
    /// duration — mirrors [`AppHold`]. A duration restarts every time the
    /// stored clause is re-read, and the grant would never end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until_unix: Option<u64>,
}

/// Apps a guardian has named as openable at any hour (spec 2026-08-03).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlwaysAvailableBody {
    pub v: u32,
    pub issued_at: u64,
    /// Empty ⇒ nothing is exempt.
    #[serde(default)]
    pub apps: Vec<AlwaysAvailableApp>,
}

/// The only lock reasons an always-available app may outlive.
///
/// A stand-down is the guardian saying stop, right now; if an app could sit
/// outside it the button stops meaning anything, and the lifeline still
/// reaches her either way. `malformed` means Charter cannot read its own
/// rules, and a rule set we cannot trust cannot be trusted to name a safe app.
///
/// `"schedule"` is DELIBERATE here for a **dormant** device too ("off unless
/// a guardian opens it", `schedule.paused`) — a dormant device reports
/// `LockReason::Schedule` (so gift/ask routing still resolves), and this
/// string-match therefore lets a named app open on a dormant device exactly
/// as it would behind a closed weekly window. This is INTENDED, not a gap:
/// the founder's ruling (2026-08-04) is that "always available" means always
/// — the naming is the standing exception to whatever the device's *default*
/// state is, and dormant is a default state (nobody just did anything to the
/// device), not an active intervention like a stand-down. Do not narrow this
/// to exclude dormant without a product decision reversing that ruling; see
/// `spec/contract.md`'s "Always available" section for the fuller argument.
const EXEMPTING_LOCK_REASONS: [&str; 2] = ["schedule", "budget"];

impl AlwaysAvailableBody {
    /// Which packages may be opened right now, given why the device is locked.
    ///
    /// Fails CLOSED at every turn, like [`ListeningBody::verdict`] and unlike
    /// [`BreakGlassCfg::safety_net`]: this clause LOOSENS enforcement, so a
    /// wrong version, an unknown lock reason, or an expired entry all resolve
    /// to "not exempt" — the behaviour before this clause existed.
    ///
    /// Note what is NOT checked here: the standing app policy. The caller
    /// subtracts this set from the LOCK-DRIVEN posture only, so a guardian's
    /// outright block always wins. See `appSuspendSet` on the Kotlin side.
    pub fn exempt_packages(&self, lock_reason: &str, now: u64) -> Vec<String> {
        if self.v != CLAUSE_VERSION || !EXEMPTING_LOCK_REASONS.contains(&lock_reason) {
            return Vec::new();
        }
        self.apps
            .iter()
            // An expired entry is inert, not poison: it drops out and the rest
            // of the list stands.
            .filter(|a| a.until_unix.is_none_or(|t| now < t))
            .map(|a| a.pkg.clone())
            .collect()
    }
}

impl BreakGlassCfg {
    /// The escape hatch every device has **unless a guardian deliberately
    /// removes it** (2026-07-29).
    ///
    /// This inverts the rule the rest of the wire follows. Clauses that LOOSEN
    /// enforcement fail CLOSED — a gift we cannot trust grants no time — because
    /// the bad outcome there is a child getting minutes nobody meant to give.
    /// Here the bad outcome is the opposite and much worse: a locked device that
    /// moves to unfamiliar Wi-Fi can no longer reach the relay, so the guardian's
    /// "give time" is published and never collected, the ward's ask never gets
    /// out, and the lock screen offers no way to join a network. The device is
    /// bricked in practice — recoverable only by cable or a recovery-mode wipe.
    ///
    /// A child holding a button for a few seconds, loudly and on the record, is
    /// a far smaller harm than a child stranded behind a lock with no recourse.
    /// So absence means ON, and only an explicit `enabled: false` turns it off.
    pub fn safety_net() -> Self {
        BreakGlassCfg {
            enabled: true,
            scope: BreakGlassScope::Full,
            duration_minutes: 10,
        }
    }
}

/// The most lifeline numbers a clause may carry (grown 3 → 5, 2026-07-24).
pub const MAX_LIFELINE_NUMBERS: usize = 5;
/// Sanity bound on a break-glass window (a whole day is not an emergency).
pub const MAX_BREAK_GLASS_MINUTES: u16 = 240;

impl LifelineBody {
    pub fn validate(&self) -> Result<(), ProtoError> {
        if self.numbers.is_empty() || self.numbers.len() > MAX_LIFELINE_NUMBERS {
            return Err(ProtoError::BadParams("lifeline needs 1–5 numbers".into()));
        }
        if let Some(bg) = &self.break_glass {
            if bg.duration_minutes == 0 || bg.duration_minutes > MAX_BREAK_GLASS_MINUTES {
                return Err(ProtoError::BadParams(
                    "break-glass duration must be 1–240 minutes".into(),
                ));
            }
        }
        for n in &self.numbers {
            if n.label.trim().is_empty() || n.label.chars().count() > 20 {
                return Err(ProtoError::BadParams(
                    "lifeline label must be 1–20 chars".into(),
                ));
            }
            if !valid_phone_number(&n.number) {
                return Err(ProtoError::BadParams("invalid lifeline number".into()));
            }
        }
        Ok(())
    }
}

/// Dial-string sanity: leading `+` or digit, then 4–19 of digits/space/()/-.
/// Deliberately loose about formats (national vs E.164) and strict about
/// characters — nothing here can smuggle a USSD/`#` payload to the dialer.
fn valid_phone_number(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '+' || first.is_ascii_digit()) {
        return false;
    }
    let rest: Vec<char> = chars.collect();
    if !(4..=19).contains(&rest.len()) {
        return false;
    }
    rest.iter()
        .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '(' | ')' | '-'))
}

/// The `update` clause body: "this device runs `package_name` at
/// `version_code` or newer, fetched from `url`". Level-triggered — a device
/// already at or past the version does nothing. Both digests must hold on
/// the device before commit (archive sha256 in the stager, signing cert in
/// the installer).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAppBody {
    pub v: u32,
    pub package_name: String,
    pub version_code: u64,
    pub version_name: String,
    pub url: String,
    pub apk_sha256: charter_primitives::Sha256Hex,
    pub signer_cert_sha256: charter_primitives::Sha256Hex,
}

impl UpdateAppBody {
    pub fn validate(&self) -> Result<(), ProtoError> {
        if !crate::params::valid_package_name(&self.package_name) {
            return Err(ProtoError::BadParams("invalid packageName".into()));
        }
        if self.version_code == 0 {
            return Err(ProtoError::BadParams("versionCode must be positive".into()));
        }
        if !self.url.starts_with("https://") {
            return Err(ProtoError::BadParams("update url must be https".into()));
        }
        Ok(())
    }
}

/// The decoded CLAUSE content. `issuedAt` drives rollback protection; `body`
/// holds the schedule/budget shape Phase 6 parses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClausePayload {
    pub v: u32,
    pub kind: ClauseKind,
    /// The child this clause targets (== Signet `dependantId`). Absent on the
    /// legacy single-child wire — then it applies to the pairing's sole subject.
    /// Omitted from JSON when `None`, so frozen golden vectors stay byte-stable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<PubKey>,
    pub issued_at: u64,
    pub body: serde_json::Value,
}

impl ClausePayload {
    /// Parse from JSON content bytes.
    pub fn from_json(s: &str) -> Result<ClausePayload, ProtoError> {
        let p: ClausePayload =
            serde_json::from_str(s).map_err(|e| ProtoError::Json(e.to_string()))?;
        if p.v != CLAUSE_VERSION {
            return Err(ProtoError::BadVersion(p.v));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod listening_tests {
    use super::*;

    const LOCK_AT: u64 = 1_000_000;
    const BOOK: &str = "com.audible.application";

    fn body(mode: ListeningMode, grace: Option<u16>) -> ListeningBody {
        ListeningBody {
            v: 1,
            issued_at: 1,
            mode,
            grace_minutes: grace,
            apps: vec![BOOK.to_string()],
        }
    }

    #[test]
    fn continue_lets_a_named_app_finish_its_story() {
        let v = body(ListeningMode::Continue, None).verdict(BOOK, true, LOCK_AT, LOCK_AT + 9_999);
        assert_eq!(v, ListeningVerdict::Continue);
    }

    /// The load-bearing gate. Without it a guardian naming an audiobook app
    /// would have granted it permanent immunity from every lock — it need only
    /// play silence. The list says WHICH app may; playing says WHEN.
    #[test]
    fn a_named_app_that_is_not_actually_playing_is_suspended_like_anything_else() {
        let v = body(ListeningMode::Continue, None).verdict(BOOK, false, LOCK_AT, LOCK_AT + 1);
        assert_eq!(v, ListeningVerdict::Stop);
    }

    #[test]
    fn an_unnamed_app_gets_nothing_however_loudly_it_plays() {
        let v =
            body(ListeningMode::Continue, None).verdict("com.some.game", true, LOCK_AT, LOCK_AT);
        assert_eq!(v, ListeningVerdict::Stop);
    }

    #[test]
    fn grace_counts_down_from_the_lock_and_then_stops() {
        let b = body(ListeningMode::Grace, Some(30));
        assert_eq!(
            b.verdict(BOOK, true, LOCK_AT, LOCK_AT + 60),
            ListeningVerdict::Grace { secs_left: 29 * 60 }
        );
        // Exactly at the boundary it is over — no off-by-one minute.
        assert_eq!(
            b.verdict(BOOK, true, LOCK_AT, LOCK_AT + 30 * 60),
            ListeningVerdict::Stop
        );
    }

    /// Every uncertainty resolves to the behaviour that existed before this
    /// clause did. It LOOSENS enforcement, so it fails closed.
    #[test]
    fn anything_we_cannot_trust_falls_back_to_stopping() {
        // Wrong version.
        let mut wrong = body(ListeningMode::Continue, None);
        wrong.v = 2;
        assert_eq!(
            wrong.verdict(BOOK, true, LOCK_AT, LOCK_AT),
            ListeningVerdict::Stop
        );

        // Grace with no duration, a zero duration, or an absurd one.
        for g in [None, Some(0), Some(MAX_LISTENING_GRACE_MINUTES + 1)] {
            assert_eq!(
                body(ListeningMode::Grace, g).verdict(BOOK, true, LOCK_AT, LOCK_AT),
                ListeningVerdict::Stop,
                "grace {g:?} must not be honoured"
            );
        }

        // Mode stop is stop, named app and playing or not.
        assert_eq!(
            body(ListeningMode::Stop, None).verdict(BOOK, true, LOCK_AT, LOCK_AT),
            ListeningVerdict::Stop
        );
    }

    /// A clock that jumps backwards must not mint listening that was already
    /// spent — `now` before the pinned lock instant is not extra grace.
    #[test]
    fn a_backwards_clock_does_not_extend_the_grace() {
        let b = body(ListeningMode::Grace, Some(10));
        match b.verdict(BOOK, true, LOCK_AT, LOCK_AT - 3_600) {
            ListeningVerdict::Grace { secs_left } => {
                assert!(secs_left <= 10 * 60, "grace grew to {secs_left}s");
            }
            other => panic!("expected a bounded grace, got {other:?}"),
        }
    }

    #[test]
    fn round_trips_over_the_wire_as_lowercase() {
        let json = r#"{"v":1,"issuedAt":7,"mode":"grace","graceMinutes":30,"apps":["a.b"]}"#;
        let b: ListeningBody = serde_json::from_str(json).unwrap();
        assert_eq!(b.mode, ListeningMode::Grace);
        assert_eq!(b.grace_minutes, Some(30));
        assert!(serde_json::to_string(&b).unwrap().contains("\"grace\""));
    }

    #[test]
    fn the_clause_kind_owns_store_key_14() {
        assert_eq!(ClauseKind::Listening.store_key(), 14);
        let k: ClauseKind = serde_json::from_str("\"listening\"").unwrap();
        assert_eq!(k, ClauseKind::Listening);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_schedule_clause() {
        let json =
            r#"{"v":1,"kind":"schedule","issuedAt":1700000000,"body":{"tz":"Europe/London"}}"#;
        let c = ClausePayload::from_json(json).unwrap();
        assert_eq!(c.kind, ClauseKind::Schedule);
        assert_eq!(c.issued_at, 1700000000);
        assert_eq!(c.kind.store_key(), 1);
    }

    #[test]
    fn schedule_and_budget_have_distinct_store_keys() {
        assert_ne!(
            ClauseKind::Schedule.store_key(),
            ClauseKind::Budget.store_key()
        );
    }

    #[test]
    fn app_rules_kind_serializes_lowercase_with_distinct_store_key() {
        let k = ClauseKind::AppRules;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"apprules\"");
        let back: ClauseKind = serde_json::from_str("\"apprules\"").unwrap();
        assert_eq!(back, ClauseKind::AppRules);
        assert_eq!(k.store_key(), 6);
        // Distinct from every other kind's store slot.
        for other in [
            ClauseKind::Schedule,
            ClauseKind::Budget,
            ClauseKind::Content,
            ClauseKind::Apps,
            ClauseKind::Learning,
        ] {
            assert_ne!(k.store_key(), other.store_key());
        }
    }

    #[test]
    fn parses_apprules_clause() {
        let json = r#"{"v":1,"kind":"apprules","issuedAt":1700000000,"body":{"v":1,"issuedAt":1700000000,"rules":[{"pkg":"com.example.game","blocked":true}]}}"#;
        let c = ClausePayload::from_json(json).unwrap();
        assert_eq!(c.kind, ClauseKind::AppRules);
        assert_eq!(c.kind.store_key(), 6);
    }

    #[test]
    fn tethering_kind_serializes_lowercase_with_distinct_store_key() {
        let k = ClauseKind::Tethering;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"tethering\"");
        let back: ClauseKind = serde_json::from_str("\"tethering\"").unwrap();
        assert_eq!(back, ClauseKind::Tethering);
        assert_eq!(k.store_key(), 7);
        for other in [
            ClauseKind::Schedule,
            ClauseKind::Budget,
            ClauseKind::Content,
            ClauseKind::Apps,
            ClauseKind::Learning,
            ClauseKind::AppRules,
        ] {
            assert_ne!(k.store_key(), other.store_key());
        }
    }

    #[test]
    fn update_kind_serializes_lowercase_with_distinct_store_key() {
        let k = ClauseKind::Update;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"update\"");
        let back: ClauseKind = serde_json::from_str("\"update\"").unwrap();
        assert_eq!(back, ClauseKind::Update);
        assert_eq!(k.store_key(), 8);
        for other in [
            ClauseKind::Schedule,
            ClauseKind::Budget,
            ClauseKind::Content,
            ClauseKind::Apps,
            ClauseKind::Learning,
            ClauseKind::AppRules,
            ClauseKind::Tethering,
        ] {
            assert_ne!(k.store_key(), other.store_key());
        }
    }

    #[test]
    fn lifeline_kind_serializes_lowercase_with_distinct_store_key() {
        let k = ClauseKind::Lifeline;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"lifeline\"");
        let back: ClauseKind = serde_json::from_str("\"lifeline\"").unwrap();
        assert_eq!(back, ClauseKind::Lifeline);
        assert_eq!(k.store_key(), 9);
        for other in [
            ClauseKind::Schedule,
            ClauseKind::Budget,
            ClauseKind::Content,
            ClauseKind::Apps,
            ClauseKind::Learning,
            ClauseKind::AppRules,
            ClauseKind::Tethering,
            ClauseKind::Update,
        ] {
            assert_ne!(k.store_key(), other.store_key());
        }
    }

    #[test]
    fn lifeline_body_roundtrips_camel_case_and_validates() {
        let body = LifelineBody {
            v: 1,
            numbers: vec![LifelineNumber {
                label: "Mum".into(),
                number: "+44 7700 900123".into(),
            }],
            issued_at: 1_700_000_000,
            emergency_services: None,
            torch: None,
            break_glass: None,
        };
        assert!(body.validate().is_ok());
        // v1 payloads stay byte-identical: the v2 fields must not appear.
        let v1_json = serde_json::to_string(&body).unwrap();
        assert!(!v1_json.contains("emergencyServices"), "{v1_json}");
        assert!(!v1_json.contains("breakGlass"), "{v1_json}");
        let json = serde_json::to_string(&body).unwrap();
        assert!(
            json.contains("\"issuedAt\":1700000000"),
            "camelCase wire: {json}"
        );
        let back: LifelineBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn lifeline_v2_fields_roundtrip_and_validate() {
        let body = LifelineBody {
            v: 1,
            numbers: (0..5)
                .map(|i| LifelineNumber {
                    label: format!("G{i}"),
                    number: "07700900123".into(),
                })
                .collect(),
            issued_at: 1_700_000_000,
            emergency_services: Some(true),
            torch: None,
            break_glass: Some(BreakGlassCfg {
                enabled: true,
                scope: BreakGlassScope::Full,
                duration_minutes: 10,
            }),
        };
        assert!(body.validate().is_ok());
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains("\"emergencyServices\":true"), "{json}");
        assert!(json.contains("\"breakGlass\""), "{json}");
        assert!(json.contains("\"scope\":\"full\""), "{json}");
        assert!(json.contains("\"durationMinutes\":10"), "{json}");
        assert_eq!(serde_json::from_str::<LifelineBody>(&json).unwrap(), body);

        // A v1 clause (no v2 keys at all) still parses — and stays off.
        let v1 = r#"{"v":1,"numbers":[{"label":"Mum","number":"07700900123"}],"issuedAt":1}"#;
        let parsed: LifelineBody = serde_json::from_str(v1).unwrap();
        assert_eq!(parsed.emergency_services, None);
        assert_eq!(parsed.break_glass, None);
        assert!(parsed.validate().is_ok());

        // Fail-closed on a nonsense window.
        let bad = LifelineBody {
            break_glass: Some(BreakGlassCfg {
                enabled: true,
                scope: BreakGlassScope::Calls,
                duration_minutes: 0,
            }),
            ..body.clone()
        };
        assert!(bad.validate().is_err());
        let too_long = LifelineBody {
            break_glass: Some(BreakGlassCfg {
                enabled: true,
                scope: BreakGlassScope::Calls,
                duration_minutes: MAX_BREAK_GLASS_MINUTES + 1,
            }),
            ..body
        };
        assert!(too_long.validate().is_err());
    }

    #[test]
    fn lifeline_body_validates_fail_closed() {
        let good = LifelineBody {
            v: 1,
            numbers: vec![LifelineNumber {
                label: "Mum".into(),
                number: "07700900123".into(),
            }],
            issued_at: 1,
            emergency_services: None,
            torch: None,
            break_glass: None,
        };
        assert!(good.validate().is_ok());
        // no numbers → rejected
        let mut b = good.clone();
        b.numbers.clear();
        assert!(b.validate().is_err());
        // FIVE numbers → accepted (grown from 3, 2026-07-24)
        let five = |n: usize| LifelineBody {
            numbers: (0..n)
                .map(|i| LifelineNumber {
                    label: format!("G{i}"),
                    number: "07700900123".into(),
                })
                .collect(),
            ..good.clone()
        };
        assert!(five(5).validate().is_ok(), "5 numbers must be accepted");
        // six numbers → rejected
        assert!(five(6).validate().is_err(), "6 numbers must be rejected");
        // USSD/star-code smuggle → rejected
        let mut b = good.clone();
        b.numbers[0].number = "*#06#".into();
        assert!(b.validate().is_err());
        // tel with letters → rejected
        let mut b = good.clone();
        b.numbers[0].number = "0800CALLME".into();
        assert!(b.validate().is_err());
        // empty / over-long label → rejected
        let mut b = good.clone();
        b.numbers[0].label = "  ".into();
        assert!(b.validate().is_err());
        let mut b = good.clone();
        b.numbers[0].label = "x".repeat(21);
        assert!(b.validate().is_err());
        // too-short number → rejected
        let mut b = good;
        b.numbers[0].number = "999".into();
        assert!(b.validate().is_err());
    }

    #[test]
    fn update_body_validates_fail_closed() {
        use charter_primitives::Sha256Hex;
        let good = UpdateAppBody {
            v: 1,
            package_name: "org.forgesworn.charter".into(),
            version_code: 21,
            version_name: "0.21.0".into(),
            url: "https://charter.mysignet.app/charter-latest.apk".into(),
            apk_sha256: Sha256Hex::from_hex(&"a".repeat(64)).unwrap(),
            signer_cert_sha256: Sha256Hex::from_hex(&"b".repeat(64)).unwrap(),
        };
        assert!(good.validate().is_ok());
        // http (not https) → rejected
        let mut b = good.clone();
        b.url = "http://charter.mysignet.app/charter-latest.apk".into();
        assert!(b.validate().is_err());
        // bad package name → rejected
        let mut b = good.clone();
        b.package_name = "not-a-package".into();
        assert!(b.validate().is_err());
        // versionCode 0 → rejected
        let mut b = good;
        b.version_code = 0;
        assert!(b.validate().is_err());
    }

    #[test]
    fn update_body_roundtrips_camel_case() {
        use charter_primitives::Sha256Hex;
        let body = UpdateAppBody {
            v: 1,
            package_name: "org.forgesworn.charter".into(),
            version_code: 21,
            version_name: "0.21.0".into(),
            url: "https://charter.mysignet.app/charter-latest.apk".into(),
            apk_sha256: Sha256Hex::from_hex(&"a".repeat(64)).unwrap(),
            signer_cert_sha256: Sha256Hex::from_hex(&"b".repeat(64)).unwrap(),
        };
        let json = serde_json::to_string(&body).unwrap();
        for key in [
            "packageName",
            "versionCode",
            "versionName",
            "url",
            "apkSha256",
            "signerCertSha256",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        let back: UpdateAppBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn parses_tethering_clause() {
        let json = r#"{"v":1,"kind":"tethering","issuedAt":1700000000,"body":{"v":1,"issuedAt":1700000000,"allow":"raw","until":1700003600}}"#;
        let c = ClausePayload::from_json(json).unwrap();
        assert_eq!(c.kind, ClauseKind::Tethering);
        assert_eq!(c.kind.store_key(), 7);
    }

    #[test]
    fn content_kind_serializes_and_has_store_key() {
        let k = ClauseKind::Content;
        assert_eq!(serde_json::to_string(&k).unwrap(), "\"content\"");
        assert_eq!(k.store_key(), 3);
        let back: ClauseKind = serde_json::from_str("\"content\"").unwrap();
        assert_eq!(back, ClauseKind::Content);
    }

    #[test]
    fn legacy_clause_without_subject_parses_as_none() {
        // The existing single-child wire (no `subject`) still parses — back-compat
        // so frozen golden vectors stay valid.
        let json =
            r#"{"v":1,"kind":"schedule","issuedAt":1700000000,"body":{"tz":"Europe/London"}}"#;
        let c = ClausePayload::from_json(json).unwrap();
        assert_eq!(c.subject, None);
    }

    #[test]
    fn per_child_clause_carries_subject_hex() {
        let hex = "aa".repeat(32); // 64 hex chars = 32 bytes
        let json =
            format!(r#"{{"v":1,"kind":"budget","subject":"{hex}","issuedAt":10,"body":{{}}}}"#);
        let c = ClausePayload::from_json(&json).unwrap();
        assert_eq!(c.subject.as_ref().map(|s| s.to_hex()), Some(hex));
    }

    #[test]
    fn none_subject_is_omitted_from_serialized_json() {
        // Serializing a subject-less clause MUST NOT emit a `subject` key — keeps
        // the byte shape identical to the pre-multi-child golden vectors.
        let c = ClausePayload {
            v: 1,
            kind: ClauseKind::Schedule,
            subject: None,
            issued_at: 5,
            body: serde_json::json!({}),
        };
        let json = serde_json::to_string(&c).unwrap();
        assert!(!json.contains("subject"), "got: {json}");
    }
}

/// The `maintenance` clause body: the wall-clock instant the window shuts.
///
/// Deliberately an ABSOLUTE expiry, not a duration: a duration would restart
/// on every re-read of a stored clause and the window would never close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceBody {
    pub v: u32,
    pub until_unix: u64,
    pub issued_at: u64,
}

/// The device-local record of one seen install window (see
/// [`MAINTENANCE_SPAN_STORE_KEY`]). `ended_at` absent means still open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MaintenanceSpan {
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

/// The `gift` clause body: time the guardian gave without being asked.
///
/// Exists because answering an ask was the ONLY way to add time — grants are
/// bound to a pending request (`verify_grant` refuses any grant whose reqId
/// doesn't echo one), so with nothing outstanding a guardian's only lever was
/// editing the schedule: rewriting a standing rule to solve a one-off, and no
/// way at all to change their mind after a "no" (decented, 2026-07-26).
///
/// Additive, not replace-the-state: each gift carries its own `id`, which is
/// the extension ledger's idempotency key. The clause is re-read every tick, so
/// the id is what stops it being applied over and over; a SECOND gift is a new
/// id and therefore genuinely adds again. Rollback protection still comes from
/// the store's monotonic `issued_at` floor, as for every other clause.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GiftBody {
    pub v: u32,
    pub issued_at: u64,
    /// This gift's identity — the ledger applies each id exactly once.
    pub id: String,
    pub minutes: u16,
    /// Unix seconds after which the gift is dead: end of day in the child's
    /// schedule tz. A phone that spent the evening offline must not wake up
    /// tomorrow and apply yesterday's half hour.
    pub expires_at: u64,
    /// The named-times bucket this gift tops up, if any. Absent means the
    /// gift lands on the whole-device schedule/budget pool as before —
    /// existing gifts from before named-times keep working unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

/// A gift is a day's worth of leeway at most. Anything beyond this is a
/// schedule change wearing a gift's clothes, and should be made as one.
pub const MAX_GIFT_MINUTES: u16 = 1440;

impl GiftBody {
    /// The minutes to apply right now, or `None` if this gift must be ignored.
    ///
    /// Fail-CLOSED like [`MaintenanceBody::is_open`], and for the same reason:
    /// this clause LOOSENS enforcement, so every uncertainty — wrong version,
    /// expired, zero, absurd, or an empty id we could not deduplicate on —
    /// resolves to "no extra time".
    pub fn minutes_now(&self, now: u64) -> Option<u16> {
        if self.v != CLAUSE_VERSION {
            return None;
        }
        if self.id.is_empty() {
            return None;
        }
        if self.expires_at <= now {
            return None;
        }
        if self.minutes == 0 || self.minutes > MAX_GIFT_MINUTES {
            return None;
        }
        Some(self.minutes)
    }
}

/// A guardian-called stand-down: "finish up now, then you're done for today".
///
/// Replace-the-state. The clause standing means the stand-down stands; lifting
/// it is a NEWER clause whose `expires_at` has already passed, which the store's
/// monotonic `issued_at` floor orders for us exactly as for any other clause.
///
/// The lock instant is deliberately NOT carried here. It is the DEVICE that
/// starts the grace, from when it first saw this `id` — see
/// [`StandDownBody::grace_secs`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StandDownBody {
    pub v: u32,
    pub issued_at: u64,
    /// This stand-down's identity. The device pins the moment it FIRST saw this
    /// id, so the ward's grace cannot be restarted by a reboot, and a re-read of
    /// the same clause every tick does not keep pushing the lock away.
    pub id: String,
    /// Unix seconds after which the stand-down is dead: end of day in the
    /// ward's schedule tz. This is the midnight lapse — a stand-down the
    /// guardian forgets about must not silently become a permanent one.
    pub expires_at: u64,
    /// Seconds of warning the ward is owed before the lock lands. A stand-down
    /// with no grace is a device that dies mid-sentence, so this is clamped
    /// rather than trusted: see [`StandDownBody::grace_secs`].
    pub grace_secs: u64,
}

/// The warning a ward gets when the guardian names none, and the floor every
/// stand-down is held to. A minute is enough to save a game or say goodbye.
pub const DEFAULT_STANDDOWN_GRACE_SECS: u64 = 60;
/// The most warning a stand-down can carry. Beyond this it isn't "finish up",
/// it's a schedule change wearing a stand-down's clothes.
pub const MAX_STANDDOWN_GRACE_SECS: u64 = 600;
/// The longest a stand-down can claim to stand past its own issue instant.
/// The honest maximum is "the rest of today" in the ward's timezone — under
/// 24 hours from wherever in the day it was issued — so 36 hours is generous
/// slack for any tz arithmetic. Beyond it the clause is a client bug, and
/// doubt fails open rather than into a week-long lock.
pub const MAX_STANDDOWN_LIFE_SECS: u64 = 36 * 60 * 60;

impl StandDownBody {
    /// Whether this stand-down still stands at `now`.
    ///
    /// Fails **OPEN** — the opposite of [`GiftBody::minutes_now`], and for the
    /// same underlying reason. A gift LOOSENS enforcement, so any doubt must
    /// resolve to "no extra time"; a stand-down TIGHTENS it, so any doubt must
    /// resolve to "no lock". Both land on the ward's standing charter: the
    /// principle is that uncertainty never invents a state harsher OR looser
    /// than what the guardian actually signed.
    ///
    /// This direction matters more than the gift's. A malformed clause that
    /// wrongly locks a child out of her own phone — with the guardian believing
    /// she is fine — is a far worse failure than one that wrongly leaves her
    /// alone, and the guardian can see it did not apply and simply try again.
    pub fn stands(&self, now: u64) -> bool {
        if self.v != CLAUSE_VERSION {
            return false;
        }
        // No id means the device cannot pin when the grace began, so it could
        // neither promise the warning nor stop the lock drifting every tick.
        if self.id.is_empty() {
            return false;
        }
        // A lift is signed as a stand-down already expired at issue. That
        // relation compares the guardian's clock with itself, so it holds on a
        // device running behind her — where `expires_at <= now` alone would
        // read the lift as a FRESH stand-down and lock the ward with the very
        // message meant to free her.
        if self.expires_at <= self.issued_at {
            return false;
        }
        // The midnight lapse, held by the device rather than trusted to the
        // client: nothing honest stands longer than the rest of today.
        if self.expires_at - self.issued_at > MAX_STANDDOWN_LIFE_SECS {
            return false;
        }
        if self.expires_at <= now {
            return false;
        }
        true
    }

    /// The warning the ward is owed, clamped into something humane.
    ///
    /// Zero would lock her mid-sentence, and the guardian asked for a warning;
    /// an absurd value would make the stand-down never arrive. Neither is worth
    /// discarding the clause over, so both are corrected instead of refused.
    pub fn grace_secs(&self) -> u64 {
        if self.grace_secs == 0 {
            return DEFAULT_STANDDOWN_GRACE_SECS;
        }
        self.grace_secs.min(MAX_STANDDOWN_GRACE_SECS)
    }
}

/// A maintenance window is minutes, not hours — long enough to cable up a
/// phone and push a build, short enough that forgetting to close it is not a
/// standing hole.
pub const MAX_MAINTENANCE_SECS: u64 = 60 * 60;

impl MaintenanceBody {
    /// Fail-CLOSED, unlike the buckets cap: anything malformed, expired, or
    /// absurdly long means the window is SHUT and the install lock stays on.
    /// This is the one clause that loosens enforcement, so every uncertainty
    /// must resolve to "locked".
    pub fn is_open(&self, now: u64) -> bool {
        if self.v != CLAUSE_VERSION {
            return false;
        }
        if self.until_unix <= now {
            return false;
        }
        self.until_unix.saturating_sub(now) <= MAX_MAINTENANCE_SECS
    }
}

#[cfg(test)]
mod gift_tests {
    use super::*;

    fn gift(minutes: u16, expires_at: u64) -> GiftBody {
        GiftBody {
            v: 1,
            issued_at: 100,
            id: "g1".into(),
            minutes,
            expires_at,
            group_id: None,
        }
    }

    #[test]
    fn a_live_gift_yields_its_minutes() {
        assert_eq!(gift(30, 1_000).minutes_now(500), Some(30));
    }

    #[test]
    fn expiry_is_exclusive_so_a_gift_dies_at_its_instant() {
        let g = gift(30, 1_000);
        assert_eq!(g.minutes_now(999), Some(30));
        assert_eq!(g.minutes_now(1_000), None);
        // The offline-overnight case: a phone waking to yesterday's gift.
        assert_eq!(g.minutes_now(90_000), None);
    }

    #[test]
    fn malformed_or_absurd_gifts_give_nothing() {
        // Every uncertainty resolves to "no extra time" — this clause loosens
        // enforcement, so it must fail CLOSED.
        assert_eq!(gift(0, 1_000).minutes_now(500), None);
        assert_eq!(gift(MAX_GIFT_MINUTES + 1, 1_000).minutes_now(500), None);
        assert_eq!(
            gift(MAX_GIFT_MINUTES, 1_000).minutes_now(500),
            Some(MAX_GIFT_MINUTES)
        );

        let mut wrong_version = gift(30, 1_000);
        wrong_version.v = 2;
        assert_eq!(wrong_version.minutes_now(500), None);

        // No id = nothing to deduplicate on, and the ledger would re-apply it
        // every single tick — refuse rather than leak unbounded minutes.
        let mut no_id = gift(30, 1_000);
        no_id.id = String::new();
        assert_eq!(no_id.minutes_now(500), None);
    }

    #[test]
    fn serializes_camel_case_for_the_wire() {
        let json = serde_json::to_string(&gift(30, 1_000)).unwrap();
        assert!(json.contains("\"issuedAt\""), "{json}");
        assert!(json.contains("\"expiresAt\""), "{json}");
        let back: GiftBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, gift(30, 1_000));
    }

    #[test]
    fn gift_owns_store_key_12() {
        // Pinned: a moved key silently orphans every gift already on a device.
        assert_eq!(ClauseKind::Gift.store_key(), 12);
    }

    #[test]
    fn group_id_round_trips_camel_case() {
        let mut g = gift(30, 1_000);
        g.group_id = Some("play".into());
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("\"groupId\":\"play\""), "{json}");
        let back: GiftBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn group_id_absent_on_old_wire_bytes() {
        // Bytes from before groupId existed must still deserialize, and the
        // field must not appear on the wire when it is None (skip_serializing).
        let old = gift(30, 1_000);
        let json = serde_json::to_string(&old).unwrap();
        assert!(!json.contains("groupId"), "{json}");
        let back: GiftBody = serde_json::from_str(&json).unwrap();
        assert_eq!(back.group_id, None);
    }

    #[test]
    fn minutes_now_unaffected_by_group_id_presence() {
        let mut with_group = gift(30, 1_000);
        with_group.group_id = Some("play".into());
        assert_eq!(with_group.minutes_now(500), Some(30));

        let mut wrong_version = with_group.clone();
        wrong_version.v = 2;
        assert_eq!(wrong_version.minutes_now(500), None);
    }
}

#[cfg(test)]
mod maintenance_tests {
    use super::*;

    fn win(until: u64) -> MaintenanceBody {
        MaintenanceBody {
            v: 1,
            until_unix: until,
            issued_at: 0,
        }
    }

    #[test]
    fn a_window_is_open_until_its_instant_passes() {
        let now = 1_000_000;
        assert!(win(now + 600).is_open(now));
        assert!(win(now + 1).is_open(now));
    }

    /// This is the ONE clause that loosens enforcement, so every uncertainty
    /// must resolve to "shut". An expired window is not a grace period.
    #[test]
    fn everything_uncertain_means_shut() {
        let now = 1_000_000;
        assert!(!win(now).is_open(now), "expiring exactly now is shut");
        assert!(!win(now - 1).is_open(now), "past is shut");
        assert!(!win(0).is_open(now));

        // An absurd window is a bug or an attack, not an instruction.
        assert!(!win(now + MAX_MAINTENANCE_SECS + 1).is_open(now));
        assert!(!win(u64::MAX).is_open(now));

        // A version we don't understand is not obeyed.
        let mut wrong = win(now + 600);
        wrong.v = 2;
        assert!(!wrong.is_open(now));
    }

    #[test]
    fn the_longest_allowed_window_still_opens() {
        let now = 1_000_000;
        assert!(win(now + MAX_MAINTENANCE_SECS).is_open(now));
        assert_eq!(MAX_MAINTENANCE_SECS, 3600, "minutes, not hours");
    }

    /// A stored clause must not restart its own countdown each time it is read
    /// — which is exactly why the body carries an ABSOLUTE expiry.
    #[test]
    fn a_stored_window_does_not_renew_itself() {
        let opened_at = 1_000_000;
        let w = win(opened_at + 600);
        assert!(w.is_open(opened_at + 599));
        assert!(!w.is_open(opened_at + 601));
        assert!(!w.is_open(opened_at + 100_000));
    }
    fn stand_down(expires_at: u64, grace: u64) -> StandDownBody {
        StandDownBody {
            v: CLAUSE_VERSION,
            issued_at: 1_000,
            id: "sd-1".into(),
            expires_at,
            grace_secs: grace,
        }
    }

    /// A stand-down TIGHTENS enforcement, so every uncertainty must resolve to
    /// "no lock" — the mirror of the gift's fail-CLOSED. Wrongly locking a child
    /// out of her own phone while her guardian believes she is fine is a worse
    /// failure than wrongly leaving her alone.
    #[test]
    fn a_stand_down_fails_open() {
        // Lapsed at end of her day — the midnight lapse.
        assert!(!stand_down(500, 60).stands(500));
        assert!(!stand_down(500, 60).stands(501));

        // No id: the device could not pin when the grace began, so it could
        // neither promise the warning nor stop the lock drifting every tick.
        let mut no_id = stand_down(1_000, 60);
        no_id.id = String::new();
        assert!(!no_id.stands(500));

        // A schema it does not understand is not a lock it should invent.
        let mut wrong_version = stand_down(1_000, 60);
        wrong_version.v = 2;
        assert!(!wrong_version.stands(500));

        // The good case still stands (expiry genuinely after issue).
        assert!(stand_down(2_000, 60).stands(500));
    }

    /// A lift is a stand-down whose expiry sits at or before its own issue
    /// instant. That relation compares the guardian's clock with itself, so it
    /// is skew-proof: a device running minutes behind sees the lift's expiry in
    /// its own future, and must NOT read the message that frees the ward as a
    /// fresh stand-down — which would warn her, and past the grace, lock her.
    #[test]
    fn a_lift_never_stands_even_on_a_slow_clock() {
        // expires_at == issued_at: the lift shape the app signs.
        let lift = stand_down(1_000, 60);
        assert!(
            !lift.stands(700),
            "a slow device clock turned a lift into a lock"
        );

        // A belt-and-braces client stamping the expiry firmly in the past.
        let mut early = stand_down(900, 60);
        early.expires_at = 900;
        assert!(!early.stands(700));
    }

    /// The midnight lapse is a promise the device holds itself: no honest
    /// "rest of today" outlives this bound in any timezone, so a longer claim
    /// is a client bug and doubt fails open — not into a week-long lock.
    #[test]
    fn a_stand_down_cannot_outlive_its_day() {
        let day = stand_down(1_000 + MAX_STANDDOWN_LIFE_SECS, 60);
        assert!(day.stands(2_000));

        let week = stand_down(1_000 + MAX_STANDDOWN_LIFE_SECS + 1, 60);
        assert!(!week.stands(2_000));
    }

    /// Grace is corrected, not grounds for discarding the clause: zero would cut
    /// her off mid-sentence, and an absurd value would mean the stand-down never
    /// arrives at all.
    #[test]
    fn stand_down_grace_is_clamped_into_something_humane() {
        assert_eq!(
            stand_down(1_000, 0).grace_secs(),
            DEFAULT_STANDDOWN_GRACE_SECS
        );
        assert_eq!(stand_down(1_000, 60).grace_secs(), 60);
        assert_eq!(
            stand_down(1_000, u64::MAX).grace_secs(),
            MAX_STANDDOWN_GRACE_SECS
        );
    }

    /// The wire tag and store slot are contract surface — pin them so a rename
    /// or a renumber cannot silently strand every deployed phone.
    #[test]
    fn stand_down_wire_shape_is_pinned() {
        assert_eq!(ClauseKind::StandDown.store_key(), 13);
        assert_eq!(
            serde_json::to_string(&ClauseKind::StandDown).unwrap(),
            "\"standdown\""
        );
        let json = serde_json::to_string(&stand_down(1_000, 60)).unwrap();
        assert!(
            json.contains("\"expiresAt\":1000"),
            "camelCase on the wire: {json}"
        );
        assert!(
            json.contains("\"graceSecs\":60"),
            "camelCase on the wire: {json}"
        );
    }
}

#[cfg(test)]
mod always_available_tests {
    use super::*;

    fn body(apps: Vec<AlwaysAvailableApp>) -> AlwaysAvailableBody {
        AlwaysAvailableBody {
            v: CLAUSE_VERSION,
            issued_at: 1_000,
            apps,
        }
    }

    fn standing(pkg: &str) -> AlwaysAvailableApp {
        AlwaysAvailableApp {
            pkg: pkg.to_string(),
            until_unix: None,
        }
    }

    fn until(pkg: &str, t: u64) -> AlwaysAvailableApp {
        AlwaysAvailableApp {
            pkg: pkg.to_string(),
            until_unix: Some(t),
        }
    }

    #[test]
    fn store_key_is_fifteen() {
        assert_eq!(ClauseKind::AlwaysAvailable.store_key(), 15);
    }

    #[test]
    fn kind_tag_is_all_lowercase() {
        let k: ClauseKind = serde_json::from_str("\"alwaysavailable\"").unwrap();
        assert_eq!(k, ClauseKind::AlwaysAvailable);
        assert_eq!(
            serde_json::to_string(&ClauseKind::AlwaysAvailable).unwrap(),
            "\"alwaysavailable\""
        );
    }

    #[test]
    fn a_standing_app_is_exempt_under_schedule_and_budget() {
        let b = body(vec![standing("com.book")]);
        assert_eq!(b.exempt_packages("schedule", 5_000), vec!["com.book"]);
        assert_eq!(b.exempt_packages("budget", 5_000), vec!["com.book"]);
    }

    /// The guardian's stop-right-now button must mean stop, or it stops
    /// meaning anything. Malformed means we cannot read our own rules, so we
    /// cannot trust the app list inside them either.
    #[test]
    fn no_app_survives_a_standdown_or_a_malformed_charter() {
        let b = body(vec![standing("com.book")]);
        assert!(b.exempt_packages("standdown", 5_000).is_empty());
        assert!(b.exempt_packages("malformed", 5_000).is_empty());
        assert!(b.exempt_packages("unknown", 5_000).is_empty());
        assert!(b.exempt_packages("", 5_000).is_empty());
    }

    #[test]
    fn an_expiry_ends_the_grant_at_the_instant() {
        let b = body(vec![until("com.chat", 5_000)]);
        assert_eq!(b.exempt_packages("schedule", 4_999), vec!["com.chat"]);
        assert!(b.exempt_packages("schedule", 5_000).is_empty());
        assert!(b.exempt_packages("schedule", 5_001).is_empty());
    }

    /// An expired entry is inert, not poison — the rest of the list stands.
    #[test]
    fn an_expired_entry_does_not_void_the_others() {
        let b = body(vec![until("com.chat", 1), standing("com.book")]);
        assert_eq!(b.exempt_packages("schedule", 5_000), vec!["com.book"]);
    }

    #[test]
    fn a_wrong_version_exempts_nothing() {
        let mut b = body(vec![standing("com.book")]);
        b.v = 99;
        assert!(b.exempt_packages("schedule", 5_000).is_empty());
    }

    #[test]
    fn an_empty_list_exempts_nothing() {
        assert!(body(vec![]).exempt_packages("schedule", 5_000).is_empty());
    }

    #[test]
    fn until_unix_is_omitted_when_absent_and_round_trips() {
        let json = serde_json::to_string(&standing("com.book")).unwrap();
        assert_eq!(json, r#"{"pkg":"com.book"}"#);
        let back: AlwaysAvailableApp = serde_json::from_str(&json).unwrap();
        assert_eq!(back.until_unix, None);

        let json = serde_json::to_string(&until("com.chat", 5_000)).unwrap();
        assert_eq!(json, r#"{"pkg":"com.chat","untilUnix":5000}"#);
    }
}
