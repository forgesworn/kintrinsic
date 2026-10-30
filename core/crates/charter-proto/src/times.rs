//! `times` clause body (v1): a child's NAMED TIMES — kinds of screen time with
//! their own posture ("Learning time: only Khan Academy and YouTube"). Free
//! time is the background and has NO entry here: it IS the standing `apps` /
//! `content` / `apprules` / `buckets` / `learning` clauses, and an untagged
//! schedule window runs it. A named time is run by a schedule window tagged
//! with its `id`, or by a gift / `time.extend` carrying its `id`.
//!
//! This module holds the wire shape, its fail-closed validation, and the PURE
//! composition of a named time over Free time's resolved standing policy
//! ([`compose_apps`], [`compose_content`]) — the one definition both wardens
//! call. Evaluation of "which time is it now" lives beside the schedule
//! evaluator (`charter_schedule::times`). Normative text: `spec/contract.md`
//! §Times; rationale: `docs/times-design.md`.

use serde::{Deserialize, Serialize};

use charter_content::domain::normalize_domain;
use charter_content::{AgeTier, GrantContent, Posture, CONTENT_VERSION};

use crate::apps::{AppPosture, GrantApps, APPS_VERSION};
use crate::error::ProtoError;
use crate::learning::{validate_app, LearningApp, LearningAppKind};
use crate::params::{MAX_NAMED_TIMES, MAX_TIME_LABEL_LEN};

/// The frozen `times` clause body version.
pub const TIMES_VERSION: u32 = 1;

/// The id Free time answers to wherever a time id is displayed or routed. It
/// is reserved: no named time may take it, so "free" can never be ambiguous
/// between the background and a definition.
pub const FREE_TIME_ID: &str = "free";

/// Longest named-time id, in bytes (`^[a-z0-9-]{1,32}$`).
pub const MAX_TIME_ID_LEN: usize = 32;

/// How a named time reads the device's standing policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimePosture {
    /// Free time plus departures either way (ban more, un-ban some).
    #[serde(rename = "everythingExcept")]
    EverythingExcept,
    /// An allow-list of apps and sites; Free time's lists are not consulted.
    #[serde(rename = "onlyThese")]
    OnlyThese,
}

/// One named time. Fields belonging to the posture NOT in force are ignored
/// (kept on parse so a clause round-trips, never read by [`compose_apps`] /
/// [`compose_content`] and never validated beyond their JSON types).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NamedTime {
    /// `^[a-z0-9-]{1,32}$`, unique within the clause, never `"free"`.
    pub id: String,
    /// Display name, 1..=[`MAX_TIME_LABEL_LEN`] characters.
    pub label: String,
    pub posture: TimePosture,
    /// everythingExcept: additionally blocked apps (`AppRule.pkg` vocabulary).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub block_apps: Vec<String>,
    /// everythingExcept: removed from Free time's `apps.blocked`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unblock_apps: Vec<String>,
    /// everythingExcept: additionally denied registrable domains.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_sites: Vec<String>,
    /// everythingExcept: removed from Free time's `parentDeny` / category blocks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub undeny_sites: Vec<String>,
    /// onlyThese: the only apps that run (plus the device's implied web runtime
    /// when `sites` is non-empty).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<String>,
    /// onlyThese: the only sites that resolve — the `learning` clause's own
    /// shape, kind `site` only, so Linux can pin and launch each and Android
    /// allowlists the union of every `domains`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sites: Vec<LearningApp>,
    /// Absent = false: time here drains neither the daily/weekly budget nor any
    /// bucket, and budget exhaustion does not lock this time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts_against_budget: Option<bool>,
}

impl NamedTime {
    /// Whether usage in this time is charged to the budget and buckets.
    /// Absent means NOT counted — the design's default (a homework hour should
    /// not eat the play allowance).
    pub fn counts_against_budget(&self) -> bool {
        self.counts_against_budget.unwrap_or(false)
    }

    /// An `onlyThese` time with neither `apps` nor `sites`: valid on the wire,
    /// runs nothing (guardian surfaces refuse to save one).
    pub fn runs_nothing(&self) -> bool {
        self.posture == TimePosture::OnlyThese && self.apps.is_empty() && self.sites.is_empty()
    }

    /// The union of every site's domain closure, in clause order, de-duplicated
    /// (case-insensitively). Meaningful for `onlyThese` only.
    pub fn site_domains(&self) -> Vec<String> {
        dedup_by_key(
            self.sites.iter().flat_map(|s| s.domains.iter().cloned()),
            normalize_domain,
        )
    }
}

/// The `times` clause body. Replace-the-set, per child, LWW by `issuedAt`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantTimes {
    pub v: u32,
    pub times: Vec<NamedTime>,
    pub issued_at: u64,
}

fn valid_time_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_TIME_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

impl GrantTimes {
    /// Parse from a clause body JSON value, FAIL-CLOSED: any error means the
    /// whole clause is unusable, and the caller must treat it as ABSENT — every
    /// tagged schedule window then locks and untagged windows are unaffected
    /// (the fail-safe direction: a named window is a restriction relative to
    /// Free, so a clause we cannot read must never open one as Free).
    ///
    /// Checks: `v` equals [`TIMES_VERSION`] (a higher `v` may mean something
    /// else entirely, so it is refused rather than read as v1); at most
    /// [`MAX_NAMED_TIMES`] times; ids match `^[a-z0-9-]{1,32}$`, are unique and
    /// are not `"free"`; labels are 1..=[`MAX_TIME_LABEL_LEN`] characters; and,
    /// for an `onlyThese` time, every `sites` entry is a valid `learning`
    /// SITE app (id slug, `https://` url, non-empty hostname-only domains —
    /// the very validation the `learning` clause applies). The fields of the
    /// posture NOT in force are ignored.
    pub fn from_value(v: &serde_json::Value) -> Result<GrantTimes, ProtoError> {
        let g: GrantTimes =
            serde_json::from_value(v.clone()).map_err(|e| ProtoError::BadParams(e.to_string()))?;
        if g.v != TIMES_VERSION {
            return Err(ProtoError::BadVersion(g.v));
        }
        g.validate()?;
        Ok(g)
    }

    /// The enforcement-side entry point: a clause body string to the clause in
    /// force, or `None` — "absent" — when it does not parse or validate. Every
    /// consumer that must fail safe should call this rather than invent its own
    /// reading of an error.
    pub fn parse_or_absent(json: &str) -> Option<GrantTimes> {
        let v: serde_json::Value = serde_json::from_str(json).ok()?;
        GrantTimes::from_value(&v).ok()
    }

    /// The structural checks of [`from_value`](Self::from_value), for a clause
    /// built in code.
    pub fn validate(&self) -> Result<(), ProtoError> {
        if self.times.len() > MAX_NAMED_TIMES {
            return Err(ProtoError::BadParams(format!(
                "times clause carries {} times (max {MAX_NAMED_TIMES})",
                self.times.len()
            )));
        }
        let mut seen: Vec<&str> = Vec::with_capacity(self.times.len());
        for t in &self.times {
            if !valid_time_id(&t.id) {
                return Err(ProtoError::BadParams(format!(
                    "time id {:?} is not [a-z0-9-]{{1,{MAX_TIME_ID_LEN}}}",
                    t.id
                )));
            }
            if t.id == FREE_TIME_ID {
                return Err(ProtoError::BadParams(
                    "time id \"free\" is reserved for Free time".into(),
                ));
            }
            if seen.contains(&t.id.as_str()) {
                return Err(ProtoError::BadParams(format!(
                    "duplicate time id {:?}",
                    t.id
                )));
            }
            seen.push(&t.id);
            let n = t.label.chars().count();
            if n == 0 || n > MAX_TIME_LABEL_LEN {
                return Err(ProtoError::BadParams(format!(
                    "time {:?} label must be 1..={MAX_TIME_LABEL_LEN} characters",
                    t.id
                )));
            }
            if t.posture == TimePosture::OnlyThese {
                for site in &t.sites {
                    if site.kind != LearningAppKind::Site {
                        return Err(ProtoError::BadParams(format!(
                            "time {:?} site {:?} must be kind \"site\"",
                            t.id, site.id
                        )));
                    }
                    validate_app(site)?;
                }
            }
        }
        Ok(())
    }

    /// The named time carrying `id`, if the clause defines one.
    pub fn get(&self, id: &str) -> Option<&NamedTime> {
        self.times.iter().find(|t| t.id == id)
    }

    /// Every defined time id, in clause order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.times.iter().map(|t| t.id.as_str())
    }
}

// --- composition -----------------------------------------------------------

/// Order-preserving de-duplication under a normalising key (first wins).
fn dedup_by_key<I, K, F>(items: I, key: F) -> Vec<String>
where
    I: IntoIterator<Item = String>,
    K: PartialEq,
    F: Fn(&str) -> K,
{
    let mut keys: Vec<K> = Vec::new();
    let mut out: Vec<String> = Vec::new();
    for s in items {
        let k = key(&s);
        if !keys.contains(&k) {
            keys.push(k);
            out.push(s);
        }
    }
    out
}

fn dedup(items: impl IntoIterator<Item = String>) -> Vec<String> {
    dedup_by_key(items, |s| s.to_string())
}

/// Free time's `apps` policy as `t` modifies it. `free_resolved` is the
/// clause AFTER `effective_at(now)` (holds already folded in); holds are not
/// re-applied here.
///
/// - `everythingExcept`: `blocked := (blocked ∪ blockApps) \ unblockApps`
///   under Free's own posture. Under an `allowlist` Free posture `blocked` is
///   not the list the device reads, so the same departures are ALSO applied to
///   `allowed` (a ban removes from it, an un-ban adds to it) — the way a hold
///   is folded — rather than leaving the time's bans silently inert. Anything
///   un-banned also leaves `askFirst`, which must stay a subset of `blocked`.
///   All else carries unchanged — with ONE exception: a time's own additions
///   are never inert. If Free's `apps` is `paused` (its lists switched off by
///   the guardian) and the time has `blockApps`, the composed clause is
///   UN-paused as a blocklist with `blocked := blockApps` only: Free's lists
///   stay off, as intended, but the time's own bans hold. Carrying `paused`
///   would silently discard a ban the guardian wrote for this time (the
///   fail-open direction). With no `blockApps` a paused clause carries.
/// - `onlyThese`: `posture := allowlist`, `allowed := apps`, `holds` emptied,
///   and `paused` CLEARED. `paused` lifts the whole per-app policy, so
///   carrying it would run every app inside a time that promised to run only
///   these: the fail-OPEN direction, closed here.
///
/// `apprules`, `buckets`, `hidden` and `alwaysavailable` are not touched (and
/// `hidden` rides through unchanged): a named time can never loosen them.
pub fn compose_apps(free_resolved: &GrantApps, t: &NamedTime) -> GrantApps {
    let mut out = free_resolved.clone();
    match t.posture {
        TimePosture::EverythingExcept if free_resolved.is_paused() && !t.block_apps.is_empty() => {
            out.paused = None;
            out.posture = AppPosture::Blocklist;
            out.blocked = dedup(t.block_apps.iter().cloned());
            out.allowed = Vec::new();
            out.ask_first = Vec::new();
        }
        TimePosture::EverythingExcept => {
            out.blocked = dedup(
                free_resolved
                    .blocked
                    .iter()
                    .chain(t.block_apps.iter())
                    .filter(|p| !t.unblock_apps.contains(p))
                    .cloned(),
            );
            if free_resolved.posture == AppPosture::Allowlist {
                out.allowed = dedup(
                    free_resolved
                        .allowed
                        .iter()
                        .filter(|p| !t.block_apps.contains(p))
                        .chain(t.unblock_apps.iter())
                        .cloned(),
                );
            }
            let blocked = out.blocked.clone();
            out.ask_first.retain(|p| blocked.contains(p));
        }
        TimePosture::OnlyThese => {
            out.posture = AppPosture::Allowlist;
            out.allowed = dedup(t.apps.iter().cloned());
            out.holds = Vec::new();
            out.paused = None;
            // Not consulted under an allowlist, but a stale Free list left in
            // a clause that now claims `allowlist` invites a reader to apply it.
            out.blocked = Vec::new();
            out.ask_first = Vec::new();
        }
    }
    out
}

/// A content clause for a child whose Free time has none, so a time's own web
/// departures have something to land on. `ageTier` is carried by the wire
/// shape but read by nothing in evaluation; `young` is the cautious label.
fn synthetic_content(posture: Posture) -> GrantContent {
    GrantContent {
        v: CONTENT_VERSION,
        tz: None,
        posture,
        age_tier: AgeTier::Young,
        curators: Vec::new(),
        quorum_n: None,
        block_categories: Vec::new(),
        safe_search: None,
        youtube_restrict: None,
        parent_allow: Vec::new(),
        parent_deny: Vec::new(),
        paused: None,
        revoked: None,
        issued_at: 0,
    }
}

/// Free time's `content` policy as `t` modifies it (`None` = Free time has no
/// content clause).
///
/// - `everythingExcept`: `parentDeny := (parentDeny ∪ denySites) \
///   undenySites`; `parentAllow := parentAllow ∪ undenySites`. Domains compare
///   in their normalised form (scheme, `www.`, case, path), so un-denying
///   `youtube.com` removes `https://www.YouTube.com/`. With no departures the
///   Free clause is returned as it is. When Free time has no content clause
///   and the time DENIES sites, a blocklist clause is synthesised to carry the
///   ban: returning `None` would silently drop a ban the guardian wrote.
///   (Nothing to un-deny ⇒ `None` stays `None`.) The same holds when Free's
///   clause is `revoked` ("unrestricted") and the time has `denySites`: the
///   time's bans must hold, so a blocklist clause is synthesised carrying
///   them (Free's lists stay off, `revoked` cleared) rather than composing
///   onto a clause that would ignore them. A Free clause whose `v` this build
///   does not implement is NOT treated as revoked: it reads as unreadable and
///   stays locked under the time too. A `paused` Free clause (block ALL
///   web) carries unchanged — it is the more restrictive state.
/// - `onlyThese`: with `sites` non-empty, `posture := allowlist`,
///   `curators := []`, `parentAllow := ⋃ sites[].domains`, SafeSearch and
///   YouTube-restrict carried from Free (a synthesised clause takes SafeSearch
///   on, the wire default). `revoked` is CLEARED — a revoked Free clause means
///   "unrestricted", which would open the web inside a time that promised
///   only these sites. `paused` is left as Free has it: it locks all web, the
///   fail-closed direction. A Free `parentDeny` entry naming one of the time's
///   own listed domains is dropped (listing a site is the un-ban, and
///   `parentDeny` beats `parentAllow`); other entries carry. With `sites`
///   empty, Free's web rules carry unchanged and no browser runs unless it is
///   listed in `apps`.
///
/// The device's sanctioned web runtime being implied-allowed is enforcement,
/// not composition — see the contract.
pub fn compose_content(free: Option<&GrantContent>, t: &NamedTime) -> Option<GrantContent> {
    match t.posture {
        TimePosture::EverythingExcept => {
            if t.deny_sites.is_empty() && t.undeny_sites.is_empty() {
                return free.cloned();
            }
            let mut out = match free {
                Some(f)
                    if f.is_supported_version()
                        && f.is_revoked()
                        && !f.is_paused()
                        && !t.deny_sites.is_empty() =>
                {
                    synthetic_content(Posture::Blocklist)
                }
                Some(f) => f.clone(),
                None if !t.deny_sites.is_empty() => synthetic_content(Posture::Blocklist),
                None => return None,
            };
            let undeny: Vec<String> = t.undeny_sites.iter().map(|d| normalize_domain(d)).collect();
            out.parent_deny = dedup_by_key(
                out.parent_deny
                    .iter()
                    .chain(t.deny_sites.iter())
                    .filter(|d| !undeny.contains(&normalize_domain(d)))
                    .cloned(),
                normalize_domain,
            );
            out.parent_allow = dedup_by_key(
                out.parent_allow
                    .iter()
                    .chain(t.undeny_sites.iter())
                    .cloned(),
                normalize_domain,
            );
            Some(out)
        }
        TimePosture::OnlyThese => {
            if t.sites.is_empty() {
                return free.cloned();
            }
            let mut out = match free {
                Some(f) => f.clone(),
                None => {
                    let mut c = synthetic_content(Posture::Allowlist);
                    c.safe_search = Some(true);
                    c
                }
            };
            let listed = t.site_domains();
            let listed_keys: Vec<String> = listed.iter().map(|d| normalize_domain(d)).collect();
            out.posture = Posture::Allowlist;
            out.curators = Vec::new();
            out.revoked = None;
            out.parent_deny
                .retain(|d| !listed_keys.contains(&normalize_domain(d)));
            out.parent_allow = listed;
            Some(out)
        }
    }
}

/// `APPS_VERSION` re-export guard: composition output keeps Free's own `v`.
/// (Referenced so a future bump of the apps version is a compile-visible
/// decision here, not a silent one.)
const _: () = assert!(APPS_VERSION == 1);

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(v: serde_json::Value) -> Result<GrantTimes, ProtoError> {
        GrantTimes::from_value(&v)
    }

    fn base_time(id: &str, posture: &str) -> serde_json::Value {
        json!({"id": id, "label": "Learning time", "posture": posture})
    }

    fn clause(times: Vec<serde_json::Value>) -> serde_json::Value {
        json!({"v": 1, "issuedAt": 7, "times": times})
    }

    fn khan() -> serde_json::Value {
        json!({"id":"khan-academy","label":"Khan Academy","kind":"site",
               "url":"https://www.khanacademy.org/","domains":["khanacademy.org","kastatic.org"]})
    }

    #[test]
    fn minimal_clause_parses() {
        let g = parse(clause(vec![base_time("learning", "onlyThese")])).unwrap();
        assert_eq!(g.times.len(), 1);
        assert!(g.get("learning").unwrap().runs_nothing());
        assert!(!g.get("learning").unwrap().counts_against_budget());
        assert!(g.get("free").is_none());
    }

    #[test]
    fn the_reserved_and_malformed_ids_are_refused() {
        for id in [
            "free",
            "",
            "Learning",
            "has space",
            "under_score",
            &"a".repeat(33),
        ] {
            assert!(
                parse(clause(vec![base_time(id, "onlyThese")])).is_err(),
                "id {id:?}"
            );
        }
        assert!(parse(clause(vec![base_time(&"a".repeat(32), "onlyThese")])).is_ok());
    }

    #[test]
    fn duplicate_ids_and_too_many_times_are_refused() {
        let dup = clause(vec![
            base_time("a", "onlyThese"),
            base_time("a", "everythingExcept"),
        ]);
        assert!(parse(dup).is_err());
        let many = |n: usize| {
            clause(
                (0..n)
                    .map(|i| base_time(&format!("t{i}"), "onlyThese"))
                    .collect(),
            )
        };
        assert!(parse(many(MAX_NAMED_TIMES)).is_ok());
        assert!(parse(many(MAX_NAMED_TIMES + 1)).is_err());
    }

    #[test]
    fn label_is_counted_in_characters() {
        let mut t = base_time("a", "onlyThese");
        t["label"] = json!("é".repeat(MAX_TIME_LABEL_LEN));
        assert!(parse(clause(vec![t.clone()])).is_ok());
        t["label"] = json!("é".repeat(MAX_TIME_LABEL_LEN + 1));
        assert!(parse(clause(vec![t.clone()])).is_err());
        t["label"] = json!("");
        assert!(parse(clause(vec![t])).is_err());
    }

    #[test]
    fn version_must_be_exactly_one() {
        for v in [0, 2] {
            let c = json!({"v": v, "issuedAt": 1, "times": []});
            assert!(parse(c).is_err(), "v {v}");
        }
        assert!(GrantTimes::parse_or_absent("not json").is_none());
        assert!(GrantTimes::parse_or_absent(r#"{"v":2,"issuedAt":1,"times":[]}"#).is_none());
    }

    #[test]
    fn only_these_sites_get_the_learning_validation() {
        let mut t = base_time("a", "onlyThese");
        t["sites"] = json!([khan()]);
        assert!(parse(clause(vec![t.clone()])).is_ok());
        for bad in [
            json!({"id":"x","label":"x","kind":"site","domains":["a.org"]}), // no url
            json!({"id":"x","label":"x","kind":"site","url":"https://a.org/","domains":[]}),
            json!({"id":"x","label":"x","kind":"site","url":"http://a.org/","domains":["a.org"]}),
            json!({"id":"x","label":"x","kind":"site","url":"https://a.org/,x","domains":["a.org"]}),
            json!({"id":"x","label":"x","kind":"site","url":"https://a.org/","domains":["a.org/, EXCLUDE *"]}),
            json!({"id":"X","label":"x","kind":"site","url":"https://a.org/","domains":["a.org"]}),
            json!({"id":"x","label":"x","kind":"native","exec":"/usr/bin/x"}),
        ] {
            t["sites"] = json!([bad]);
            assert!(parse(clause(vec![t.clone()])).is_err(), "{t}");
        }
    }

    #[test]
    fn fields_of_the_inactive_posture_are_ignored() {
        // An everythingExcept time carrying a bad `sites` is fine: not read.
        let mut t = base_time("a", "everythingExcept");
        t["sites"] = json!([{"id":"x","label":"x","kind":"native","exec":"/usr/bin/x"}]);
        assert!(parse(clause(vec![t])).is_ok());
    }

    fn free_apps() -> GrantApps {
        GrantApps {
            v: 1,
            posture: AppPosture::Blocklist,
            blocked: vec!["minecraft".into(), "youtube".into()],
            allowed: vec![],
            paused: None,
            holds: vec![],
            ask_first: vec!["youtube".into()],
            hidden: vec!["bloat".into()],
            issued_at: 1,
        }
    }

    fn time(posture: TimePosture) -> NamedTime {
        NamedTime {
            id: "learning".into(),
            label: "Learning".into(),
            posture,
            block_apps: vec![],
            unblock_apps: vec![],
            deny_sites: vec![],
            undeny_sites: vec![],
            apps: vec![],
            sites: vec![],
            counts_against_budget: None,
        }
    }

    #[test]
    fn everything_except_applies_departures_either_way() {
        let mut t = time(TimePosture::EverythingExcept);
        t.block_apps = vec!["roblox".into()];
        t.unblock_apps = vec!["youtube".into()];
        let out = compose_apps(&free_apps(), &t);
        assert_eq!(out.blocked, vec!["minecraft".to_string(), "roblox".into()]);
        assert!(out.ask_first.is_empty(), "un-banned app leaves askFirst");
        assert_eq!(out.hidden, vec!["bloat".to_string()]);
        // Un-ban wins over a same-time ban of the same app.
        t.block_apps = vec!["youtube".into()];
        assert!(!compose_apps(&free_apps(), &t)
            .blocked
            .contains(&"youtube".to_string()));
    }

    #[test]
    fn every_except_bans_hold_over_a_paused_free_apps_policy() {
        let mut free = free_apps();
        free.paused = Some(true);
        let mut t = time(TimePosture::EverythingExcept);
        t.block_apps = vec!["roblox".into()];
        let out = compose_apps(&free, &t);
        assert_eq!(out.paused, None);
        assert_eq!(out.posture, AppPosture::Blocklist);
        assert_eq!(out.blocked, vec!["roblox".to_string()]);
        // No bans of its own: a paused clause carries.
        t.block_apps = vec![];
        assert_eq!(compose_apps(&free, &t).paused, Some(true));
    }

    #[test]
    fn every_except_denies_hold_over_a_revoked_free_content_clause() {
        let mut t = time(TimePosture::EverythingExcept);
        t.deny_sites = vec!["roblox.com".into()];
        let out = compose_content(Some(&free_content()), &t).unwrap();
        assert_eq!(out.revoked, None);
        assert_eq!(out.posture, Posture::Blocklist);
        assert_eq!(out.parent_deny, vec!["roblox.com".to_string()]);
        // A paused clause (block all web) carries.
        let mut paused = free_content();
        paused.paused = Some(true);
        let out = compose_content(Some(&paused), &t).unwrap();
        assert_eq!(out.paused, Some(true));
    }

    #[test]
    fn an_unsupported_version_free_content_stays_locked_under_a_time() {
        let mut t = time(TimePosture::EverythingExcept);
        t.deny_sites = vec!["roblox.com".into()];
        let mut future = free_content(); // revoked: true
        future.v = 2;
        let out = compose_content(Some(&future), &t).unwrap();
        assert_eq!(out.v, 2, "must stay unreadable, not become a v1 blocklist");
        assert_eq!(out.revoked, Some(true));
    }

    #[test]
    fn only_these_is_an_allowlist_and_lifts_a_pause() {
        let mut t = time(TimePosture::OnlyThese);
        t.apps = vec!["khan".into()];
        let mut free = free_apps();
        free.paused = Some(true);
        let out = compose_apps(&free, &t);
        assert_eq!(out.posture, AppPosture::Allowlist);
        assert_eq!(out.allowed, vec!["khan".to_string()]);
        assert_eq!(out.paused, None, "a paused Free policy must not open this");
        assert_eq!(out.hidden, vec!["bloat".to_string()]);
    }

    fn free_content() -> GrantContent {
        let mut c = synthetic_content(Posture::Blocklist);
        c.parent_deny = vec!["https://www.YouTube.com/".into(), "tiktok.com".into()];
        c.safe_search = Some(false);
        c.youtube_restrict = Some(charter_content::YoutubeRestrict::Strict);
        c.revoked = Some(true);
        c
    }

    #[test]
    fn content_departures_normalise_domains() {
        let mut unrevoked = free_content();
        unrevoked.revoked = None;
        let mut t = time(TimePosture::EverythingExcept);
        t.deny_sites = vec!["roblox.com".into()];
        t.undeny_sites = vec!["youtube.com".into()];
        let out = compose_content(Some(&unrevoked), &t).unwrap();
        assert_eq!(
            out.parent_deny,
            vec!["tiktok.com".to_string(), "roblox.com".into()]
        );
        assert_eq!(out.parent_allow, vec!["youtube.com".to_string()]);
    }

    #[test]
    fn content_with_no_free_clause_keeps_a_ban_but_not_a_phantom() {
        let mut t = time(TimePosture::EverythingExcept);
        t.undeny_sites = vec!["youtube.com".into()];
        assert_eq!(compose_content(None, &t), None);
        t.deny_sites = vec!["roblox.com".into()];
        let out = compose_content(None, &t).unwrap();
        assert_eq!(out.posture, Posture::Blocklist);
        assert_eq!(out.parent_deny, vec!["roblox.com".to_string()]);
    }

    #[test]
    fn only_these_sites_make_an_allowlist_and_unrevoke() {
        let g = parse(clause(vec![{
            let mut t = base_time("a", "onlyThese");
            t["sites"] = json!([khan()]);
            t
        }]))
        .unwrap();
        let t = g.get("a").unwrap();
        let mut free = free_content();
        free.curators = vec!["aa".into()];
        free.parent_deny.push("kastatic.org".into());
        let out = compose_content(Some(&free), t).unwrap();
        assert_eq!(out.posture, Posture::Allowlist);
        assert!(out.curators.is_empty());
        assert_eq!(out.revoked, None);
        assert_eq!(
            out.parent_allow,
            vec!["khanacademy.org".to_string(), "kastatic.org".into()]
        );
        assert_eq!(out.safe_search, Some(false), "SafeSearch carried from Free");
        assert_eq!(out.youtube_restrict, free.youtube_restrict);
        assert!(!out.parent_deny.contains(&"kastatic.org".to_string()));
        assert!(out.parent_deny.contains(&"tiktok.com".to_string()));
    }

    #[test]
    fn only_these_without_sites_carries_free_web_rules() {
        let t = time(TimePosture::OnlyThese);
        assert_eq!(
            compose_content(Some(&free_content()), &t),
            Some(free_content())
        );
        assert_eq!(compose_content(None, &t), None);
    }
}
