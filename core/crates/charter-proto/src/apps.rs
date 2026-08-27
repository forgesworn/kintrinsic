//! `apps` clause body (D3 v1): the standing, device-enforced per-app policy for
//! one child. Apps are identified by package name (Android). Unlike the
//! per-artifact `install.apk` / `exec.allow` GRANTS (single-use), this is a
//! standing CLAUSE — a blocked app stays blocked even during allowed screen
//! time. `blocklist` blocks `blocked`; `allowlist` runs ONLY `allowed`
//! (everything else suspended). `paused` lifts the policy.

use serde::{Deserialize, Serialize};

use crate::error::ProtoError;

/// The frozen `apps` clause body version.
pub const APPS_VERSION: u32 = 1;

/// The longest a single hold may run, measured from the clause's `issuedAt`.
///
/// A clause claiming a three-year allowance is a bug or an attack, and the
/// fail-closed reading of an over-long LOOSENING is to refuse it outright. Same
/// intent as `MAX_MAINTENANCE_SECS`: a guardian can only be honoured for a
/// window they could plausibly have meant.
pub const MAX_APP_HOLD_SECS: u64 = 24 * 60 * 60;

/// The most holds one clause may carry. Bounds the body; 64 apps held at once
/// is not a family, it is a bug.
pub const MAX_APP_HOLDS: usize = 64;

/// What an app IS for the duration of a hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HoldState {
    /// Runs, even if the standing lists would block it.
    Allowed,
    /// Blocked, even if the standing lists would run it.
    Blocked,
}

/// A time-boxed departure from the standing per-app lists, for ONE app.
///
/// `until_unix` is an ABSOLUTE instant, never a duration — a duration restarts
/// every time the stored clause is re-read and the hold would never end. The
/// device resolves it against its own clock at every tick, so a reboot inside a
/// hold does not extend it by a second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppHold {
    /// On-device identity — the same vocabulary as the standing lists.
    pub pkg: String,
    pub state: HoldState,
    /// Unix seconds. The hold ends AT this instant.
    pub until_unix: u64,
}

/// How the per-app lists are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppPosture {
    /// Everything runs EXCEPT `blocked`.
    Blocklist,
    /// ONLY `allowed` runs; everything else is suspended.
    Allowlist,
}

/// Standing per-app policy for one child. Package names are the app identity;
/// the device suspends the resulting set (level-triggered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantApps {
    pub v: u32,
    pub posture: AppPosture,
    /// Blocklist: the apps to block. (Ignored in allowlist posture.)
    #[serde(default)]
    pub blocked: Vec<String>,
    /// Allowlist: the ONLY apps that run. (Ignored in blocklist posture.)
    #[serde(default)]
    pub allowed: Vec<String>,
    /// Lift the whole per-app policy (no app is blocked by it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    /// Time-boxed overrides of the lists above. Absent/empty = the standing
    /// lists alone. An older warden that does not know this field ignores it
    /// and keeps enforcing the standing lists, which is why the clause stays
    /// `v: 1` — the guardian surface version-gates the feature instead.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holds: Vec<AppHold>,
    /// Hint only: apps the guardian surface should offer an "ask to open"
    /// affordance for, instead of a flat block. Every listed pkg MUST also be
    /// in `blocked` — an older ward that does not know this field ignores it
    /// and keeps enforcing `blocked` exactly as before (fail closed, no ask
    /// affordance). Enforcement never reads this list; only `blocked` gates.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask_first: Vec<String>,
    /// Apps the device REMOVES from the ward's surface entirely (Android:
    /// Device Owner `setApplicationHidden`) — gone from the launcher, the
    /// drawer and Settings as if uninstalled, and put back the moment the
    /// guardian drops the pkg from this list. A device tidy, not a
    /// time-of-day policy: independent of `posture`/`blocked`/`allowed`, and
    /// NOT lifted by `paused` (pausing app blocks must not make a tablet's
    /// OEM bloatware reappear for an hour). Additive at `v: 1`: an older
    /// warden ignores it and the app simply stays, gated by posture as
    /// before; guardian surfaces version-gate instead (`wardenSupport`,
    /// `appHide`). Born 2026-08-27: a Samsung ward tablet full of junk and,
    /// since 0.6.9 locks USB debugging by design, no cable to clean it with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
    pub issued_at: u64,
}

impl GrantApps {
    /// Parse from a clause body JSON value (fail-closed on version/shape).
    pub fn from_value(v: &serde_json::Value) -> Result<GrantApps, ProtoError> {
        let g: GrantApps =
            serde_json::from_value(v.clone()).map_err(|e| ProtoError::BadParams(e.to_string()))?;
        if g.v != APPS_VERSION {
            return Err(ProtoError::BadVersion(g.v));
        }
        Ok(g)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.unwrap_or(false)
    }

    /// The holds that are actually in force at `now_unix`, in clause order.
    ///
    /// Dead, implausible and surplus holds are dropped here rather than at the
    /// point of use, so every consumer — enforcement and the ward's own notices
    /// — agrees about what is running.
    pub fn live_holds(&self, now_unix: u64) -> Vec<&AppHold> {
        if self.is_paused() {
            // `paused` lifts the whole clause; nothing in it is being enforced,
            // so nothing in it should be announced either.
            return Vec::new();
        }
        self.holds
            .iter()
            .filter(|h| h.until_unix > now_unix)
            .filter(|h| h.until_unix <= self.issued_at.saturating_add(MAX_APP_HOLD_SECS))
            .take(MAX_APP_HOLDS)
            .collect()
    }

    /// The clause as the device must actually enforce it at `now_unix`: every
    /// live hold folded into the standing lists, `holds` emptied.
    ///
    /// Pure, and level-triggered from an absolute instant — so a device that was
    /// switched off for a whole hold comes back to its standing rule with
    /// nothing to undo, and enforcement downstream never has to know that holds
    /// exist at all. It reads lists, as it always did.
    pub fn effective_at(&self, now_unix: u64) -> GrantApps {
        let mut out = self.clone();
        out.holds = Vec::new();
        // Only the list this posture actually READS is edited; the other stays a
        // faithful record of what the guardian authored. Applied in clause
        // order, so two holds naming the same app resolve last-writer-wins.
        for hold in self.live_holds(now_unix) {
            match (self.posture, hold.state) {
                (AppPosture::Blocklist, HoldState::Allowed) => {
                    out.blocked.retain(|p| p != &hold.pkg)
                }
                (AppPosture::Blocklist, HoldState::Blocked) => {
                    push_unique(&mut out.blocked, &hold.pkg)
                }
                (AppPosture::Allowlist, HoldState::Allowed) => {
                    push_unique(&mut out.allowed, &hold.pkg)
                }
                (AppPosture::Allowlist, HoldState::Blocked) => {
                    out.allowed.retain(|p| p != &hold.pkg)
                }
            }
        }
        out
    }
}

fn push_unique(list: &mut Vec<String>, pkg: &str) {
    if !list.iter().any(|p| p == pkg) {
        list.push(pkg.to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_camel_case_blocklist() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["app.youtube"],"issuedAt":7}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(g.posture, AppPosture::Blocklist);
        assert_eq!(g.blocked, vec!["app.youtube".to_string()]);
        assert!(g.allowed.is_empty());
        assert!(!g.is_paused());
        // Re-serialize keeps camelCase issuedAt.
        assert!(serde_json::to_string(&g).unwrap().contains("\"issuedAt\""));
    }

    #[test]
    fn allowlist_and_paused_parse() {
        let json = r#"{"v":1,"posture":"allowlist","allowed":["org.example.a"],"paused":true,"issuedAt":9}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(g.posture, AppPosture::Allowlist);
        assert_eq!(g.allowed, vec!["org.example.a".to_string()]);
        assert!(g.is_paused());
    }

    #[test]
    fn wrong_version_is_rejected() {
        let json = r#"{"v":2,"posture":"blocklist","issuedAt":1}"#;
        assert!(GrantApps::from_value(&serde_json::from_str(json).unwrap()).is_err());
    }

    // --- holds ------------------------------------------------------------

    const T0: u64 = 1_782_734_400;

    fn hold(pkg: &str, state: HoldState, until: u64) -> AppHold {
        AppHold {
            pkg: pkg.to_string(),
            state,
            until_unix: until,
        }
    }

    fn blocklist(blocked: &[&str], holds: Vec<AppHold>) -> GrantApps {
        GrantApps {
            v: 1,
            posture: AppPosture::Blocklist,
            blocked: blocked.iter().map(|s| s.to_string()).collect(),
            allowed: Vec::new(),
            paused: None,
            holds,
            ask_first: Vec::new(),
            hidden: Vec::new(),
            issued_at: T0,
        }
    }

    fn allowlist(allowed: &[&str], holds: Vec<AppHold>) -> GrantApps {
        GrantApps {
            v: 1,
            posture: AppPosture::Allowlist,
            blocked: Vec::new(),
            allowed: allowed.iter().map(|s| s.to_string()).collect(),
            paused: None,
            holds,
            ask_first: Vec::new(),
            hidden: Vec::new(),
            issued_at: T0,
        }
    }

    /// decented's case: Vanadium is on the blocklist, held open for an hour.
    #[test]
    fn allow_hold_lifts_a_blocked_app_then_gives_it_back() {
        let g = blocklist(
            &["org.chromium.vanadium", "com.google.android.youtube"],
            vec![hold("org.chromium.vanadium", HoldState::Allowed, T0 + 3600)],
        );
        let during = g.effective_at(T0 + 60);
        assert_eq!(
            during.blocked,
            vec!["com.google.android.youtube".to_string()]
        );
        assert!(during.holds.is_empty(), "resolved clause carries no holds");

        // AT the instant it ends, not a tick later.
        let after = g.effective_at(T0 + 3600);
        assert_eq!(
            after.blocked,
            vec![
                "org.chromium.vanadium".to_string(),
                "com.google.android.youtube".to_string()
            ],
        );
    }

    #[test]
    fn block_hold_closes_an_otherwise_open_app() {
        let g = blocklist(
            &[],
            vec![hold("com.mojang.minecraftpe", HoldState::Blocked, T0 + 900)],
        );
        assert_eq!(
            g.effective_at(T0).blocked,
            vec!["com.mojang.minecraftpe".to_string()]
        );
        assert!(g.effective_at(T0 + 900).blocked.is_empty());
    }

    #[test]
    fn allowlist_holds_edit_the_allowed_list() {
        let open = allowlist(
            &["org.mozilla.fenix"],
            vec![hold("org.chromium.vanadium", HoldState::Allowed, T0 + 600)],
        );
        assert_eq!(
            open.effective_at(T0).allowed,
            vec![
                "org.mozilla.fenix".to_string(),
                "org.chromium.vanadium".to_string()
            ],
        );

        let shut = allowlist(
            &["org.mozilla.fenix", "org.chromium.vanadium"],
            vec![hold("org.mozilla.fenix", HoldState::Blocked, T0 + 600)],
        );
        assert_eq!(
            shut.effective_at(T0).allowed,
            vec!["org.chromium.vanadium".to_string()]
        );
    }

    /// A hold already dead when the clause lands changes nothing — the phone was
    /// simply off through the whole window.
    #[test]
    fn expired_holds_are_ignored() {
        let g = blocklist(
            &["a.b"],
            vec![hold("a.b", HoldState::Allowed, T0.saturating_sub(1))],
        );
        assert_eq!(g.effective_at(T0).blocked, vec!["a.b".to_string()]);
    }

    /// The fail-closed reading of an over-long loosening is to refuse it.
    #[test]
    fn a_hold_past_the_cap_is_refused() {
        let g = blocklist(
            &["a.b"],
            vec![hold("a.b", HoldState::Allowed, T0 + MAX_APP_HOLD_SECS + 1)],
        );
        assert_eq!(
            g.effective_at(T0).blocked,
            vec!["a.b".to_string()],
            "a three-year allowance is a bug or an attack"
        );

        // …but exactly at the cap is honoured.
        let ok = blocklist(
            &["a.b"],
            vec![hold("a.b", HoldState::Allowed, T0 + MAX_APP_HOLD_SECS)],
        );
        assert!(ok.effective_at(T0).blocked.is_empty());
    }

    #[test]
    fn holds_past_the_count_cap_are_dropped() {
        let holds: Vec<AppHold> = (0..MAX_APP_HOLDS + 5)
            .map(|i| hold(&format!("app.{i}"), HoldState::Blocked, T0 + 60))
            .collect();
        let g = blocklist(&[], holds);
        assert_eq!(g.effective_at(T0).blocked.len(), MAX_APP_HOLDS);
    }

    /// Two words about one app: the later one is the guardian's mind.
    #[test]
    fn last_hold_for_a_pkg_wins() {
        let g = blocklist(
            &["a.b"],
            vec![
                hold("a.b", HoldState::Allowed, T0 + 600),
                hold("a.b", HoldState::Blocked, T0 + 600),
            ],
        );
        assert_eq!(g.effective_at(T0).blocked, vec!["a.b".to_string()]);

        let other = blocklist(
            &["a.b"],
            vec![
                hold("a.b", HoldState::Blocked, T0 + 600),
                hold("a.b", HoldState::Allowed, T0 + 600),
            ],
        );
        assert!(other.effective_at(T0).blocked.is_empty());
    }

    /// A held app must not be added twice when it is already on the list.
    #[test]
    fn a_block_hold_on_an_already_blocked_app_is_a_no_op() {
        let g = blocklist(&["a.b"], vec![hold("a.b", HoldState::Blocked, T0 + 600)]);
        assert_eq!(g.effective_at(T0).blocked, vec!["a.b".to_string()]);
    }

    /// The section's own "off" lifts everything, holds included — nothing in a
    /// paused clause is being enforced, so nothing in it should be announced.
    #[test]
    fn paused_beats_every_hold() {
        let mut g = blocklist(&["a.b"], vec![hold("a.b", HoldState::Allowed, T0 + 600)]);
        g.paused = Some(true);
        assert!(g.live_holds(T0).is_empty());
        assert_eq!(g.effective_at(T0).blocked, vec!["a.b".to_string()]);
    }

    #[test]
    fn holds_round_trip_camel_case_and_are_omitted_when_empty() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["a.b"],
            "holds":[{"pkg":"a.b","state":"allowed","untilUnix":1782738000}],"issuedAt":1782734400}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(g.holds.len(), 1);
        assert_eq!(g.holds[0].state, HoldState::Allowed);
        assert_eq!(g.holds[0].until_unix, 1_782_738_000);
        let back = serde_json::to_string(&g).unwrap();
        assert!(back.contains("\"untilUnix\""));
        assert!(back.contains("\"state\":\"allowed\""));

        // A clause with no holds must not grow a field older wardens never saw.
        let plain = blocklist(&["a.b"], Vec::new());
        assert!(!serde_json::to_string(&plain).unwrap().contains("holds"));
    }

    /// An older ward simply ignores the field rather than rejecting the clause —
    /// that is what lets this ship at `v: 1`.
    #[test]
    fn unknown_hold_shaped_clause_still_parses_without_holds() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["a.b"],"issuedAt":7}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert!(g.holds.is_empty());
        assert_eq!(g.effective_at(9).blocked, vec!["a.b".to_string()]);
    }

    // --- askFirst -----------------------------------------------------------

    /// An older ward that has never heard of `askFirst` still parses the
    /// clause and keeps enforcing `blocked` exactly as before.
    #[test]
    fn ask_first_defaults_empty_on_old_bytes() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["a.b"],"issuedAt":7}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert!(g.ask_first.is_empty());
        // Effective enforcement is untouched by askFirst either way.
        assert_eq!(g.effective_at(9).blocked, vec!["a.b".to_string()]);
    }

    #[test]
    fn ask_first_round_trips_camel_case() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["app.youtube","com.mojang.minecraftpe"],
            "askFirst":["com.mojang.minecraftpe"],"issuedAt":7}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(g.ask_first, vec!["com.mojang.minecraftpe".to_string()]);
        // Every askFirst pkg is still present in blocked (the hint's own
        // invariant) — enforcement never reads askFirst itself.
        assert!(g.blocked.contains(&"com.mojang.minecraftpe".to_string()));

        let back = serde_json::to_string(&g).unwrap();
        assert!(
            back.contains("\"askFirst\":[\"com.mojang.minecraftpe\"]"),
            "{back}"
        );

        // Empty is omitted from the wire, so an old client sees the old shape.
        let mut plain = g.clone();
        plain.ask_first = Vec::new();
        assert!(!serde_json::to_string(&plain).unwrap().contains("askFirst"));
    }

    // --- hidden ("remove from device") ---------------------------------

    #[test]
    fn hidden_defaults_empty_on_old_bytes() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["a.b"],"issuedAt":7}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert!(g.hidden.is_empty());
        // Unchanged families keep byte-identical clauses: no `hidden` key.
        assert!(!serde_json::to_string(&g).unwrap().contains("hidden"));
    }

    #[test]
    fn hidden_round_trips_and_survives_pause_and_holds() {
        let json = r#"{"v":1,"posture":"blocklist","paused":true,
            "hidden":["com.samsung.android.bixby.agent","com.facebook.katana"],
            "holds":[{"pkg":"a.b","state":"allowed","untilUnix":100}],"issuedAt":7}"#;
        let g = GrantApps::from_value(&serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!(g.hidden.len(), 2);
        assert!(g.is_paused());
        // `paused` lifts blocking, never the tidy; `effective_at` dissolves
        // holds but carries `hidden` through untouched for enforcement.
        let eff = g.effective_at(50);
        assert_eq!(eff.hidden, g.hidden);
        assert!(eff.holds.is_empty());
        let out = serde_json::to_string(&eff).unwrap();
        assert!(out.contains("\"hidden\":[\"com.samsung.android.bixby.agent\""));
    }
}
