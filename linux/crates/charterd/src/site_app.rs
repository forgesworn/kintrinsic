//! Site-app launch identity — the single source of truth for "what does a
//! sanctioned educational window's command line look like?".
//!
//! Kintrinsic installs Chromium on a ward's machine so that guardian-designated
//! educational sites can run in a resolver-pinned window. That makes Chromium a
//! **runtime**, not a browser: the ward's browser is Firefox, which Kintrinsic
//! governs through `/etc/firefox/policies/policies.json`. There is no Chromium
//! managed-policy renderer anywhere in Kintrinsic, so a Chromium the ward can
//! drive freely would be an unmanaged browser sitting next to a managed one.
//!
//! So while a child has any site app in force, a Chromium process owned by that
//! child is swept UNLESS it is a sanctioned site-app launch. This module owns
//! that predicate, and owns the rendering of the command line it recognises —
//! deliberately the same function, because a renderer and a checker that drift
//! apart is precisely the hole this is meant to close.
//!
//! # What "sanctioned" checks, and what it deliberately doesn't
//!
//! The **resolver pin is the security boundary**; the `--class` marker is only
//! the meter. A ward can read the world-readable `.desktop` file and retype the
//! command themselves — and that is fine, because reproducing a sanctioned
//! command line yields a sanctioned sandbox and nothing else. Mixing one app's
//! class with another's pin yields the other app's sandbox. No combination of
//! Kintrinsic-rendered pins reaches an unpinned network.
//!
//! What must therefore be impossible is a command line that keeps the *marker*
//! while weakening the *pin*. The old check — "has a `--class=charter-*` and
//! some `--host-resolver-rules=` token" — allowed exactly that, and every one
//! of these got through it:
//!
//! | Forgery | Why it worked before |
//! |---|---|
//! | `--host-resolver-rules=MAP nothing` | the value was never compared |
//! | two `--host-resolver-rules` tokens | Chromium honours the last |
//! | `--host-rules=…` beside a valid pin | a second host-mapping flag |
//! | `--proxy-server=…` beside a valid pin | resolution moves to the proxy |
//!
//! Rather than enumerate dangerous flags forever, the argument list must match
//! **exactly**: same tokens, no extras, nothing missing. An allowlist of one
//! shape, so nothing has to be anticipated.
//!
//! Matching is on the argument **multiset**, not the sequence, so a future
//! Chromium that reorders its own argv cannot silently un-sanction — and
//! killing a child's Khan window because Chromium shuffled a flag would be a
//! miserable way to find that out. Multiset equality is exactly as strict:
//! identical multisets admit no extra flag.
//!
//! `--user-data-dir` is checked for **presence and uniqueness only**, never
//! value. Its job is to stop an already-open session swallowing the `--app`
//! launch (which silently discards the pin); it is not a boundary, and not
//! comparing it means this module needs no knowledge of the child's home
//! directory, so the predicate stays pure and `focus::classify` stays testable.

use charter_proto::{LearningApp, LearningAppKind};

/// Where the shim lives. The launcher's `Exec=` points here rather than at the
/// browser: `.desktop` `Exec` is not run through a shell, so `$HOME` would not
/// expand, and the profile directory has to be per-child (see the module docs
/// on `--user-data-dir`, and `bin/charter-learn.rs`).
pub const SHIM_PATH: &str = "/usr/lib/charter/charter-learn";

/// Root-owned per-app launch manifests the shim reads.
pub const MANIFEST_DIR: &str = "/var/lib/charter/learn";

/// Chromium-family binaries Kintrinsic will drive as a site-app runtime, in
/// preference order. Google Chrome is a last resort: it is the least likely to
/// be the family's deliberate choice, but a machine with only Chrome should get
/// a working sandbox rather than nothing. Every guarantee here holds for it
/// identically — same `--app`, `--class`, `--host-resolver-rules`,
/// `--user-data-dir` flags.
pub const KNOWN_RUNTIMES: [&str; 3] = [
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    "/usr/bin/google-chrome-stable",
];

/// The manifest path for a learning app id.
pub fn manifest_path(id: &str) -> String {
    format!("{MANIFEST_DIR}/{id}.json")
}

/// The per-child, per-app Chromium profile. Under the child's own home, so two
/// children on one laptop never share a login: the enactor writes ONE launcher
/// per app across every managed child, so a system-wide path would be shared.
pub fn profile_dir(home: &str, id: &str) -> String {
    format!(
        "{}/.local/share/charter/learn/{id}",
        home.trim_end_matches('/')
    )
}

/// The Chromium resolver pin for a verified domain closure: everything is
/// NOTFOUND except the app's domains (and their subdomains).
pub fn resolver_pin(domains: &[String]) -> String {
    let mut pin = String::from("MAP * ~NOTFOUND");
    for d in domains {
        pin.push_str(&format!(", EXCLUDE {d}, EXCLUDE *.{d}"));
    }
    pin
}

/// The window-class marker for an app id — the attribution marker, not a
/// boundary (see module docs).
pub fn class_marker(id: &str) -> String {
    format!("--class=charter-{id}")
}

/// The three fixed flags a sanctioned launch of `app` carries, in render order.
/// `--user-data-dir` is added by the shim, which is the only part that needs to
/// know the invoking user's home.
pub fn fixed_flags(app: &LearningApp) -> Vec<String> {
    vec![
        format!("--app={}", app.url.as_deref().unwrap_or_default()),
        class_marker(&app.id),
        format!("--host-resolver-rules={}", resolver_pin(&app.domains)),
    ]
}

/// The full argv the shim execs, and therefore exactly what `/proc/<pid>/cmdline`
/// reads back for a sanctioned browser process. One function renders the launch
/// and defines what the checker accepts.
pub fn sanctioned_argv(runtime: &str, app: &LearningApp, home: &str) -> Vec<String> {
    let mut argv = vec![runtime.to_string()];
    argv.extend(fixed_flags(app));
    argv.push(format!("--user-data-dir={}", profile_dir(home, &app.id)));
    argv
}

/// Which site app, if any, this command line is a sanctioned launch of.
///
/// `exe_uid` is the owning uid of the RESOLVED exe file backing this process
/// (not the process's own owner) — root-owned (`0`) is required, because
/// `argv0`/`cmdline` are attacker-controlled in a way the exe file's
/// ownership is not: `exec -a /usr/bin/chromium ~/patched-chromium <exact
/// sanctioned flags>` puts a Kintrinsic-recognised runtime path at `argv[0]`
/// while actually running an arbitrary, ward-owned binary. Without this
/// check that forgery classified as `Learning` (free time) AND was spared by
/// the sweep (it uses this very same predicate) — an unlimited, unfiltered,
/// unkillable browser, strictly worse than the bare launcher-rename bypass
/// this feature exists to close. A HAND-TYPED but otherwise identical launch
/// of the REAL, root-owned runtime is unaffected and still sanctioned — this
/// checks WHICH BINARY is actually running, never how the launch was typed
/// (that decided design point, and the resolver-pin-is-the-boundary
/// invariant above, are both unchanged).
///
/// `None` for anything else — including a bare browser, a forged marker, a
/// forged/impersonated runtime, and a launch of an app that is not currently
/// in force. Callers use this for BOTH "does this tick credit learning
/// time?" and "is this process exempt from the sweep?", which is the point:
/// attribution and enforcement must never disagree about what a sanctioned
/// window is.
pub fn sanctioned_app_id(
    cmdline: &[String],
    exe_uid: Option<u32>,
    apps: &[LearningApp],
) -> Option<String> {
    let (argv0, rest) = cmdline.split_first()?;
    if !KNOWN_RUNTIMES.contains(&argv0.as_str()) {
        return None;
    }
    // THE RUNTIME MUST BE ENFORCEMENT-GRADE — see the doc comment above.
    if exe_uid != Some(0) {
        return None;
    }
    // Exactly one profile flag: none means an already-open session can swallow
    // the launch and drop the pin; two is an attempt to confuse the comparison.
    if rest
        .iter()
        .filter(|t| t.starts_with("--user-data-dir="))
        .count()
        != 1
    {
        return None;
    }
    let mut got: Vec<&str> = rest
        .iter()
        .map(String::as_str)
        .filter(|t| !t.starts_with("--user-data-dir="))
        .collect();
    got.sort_unstable();

    for app in apps.iter().filter(|a| a.kind == LearningAppKind::Site) {
        let mut want: Vec<String> = fixed_flags(app);
        want.sort();
        if got.len() == want.len() && got.iter().zip(&want).all(|(g, w)| *g == w.as_str()) {
            return Some(app.id.clone());
        }
    }
    None
}

/// Whether this command line is a sanctioned launch of any app in force. See
/// [`sanctioned_app_id`] for what `exe_uid` gates and why.
pub fn is_sanctioned(cmdline: &[String], exe_uid: Option<u32>, apps: &[LearningApp]) -> bool {
    sanctioned_app_id(cmdline, exe_uid, apps).is_some()
}

/// Whether this command line is a Chromium-family runtime Kintrinsic manages —
/// sanctioned or not. The sweep uses this to decide whether a process is even a
/// candidate for the site-app lockdown, so an unrelated binary is never touched
/// by it.
pub fn is_runtime_process(cmdline: &[String], exe: Option<&str>) -> bool {
    let argv0_hit = cmdline
        .first()
        .is_some_and(|a| KNOWN_RUNTIMES.contains(&a.as_str()));
    // Chrome's /usr/bin wrapper `exec -a "$0"`s /opt/google/chrome/chrome, so
    // the exe path differs from every known runtime path; match its real binary
    // too, or the lockdown would miss a bare Chrome entirely.
    let exe_hit = exe.is_some_and(|e| {
        KNOWN_RUNTIMES.contains(&e) || e == "/opt/google/chrome/chrome" || e.ends_with("/chromium")
    });
    argv0_hit || exe_hit
}

/// The reason the sweep gives for terminating an unsanctioned runtime process.
/// Distinct from an `appRules` pkg so the audit line says which rule fired.
pub const LOCKDOWN_REASON: &str = "site-app runtime (unsanctioned)";

/// One process's fate, decided purely.
///
/// `Some(reason)` means terminate. The `/proc` reading, the ancestry walk and
/// the signal live in `runtime.rs`; the DECISION lives here so it can be tested
/// exhaustively without a live system — this is the function that decides
/// whether a child's homework window survives the tick.
///
/// * `sanctioned` — this process or an ancestor is a sanctioned launch.
/// * `sites` — whether the child has any site app in force at all. `false`
///   means no lockdown: Chromium is an ordinary browser and nobody is
///   pretending otherwise.
/// * `blocked_hit` — an `appRules`/spent-bucket identity matched.
pub fn sweep_decision(
    sanctioned: bool,
    sites_in_force: bool,
    is_runtime: bool,
    blocked_hit: Option<&str>,
) -> Option<String> {
    // A sanctioned educational window survives everything: the implied
    // lockdown AND an explicit rule naming its own binary. A family whose
    // runtime is Chrome and who also block Chrome must not thereby kill their
    // child's homework — that is the original problem in a new hat.
    if sanctioned {
        return None;
    }
    // The lockdown. Kintrinsic installed this browser to host pinned educational
    // windows; a bare one is a browser it cannot govern.
    if sites_in_force && is_runtime {
        return Some(LOCKDOWN_REASON.to_string());
    }
    blocked_hit.map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn khan() -> LearningApp {
        LearningApp {
            id: "khan-academy".into(),
            label: "Khan Academy".into(),
            kind: LearningAppKind::Site,
            domains: vec!["khanacademy.org".into(), "kastatic.org".into()],
            url: Some("https://www.khanacademy.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        }
    }

    fn wikipedia() -> LearningApp {
        LearningApp {
            id: "wikipedia".into(),
            label: "Wikipedia".into(),
            kind: LearningAppKind::Site,
            domains: vec!["wikipedia.org".into()],
            url: Some("https://www.wikipedia.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        }
    }

    const HOME: &str = "/home/robin";

    /// Root-owned: the enforcement-grade signal every "real launch" test in
    /// this module uses, so it isn't accidentally exercising the new C2 gate
    /// (see `a_forged_user_owned_runtime_...` below for that).
    const ROOT: Option<u32> = Some(0);

    #[test]
    fn what_the_shim_launches_is_what_the_checker_accepts() {
        let argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        assert_eq!(
            sanctioned_app_id(&argv, ROOT, &[khan()]).as_deref(),
            Some("khan-academy")
        );
    }

    /// The whole point of one renderer: the launcher's Exec and the sweep's
    /// expectation are the same bytes, so they cannot drift.
    #[test]
    fn every_known_runtime_is_accepted() {
        for rt in KNOWN_RUNTIMES {
            let argv = sanctioned_argv(rt, &khan(), HOME);
            assert!(
                is_sanctioned(&argv, ROOT, &[khan()]),
                "{rt} should be accepted"
            );
        }
    }

    #[test]
    fn a_bare_browser_is_not_sanctioned() {
        let argv = vec!["/usr/bin/chromium".to_string()];
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    #[test]
    fn an_app_not_in_force_is_not_sanctioned() {
        let argv = sanctioned_argv("/usr/bin/chromium", &wikipedia(), HOME);
        // Only Khan is in force; a perfectly-formed Wikipedia launch is not.
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
        assert!(is_sanctioned(&argv, ROOT, &[khan(), wikipedia()]));
    }

    // ---- C2: the runtime itself must be enforcement-grade --------------

    /// THE CRITICAL FORGERY: `exec -a /usr/bin/chromium ~/patched-chromium
    /// <exact sanctioned flags>` — a ward-owned binary (uid 1002) wearing a
    /// Kintrinsic-recognised runtime's argv0 and reproducing every flag exactly.
    /// Before this gate, `argv0 ∈ KNOWN_RUNTIMES` was the ENTIRE runtime
    /// check: this classified as free learning time, and because the sweep
    /// spares site apps via this same predicate, it was also unkillable — an
    /// unlimited, unfiltered, unstoppable browser. Must be rejected.
    #[test]
    fn a_forged_user_owned_runtime_with_perfect_flags_is_rejected() {
        let argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        assert!(!is_sanctioned(&argv, Some(1002), &[khan()]));
        assert_eq!(sanctioned_app_id(&argv, Some(1002), &[khan()]), None);
        // Unknown ownership (the exe link/metadata could not be read) must
        // fail the SAME way — an unreadable fact is never treated as root.
        assert!(!is_sanctioned(&argv, None, &[khan()]));
    }

    /// The decided design point, preserved exactly: reproducing the
    /// sanctioned command line BY HAND still yields a sanctioned sandbox —
    /// what must fail is a DIFFERENT BINARY wearing the runtime's argv0, not
    /// a hand-typed launch of the real one. `sanctioned_argv` here stands in
    /// for a ward who retyped the command themselves; the runtime it names
    /// is still the real, root-owned Chromium.
    #[test]
    fn a_hand_typed_launch_of_the_real_root_owned_runtime_is_still_sanctioned() {
        let argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        assert_eq!(
            sanctioned_app_id(&argv, ROOT, &[khan()]).as_deref(),
            Some("khan-academy")
        );
    }

    // ---- the forgery table from the module docs ------------------------

    /// The hole this module exists to close: the marker was checked, the pin's
    /// VALUE never was. This forgery used to read as free learning time, and
    /// under the runtime lockdown would have been a full browser escape.
    #[test]
    fn a_permissive_pin_with_a_valid_marker_is_rejected() {
        let argv = vec![
            "/usr/bin/chromium".to_string(),
            "--app=https://www.khanacademy.org/".to_string(),
            class_marker("khan-academy"),
            "--host-resolver-rules=MAP nothing".to_string(),
            format!("--user-data-dir={}", profile_dir(HOME, "khan-academy")),
        ];
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// Chromium honours the LAST occurrence, so a valid pin followed by a
    /// permissive one is an escape.
    #[test]
    fn a_second_resolver_rules_flag_is_rejected() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        argv.push("--host-resolver-rules=MAP * 127.0.0.1".to_string());
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// A different host-mapping flag alongside an untouched valid pin.
    #[test]
    fn an_extra_host_rules_flag_is_rejected() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        argv.push("--host-rules=MAP * example.com".to_string());
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// A proxy resolves hostnames at the proxy, bypassing --host-resolver-rules
    /// entirely — the pin is intact and completely bypassed.
    #[test]
    fn an_extra_proxy_flag_is_rejected() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        argv.push("--proxy-server=socks5://127.0.0.1:9050".to_string());
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// Nothing has to be anticipated: ANY extra token fails the multiset.
    #[test]
    fn any_unanticipated_extra_flag_is_rejected() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        argv.push("--some-flag-invented-in-2027".to_string());
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    #[test]
    fn a_missing_pin_is_rejected() {
        let argv: Vec<String> = sanctioned_argv("/usr/bin/chromium", &khan(), HOME)
            .into_iter()
            .filter(|t| !t.starts_with("--host-resolver-rules="))
            .collect();
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// One app's marker with another app's pin lands in the OTHER app's
    /// sandbox — harmless, but it must not read as the app it claims to be.
    #[test]
    fn a_mixed_marker_and_pin_is_rejected() {
        let argv = vec![
            "/usr/bin/chromium".to_string(),
            "--app=https://www.khanacademy.org/".to_string(),
            class_marker("khan-academy"),
            format!(
                "--host-resolver-rules={}",
                resolver_pin(&wikipedia().domains)
            ),
            format!("--user-data-dir={}", profile_dir(HOME, "khan-academy")),
        ];
        assert!(!is_sanctioned(&argv, ROOT, &[khan(), wikipedia()]));
    }

    #[test]
    fn a_non_runtime_argv0_is_rejected() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        argv[0] = "/home/robin/my-chromium".to_string();
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    // ---- --user-data-dir: presence and uniqueness, never value ---------

    /// Without a profile flag an already-open session swallows the --app launch
    /// and the pin is silently discarded — the window looks right and isn't.
    #[test]
    fn a_missing_profile_flag_is_rejected() {
        let argv: Vec<String> = sanctioned_argv("/usr/bin/chromium", &khan(), HOME)
            .into_iter()
            .filter(|t| !t.starts_with("--user-data-dir="))
            .collect();
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    #[test]
    fn two_profile_flags_are_rejected() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        argv.push("--user-data-dir=/tmp/elsewhere".to_string());
        assert!(!is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// The profile path is not a boundary — the pin is. Accepting any value is
    /// deliberate, and is what keeps this predicate free of home-directory
    /// lookups (see module docs).
    #[test]
    fn any_profile_path_is_accepted() {
        let mut argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        let i = argv
            .iter()
            .position(|t| t.starts_with("--user-data-dir="))
            .unwrap();
        argv[i] = "--user-data-dir=/tmp/somewhere-else".to_string();
        assert!(is_sanctioned(&argv, ROOT, &[khan()]));
    }

    /// A future Chromium that reorders its own argv must not un-sanction a
    /// window and get a child's homework app killed mid-lesson.
    #[test]
    fn flag_order_does_not_matter() {
        let argv = sanctioned_argv("/usr/bin/chromium", &khan(), HOME);
        let mut shuffled = vec![argv[0].clone()];
        shuffled.extend(argv[1..].iter().rev().cloned());
        assert!(is_sanctioned(&shuffled, ROOT, &[khan()]));
    }

    // ---- runtime-process detection -------------------------------------

    #[test]
    fn chrome_real_binary_counts_as_a_runtime_process() {
        // Chrome's wrapper `exec -a "$0"`s this, so argv0 and exe disagree.
        assert!(is_runtime_process(
            &["/usr/bin/google-chrome-stable".to_string()],
            Some("/opt/google/chrome/chrome")
        ));
        // …and a bare Chrome started some other way, argv0 mangled.
        assert!(is_runtime_process(
            &["chrome".to_string()],
            Some("/opt/google/chrome/chrome")
        ));
    }

    #[test]
    fn an_unrelated_binary_is_not_a_runtime_process() {
        assert!(!is_runtime_process(
            &["/usr/bin/firefox".to_string()],
            Some("/usr/lib/firefox/firefox")
        ));
        assert!(!is_runtime_process(
            &["/usr/bin/minecraft-launcher".to_string()],
            Some("/usr/bin/minecraft-launcher")
        ));
    }

    // ---- rendering ------------------------------------------------------

    #[test]
    fn the_pin_excludes_each_domain_and_its_subdomains() {
        let pin = resolver_pin(&["khanacademy.org".into(), "kastatic.org".into()]);
        assert_eq!(
            pin,
            "MAP * ~NOTFOUND, EXCLUDE khanacademy.org, EXCLUDE *.khanacademy.org, \
             EXCLUDE kastatic.org, EXCLUDE *.kastatic.org"
        );
    }

    #[test]
    fn profiles_are_per_child_and_per_app() {
        assert_eq!(
            profile_dir("/home/robin", "khan-academy"),
            "/home/robin/.local/share/charter/learn/khan-academy"
        );
        // Two children, one app — never a shared login.
        assert_ne!(
            profile_dir("/home/robin", "khan-academy"),
            profile_dir("/home/mia", "khan-academy")
        );
        // Two apps, one child — never a shared pin (the handoff bug).
        assert_ne!(
            profile_dir("/home/robin", "khan-academy"),
            profile_dir("/home/robin", "wikipedia")
        );
    }

    #[test]
    fn a_trailing_slash_on_home_does_not_double_up() {
        assert_eq!(
            profile_dir("/home/robin/", "wikipedia"),
            "/home/robin/.local/share/charter/learn/wikipedia"
        );
    }

    // ---- the sweep's decision -------------------------------------------

    /// A bare Chromium, with educational windows in force, is a browser
    /// Kintrinsic cannot govern — there is no Chromium managed-policy renderer
    /// anywhere in the tree. It goes.
    #[test]
    fn an_unsanctioned_runtime_is_swept_while_site_apps_are_in_force() {
        assert_eq!(
            sweep_decision(false, true, true, None).as_deref(),
            Some(LOCKDOWN_REASON)
        );
    }

    /// No site apps → no lockdown. Turning every one of them off hands the
    /// browser straight back, and a family who never used the feature sees no
    /// change at all.
    #[test]
    fn with_no_site_apps_in_force_a_runtime_is_left_alone() {
        assert_eq!(sweep_decision(false, false, true, None), None);
    }

    #[test]
    fn a_sanctioned_window_survives_the_lockdown() {
        assert_eq!(sweep_decision(true, true, true, None), None);
    }

    /// The interaction that would otherwise reintroduce the original bug: if a
    /// family's runtime is Chrome AND the guardian blocks Chrome through
    /// appRules, the educational windows must still open. One predicate,
    /// checked at the one place that decides to kill.
    #[test]
    fn a_sanctioned_window_survives_an_explicit_block_of_its_own_binary() {
        assert_eq!(
            sweep_decision(true, true, true, Some("/usr/bin/google-chrome-stable")),
            None
        );
    }

    /// …but an UNsanctioned one does not get to hide behind the same rule.
    #[test]
    fn an_unsanctioned_runtime_still_answers_to_an_explicit_block() {
        assert!(sweep_decision(false, true, true, Some("/usr/bin/google-chrome-stable")).is_some());
        // Even with no site apps in force, the explicit rule stands on its own.
        assert_eq!(
            sweep_decision(false, false, true, Some("/usr/bin/google-chrome-stable")).as_deref(),
            Some("/usr/bin/google-chrome-stable")
        );
    }

    /// The lockdown is about the runtime and nothing else. Minecraft is swept
    /// when its bucket is spent, never because Khan Academy is switched on.
    #[test]
    fn a_non_runtime_process_is_untouched_by_the_lockdown() {
        assert_eq!(sweep_decision(false, true, false, None), None);
        assert_eq!(
            sweep_decision(false, true, false, Some("/usr/bin/minecraft-launcher")).as_deref(),
            Some("/usr/bin/minecraft-launcher")
        );
    }
}
