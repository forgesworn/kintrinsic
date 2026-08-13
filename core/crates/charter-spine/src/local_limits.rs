//! The pure device-only-limits model: a parent's standalone allowed-hours +
//! daily-cap, converted into the same `GrantSchedule`/`GrantBudget` clauses
//! the enforcer already consumes, plus the per-child config document
//! (guardian `subject` binding + optional local `limits`) and the pure
//! passwd/group-document parsers the Linux roster uses. Extracted from
//! `charterd::device_limits` (which keeps the file-IO half: loading/saving
//! `/etc/charter` documents) so the spine's child policy compiles on every
//! warden. Pure: no filesystem access, no OS calls.

use charter_proto::{ClauseKind, GrantLearning};
use charter_schedule::{
    GrantBudget, GrantSchedule, GrantScheduleWindow, WeekStart, WeeklySchedule,
};
use charter_sys::persistence::ClauseStore;
use charter_sys::{SysResult, SystemLayer};
use serde::{Deserialize, Serialize};
/// An allowed-hours window (`"HH:MM"` 24h local time).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DayWindow {
    pub wake: String,
    pub bedtime: String,
}

/// The parent's standalone limits (the `/etc/charter/limits.json` shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLimits {
    /// IANA timezone the limits are interpreted in (e.g. `"Europe/London"`).
    pub tz: String,
    /// Allowed-hours window applied every day (overridden on the weekend if set).
    pub wake: String,
    pub bedtime: String,
    /// Daily on-screen time cap, in minutes.
    pub daily_minutes: u32,
    /// Optional separate Sat/Sun allowed-hours window.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekend: Option<DayWindow>,
}

/// `true` if `s` is a 24h `"HH:MM"` clock time (so the UI/CLI can reject typos
/// before they reach enforcement).
pub fn valid_hhmm(s: &str) -> bool {
    let (h, m) = match s.split_once(':') {
        Some(x) => x,
        None => return false,
    };
    if h.len() != 2 || m.len() != 2 {
        return false;
    }
    match (h.parse::<u8>(), m.parse::<u8>()) {
        (Ok(h), Ok(m)) => h < 24 && m < 60,
        _ => false,
    }
}

impl DeviceLimits {
    /// Validate the limits — well-formed times and a non-zero, sane daily cap.
    pub fn validate(&self) -> Result<(), String> {
        if !valid_hhmm(&self.wake) || !valid_hhmm(&self.bedtime) {
            return Err("wake/bedtime must be HH:MM".into());
        }
        if let Some(w) = &self.weekend {
            if !valid_hhmm(&w.wake) || !valid_hhmm(&w.bedtime) {
                return Err("weekend wake/bedtime must be HH:MM".into());
            }
        }
        if self.daily_minutes == 0 || self.daily_minutes > 24 * 60 {
            return Err("dailyMinutes must be between 1 and 1440".into());
        }
        if self.tz.trim().is_empty() {
            return Err("tz is required".into());
        }
        Ok(())
    }

    fn weekday_window(&self) -> Vec<GrantScheduleWindow> {
        vec![GrantScheduleWindow {
            start: self.wake.clone(),
            end: self.bedtime.clone(),
        }]
    }

    fn weekend_window(&self) -> Vec<GrantScheduleWindow> {
        match &self.weekend {
            Some(w) => vec![GrantScheduleWindow {
                start: w.wake.clone(),
                end: w.bedtime.clone(),
            }],
            None => self.weekday_window(),
        }
    }

    /// The allowed-hours schedule clause (Mon–Fri weekday window, Sat/Sun
    /// weekend window).
    pub fn to_schedule(&self, issued_at: u64) -> GrantSchedule {
        let wd = self.weekday_window();
        let we = self.weekend_window();
        GrantSchedule {
            v: 1,
            tz: self.tz.clone(),
            paused: None,
            weekly: WeeklySchedule {
                mon: Some(wd.clone()),
                tue: Some(wd.clone()),
                wed: Some(wd.clone()),
                thu: Some(wd.clone()),
                fri: Some(wd),
                sat: Some(we.clone()),
                sun: Some(we),
            },
            overrides: None,
            issued_at,
        }
    }

    /// The daily-cap budget clause.
    pub fn to_budget(&self, issued_at: u64) -> GrantBudget {
        GrantBudget {
            model: None,
            v: 1,
            tz: self.tz.clone(),
            daily_minutes: Some(self.daily_minutes),
            weekly_minutes: None,
            week_start: Some(WeekStart::Mon),
            paused: None,
            revoked: None,
            issued_at,
        }
    }
}

/// Materialize the limits into the clause store the enforcer reads. Idempotent:
/// `put_clause`'s monotonic `issued_at` makes a re-apply of unchanged limits a
/// no-op, and a newer `issued_at` (a fresh edit) supersedes. Returns `true` if
/// anything was newly written.
pub fn apply_device_limits<S: SystemLayer>(
    sys: &S,
    limits: &DeviceLimits,
    issued_at: u64,
) -> SysResult<bool> {
    let sched = serde_json::to_string(&limits.to_schedule(issued_at))
        .map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    let budget = serde_json::to_string(&limits.to_budget(issued_at))
        .map_err(|e| charter_sys::SysError::Io(e.to_string()))?;
    let a = sys
        .clauses()
        .put_clause(ClauseKind::Schedule.store_key(), issued_at, &sched)?;
    let b = sys
        .clauses()
        .put_clause(ClauseKind::Budget.store_key(), issued_at, &budget)?;
    Ok(a || b)
}
/// A managed child's on-disk config: an optional guardian **subject** binding
/// (the child == this Signet `dependantId`, 64-hex) and optional device-only
/// **limits** (the local fallback). A child may be *binding-only* (Signet-first,
/// no local fallback yet), *device-only* (the standalone path), or *both*.
///
/// The file is `<dir>/<username>.json`. A **legacy flat `DeviceLimits`** document
/// (no `subject`/`limits` keys) still loads as `{ subject: None, limits: Some }`
/// — back-compat with files the settings GUI already wrote.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChildConfig {
    /// Guardian subject pubkey hex (== Signet `dependantId`), if bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<String>,
    /// Device-only limits — the fallback when the guardian hasn't set this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<DeviceLimits>,
    /// Device-only learning apps — the fallback when no guardian learning
    /// clause exists AND the guardian doesn't time-govern this child.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub learning: Option<GrantLearning>,
}

/// `true` if `s` is a 64-char lowercase-or-uppercase hex string (a 32-byte
/// pubkey) — the shape of a valid subject binding.
pub fn valid_subject_hex(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Parse a per-child config from JSON, accepting BOTH the structured
/// `{subject?, limits?}` shape and a legacy flat `DeviceLimits` document. The
/// binding and the fallback are handled **independently**: an invalid `subject`
/// is dropped, and invalid `limits` are dropped — neither discards the other.
/// The whole config is `None` only when *nothing usable* remains (no valid
/// subject and no valid limits). This matters for fail-safety: a corrupt
/// device-only fallback must never knock a guardian-governed child out of
/// enforcement. The legacy flat path is taken only when the document carries
/// neither `subject` nor `limits` keys.
pub fn parse_child_config(text: &str) -> Option<ChildConfig> {
    // The structured form, parsed so `subject` and `limits` are INDEPENDENT: a
    // raw `limits` value (so even a type-broken `limits` — `42`, a string — does
    // not abort the whole deserialize and discard a valid `subject`).
    #[derive(Deserialize)]
    struct RawChildConfig {
        #[serde(default)]
        subject: Option<String>,
        #[serde(default)]
        limits: Option<serde_json::Value>,
        #[serde(default)]
        learning: Option<serde_json::Value>,
    }
    // Structured: detected by the presence of a `subject` and/or `limits` key.
    if let Ok(raw) = serde_json::from_str::<RawChildConfig>(text) {
        if raw.subject.is_some() || raw.limits.is_some() || raw.learning.is_some() {
            let subject = raw
                .subject
                .filter(|s| valid_subject_hex(s))
                .map(|s| s.to_lowercase());
            // Parse + validate limits independently; drop them if either fails,
            // but never let that drop the (valid) subject binding.
            let limits = raw
                .limits
                .and_then(|v| serde_json::from_value::<DeviceLimits>(v).ok())
                .filter(|l| l.validate().is_ok());
            // Learning is independent + fail-soft too (invalid → dropped, the
            // fail-closed direction: attribution charges screen without it).
            let learning = raw
                .learning
                .and_then(|v| GrantLearning::from_value(&v).ok());
            // Nothing usable survived -> not a config.
            if subject.is_none() && limits.is_none() && learning.is_none() {
                return None;
            }
            return Some(ChildConfig {
                subject,
                limits,
                learning,
            });
        }
    }
    // Legacy flat DeviceLimits (no subject/limits keys).
    let limits: DeviceLimits = serde_json::from_str(text).ok()?;
    limits.validate().ok()?;
    Some(ChildConfig {
        subject: None,
        limits: Some(limits),
        learning: None,
    })
}
#[cfg(test)]
mod learning_config_tests {
    use super::*;

    fn khan_json() -> serde_json::Value {
        serde_json::json!({
            "v": 1, "issuedAt": 1,
            "apps": [{"id": "khan-academy", "label": "Khan Academy", "kind": "site",
                       "url": "https://www.khanacademy.org/", "domains": ["khanacademy.org"]}]
        })
    }

    #[test]
    fn structured_config_carries_learning() {
        let text = serde_json::json!({
            "limits": {"tz": "UTC", "wake": "07:00", "bedtime": "20:00", "dailyMinutes": 60},
            "learning": khan_json(),
        })
        .to_string();
        let cfg = parse_child_config(&text).expect("parses");
        assert_eq!(cfg.learning.as_ref().unwrap().apps.len(), 1);
        assert!(cfg.limits.is_some());
    }

    #[test]
    fn invalid_learning_is_dropped_without_discarding_the_rest() {
        // Fail-soft doctrine: a corrupt learning blob must not knock out the
        // child's limits or binding.
        let text = serde_json::json!({
            "limits": {"tz": "UTC", "wake": "07:00", "bedtime": "20:00", "dailyMinutes": 60},
            "learning": {"v": 99, "bogus": true},
        })
        .to_string();
        let cfg = parse_child_config(&text).expect("parses");
        assert!(cfg.learning.is_none());
        assert!(cfg.limits.is_some());
    }

    #[test]
    fn learning_only_config_is_usable() {
        // A guardian-unbound child with ONLY learning apps set (no local
        // limits): still a valid config (learning is worth keeping alone).
        let text = serde_json::json!({ "learning": khan_json() }).to_string();
        let cfg = parse_child_config(&text).expect("parses");
        assert!(cfg.learning.is_some());
        assert!(cfg.limits.is_none() && cfg.subject.is_none());
    }
}

/// Members of `group` from a `getent group` document
/// (`name:passwd:gid:member,member`). The managed-children list for the UI.
pub fn parse_group_members(getent: &str, group: &str) -> Vec<String> {
    for line in getent.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.first() == Some(&group) {
            return f
                .get(3)
                .map(|m| {
                    m.split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect()
                })
                .unwrap_or_default();
        }
    }
    Vec::new()
}

/// Resolve a username to its uid from an `/etc/passwd` document
/// (`name:passwd:uid:gid:…`).
pub fn uid_for_user(passwd: &str, username: &str) -> Option<u32> {
    for line in passwd.lines() {
        let mut f = line.split(':');
        if f.next() == Some(username) {
            return f.nth(1).and_then(|u| u.parse().ok()); // skip passwd, take uid
        }
    }
    None
}

/// Home directory for `uid` from an `/etc/passwd` document — used to find the
/// active child's `.Xauthority` so the lock can draw on their session.
pub fn home_for_uid(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 6 && f[2].parse::<u32>().ok() == Some(uid) {
            return Some(f[5].to_string());
        }
    }
    None
}

/// Username for `uid` from an `/etc/passwd` document — used for readable
/// observe-mode logs (and, later, the STATUS feed).
pub fn user_for_uid(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 3 && f[2].parse::<u32>().ok() == Some(uid) {
            return Some(f[0].to_string());
        }
    }
    None
}
/// Build [`DeviceLimits`] from the settings form's fields
/// `[wake, bedtime, daily_minutes, weekend_wake?, weekend_bedtime?]` + the
/// detected `tz`. Pure, so the UI's parse is verified without a display.
pub fn parse_form_fields(fields: &[&str], tz: &str) -> Result<DeviceLimits, String> {
    let get = |i: usize| fields.get(i).map(|s| s.trim()).unwrap_or("");
    let daily_minutes: u32 = get(2)
        .parse()
        .map_err(|_| "daily minutes must be a whole number".to_string())?;
    let (ww, wb) = (get(3), get(4));
    let weekend = if ww.is_empty() && wb.is_empty() {
        None
    } else {
        Some(DayWindow {
            wake: ww.to_string(),
            bedtime: wb.to_string(),
        })
    };
    let limits = DeviceLimits {
        tz: tz.to_string(),
        wake: get(0).to_string(),
        bedtime: get(1).to_string(),
        daily_minutes,
        weekend,
    };
    limits.validate()?;
    Ok(limits)
}

/// Parse the settings form, inheriting the CURRENT value for any **blank** field.
/// zenity `--forms` can't pre-fill its entries, so the parent is shown the current
/// values (in the labels + prompt) and a blank field means "keep current" — they
/// edit only what they want to change, and leaving "daily minutes" blank no longer
/// errors. `tz` is always preserved from `current` (it isn't in the form).
pub fn parse_form_fields_over(
    fields: &[&str],
    current: &DeviceLimits,
) -> Result<DeviceLimits, String> {
    let get = |i: usize| fields.get(i).map(|s| s.trim()).unwrap_or("");
    let or_current = |i: usize, cur: &str| {
        let v = get(i);
        if v.is_empty() {
            cur.to_string()
        } else {
            v.to_string()
        }
    };
    let daily_minutes: u32 = if get(2).is_empty() {
        current.daily_minutes
    } else {
        get(2)
            .parse()
            .map_err(|_| "daily minutes must be a whole number".to_string())?
    };
    let (ww, wb) = (get(3), get(4));
    let weekend = if ww.is_empty() && wb.is_empty() {
        current.weekend.clone() // both blank → keep the current weekend rule
    } else {
        Some(DayWindow {
            wake: ww.to_string(),
            bedtime: wb.to_string(),
        })
    };
    let limits = DeviceLimits {
        tz: current.tz.clone(),
        wake: or_current(0, &current.wake),
        bedtime: or_current(1, &current.bedtime),
        daily_minutes,
        weekend,
    };
    limits.validate()?;
    Ok(limits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_proto::ClauseKind;
    use charter_sys::persistence::ClauseStore;
    use charter_sys::MockSystem;

    const HEX64: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    fn sample() -> DeviceLimits {
        DeviceLimits {
            tz: "Europe/London".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 120,
            weekend: Some(DayWindow {
                wake: "08:00".into(),
                bedtime: "21:00".into(),
            }),
        }
    }

    #[test]
    fn user_for_uid_reads_the_username_column() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                      bob:x:1001:1001:Bob:/home/bob:/bin/bash\n";
        assert_eq!(user_for_uid(passwd, 1001).as_deref(), Some("bob"));
        assert_eq!(user_for_uid(passwd, 4242), None);
    }

    #[test]
    fn parse_form_fields_over_keeps_current_for_blank_fields() {
        let current = DeviceLimits {
            tz: "Europe/London".into(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 120,
            weekend: None,
        };
        // Parent changed only the bedtime and left everything else blank — the
        // exact case that used to fail with "daily minutes must be a whole number".
        let fields = ["", "21:00", "", "", ""];
        let got = parse_form_fields_over(&fields, &current).unwrap();
        assert_eq!(got.wake, "07:00", "blank wake kept");
        assert_eq!(got.bedtime, "21:00", "edited bedtime applied");
        assert_eq!(got.daily_minutes, 120, "blank minutes kept (no error)");
        assert_eq!(got.tz, "Europe/London", "tz preserved");
        assert!(got.weekend.is_none(), "blank weekend keeps current (none)");

        // A non-blank, non-numeric daily-minutes still errors.
        let bad = ["", "", "abc", "", ""];
        assert!(parse_form_fields_over(&bad, &current).is_err());
    }
    #[test]
    fn hhmm_validation() {
        assert!(valid_hhmm("07:00") && valid_hhmm("23:59") && valid_hhmm("00:00"));
        assert!(!valid_hhmm("7:00") && !valid_hhmm("24:00") && !valid_hhmm("12:60"));
        assert!(!valid_hhmm("noon") && !valid_hhmm("12"));
    }

    #[test]
    fn validate_rejects_bad_limits() {
        let mut l = sample();
        assert!(l.validate().is_ok());
        l.daily_minutes = 0;
        assert!(l.validate().is_err());
        let mut l = sample();
        l.bedtime = "8pm".into();
        assert!(l.validate().is_err());
    }

    #[test]
    fn schedule_uses_weekday_and_weekend_windows() {
        let s = sample().to_schedule(100);
        assert_eq!(s.weekly.mon.as_ref().unwrap()[0].start, "07:00");
        assert_eq!(s.weekly.mon.as_ref().unwrap()[0].end, "20:00");
        // Saturday gets the weekend window.
        assert_eq!(s.weekly.sat.as_ref().unwrap()[0].start, "08:00");
        assert_eq!(s.weekly.sat.as_ref().unwrap()[0].end, "21:00");
        assert_eq!(s.tz, "Europe/London");
    }

    #[test]
    fn budget_carries_the_daily_cap() {
        let b = sample().to_budget(100);
        assert_eq!(b.daily_minutes, Some(120));
        assert_eq!(b.tz, "Europe/London");
    }

    #[test]
    fn parse_form_fields_builds_and_validates() {
        // Full form with a weekend window.
        let l = parse_form_fields(&["07:00", "20:00", "120", "08:00", "21:00"], "UTC").unwrap();
        assert_eq!(l.daily_minutes, 120);
        assert_eq!(l.weekend.as_ref().unwrap().wake, "08:00");
        // No weekend fields -> no weekend override.
        let l = parse_form_fields(&["07:00", "20:00", "90"], "UTC").unwrap();
        assert!(l.weekend.is_none());
        // Bad time / bad number are rejected (the UI shows the error).
        assert!(parse_form_fields(&["7am", "20:00", "120"], "UTC").is_err());
        assert!(parse_form_fields(&["07:00", "20:00", "lots"], "UTC").is_err());
    }

    #[test]
    fn uid_lookup_from_passwd() {
        let passwd = "root:x:0:0:root:/root:/bin/bash\nalice:x:1001:1001:Alice:/home/alice:/bin/bash\nbob:x:1002:1002::/home/bob:/bin/bash\n";
        assert_eq!(uid_for_user(passwd, "alice"), Some(1001));
        assert_eq!(uid_for_user(passwd, "bob"), Some(1002));
        assert_eq!(uid_for_user(passwd, "nobody"), None);
        assert_eq!(home_for_uid(passwd, 1001).as_deref(), Some("/home/alice"));
        assert_eq!(home_for_uid(passwd, 4242), None);
    }

    #[test]
    fn group_members_parse() {
        let g = "sudo:x:27:dad\ncharter-managed:x:990:alice,bob\nother:x:991:\n";
        assert_eq!(
            parse_group_members(g, "charter-managed"),
            vec!["alice", "bob"]
        );
        assert!(parse_group_members(g, "other").is_empty());
        assert!(parse_group_members(g, "missing").is_empty());
    }

    #[test]
    fn apply_writes_both_clauses_and_is_idempotent() {
        let sys = MockSystem::new(1000);
        assert!(apply_device_limits(&sys, &sample(), 100).unwrap());
        // The enforcer's clauses are now present + parseable as the real types.
        let sched_json = sys
            .clauses()
            .get_clause(ClauseKind::Schedule.store_key())
            .unwrap()
            .unwrap();
        assert!(serde_json::from_str::<GrantSchedule>(&sched_json).is_ok());
        let budget_json = sys
            .clauses()
            .get_clause(ClauseKind::Budget.store_key())
            .unwrap()
            .unwrap();
        assert!(serde_json::from_str::<GrantBudget>(&budget_json).is_ok());
        // Same issued_at -> rollback-protected no-op.
        assert!(!apply_device_limits(&sys, &sample(), 100).unwrap());
        // A newer edit supersedes.
        assert!(apply_device_limits(&sys, &sample(), 200).unwrap());
    }

    #[test]
    fn legacy_flat_file_parses_as_device_only() {
        // A file the settings GUI already wrote (a bare DeviceLimits) still loads.
        let flat = serde_json::to_string(&sample()).unwrap();
        let cfg = parse_child_config(&flat).expect("legacy flat parses");
        assert_eq!(cfg.subject, None);
        assert_eq!(cfg.limits.as_ref().unwrap().daily_minutes, 120);
    }

    #[test]
    fn structured_file_with_subject_and_limits() {
        let cfg = ChildConfig {
            subject: Some(HEX64.into()),
            limits: Some(sample()),
            learning: None,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back = parse_child_config(&json).unwrap();
        assert_eq!(back.subject.as_deref(), Some(HEX64));
        assert!(back.limits.is_some());
    }

    #[test]
    fn binding_only_file_has_subject_no_limits() {
        let json = format!(r#"{{"subject":"{HEX64}"}}"#);
        let cfg = parse_child_config(&json).unwrap();
        assert_eq!(cfg.subject.as_deref(), Some(HEX64));
        assert!(
            cfg.limits.is_none(),
            "binding-only: no device-only fallback"
        );
    }

    #[test]
    fn invalid_subject_hex_is_dropped_but_limits_kept() {
        let json = r#"{"subject":"not-hex","limits":{"tz":"UTC","wake":"07:00","bedtime":"20:00","dailyMinutes":60}}"#;
        let cfg = parse_child_config(json).unwrap();
        assert_eq!(cfg.subject, None, "ill-formed subject binding is ignored");
        assert_eq!(cfg.limits.as_ref().unwrap().daily_minutes, 60);
    }

    #[test]
    fn invalid_limits_keeps_valid_subject_binding() {
        // A corrupt device-only fallback (dailyMinutes=0 fails validate) must NOT
        // discard a valid guardian subject binding — else a guardian-governed
        // child silently drops out of enforcement entirely.
        let json = format!(
            r#"{{"subject":"{HEX64}","limits":{{"tz":"UTC","wake":"07:00","bedtime":"20:00","dailyMinutes":0}}}}"#
        );
        let cfg = parse_child_config(&json).expect("subject binding survives corrupt limits");
        assert_eq!(cfg.subject.as_deref(), Some(HEX64));
        assert!(
            cfg.limits.is_none(),
            "the invalid limits are dropped, binding kept"
        );
    }

    #[test]
    fn invalid_limits_and_no_subject_is_none() {
        // Nothing usable -> the whole config is dropped.
        let json = r#"{"limits":{"tz":"UTC","wake":"07:00","bedtime":"20:00","dailyMinutes":0}}"#;
        assert!(parse_child_config(json).is_none());
    }

    #[test]
    fn type_broken_limits_still_keeps_valid_subject_binding() {
        // Even a TYPE-broken limits (not a DeviceLimits object at all) must not
        // discard a valid guardian binding — limits are parsed independently.
        let json = format!(r#"{{"subject":"{HEX64}","limits":42}}"#);
        let cfg = parse_child_config(&json).expect("subject survives type-broken limits");
        assert_eq!(cfg.subject.as_deref(), Some(HEX64));
        assert!(cfg.limits.is_none());
    }
}
