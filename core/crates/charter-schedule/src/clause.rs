//! Clause data shapes: the runtime-canonical `GrantSchedule` / `GrantBudget`
//! (frozen in `spec/contract.md`) and the SDK-parity `ChartedClause` (used only
//! to cross-check the Rust evaluator against the live TS `evaluateSchedule`).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

// --- SDK-parity shapes (non-runtime; parity shim only) --------------------

/// One window in the SDK's `ChartedClause`. `endMinute <= startMinute` means
/// the window crosses midnight (a same-day quirk — see `evaluate_schedule`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduleWindow {
    pub days_of_week: Vec<u8>,
    pub start_minute: u32,
    pub end_minute: u32,
}

/// The SDK's parsed clause payload (parity shim).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChartedClause {
    pub schema_version: u32,
    pub kind: String,
    pub mechanism: String,
    pub windows: Vec<ScheduleWindow>,
    pub timezone: String,
    pub end_date: Option<i64>,
    pub revoked: bool,
}

// --- Runtime-canonical shapes (contract.md) -------------------------------

/// `"HH:MM"` window in the schedule tz; `start < end` (no midnight crossing —
/// split into two day-bound windows instead).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantScheduleWindow {
    pub start: String,
    pub end: String,
}

/// Recurring weekly windows.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WeeklySchedule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mon: Option<Vec<GrantScheduleWindow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tue: Option<Vec<GrantScheduleWindow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wed: Option<Vec<GrantScheduleWindow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thu: Option<Vec<GrantScheduleWindow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fri: Option<Vec<GrantScheduleWindow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sat: Option<Vec<GrantScheduleWindow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sun: Option<Vec<GrantScheduleWindow>>,
}

impl WeeklySchedule {
    /// Windows for a weekday (`0=Sun..6=Sat`).
    pub fn for_day(&self, day: u32) -> Option<&Vec<GrantScheduleWindow>> {
        match day {
            0 => self.sun.as_ref(),
            1 => self.mon.as_ref(),
            2 => self.tue.as_ref(),
            3 => self.wed.as_ref(),
            4 => self.thu.as_ref(),
            5 => self.fri.as_ref(),
            6 => self.sat.as_ref(),
            _ => None,
        }
    }

    /// Whether any day defines windows.
    pub fn is_empty(&self) -> bool {
        [
            &self.mon, &self.tue, &self.wed, &self.thu, &self.fri, &self.sat, &self.sun,
        ]
        .iter()
        .all(|d| d.as_ref().map(|w| w.is_empty()).unwrap_or(true))
    }
}

/// The runtime-canonical schedule clause (the device authority).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantSchedule {
    pub v: u32,
    pub tz: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(default)]
    pub weekly: WeeklySchedule,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overrides: Option<BTreeMap<String, Vec<GrantScheduleWindow>>>,
    pub issued_at: u64,
}

/// The frozen `appRules` clause body version.
pub const APP_RULES_VERSION: u32 = 1;

/// One per-app rule: block a specific app/game outright, and/or constrain it to
/// its own allowed hours. Keyed by `pkg` (Android package id, a Linux exec
/// path / flatpak id, or a Linux `cmdline:` id — see [`is_cmdline_id`]) — the
/// enforcer's identity for the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRule {
    /// The app's on-device identity (Android package / Linux exec, flatpak, or
    /// `cmdline:` id).
    pub pkg: String,
    /// Display name (guardian-facing); advisory only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Blocked outright — the app may never open, regardless of schedule.
    #[serde(default)]
    pub blocked: bool,
    /// Optional per-app allowed-hours. When present, the app is usable only
    /// INSIDE these windows (on top of the whole-device schedule). Absent = no
    /// per-app time constraint. Reuses the device `GrantSchedule` shape so the
    /// same tz-aware evaluator governs it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedule: Option<GrantSchedule>,
}

/// The `cmdline:` identity-form prefix — see [`is_cmdline_id`].
pub const CMDLINE_ID_PREFIX: &str = "cmdline:";

/// The minimum length, in bytes, a `cmdline:` needle may carry. Below this a
/// substring would match almost any running process's command line, turning a
/// rule meant to name one game into an accidental blanket one — see
/// [`cmdline_needle`].
pub const MIN_CMDLINE_NEEDLE: usize = 8;

/// Whether `pkg` is the third `AppRule`/`AppBucket` identity form: not an exec
/// path or a flatpak app-id, but a substring to find in the RUNNING PROCESS's
/// own command line. `cmdline:net.minecraft.client.main.Main` names
/// Minecraft's actual JVM main class rather than whichever launcher (the
/// official one, Prism, MultiMC, ATLauncher, Modrinth, or none at all —
/// `java -jar` directly) happened to start it: the launcher can be renamed,
/// swapped, or skipped; the class name cannot.
///
/// Pure vocabulary recognition only. Matching a running process against the
/// extracted needle is the enforcer's job (Linux `pkg_matches_process`,
/// alongside the existing flatpak/path forms), deliberately not repeated
/// here.
pub fn is_cmdline_id(pkg: &str) -> bool {
    pkg.starts_with(CMDLINE_ID_PREFIX)
}

/// The substring a `cmdline:` identity asks the enforcer to find in a
/// process's command line, or `None` if `pkg` is not a well-formed one.
///
/// The needle is TRIMMED before the length floor is applied — a clause
/// authored as `cmdline:  net.minecraft.client.main.Main` (stray leading
/// whitespace) would otherwise return the untrimmed needle, which then
/// matches NO real argv token (a token is never itself padded with the
/// clause author's whitespace) and NEVER matches the joined line either
/// unless a real command line happens to carry the same padding. That is a
/// silent, total non-match on an apparently well-formed rule — precisely the
/// class of failure this whole feature exists to kill, so trimming happens
/// here, once, rather than trusting every caller to remember it.
///
/// `None` covers: the prefix is absent; the substring is empty; the substring
/// is whitespace only; or it is shorter than [`MIN_CMDLINE_NEEDLE`] AFTER
/// trimming. All four are "too thin to mean anything" in the same sense — a
/// needle of a handful of characters (or none) would match nearly every
/// process on the machine, turning a rule meant to name one game into one
/// that silently covers unrelated software.
///
/// Returning `None` (no match) is the FAIL-OPEN direction, which is the
/// correct one here even though `cmdline:` is most often used to cap or
/// block: this helper only recognises the identity, it never decides
/// enforcement, and a malformed clause already fails this same way elsewhere
/// in this module (e.g. [`GrantBuckets::is_valid`]) — an error must never
/// manufacture a match nobody authored.
pub fn cmdline_needle(pkg: &str) -> Option<&str> {
    let needle = pkg.strip_prefix(CMDLINE_ID_PREFIX)?.trim();
    if needle.len() < MIN_CMDLINE_NEEDLE {
        return None;
    }
    Some(needle)
}

/// The `site:` identity-form prefix — see [`is_site_id`].
pub const SITE_ID_PREFIX: &str = "site:";

/// Whether `pkg` is the fourth `AppRule`/`AppBucket` identity form: one of the
/// guardian's own **site apps**, named by its `learning`-clause id —
/// `site:khan-academy`, `site:youtube`.
///
/// # Why this form has to exist
///
/// A site app's process is Chromium. Its exec path is the same Chromium every
/// other site app runs as, so the path form cannot tell two of them apart, and
/// a `cmdline:` needle aimed at the `--class=charter-<id>` marker would match
/// a forgery just as happily as the real launcher — the marker is only the
/// meter, never the boundary (see `charterd::site_app`).
///
/// So `site:` resolves through the SANCTIONED-LAUNCH predicate instead: the
/// process's whole argument list must be exactly what the device rendered for
/// that one site app. Same predicate as the kill sweep, so what is metered is
/// exactly what is stopped.
///
/// # Why it is not just "the learning clause"
///
/// Until now a site app could only ever be FREE, because one clause both
/// *defined* the pinned window and *made it free*. This form is what takes
/// those apart: `learning` keeps defining the windows, and whether one costs
/// is decided by the buckets clause like any other app. "YouTube is half an
/// hour a day" is unsayable without it.
///
/// Pure vocabulary recognition only — resolving it against a running process
/// needs the child's site-app catalogue and is the enforcer's job.
pub fn is_site_id(pkg: &str) -> bool {
    pkg.starts_with(SITE_ID_PREFIX)
}

/// The site-app id a `site:` identity names, or `None` if `pkg` is not a
/// well-formed one (absent prefix, or an empty/whitespace-only id).
///
/// Trimmed for the same reason [`cmdline_needle`] trims: an id padded by the
/// clause author would otherwise match no catalogue entry at all and fail
/// silently on an apparently well-formed rule.
///
/// There is no minimum length here, unlike `cmdline:`. That floor exists
/// because a short substring matches half the process table; a site id is
/// compared for EQUALITY against the guardian's own catalogue, so a short one
/// is merely a short name and cannot be accidentally broad.
pub fn site_id(pkg: &str) -> Option<&str> {
    let id = pkg.strip_prefix(SITE_ID_PREFIX)?.trim();
    (!id.is_empty()).then_some(id)
}

/// The `appRules` clause body: the guardian's FULL per-app rule set for one
/// child. Replace-the-set semantics (like `learning`) — one clause carries every
/// rule, so it reuses the per-kind monotonic `issuedAt` rollback protection
/// without any per-app store keying. A standing CLAUSE, inert until issued.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantAppRules {
    pub v: u32,
    #[serde(default)]
    pub rules: Vec<AppRule>,
    pub issued_at: u64,
}

/// The week-start day for weekly budget reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WeekStart {
    Sun,
    #[default]
    Mon,
}

/// The runtime-canonical, **device-enforced** budget clause (frozen here +
/// `spec/contract.md`). `paused` ⇒ 0; `revoked` ⇒ no constraint; absent caps
/// unconstrained.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantBudget {
    pub v: u32,
    pub tz: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_minutes: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_start: Option<WeekStart>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked: Option<bool>,
    /// WHAT the allowance is spent on. Absent means [`TimeModel::Session`] —
    /// every budget clause signed before this field existed keeps its exact
    /// meaning, so no ward changes behaviour on upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<TimeModel>,
    pub issued_at: u64,
}

/// What a child's daily/weekly allowance is actually spent on.
///
/// # Why this exists
///
/// The original model charges for *being logged in*, and then bolts two
/// opposing corrections onto it: the `learning` clause WAIVES a second, and a
/// bucket CHARGES one alongside. Three rules adjudicating the same second, all
/// decided from a single focused window, is unexplainable to the family living
/// under it — and the waiver is the only mechanism in the system that creates
/// time out of nothing, which is why it is where every serious attack has been
/// (forged class markers, `flatpak install --user` impostors, bind-mounted
/// root ownership).
///
/// `Named` deletes the waiver rather than hardening it further. Free stops
/// being something Charter grants and becomes the absence of anything that
/// costs — and an absence cannot be forged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TimeModel {
    /// Being at the device costs. Everything is charged unless a live
    /// `learning` clause waives it. The original model, and the default for
    /// every clause that does not name one.
    Session,
    /// Only NAMED apps cost. Being at the device is free; a costing app is
    /// charged while it is open, several open at once are charged once, and
    /// nothing open costs nothing.
    Named,
}

impl TimeModel {
    /// The model a budget clause selects. Absent means [`TimeModel::Session`]
    /// — see [`GrantBudget::model`].
    pub fn of(budget: Option<&GrantBudget>) -> TimeModel {
        budget.and_then(|b| b.model).unwrap_or(TimeModel::Session)
    }
}

/// The frozen `buckets` clause body version (daily-only allowances).
pub const BUCKETS_VERSION: u32 = 1;

/// The `buckets` clause body version once a bucket may carry a weekly
/// allowance (daily-only, weekly-only, or both). Superset of `v:1` on the
/// wire — old bodies keep parsing verbatim; this const exists only so
/// `is_valid` can accept both.
pub const BUCKETS_VERSION_WEEKLY: u32 = 2;

/// Sanity bounds. A bucket capped at 0 minutes would be a silent permanent
/// block dressed up as an allowance; one over a full day (or full week, for
/// the weekly axis) is not a cap at all.
pub const MAX_BUCKETS: usize = 12;
pub const MAX_BUCKET_APPS: usize = 64;
pub const MAX_BUCKET_MINUTES: u16 = 1440;
pub const MAX_BUCKET_WEEK_MINUTES: u16 = 10080;

/// One named app bucket with its own allowance ("Play", 60 min/day, or 5h/week,
/// or both — whichever axis is set closes the bucket first).
///
/// Members carry the SAME identity vocabulary as [`AppRule::pkg`] — an Android
/// package id, or a Linux exec path / flatpak id — so a bucket listing a
/// laptop's games never matches anything on a phone. That is how "limit this
/// on his laptop only" works without any per-device setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppBucket {
    /// Stable `[a-z0-9-]` slug — the key the device's day/week counters live
    /// under.
    pub id: String,
    /// The name the family uses for it.
    pub label: String,
    /// On-device identities of the apps in this bucket.
    #[serde(default)]
    pub apps: Vec<String>,
    /// The bucket's daily allowance, in minutes. `None` = no daily cap; at
    /// least one of `daily_minutes` / `weekly_minutes` must be set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_minutes: Option<u16>,
    /// The bucket's weekly allowance, in minutes. `None` = no weekly cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weekly_minutes: Option<u16>,
}

/// The `buckets` clause body: named app buckets, each with a daily and/or
/// weekly allowance.
///
/// Generalises the `learning` bucket. Where learning time is FREE (it never
/// drains the day), a bucket here is CAPPED — and spending it closes THAT
/// BUCKET only, never the device. The ward keeps the rest of their day; only
/// the bucket's own apps stop. That is the difference between "games are done
/// for today" and "your computer is confiscated".
///
/// Replace-the-set (like `appRules`): one clause carries every bucket, so it
/// reuses the per-kind monotonic `issuedAt` rollback protection with no
/// per-bucket store keying.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantBuckets {
    pub v: u32,
    #[serde(default)]
    pub buckets: Vec<AppBucket>,
    /// Lift every bucket (nothing is capped) without deleting the set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    /// The week-start day the weekly axis resets on. `None` defaults to
    /// [`WeekStart::Mon`] — same discipline as `budget`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub week_start: Option<WeekStart>,
    /// IANA tz the day/week boundary is computed in — same discipline as `budget`.
    #[serde(default)]
    pub tz: String,
    pub issued_at: u64,
}

impl GrantBuckets {
    pub fn is_paused(&self) -> bool {
        self.paused.unwrap_or(false)
    }

    /// Fail-safe in the direction a CAP demands: a malformed body must cap
    /// NOTHING rather than block everything. Callers treat `false` as "no
    /// buckets in force", so a garbage clause can never confiscate an app the
    /// ward is entitled to. (`learning` fails the opposite way — to Screen —
    /// because there an error must never make time free. Both refuse to let an
    /// error quietly favour enforcement.)
    pub fn is_valid(&self) -> bool {
        if (self.v != BUCKETS_VERSION && self.v != BUCKETS_VERSION_WEEKLY)
            || self.buckets.len() > MAX_BUCKETS
        {
            return false;
        }
        let mut seen = std::collections::BTreeSet::new();
        for b in &self.buckets {
            if !valid_bucket_id(&b.id) || !seen.insert(b.id.as_str()) {
                return false;
            }
            if b.label.trim().is_empty() || b.label.chars().count() > 32 {
                return false;
            }
            if b.apps.is_empty() || b.apps.len() > MAX_BUCKET_APPS {
                return false;
            }
            if b.apps.iter().any(|a| a.trim().is_empty()) {
                return false;
            }
            if b.daily_minutes.is_none() && b.weekly_minutes.is_none() {
                return false;
            }
            if b.daily_minutes
                .is_some_and(|m| m == 0 || m > MAX_BUCKET_MINUTES)
            {
                return false;
            }
            if b.weekly_minutes
                .is_some_and(|m| m == 0 || m > MAX_BUCKET_WEEK_MINUTES)
            {
                return false;
            }
        }
        true
    }
}

/// `[a-z0-9-]`, non-empty, bounded — the id keys a durable day counter, so it
/// must be stable and safe to use as a store key.
fn valid_bucket_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 40
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod cmdline_id_tests {
    use super::*;

    #[test]
    fn cmdline_id_is_recognised_by_its_prefix() {
        assert!(is_cmdline_id("cmdline:net.minecraft.client.main.Main"));
        assert!(!is_cmdline_id("/usr/bin/java"));
        assert!(!is_cmdline_id("org.x.App"));
    }

    #[test]
    fn cmdline_needle_extracts_the_substring_after_the_prefix() {
        assert_eq!(
            cmdline_needle("cmdline:net.minecraft.client.main.Main"),
            Some("net.minecraft.client.main.Main")
        );
    }

    #[test]
    fn site_id_extracts_the_id_after_the_prefix() {
        assert_eq!(site_id("site:khan-academy"), Some("khan-academy"));
        assert!(is_site_id("site:khan-academy"));
    }

    #[test]
    fn site_id_rejects_an_empty_or_whitespace_only_id() {
        assert_eq!(site_id("site:"), None);
        assert_eq!(site_id("site:   "), None);
    }

    /// The same paste-artifact protection `cmdline_needle` has. An id padded
    /// by the clause author would match no catalogue entry at all, which is a
    /// silent total non-match on an apparently well-formed rule.
    #[test]
    fn site_id_trims_surrounding_whitespace() {
        assert_eq!(site_id("site:  youtube  "), Some("youtube"));
    }

    /// Unlike `cmdline:`, a SHORT site id is perfectly valid — the floor on
    /// `cmdline:` exists because a short substring matches half the process
    /// table, whereas a site id is compared for equality against the
    /// guardian's own catalogue and so cannot be accidentally broad.
    #[test]
    fn a_short_site_id_is_accepted() {
        assert_eq!(site_id("site:bbc"), Some("bbc"));
    }

    /// The ordering hazard, asserted rather than assumed. `is_flatpak_id` on
    /// the enforcer side is "no slash AND has a dot", which a site id like
    /// `site:my.school` satisfies — so the enforcer must test `is_site_id`
    /// FIRST, exactly as it already must for `cmdline:`. This test pins the
    /// vocabulary fact that makes that ordering necessary.
    #[test]
    fn a_site_id_may_contain_a_dot_and_so_must_be_tested_before_the_flatpak_form() {
        assert!(is_site_id("site:my.school"));
        assert_eq!(site_id("site:my.school"), Some("my.school"));
        // No slash, has a dot — indistinguishable from a flatpak app id if the
        // `site:` arm were checked second.
        let bare = "my.school";
        assert!(!bare.contains('/') && bare.contains('.'));
    }

    #[test]
    fn a_non_site_pkg_is_not_a_site_id() {
        assert!(!is_site_id("/usr/bin/firefox"));
        assert!(!is_site_id("org.kde.gcompris"));
        assert!(!is_site_id("cmdline:net.minecraft.client.main.Main"));
        assert_eq!(site_id("/usr/bin/firefox"), None);
    }

    #[test]
    fn cmdline_needle_rejects_a_needle_shorter_than_the_minimum() {
        // "short" is 5 bytes — below MIN_CMDLINE_NEEDLE (8).
        assert_eq!(cmdline_needle("cmdline:short"), None);
    }

    #[test]
    fn cmdline_needle_rejects_an_empty_needle() {
        assert_eq!(cmdline_needle("cmdline:"), None);
    }

    #[test]
    fn cmdline_needle_rejects_a_whitespace_only_needle() {
        // Eight spaces clears the length floor but carries no real substring
        // — trimming, not just length, decides "thin enough to reject".
        assert_eq!(cmdline_needle("cmdline:        "), None);
    }

    /// A clause authored with stray whitespace around the needle (a paste
    /// artifact, an editing slip) must not become a silent never-match: the
    /// returned needle is trimmed, so it still finds the real argv token,
    /// which carries none of the clause author's padding.
    #[test]
    fn cmdline_needle_trims_surrounding_whitespace() {
        assert_eq!(
            cmdline_needle("cmdline:  net.minecraft.client.main.Main  "),
            Some("net.minecraft.client.main.Main")
        );
        // Trimming happens BEFORE the length floor: padding out a too-short
        // needle must not smuggle it past the minimum.
        assert_eq!(cmdline_needle("cmdline:   short   "), None);
    }

    #[test]
    fn cmdline_needle_is_none_without_the_prefix() {
        assert_eq!(cmdline_needle("/usr/bin/java"), None);
        assert_eq!(cmdline_needle("org.x.App"), None);
    }

    /// The three identity forms must partition the vocabulary: a
    /// flatpak-looking id is never read as a `cmdline:` id and vice versa; a
    /// native exec path is neither.
    #[test]
    fn flatpak_and_path_forms_are_never_read_as_cmdline_ids() {
        assert!(!is_cmdline_id("org.mozilla.firefox"));
        assert!(!is_cmdline_id("/usr/bin/firefox"));
        assert!(cmdline_needle("org.mozilla.firefox").is_none());
        assert!(cmdline_needle("/usr/bin/firefox").is_none());
    }

    /// A subtlety worth pinning explicitly: the flatpak heuristic elsewhere
    /// in the codebase (`charterd::app_rules::is_flatpak_id` — no path
    /// separator, at least one dot) does not itself know about the
    /// `cmdline:` prefix. `cmdline:net.minecraft.client.main.Main` has no `/`
    /// and does have a `.`, so it would ALSO pass that heuristic — meaning a
    /// `cmdline:` id must be checked (and consumed) before the flatpak form
    /// is ever considered, not just recognised in isolation. If this
    /// assertion ever fails, the flatpak heuristic changed shape and the
    /// ordering requirement on callers should be re-examined.
    #[test]
    fn a_cmdline_id_would_otherwise_pass_the_flatpak_shape_heuristic() {
        let looks_flatpak_shaped = |s: &str| !s.contains('/') && s.contains('.');
        let cmdline_id = "cmdline:net.minecraft.client.main.Main";

        assert!(is_cmdline_id(cmdline_id));
        assert!(looks_flatpak_shaped(cmdline_id));
    }
}
