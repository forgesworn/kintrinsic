//! The standing per-app (`apps`) clause on Linux — the pure decision half.
//!
//! Until charterd 0.7.1 this clause (kind 4) was stored by the broker and then
//! never read: the toggles in Kintrinsic's Apps list, and the holds shipped on
//! 2026-08-02, bound a phone and silently did nothing to a laptop. This module
//! closes that gap by answering ONE question, with no I/O and no clock of its
//! own: given the child's raw `apps` clause JSON and the machine's app
//! inventory, which `pkg`s must not run at `now_unix`?
//!
//! The runtime unions the answer into the same `blocked_by_uid` map the
//! `appRules` and spent-bucket decisions feed, so the existing sweep does the
//! rest — argv0/flatpak/ancestry matching, and the sparing of sanctioned
//! learning windows (`site_app::sweep_decision`), all come for free and cannot
//! diverge between the clauses.
//!
//! Holds are resolved HERE, via the shared [`charter_proto::GrantApps::
//! effective_at`] — the same function the Android warden calls — so "allow
//! Vanadium for an hour" ends at the same instant on every platform a family
//! owns.

use charter_ipc::dto::AskFirstAppView;
use charter_proto::status::AppRef;
use charter_proto::{AppPosture, GrantApps, APPS_VERSION};

/// The `pkg`s the `apps` clause says must NOT run at `now_unix`.
///
/// Fail-safe like [`crate::app_rules::blocked_pkgs_now`]: an absent clause, a
/// body that won't parse, a version we don't understand, or `paused` all yield
/// an empty set — block nothing, never over-kill on a malformed clause.
///
/// `inventory_pkgs` is the machine's own launchable-app inventory (the same
/// identities STATUS reports and the guardian ticks in Kintrinsic — resolved
/// exec paths / flatpak ids). It matters only under the `allowlist` posture:
///
/// - **blocklist** blocks exactly the resolved `blocked` list.
/// - **allowlist** blocks every INVENTORY app not on the resolved `allowed`
///   list — and only inventory apps. On Android "everything else" is the
///   enumerable set of launchable packages; on Linux the sweep kills
///   processes, so the set must be enumerable here too or an allowlist would
///   slaughter the session (shells, the desktop, charterd's own helpers).
///   The inventory is also precisely the list the guardian was shown when they
///   ticked the allowlist, so what they saw and what is enforced agree. An
///   EMPTY inventory (daemon just started, dirs unreadable) blocks nothing —
///   we cannot enumerate, so we must not guess.
pub fn blocked_pkgs_from_apps(
    apps_json: Option<&str>,
    inventory_pkgs: &[String],
    now_unix: i64,
) -> Vec<String> {
    let Some(json) = apps_json else {
        return Vec::new();
    };
    let g: GrantApps = match serde_json::from_str(json) {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    if g.v != APPS_VERSION || g.is_paused() {
        return Vec::new();
    }
    // Live holds folded into the lists — enforcement below reads lists, exactly
    // as it does on Android, and a reboot inside a hold extends nothing.
    let effective = g.effective_at(now_unix.max(0) as u64);
    match effective.posture {
        AppPosture::Blocklist => effective.blocked,
        AppPosture::Allowlist => inventory_pkgs
            .iter()
            .filter(|p| !effective.allowed.contains(p))
            .cloned()
            .collect(),
    }
}

/// The `askFirst` apps still gated right now, labelled from `inventory` where
/// possible — so the ward's "Ask to open" list can be built with no policy
/// read of its own (mirrors [`blocked_pkgs_from_apps`]'s fail-safe rules
/// exactly, since it is the same clause).
///
/// "Gated" is POSTURE-aware, same as [`blocked_pkgs_from_apps`]:
/// - **blocklist**: a pkg is gated when it is BOTH named in `askFirst` AND
///   still actually in `blocked` right now (after holds resolve) — `askFirst`
///   is a hint over `blocked` (every listed pkg must also be in `blocked` —
///   the clause's own invariant).
/// - **allowlist**: `blocked` is never populated by the clause itself (it is
///   DERIVED — everything not in `allowed` — see `blocked_pkgs_from_apps`), so
///   checking `effective.blocked` here would always be empty and the ask
///   affordance would be permanently dead (found in review). A pkg is gated
///   instead whenever it is NOT in `allowed` right now.
///
/// Either way, a pkg a hold has opened for the moment is already open — there
/// is nothing left to ask for until the hold ends.
pub fn ask_first_apps_from_apps(
    apps_json: Option<&str>,
    inventory: &[AppRef],
    now_unix: i64,
) -> Vec<AskFirstAppView> {
    let Some(json) = apps_json else {
        return Vec::new();
    };
    let g: GrantApps = match serde_json::from_str(json) {
        Ok(g) => g,
        Err(_) => return Vec::new(),
    };
    if g.v != APPS_VERSION || g.is_paused() || g.ask_first.is_empty() {
        return Vec::new();
    }
    let effective = g.effective_at(now_unix.max(0) as u64);
    let gated = |pkg: &str| -> bool {
        match effective.posture {
            AppPosture::Blocklist => effective.blocked.iter().any(|b| b == pkg),
            AppPosture::Allowlist => !effective.allowed.iter().any(|a| a == pkg),
        }
    };
    g.ask_first
        .iter()
        .filter(|pkg| gated(pkg))
        .map(|pkg| AskFirstAppView {
            pkg: pkg.clone(),
            label: inventory
                .iter()
                .find(|a| &a.pkg == pkg)
                .map(|a| a.label.clone())
                .unwrap_or_else(|| pkg.clone()),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_782_734_400;

    fn inv(pkgs: &[&str]) -> Vec<String> {
        pkgs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn absent_malformed_paused_or_wrong_version_block_nothing() {
        let inventory = inv(&["/usr/bin/vanadium"]);
        assert!(blocked_pkgs_from_apps(None, &inventory, T0).is_empty());
        assert!(blocked_pkgs_from_apps(Some("not json"), &inventory, T0).is_empty());
        assert!(blocked_pkgs_from_apps(Some(r#"{"weird":1}"#), &inventory, T0).is_empty());
        let paused = r#"{"v":1,"posture":"blocklist","blocked":["/usr/bin/vanadium"],"paused":true,"issuedAt":1}"#;
        assert!(blocked_pkgs_from_apps(Some(paused), &inventory, T0).is_empty());
        let v2 = r#"{"v":2,"posture":"blocklist","blocked":["/usr/bin/vanadium"],"issuedAt":1}"#;
        assert!(blocked_pkgs_from_apps(Some(v2), &inventory, T0).is_empty());
    }

    #[test]
    fn blocklist_blocks_exactly_its_list() {
        let json = r#"{"v":1,"posture":"blocklist",
            "blocked":["/usr/games/supertuxkart","org.mozilla.firefox"],"issuedAt":1}"#;
        let out = blocked_pkgs_from_apps(Some(json), &inv(&["/usr/bin/other"]), T0);
        assert_eq!(
            out,
            vec![
                "/usr/games/supertuxkart".to_string(),
                "org.mozilla.firefox".to_string()
            ]
        );
    }

    /// decented's case, on a laptop: the browser stays ON the blocklist and is
    /// held open for an hour; the machine takes it back on its own.
    #[test]
    fn an_allow_hold_lifts_a_blocked_app_until_its_instant() {
        let json = format!(
            r#"{{"v":1,"posture":"blocklist",
                "blocked":["/usr/bin/vanadium","/usr/games/supertuxkart"],
                "holds":[{{"pkg":"/usr/bin/vanadium","state":"allowed","untilUnix":{}}}],
                "issuedAt":{T0}}}"#,
            T0 + 3600
        );
        let inventory = inv(&[]);
        let during = blocked_pkgs_from_apps(Some(&json), &inventory, T0 + 60);
        assert_eq!(during, vec!["/usr/games/supertuxkart".to_string()]);
        // AT the instant it ends — no new clause, no guardian action.
        let after = blocked_pkgs_from_apps(Some(&json), &inventory, T0 + 3600);
        assert_eq!(
            after,
            vec![
                "/usr/bin/vanadium".to_string(),
                "/usr/games/supertuxkart".to_string()
            ]
        );
    }

    #[test]
    fn a_block_hold_closes_an_otherwise_open_app_for_a_while() {
        let json = format!(
            r#"{{"v":1,"posture":"blocklist","blocked":[],
                "holds":[{{"pkg":"/usr/games/supertuxkart","state":"blocked","untilUnix":{}}}],
                "issuedAt":{T0}}}"#,
            T0 + 900
        );
        assert_eq!(
            blocked_pkgs_from_apps(Some(&json), &inv(&[]), T0),
            vec!["/usr/games/supertuxkart".to_string()]
        );
        assert!(blocked_pkgs_from_apps(Some(&json), &inv(&[]), T0 + 900).is_empty());
    }

    #[test]
    fn allowlist_blocks_the_inventory_minus_the_allowed() {
        let json = r#"{"v":1,"posture":"allowlist","allowed":["/usr/bin/firefox"],"issuedAt":1}"#;
        let inventory = inv(&[
            "/usr/bin/firefox",
            "/usr/games/supertuxkart",
            "org.videolan.VLC",
        ]);
        let out = blocked_pkgs_from_apps(Some(json), &inventory, T0);
        assert_eq!(
            out,
            vec![
                "/usr/games/supertuxkart".to_string(),
                "org.videolan.VLC".to_string()
            ]
        );
    }

    /// The kill set must be enumerable: an allowlist against an inventory we
    /// don't have yet (daemon just started) must block NOTHING rather than
    /// guess at "everything else" on a machine full of processes.
    #[test]
    fn allowlist_with_no_inventory_blocks_nothing() {
        let json = r#"{"v":1,"posture":"allowlist","allowed":["/usr/bin/firefox"],"issuedAt":1}"#;
        assert!(blocked_pkgs_from_apps(Some(json), &[], T0).is_empty());
    }

    /// Holds work under an allowlist too: "allowed" opens one more inventory
    /// app for a while, "blocked" pauses one that is usually allowed.
    #[test]
    fn holds_edit_the_allowlist_both_ways() {
        let inventory = inv(&["/usr/bin/firefox", "/usr/games/supertuxkart"]);
        let open = format!(
            r#"{{"v":1,"posture":"allowlist","allowed":["/usr/bin/firefox"],
                "holds":[{{"pkg":"/usr/games/supertuxkart","state":"allowed","untilUnix":{}}}],
                "issuedAt":{T0}}}"#,
            T0 + 600
        );
        assert!(blocked_pkgs_from_apps(Some(&open), &inventory, T0).is_empty());

        let shut = format!(
            r#"{{"v":1,"posture":"allowlist","allowed":["/usr/bin/firefox"],
                "holds":[{{"pkg":"/usr/bin/firefox","state":"blocked","untilUnix":{}}}],
                "issuedAt":{T0}}}"#,
            T0 + 600
        );
        assert_eq!(
            blocked_pkgs_from_apps(Some(&shut), &inventory, T0),
            vec![
                "/usr/bin/firefox".to_string(),
                "/usr/games/supertuxkart".to_string()
            ]
        );
    }

    // --- ask_first_apps_from_apps ------------------------------------------

    fn ar(pkg: &str, label: &str) -> AppRef {
        AppRef {
            pkg: pkg.to_string(),
            label: label.to_string(),
            user_installed: None,
            hidden: None,
        }
    }

    #[test]
    fn absent_malformed_paused_or_empty_ask_first_offer_nothing() {
        let inventory = [ar("com.mojang.minecraftpe", "Minecraft")];
        assert!(ask_first_apps_from_apps(None, &inventory, T0).is_empty());
        assert!(ask_first_apps_from_apps(Some("not json"), &inventory, T0).is_empty());
        let paused = r#"{"v":1,"posture":"blocklist","blocked":["com.mojang.minecraftpe"],
            "askFirst":["com.mojang.minecraftpe"],"paused":true,"issuedAt":1}"#;
        assert!(ask_first_apps_from_apps(Some(paused), &inventory, T0).is_empty());
        let no_hint =
            r#"{"v":1,"posture":"blocklist","blocked":["com.mojang.minecraftpe"],"issuedAt":1}"#;
        assert!(ask_first_apps_from_apps(Some(no_hint), &inventory, T0).is_empty());
    }

    /// The normal case: an askFirst pkg still on the standing blocklist is
    /// offered, with its label resolved from the inventory.
    #[test]
    fn ask_first_entries_are_labelled_from_the_inventory() {
        let json = r#"{"v":1,"posture":"blocklist",
            "blocked":["com.mojang.minecraftpe","org.mozilla.firefox"],
            "askFirst":["com.mojang.minecraftpe"],"issuedAt":1}"#;
        let inventory = [ar("com.mojang.minecraftpe", "Minecraft")];
        let out = ask_first_apps_from_apps(Some(json), &inventory, T0);
        assert_eq!(
            out,
            vec![AskFirstAppView {
                pkg: "com.mojang.minecraftpe".into(),
                label: "Minecraft".into(),
            }]
        );
    }

    /// An askFirst pkg the daemon's own inventory hasn't matched (not yet
    /// scanned, or an identity mismatch) still offers the ask — with its raw
    /// pkg as a fallback label rather than being dropped.
    #[test]
    fn an_unmatched_ask_first_pkg_falls_back_to_its_own_identity() {
        let json = r#"{"v":1,"posture":"blocklist","blocked":["com.mojang.minecraftpe"],
            "askFirst":["com.mojang.minecraftpe"],"issuedAt":1}"#;
        let out = ask_first_apps_from_apps(Some(json), &[], T0);
        assert_eq!(out[0].label, "com.mojang.minecraftpe");
    }

    /// A held-open askFirst app is already open — there is nothing to ask
    /// for, so it drops out of the list for the hold's duration and returns
    /// the moment the hold ends.
    #[test]
    fn a_held_open_ask_first_app_is_not_offered_while_the_hold_lasts() {
        let json = format!(
            r#"{{"v":1,"posture":"blocklist","blocked":["com.mojang.minecraftpe"],
                "askFirst":["com.mojang.minecraftpe"],
                "holds":[{{"pkg":"com.mojang.minecraftpe","state":"allowed","untilUnix":{}}}],
                "issuedAt":{T0}}}"#,
            T0 + 3600
        );
        let inventory = [ar("com.mojang.minecraftpe", "Minecraft")];
        assert!(ask_first_apps_from_apps(Some(&json), &inventory, T0 + 60).is_empty());
        // The instant the hold ends, the ask is offered again.
        let out = ask_first_apps_from_apps(Some(&json), &inventory, T0 + 3600);
        assert_eq!(out[0].pkg, "com.mojang.minecraftpe");
    }

    // --- F1: askFirst under an ALLOWLIST posture -----------------------------
    // Under allowlist `blocked` is never populated by the clause itself — it
    // is derived (everything not in `allowed`) — so the ask affordance can
    // only be offered by checking `allowed` directly. Before this fix
    // `ask_first_apps_from_apps` always checked `effective.blocked`, which is
    // permanently empty under allowlist, so an allowlist family's on-request
    // app never rendered an "Ask to open" row at all.
    #[test]
    fn allowlist_ask_first_pkg_not_in_allowed_is_offered() {
        let json = r#"{"v":1,"posture":"allowlist","allowed":["org.mozilla.firefox"],
            "askFirst":["com.mojang.minecraftpe"],"issuedAt":1}"#;
        let inventory = [ar("com.mojang.minecraftpe", "Minecraft")];
        let out = ask_first_apps_from_apps(Some(json), &inventory, T0);
        assert_eq!(
            out,
            vec![AskFirstAppView {
                pkg: "com.mojang.minecraftpe".into(),
                label: "Minecraft".into(),
            }]
        );
    }

    /// If an askFirst pkg is (incorrectly, upstream) ALSO in `allowed`, it is
    /// already running unconditionally — there is nothing to ask for, so it
    /// must not be offered (the mirror of the blocklist invariant).
    #[test]
    fn allowlist_ask_first_pkg_already_in_allowed_is_not_offered() {
        let json = r#"{"v":1,"posture":"allowlist","allowed":["com.mojang.minecraftpe"],
            "askFirst":["com.mojang.minecraftpe"],"issuedAt":1}"#;
        assert!(ask_first_apps_from_apps(Some(json), &[], T0).is_empty());
    }

    /// Holds work under allowlist askFirst too: a hold that opens the app
    /// (adds it to `allowed`) takes it off the ask list for the hold's
    /// duration, exactly like the blocklist case.
    #[test]
    fn allowlist_holds_edit_the_ask_first_offer() {
        let json = format!(
            r#"{{"v":1,"posture":"allowlist","allowed":["org.mozilla.firefox"],
                "askFirst":["com.mojang.minecraftpe"],
                "holds":[{{"pkg":"com.mojang.minecraftpe","state":"allowed","untilUnix":{}}}],
                "issuedAt":{T0}}}"#,
            T0 + 3600
        );
        assert!(ask_first_apps_from_apps(Some(&json), &[], T0 + 60).is_empty());
        let out = ask_first_apps_from_apps(Some(&json), &[], T0 + 3600);
        assert_eq!(out[0].pkg, "com.mojang.minecraftpe");
    }
}
