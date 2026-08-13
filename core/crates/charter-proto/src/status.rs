//! The STATUS payload: the content of a `CHARTER_DEVICE_STATUS` (kind 31114)
//! rumor the machine gift-wraps to the guardian, one per child per device.
//!
//! It carries numbers and enums only, never any PII (no child content, exec
//! source path, `time.extend` reason, name, or DOB) per the contract's "Device
//! STATUS feed" / "Privacy" sections. Two consumers read it: the MyCharter PWA
//! for display, and the multi-device consolidation aggregator.

use serde::{Deserialize, Serialize};

use charter_primitives::PubKey;

use crate::error::ProtoError;

/// Current STATUS schema version.
pub const STATUS_VERSION: u32 = 1;

/// The most named-group ("bucket") rows a STATUS may carry. Mirrors
/// `charter_schedule::clause::MAX_BUCKETS` (12) — a device cannot have more
/// live buckets than a clause can hold, so a payload claiming more is
/// malformed or an attack; `from_json` truncates rather than rejecting the
/// whole payload, the same fail-safe direction as `AppHold`'s count cap.
pub const MAX_STATUS_GROUPS: usize = 12;

/// The most installed-app rows a STATUS may carry. Now that the inventory
/// scans ward-writable dirs too (§2.4, `AppRef.userInstalled`), the guardian's
/// own device is no longer the only thing populating this list — a ward could
/// flood their own `~/.local/share/applications` with entries. A generous but
/// bounded cap (well past any real desktop's launcher count) so a flooded
/// payload is truncated rather than trusted whole, the same fail-safe
/// direction as `MAX_STATUS_GROUPS`/`AppHold`'s count cap.
pub const MAX_STATUS_APPS: usize = 500;

/// Which policy governs a child right now — matches the device `PolicySource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusSource {
    /// A paired guardian's per-child clauses.
    Guardian,
    /// The local device-only limits (no guardian clause for this child).
    DeviceOnly,
    /// Neither source set anything — unconstrained.
    Unconstrained,
}

/// Why a child is locked — mirrors the enforcer `LockReason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StatusLockReason {
    Schedule,
    Budget,
    Malformed,
    /// The guardian called a stand-down. Reported distinctly so the guardian's
    /// app shows the true state rather than inferring it from local state — the
    /// device is the authority on whether the stand-down actually landed.
    StandDown,
}

/// The decoded STATUS content (JSON, `camelCase`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub v: u32,
    /// Which child (== the Signet `dependantId` / `subject` pubkey).
    pub subject: PubKey,
    /// Which device (== `Pairing.machine`) — the aggregation key.
    pub machine: PubKey,
    pub ts: u64,
    /// `YYYY-MM-DD` in the budget clause's tz (the daily reset boundary).
    pub day_key: String,
    /// ISO week key in the budget tz, present only when a weekly cap is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_key: Option<String>,
    /// THIS device's raw contribution today (the aggregation input).
    pub used_today_secs: u64,
    /// THIS device's raw contribution this week.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used_week_secs: Option<u64>,
    /// Learning-bucket seconds today (present only when a learning clause is
    /// in force — absent keeps pre-learning payloads byte-identical).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning_today_secs: Option<u64>,
    /// `true` when the child has educational SITE apps in force but the device
    /// has no Chromium-family runtime to host them, so none of their windows
    /// can open.
    ///
    /// Reported because the alternative is silence. The enactor retries
    /// forever and told nobody, so a guardian ticked Khan Academy and simply
    /// nothing happened on the laptop — which is how it went unnoticed that
    /// learning had never worked on a Chrome-only machine at all. Absent
    /// unless the condition holds, so payloads stay byte-identical otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub site_runtime_missing: Option<bool>,
    /// THIS device's active-minutes journal today: a MinuteSet base64url
    /// string (240 chars) — the union-rule input (contract §USAGE_SYNC
    /// extension). Absent when the day saw no activity or on pre-B3 devices;
    /// additive under the open STATUS field-set decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_minutes_today: Option<String>,
    /// The limits actually in force on this device, so the guardian's app can
    /// SHOW them rather than a blank.
    ///
    /// Limits set on the machine itself (`charter-setup`) were invisible to
    /// MyCharter, which then displayed "time limit each day: not selected"
    /// while the device was enforcing two hours — and gave the guardian no
    /// warning that setting a limit from the phone would discard the device's
    /// wholesale (precedence is winner-takes-all, see `child_policy`). Read
    /// with `source`, these say both what the rule is and who set it.
    ///
    /// Numbers only, per the STATUS field rule. Absent when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_minutes: Option<u32>,
    /// Device's local view: schedule window left (display).
    pub window_left_secs: u64,
    /// Device's local view: budget quota left (display).
    pub quota_left_secs: u64,
    /// `min(window, quota)` on this device (display).
    pub effective_secs: u64,
    pub locked: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_reason: Option<StatusLockReason>,
    /// Which policy is in force for this child now.
    pub source: StatusSource,
    /// One-time pairing echo (QR onboarding): the token carried in the scanned
    /// pairing QR, echoed for a short window after a fresh pairing so the
    /// guardian app can match the device that scanned ITS code and bind the
    /// machine pubkey without anyone typing it. Random hex, never PII; absent
    /// in steady state. Additive under the open STATUS field-set decision
    /// (contract.md "[decide]"; port-spec D12).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pair_token: Option<String>,
    /// The device's installed launchable apps (package + display label), so the
    /// guardian can pick apps to control by name rather than typing package IDs
    /// (D3). Device-management data to the AUTHORIZED guardian over the E2E
    /// gift-wrap — never ambient. Absent until the device reports it; additive
    /// under the open STATUS field-set decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub apps: Option<Vec<AppRef>>,
    /// The device's own Charter app versionCode — lets the guardian see
    /// update state (issue #44). Absent on older devices; additive under the
    /// open STATUS field-set decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version_code: Option<u64>,
    /// The device's own Charter version as humans write it ("0.3.5"). The
    /// numeric `app_version_code` is what comparisons use; this is what a
    /// guardian is shown, because "305" is not a version to a parent. Absent
    /// on older devices; additive under the open STATUS field-set decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version_name: Option<String>,
    /// A Charter update that keeps failing, so the guardian sees WHY instead of
    /// an Update button that silently reappears. Absent when nothing is stuck.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_health: Option<UpdateHealth>,
    /// What actually happened during the last install window the guardian
    /// opened. Absent until a window has been opened on this device.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_window: Option<InstallWindowReport>,
    /// Per-bucket ("named time") usage today/this week, so the guardian's app
    /// can show progress against each named allowance ("Play: 22 of 60 used")
    /// rather than a blank. Numbers + a stable id only, per the STATUS field
    /// rule. Absent until a `buckets` clause is in force; additive under the
    /// open STATUS field-set decision. Capped at [`MAX_STATUS_GROUPS`] on
    /// parse.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<GroupSpent>>,
    /// Foreground-screen seconds today whose resolved identity was ABSENT
    /// from the device's own inventory — software Charter does not know
    /// about at all (a renamed binary, a home-dir launcher, a repacked jar),
    /// never merely an installed app that happens to be in no group (that is
    /// ordinary screen time and is not counted here). Deliberately ONE
    /// AGGREGATE NUMBER: STATUS carries no per-app usage today, and never
    /// will for this counter — naming *which* unrecognised program ran would
    /// be per-app behavioural reporting of a child. Absent when zero or
    /// unknown; additive under the open STATUS field-set decision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrecognised_today_secs: Option<u64>,
    /// Seconds spent, TODAY, in an `alwaysavailable` app while the device was
    /// LOCKED (spec 2026-08-03). A fact about the day, never a pool of time:
    /// it is deliberately absent from `used_today_secs`. Android is the ONLY
    /// platform that ever stamps this — the always-available clause only
    /// exists there — the mirror image of `unrecognised_today_secs`, which is
    /// Linux-only. Absent when zero or unknown; additive under the open
    /// STATUS field-set decision.
    ///
    /// CARRIED, NOT YET DISPLAYED (M1, review 2026-08-04): credited, stamped,
    /// serialised, parsed and tested on both sides of the wire, but no
    /// surface reads it today — the week-keyed pair below is what the
    /// guardian's and ward's weekly lines actually render. Kept (not
    /// deleted) because a future PER-DAY account of out-of-hours use ("just
    /// tonight", as opposed to the week so far) will want a day-keyed figure
    /// already flowing over the wire rather than a new field to plumb.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_of_hours_today_secs: Option<u64>,
    /// Week-keyed twin of `out_of_hours_today_secs`, for the guardian's/
    /// ward's weekly line ("3 nights so far this week · 2h 15m"). Rolls with
    /// `used_week_secs`, not with the day. Android-only, same posture as
    /// `out_of_hours_today_secs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_of_hours_week_secs: Option<u64>,
    /// Count of distinct days this week that saw any out-of-hours use — the
    /// "3 nights" half of the guardian's/ward's line. Android-only, same
    /// posture as `out_of_hours_today_secs`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_of_hours_nights_week: Option<u32>,
    /// Boots this device went through with no warden running (S1). Absent
    /// when there have been none — which is the ordinary state, and the
    /// reassuring one. See [`EnforcementGap`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement_gap: Option<EnforcementGap>,
}

/// An account of time the device spent NOT being warded, that the warden
/// itself could not have witnessed.
///
/// Safe mode is the case this exists for: Android disables every third-party
/// package in safe mode, a Device Owner included, so there is no tick to
/// notice it and nothing to report while it lasts. What the warden CAN do is
/// count boots. The platform's own boot counter increments on every boot,
/// including ones the warden slept through; the warden records the count it
/// last ran under, and any daylight between that and the count at the next
/// start is a boot that happened without it.
///
/// This is a FLOOR and a fact, not an accusation: a phone that was flashed,
/// restored from backup, or had the app force-stopped by the platform can
/// show a gap too, and the honest report is "the warden was not running for
/// N boots", which is exactly what it says. The remedy for a false positive
/// is a conversation, which is the remedy Charter is for.
///
/// Numbers only, per the STATUS field rule — it never names what ran.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnforcementGap {
    /// Cumulative count of boots the warden did not run through. Monotone
    /// for the life of the install: there is no acknowledge-and-clear,
    /// deliberately — a counter a ward could reset by waiting is not a
    /// counter. It resets only when the boot counter itself does (a wipe or
    /// a restore), which is itself the larger event.
    pub unexplained_boots: u32,
    /// When the warden NOTICED the most recent gap (unix secs) — i.e. the
    /// start of the first tick after it. Never when the gap began: nothing
    /// was running then to see it, and inventing a start would be a guess.
    pub last_noticed_at: u64,
}

/// One named bucket's usage-to-date, for STATUS display. Mirrors
/// `charter_schedule::buckets::BucketSpent` on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupSpent {
    /// The bucket's stable slug — the same id the guardian authored in the
    /// `buckets` clause.
    pub id: String,
    pub day_secs: u64,
    pub week_secs: u64,
}

/// An account of ONE guardian-opened install window: when the phone saw it
/// open, when it shut, and what changed while it was open.
///
/// This is deliberately scoped to the window and nothing else. Charter already
/// reports the device's launchable `apps` so a guardian can pick apps to
/// control by name, and that is as much as ambient reporting should ever do —
/// a running feed of what a child installs and when would be surveillance, not
/// wardship. What a guardian IS owed is an account of a loosening THEY
/// authorised: they took the lock off, so they get to see what came through it.
/// Outside a window this reports nothing, because outside a window Charter did
/// not open anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallWindowReport {
    /// When THIS DEVICE first saw the window open (unix secs) — not when the
    /// guardian signed it. A phone that was asleep when the clause arrived has
    /// its window start where the lock actually came down, so the account can
    /// never blame it for an install that happened before it was let through.
    pub started_at: u64,
    /// When the window shut. Absent while it is still open.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// Everything installed or updated inside the span, newest first. Empty is
    /// a real and common answer, and it is the reassuring one.
    pub changes: Vec<InstallChange>,
}

/// One app that arrived or moved while the window was open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallChange {
    pub pkg: String,
    pub label: String,
    pub kind: InstallChangeKind,
    /// Unix secs, from the platform's own install/update record.
    pub at: u64,
}

/// Whether the app was NEW to the phone or an update to one already there.
/// Worth distinguishing: "Robin updated a game he already had" and
/// "a game appeared that wasn't there before" are different conversations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstallChangeKind {
    Installed,
    Updated,
}

/// A stuck install directive, summarised for the guardian. Carries no per-app
/// usage or content — only the fact that OUR OWN update isn't landing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateHealth {
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_code: Option<u64>,
    /// Failed attempts so far. One is noise; fifty is a broken updater.
    pub attempts: u32,
    /// When the directive was first parked (unix secs).
    pub since_unix: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// One installed launchable app on the device: its package name (the control
/// identity) and human display label.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRef {
    pub pkg: String,
    pub label: String,
    /// `true` when this entry came from a ward-writable scan location (the
    /// managed user's own `~/.local/share/applications` or
    /// `~/.local/share/flatpak/exports/share/applications`), so the guardian
    /// can see what the ward installed themselves. `None`/absent for a
    /// root-owned entry — additive, so old payloads stay byte-identical.
    /// Flagging this is what makes showing a ward-writable entry safe; it is
    /// also the signal `focus::classify` uses to refuse it FREE (learning)
    /// time (§2.2/§2.4) — a ward-writable entry must never masquerade as an
    /// installable identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_installed: Option<bool>,
}

impl StatusPayload {
    /// Serialize to compact JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("status serializes")
    }

    /// Parse from JSON content bytes.
    pub fn from_json(s: &str) -> Result<StatusPayload, ProtoError> {
        let mut p: StatusPayload =
            serde_json::from_str(s).map_err(|e| ProtoError::Json(e.to_string()))?;
        if p.v != STATUS_VERSION {
            return Err(ProtoError::BadVersion(p.v));
        }
        // Fail-safe in the display direction: a payload claiming more groups
        // than a clause can ever hold is truncated, not refused outright —
        // losing the whole STATUS over a malformed tail would hide real,
        // trustworthy numbers the guardian is owed.
        if let Some(groups) = &mut p.groups {
            groups.truncate(MAX_STATUS_GROUPS);
        }
        // Same fail-safe direction, same reason (I8): the inventory is now
        // ward-populated too (§2.4), so a flooded `apps` list is truncated
        // rather than trusted whole or rejected outright.
        if let Some(apps) = &mut p.apps {
            apps.truncate(MAX_STATUS_APPS);
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> StatusPayload {
        StatusPayload {
            v: 1,
            subject: PubKey::from_bytes([0xab; 32]),
            machine: PubKey::from_bytes([0xcd; 32]),
            ts: 1_700_000_000,
            day_key: "2026-07-02".into(),
            week_key: Some("2026-W27".into()),
            used_today_secs: 3600,
            used_week_secs: Some(7200),
            learning_today_secs: None,
            site_runtime_missing: None,
            daily_minutes: Some(120),
            weekly_minutes: None,
            window_left_secs: 1800,
            quota_left_secs: 900,
            effective_secs: 900,
            locked: false,
            lock_reason: None,
            source: StatusSource::Guardian,
            pair_token: None,
            apps: None,
            app_version_code: None,
            app_version_name: None,
            update_health: None,
            install_window: None,
            active_minutes_today: None,
            groups: None,
            unrecognised_today_secs: None,
            out_of_hours_today_secs: None,
            out_of_hours_week_secs: None,
            out_of_hours_nights_week: None,
            enforcement_gap: None,
        }
    }

    #[test]
    fn enforcement_gap_omits_when_none_and_round_trips_camel_case() {
        let mut s = sample();
        assert!(
            !s.to_json().contains("enforcementGap"),
            "an unwarded boot the device never had must not be reported"
        );
        s.enforcement_gap = Some(EnforcementGap {
            unexplained_boots: 2,
            last_noticed_at: 1_700_000_500,
        });
        let json = s.to_json();
        assert!(json.contains("\"unexplainedBoots\":2"), "got {json}");
        assert!(json.contains("\"lastNoticedAt\":1700000500"), "got {json}");
        assert_eq!(StatusPayload::from_json(&json).unwrap(), s);
    }

    #[test]
    fn unrecognised_today_secs_serializes_camel_case_and_omits_when_none() {
        let mut s = sample();
        let json = s.to_json();
        assert!(!json.contains("unrecognisedTodaySecs"), "None must omit");
        s.unrecognised_today_secs = Some(192);
        let json = s.to_json();
        assert!(json.contains("\"unrecognisedTodaySecs\":192"), "got {json}");
        let back: StatusPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.unrecognised_today_secs, Some(192));
    }

    /// PRIVACY BOUNDARY (spec §2.3, binding): `unrecognisedTodaySecs` is ONE
    /// AGGREGATE NUMBER. Even with the field populated AND a user-installed
    /// app present in `apps`, the serialized STATUS payload must carry no
    /// path/pkg/label that could identify WHICH program went unrecognised —
    /// that would be per-app behavioural reporting of a child, which STATUS
    /// deliberately never does.
    #[test]
    fn unrecognised_today_secs_carries_no_per_app_identifier() {
        let mut s = sample();
        s.unrecognised_today_secs = Some(11_520);
        // A real inventory entry alongside it, so a naive implementation that
        // leaked the unrecognised process's identity into some OTHER field
        // would still be caught by scanning the whole payload.
        s.apps = Some(vec![AppRef {
            pkg: "/home/robin/.local/bin/prismlauncher".into(),
            label: "Prism Launcher".into(),
            user_installed: Some(true),
        }]);
        let json = s.to_json();
        assert!(json.contains("\"unrecognisedTodaySecs\":11520"), "{json}");
        // The unrecognised program's own identity — a home-dir path, a repacked
        // jar's class name, anything naming what ACTUALLY ran unrecognised —
        // never appears. (The known inventory entry above is a DIFFERENT,
        // guardian-authorised app the ward installed; it is fine for STATUS to
        // name that. What must never appear is a name for the unrecognised
        // process itself, and there is no field here that could carry one.)
        for leak in [
            "net.minecraft.client.main.Main",
            "repacked",
            "unrecognisedPkg",
            "unrecognisedPath",
            "unrecognisedLabel",
        ] {
            assert!(
                !json.contains(leak),
                "leaked an identifier: {leak} in {json}"
            );
        }
    }

    /// The three out-of-hours counters (spec 2026-08-03): `camelCase` on the
    /// wire, absent (never a zero) while `None` — same posture as
    /// `unrecognised_today_secs`, its Linux-only mirror image.
    #[test]
    fn out_of_hours_counters_serialize_camel_case_and_omit_when_none() {
        let mut s = sample();
        let json = s.to_json();
        for key in [
            "outOfHoursTodaySecs",
            "outOfHoursWeekSecs",
            "outOfHoursNightsWeek",
        ] {
            assert!(!json.contains(key), "None must omit {key}: {json}");
        }
        s.out_of_hours_today_secs = Some(600);
        s.out_of_hours_week_secs = Some(900);
        s.out_of_hours_nights_week = Some(3);
        let json = s.to_json();
        assert!(json.contains("\"outOfHoursTodaySecs\":600"), "{json}");
        assert!(json.contains("\"outOfHoursWeekSecs\":900"), "{json}");
        assert!(json.contains("\"outOfHoursNightsWeek\":3"), "{json}");
        let back: StatusPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.out_of_hours_today_secs, Some(600));
        assert_eq!(back.out_of_hours_week_secs, Some(900));
        assert_eq!(back.out_of_hours_nights_week, Some(3));
    }

    /// A pre-2026-08-03 payload (no out-of-hours keys at all) must still
    /// parse — an older ward that never sends them must not break a newer
    /// MyCharter.
    #[test]
    fn out_of_hours_counters_absent_on_pre_existing_bytes() {
        let old = sample();
        let mut json = serde_json::to_value(&old).unwrap();
        let obj = json.as_object_mut().unwrap();
        obj.remove("outOfHoursTodaySecs");
        obj.remove("outOfHoursWeekSecs");
        obj.remove("outOfHoursNightsWeek");
        let back = StatusPayload::from_json(&json.to_string()).unwrap();
        assert_eq!(back.out_of_hours_today_secs, None);
        assert_eq!(back.out_of_hours_week_secs, None);
        assert_eq!(back.out_of_hours_nights_week, None);
    }

    #[test]
    fn active_minutes_today_serializes_camel_case_and_omits_when_none() {
        let mut s = sample();
        let json = s.to_json();
        assert!(!json.contains("activeMinutesToday"), "None must omit");
        s.active_minutes_today = Some("A".repeat(240));
        let json = s.to_json();
        assert!(json.contains("\"activeMinutesToday\""), "got {json}");
        let back: StatusPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.active_minutes_today.as_deref().map(str::len),
            Some(240)
        );
    }

    #[test]
    fn app_version_code_serializes_camel_case_and_omits_when_none() {
        let mut s = sample();
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("appVersionCode"), "None must omit the key");
        s.app_version_code = Some(21);
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"appVersionCode\":21"), "got {json}");
        let back: StatusPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.app_version_code, Some(21));
    }

    /// `AppRef.userInstalled` (§2.4): additive, camelCase, omitted for a
    /// root-owned entry (`None`) — old payloads and root-only inventories
    /// stay byte-identical.
    #[test]
    fn app_ref_user_installed_serializes_camel_case_and_omits_when_none() {
        let mut s = sample();
        s.apps = Some(vec![AppRef {
            pkg: "/usr/bin/gcompris-qt".into(),
            label: "GCompris".into(),
            user_installed: None,
        }]);
        let json = s.to_json();
        assert!(!json.contains("userInstalled"), "None must omit: {json}");

        s.apps = Some(vec![AppRef {
            pkg: "/home/kid/.local/bin/prismlauncher".into(),
            label: "Prism Launcher".into(),
            user_installed: Some(true),
        }]);
        let json = s.to_json();
        assert!(json.contains("\"userInstalled\":true"), "got {json}");
        let back: StatusPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(back.apps.unwrap()[0].user_installed, Some(true));
    }

    #[test]
    fn status_roundtrips() {
        let s = sample();
        assert_eq!(StatusPayload::from_json(&s.to_json()).unwrap(), s);
    }

    #[test]
    fn wire_shape_is_camel_case_with_contract_enum_values() {
        let json = sample().to_json();
        for key in [
            "\"v\"",
            "\"subject\"",
            "\"machine\"",
            "\"ts\"",
            "\"dayKey\"",
            "\"weekKey\"",
            "\"usedTodaySecs\"",
            "\"usedWeekSecs\"",
            "\"windowLeftSecs\"",
            "\"quotaLeftSecs\"",
            "\"effectiveSecs\"",
            "\"locked\"",
            "\"source\"",
        ] {
            assert!(json.contains(key), "missing {key} in {json}");
        }
        assert!(json.contains("\"source\":\"guardian\""));

        // The kebab/lowercase enum wire values the contract pins.
        let mut locked = sample();
        locked.locked = true;
        locked.lock_reason = Some(StatusLockReason::Budget);
        locked.source = StatusSource::DeviceOnly;
        let lj = locked.to_json();
        assert!(lj.contains("\"lockReason\":\"budget\""), "{lj}");
        assert!(lj.contains("\"source\":\"device-only\""), "{lj}");
    }

    #[test]
    fn optional_fields_omitted_when_absent() {
        let mut s = sample();
        s.week_key = None;
        s.used_week_secs = None;
        s.lock_reason = None;
        let json = s.to_json();
        assert!(!json.contains("weekKey"), "{json}");
        assert!(!json.contains("usedWeekSecs"), "{json}");
        assert!(!json.contains("lockReason"), "{json}");
    }

    #[test]
    fn version_mismatch_rejected() {
        let mut s = sample();
        s.v = 2;
        assert!(StatusPayload::from_json(&s.to_json()).is_err());
    }

    // --- groups ---------------------------------------------------------

    #[test]
    fn groups_round_trip_camel_case_and_omitted_when_absent() {
        let mut s = sample();
        let json = s.to_json();
        assert!(!json.contains("\"groups\""), "None must omit: {json}");

        s.groups = Some(vec![GroupSpent {
            id: "play".into(),
            day_secs: 1320,
            week_secs: 4800,
        }]);
        let json = s.to_json();
        assert!(json.contains("\"groups\""), "{json}");
        assert!(json.contains("\"daySecs\":1320"), "{json}");
        assert!(json.contains("\"weekSecs\":4800"), "{json}");
        let back = StatusPayload::from_json(&json).unwrap();
        assert_eq!(back.groups, s.groups);
    }

    #[test]
    fn groups_absent_on_pre_groups_bytes() {
        let old = sample();
        let mut json = serde_json::to_value(&old).unwrap();
        // Simulate a pre-groups device's payload: the key never appears.
        json.as_object_mut().unwrap().remove("groups");
        let back = StatusPayload::from_json(&json.to_string()).unwrap();
        assert_eq!(back.groups, None);
    }

    #[test]
    fn groups_capped_at_max_on_parse() {
        let mut s = sample();
        s.groups = Some(
            (0..MAX_STATUS_GROUPS + 5)
                .map(|i| GroupSpent {
                    id: format!("bucket-{i}"),
                    day_secs: 0,
                    week_secs: 0,
                })
                .collect(),
        );
        let json = s.to_json();
        let back = StatusPayload::from_json(&json).unwrap();
        assert_eq!(back.groups.unwrap().len(), MAX_STATUS_GROUPS);
    }

    /// I8: the inventory is ward-populated (§2.4) now, so `apps` needs the
    /// same fail-safe cap `groups` already has.
    #[test]
    fn apps_capped_at_max_on_parse() {
        let mut s = sample();
        s.apps = Some(
            (0..MAX_STATUS_APPS + 5)
                .map(|i| AppRef {
                    pkg: format!("/usr/bin/app-{i}"),
                    label: format!("App {i}"),
                    user_installed: None,
                })
                .collect(),
        );
        let json = s.to_json();
        let back = StatusPayload::from_json(&json).unwrap();
        assert_eq!(back.apps.unwrap().len(), MAX_STATUS_APPS);
    }
}
