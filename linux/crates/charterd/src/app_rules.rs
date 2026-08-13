//! Per-app enforcement decision — the pure, clock-injected core.
//!
//! The guardian's `appRules` clause (kind 6, routed per-child into the child
//! clause store exactly like `learning`/`apps`) names apps to block outright or
//! constrain to their own allowed hours. On Linux a rule's `pkg` is a native
//! exec path or a flatpak app-id (see `spec/contract.md`).
//!
//! This module holds the two pure, default-feature-testable pieces the runtime
//! tick wires together:
//!   1. [`blocked_pkgs_now`] — parse the clause JSON (fail-safe: absent or
//!      unparseable → empty set → block nothing) and, via the committed
//!      [`charter_schedule::evaluate_app_rules`] evaluator, return the `pkg`s
//!      that must not run at `now_unix`.
//!   2. [`pkg_matches_process`] — decide whether one running process (its
//!      resolved exe + cgroup) IS an instance of a blocked `pkg`.
//!
//! The privileged `/proc` scan and the SIGTERM live in `runtime.rs` (the `real`
//! feature); they call these helpers, so every decision is unit-tested here
//! without a live system.

use charter_schedule::{
    cmdline_needle, evaluate_app_rules, is_cmdline_id, is_site_id, site_id, AppAccess,
    GrantAppRules, APP_RULES_VERSION,
};

/// An empty rule set — the fail-safe value when no clause is present or the
/// clause body won't parse. Empty ⇒ nothing is blocked (never over-kills on a
/// malformed clause), matching the `learning` clause's fail-to-`None` posture.
fn empty_rules() -> GrantAppRules {
    GrantAppRules {
        v: APP_RULES_VERSION,
        rules: Vec::new(),
        issued_at: 0,
    }
}

/// The `pkg`s that must NOT run at `now_unix`, given the child's raw `appRules`
/// clause JSON (or `None`). Fail-safe: an absent clause or a body that won't
/// parse yields an empty set (block nothing) rather than an error. Blocking is
/// the committed [`evaluate_app_rules`] decision — `AppAccess::Blocked` means
/// blocked outright OR outside the app's allowed hours.
pub fn blocked_pkgs_now(app_rules_json: Option<&str>, now_unix: i64) -> Vec<String> {
    let rules: GrantAppRules = match app_rules_json {
        Some(json) => serde_json::from_str(json).unwrap_or_else(|_| empty_rules()),
        None => empty_rules(),
    };
    evaluate_app_rules(&rules, now_unix)
        .into_iter()
        .filter(|(_, access)| *access == AppAccess::Blocked)
        .map(|(pkg, _)| pkg)
        .collect()
}

/// The last path component of `p` (the binary's name).
fn basename(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Whether `pkg` names a flatpak app rather than a native exec path: a flatpak
/// app-id has no path separator and at least one dot (`org.mozilla.firefox`),
/// whereas a native exec is an absolute path or a bare command name.
fn is_flatpak_id(pkg: &str) -> bool {
    !pkg.contains('/') && pkg.contains('.')
}

/// Whether a running process — identified by its resolved `exe` path (the
/// `/proc/<pid>/exe` link target), its `argv0` (first token of
/// `/proc/<pid>/cmdline`), its full `cmdline` (NUL-split argv), that same
/// argv pre-joined with single spaces (`cmdline_joined` — computed ONCE by
/// the caller, since a caller typically tests one process against several
/// `pkg`s per tick and the join must not be reallocated for each) and its
/// `cgroup` line — is an instance of the blocked `pkg`.
///
/// - A `cmdline:` identity (see [`is_cmdline_id`]) matches when its needle is
///   a substring of any single argv token, or of the tokens joined with a
///   single space — this is the process's OWN command line, so it names the
///   game (`net.minecraft.client.main.Main`) regardless of which launcher, if
///   any, started it. Checked FIRST, before every other form — see below.
/// - A flatpak app-id matches when the process's cgroup carries that app's
///   systemd scope (`app-flatpak-<id>-<instance>.scope`); the trailing
///   delimiter is required so `org.foo.Bar` never matches `org.foo.BarBaz`.
/// - A native `pkg` (absolute path or bare name) matches on an exact path
///   equality or a binary-name equality, against EITHER the resolved exe or
///   argv0 — so a wrapper/symlink launch of the same binary is still caught.
///
/// # Why `is_cmdline_id` must be checked before `is_flatpak_id`
///
/// [`is_flatpak_id`] is `!contains('/') && contains('.')` — a heuristic that
/// ALSO returns true for `cmdline:net.minecraft.client.main.Main` (no slash,
/// has dots). Testing the flatpak branch first would silently misroute every
/// `cmdline:` rule into cgroup matching, where it can never match anything: a
/// rule the guardian believes is armed would fail open in total silence,
/// exactly the class of bug this identity form exists to close. See
/// `cmdline_id_is_never_treated_as_a_flatpak_id` below.
///
/// # Why argv0 and not just the exe
///
/// Google Chrome ships `/usr/bin/google-chrome-stable` as a shell wrapper whose
/// last line is `exec -a "$0" "$HERE/chrome" "$@"`. The resolved exe is
/// therefore `/opt/google/chrome/chrome`, while the identity the app inventory
/// reports (and the guardian ticks in Kintrinsic) is the `/usr/bin` path from
/// the `.desktop` file. Neither path equality nor basename equality
/// (`chrome` vs `google-chrome-stable`) fires, so blocking Chrome used to be
/// silently inert — the guardian sets a control and nothing happens, which is
/// worse than not offering the control. argv0 survives the `exec -a` and is
/// exactly the reported identity.
///
/// This can only ever match MORE processes, never fewer: a process matches if
/// the exe matches OR argv0 does. A ward who forges a misleading argv0 can get
/// their own process killed, never spare one.
///
/// Pure over its inputs (the caller reads `/proc`), so the match rule is
/// exhaustively unit-tested.
pub fn pkg_matches_process(
    pkg: &str,
    exe: Option<&str>,
    argv0: Option<&str>,
    cmdline: &[String],
    cmdline_joined: &str,
    cgroup: Option<&str>,
) -> bool {
    matches_pkg(
        pkg,
        exe,
        argv0,
        cmdline,
        cmdline_joined,
        cgroup,
        Trust::Wide,
    )
}

/// [`pkg_matches_process`] restricted to **kernel-resolved identity only**:
/// an exact path equality against the kernel-resolved `exe`, and NOTHING
/// else.
///
/// # The rule
///
/// Of everything the wide matcher can look at, only `/proc/<pid>/exe` is
/// resolved by the KERNEL and unforgeable by an unprivileged ward. **argv0,
/// the rest of argv, and the cgroup path are all ward-writable — and they
/// stay ward-writable even when the exe is root-owned**, because the ward
/// launches the process and hands it its argv and its scope. So this form:
///
/// - drops the argv0 arm entirely (reaching this matcher already implies a
///   readable `exe`, so nothing legitimate is lost);
/// - requires an exact path equality on `exe` — no basename fallback;
/// - never consults the `cmdline:` or flatpak arms at all. A prior round
///   believed those two arms for a root-owned exe, reasoning the ward could
///   not have written the binary — but the binary was never the forged part:
///   `systemd-run --user --scope --unit=app-flatpak-<id>-9999 /bin/bash -c
///   '<needle>; ~/evil'` puts a ROOT-owned binary inside a ward-authored
///   cgroup wearing ward-authored argv, unprivileged, verified live. Root
///   ownership of the exe says nothing about who wrote the strings — the
///   ward launched it and supplied both.
///
/// # Why a second, stricter matcher exists
///
/// The two directions are not symmetric. `pkg_matches_process` errs wide on
/// purpose because a wide match can only ever *restrict* more processes: it
/// catches a wrapper launch of the same binary, and a ward who forges a
/// misleading argv0 or cgroup name merely gets their own process killed.
///
/// [`crate::focus::is_unrecognised`] asks the opposite question — *is this
/// process already accounted for, so the unrecognised counter should stay
/// silent?* — where a match SPARES, and every ward-writable field becomes an
/// off switch the ward holds. All of these were verified live against a
/// governed identity, and all went quiet under wider matchers:
///
/// | forgery | ward-writable field abused |
/// |---|---|
/// | `cp game ~/.local/bin/gcompris-qt` | basename of `exe` |
/// | `exec -a /usr/bin/gcompris-qt ~/.local/bin/evil` | argv0 |
/// | `systemd-run --user --scope --unit=app-flatpak-org.kde.gcompris-9999 …` | cgroup path (needs no privilege) |
/// | putting the needle in your own argv | the `cmdline:` substring test |
/// | all of the above on a ROOT-owned process the ward launched, or laundered through a root-owned ANCESTOR | argv + cgroup, past a root-exe gate |
///
/// None of them earns free time and none of them is stopped by anything —
/// they only ever bought silence. Hence: suppression trusts the
/// kernel-resolved exe path only.
pub fn pkg_names_process_exactly(pkg: &str, exe: Option<&str>) -> bool {
    matches_pkg(pkg, exe, None, &[], "", None, Trust::KernelResolvedOnly)
}

/// Which fields a match is allowed to believe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    /// Everything, including ward-writable argv0/argv/cgroup. Correct where a
    /// match RESTRICTS.
    Wide,
    /// The kernel-resolved `exe` path only, exact. Correct where a match
    /// SPARES.
    KernelResolvedOnly,
}

/// The shared body. `trust` is the ONLY difference between the two public
/// forms — kept as one function so the `cmdline:`-before-flatpak ordering
/// hazard, the empty-argv0 rule and the derived-parameter assert can never
/// drift between them.
fn matches_pkg(
    pkg: &str,
    exe: Option<&str>,
    argv0: Option<&str>,
    cmdline: &[String],
    cmdline_joined: &str,
    cgroup: Option<&str>,
    trust: Trust,
) -> bool {
    // `cmdline_joined` is a DERIVED parameter — callers hoist `cmdline.join("
    // ")` for performance (computed once per process, not once per pkg
    // tested against it), never to pass a different value. Self-policing in
    // tests/debug builds: a caller that drifts from the contract fails loudly
    // here rather than silently mismatching on whichever pkg needed the
    // joined-line fallback.
    debug_assert_eq!(
        cmdline_joined,
        cmdline.join(" "),
        "cmdline_joined must equal cmdline.join(\" \") exactly"
    );
    if pkg.is_empty() {
        return false;
    }
    // Both the `cmdline:` and flatpak arms below read WARD-WRITABLE strings —
    // the process's own argv, and a cgroup path any unprivileged user can
    // name (`systemd-run --user --scope --unit=app-flatpak-<id>-9999` needs
    // no privilege at all and produces a scope the flatpak arm accepts). When
    // a match SPARES, believing either of those hands the ward the off
    // switch, so under `KernelResolvedOnly` neither arm is consulted AT ALL.
    // (A prior round believed them for a root-owned exe; that gate was inert
    // exactly where it had to bite — the ward hands even a root-owned
    // root-owned program its argv and its scope. See
    // `pkg_names_process_exactly`.)
    // MUST run before `is_flatpak_id` — see the doc comment above.
    if is_cmdline_id(pkg) {
        return trust == Trust::Wide
            && cmdline_needle(pkg).is_some_and(|needle| {
                cmdline.iter().any(|tok| tok.contains(needle)) || cmdline_joined.contains(needle)
            });
    }
    // `site:<id>` — one of the guardian's own site apps. MUST also run before
    // `is_flatpak_id`, and for the same reason `cmdline:` does: a site id is
    // free-form (`site:bbc-bitesize` is fine, so is `site:my.school`), and one
    // containing a dot but no slash satisfies `is_flatpak_id` exactly.
    //
    // Matching is on the window-class marker, NOT the full sanctioned argv,
    // and the asymmetry is the point. This matcher only ever RESTRICTS —
    // metering a bucket, blocking an app, deciding a kill. In that direction
    // believing a ward-writable string is free: forging
    // `--class=charter-youtube` onto your own process gets that process
    // metered and stopped, which is self-harm and nothing else. It is the
    // SPARING direction where the marker must never be believed, and that
    // direction goes through `site_app::sanctioned_app_id`, which compares the
    // whole argv multiset and demands a root-owned runtime.
    //
    // Nor can a ward escape the meter by dropping the marker: an unmarked
    // launch is not a sanctioned argv, so while any site app is in force the
    // lockdown sweep terminates it (`site_app::sweep_decision`). Evasion here
    // costs the window, which is the invariant working, not a hole in it.
    if is_site_id(pkg) {
        return trust == Trust::Wide
            && site_id(pkg).is_some_and(|id| {
                let marker = crate::site_app::class_marker(id);
                cmdline.iter().any(|tok| tok == &marker)
            });
    }
    if is_flatpak_id(pkg) {
        return trust == Trust::Wide
            && cgroup.is_some_and(|c| {
                c.contains(&format!("flatpak-{pkg}-")) || c.contains(&format!("flatpak-{pkg}."))
            });
    }
    let names_pkg = |candidate: &str| {
        candidate == pkg || (trust == Trust::Wide && basename(candidate) == basename(pkg))
    };
    if trust == Trust::KernelResolvedOnly {
        // argv0 is not consulted at all: `exec -a /usr/bin/gcompris-qt
        // ~/.local/bin/evil` is one shell builtin, needs no privilege, and
        // was byte-for-byte as effective as the `cp` rename at silencing the
        // counter. Nothing legitimate is lost — reaching this matcher on the
        // suppression path already implies a readable, stattable `exe`.
        return exe.is_some_and(names_pkg);
    }
    // An empty argv0 is not an identity — a kernel thread has no cmdline, and
    // basename("") == basename("") would otherwise match every bare pkg.
    exe.is_some_and(names_pkg) || argv0.filter(|a| !a.is_empty()).is_some_and(names_pkg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_schedule::{AppRule, GrantSchedule, GrantScheduleWindow, WeeklySchedule};

    // 2026-07-04 (Sat) UTC — the same anchor the committed evaluator tests use.
    const SAT_NOON: i64 = 1783166400; // 12:00 UTC
    const SAT_0730: i64 = 1783166400 - 4 * 3600 - 30 * 60; // 07:30 UTC

    /// An every-day allowed-hours window (reuses the device schedule shape).
    fn every_day_window(start: &str, end: &str) -> GrantSchedule {
        let win = || {
            Some(vec![GrantScheduleWindow {
                start: start.into(),
                end: end.into(),
            }])
        };
        GrantSchedule {
            v: 1,
            tz: "Etc/UTC".into(),
            paused: None,
            weekly: WeeklySchedule {
                mon: win(),
                tue: win(),
                wed: win(),
                thu: win(),
                fri: win(),
                sat: win(),
                sun: win(),
            },
            overrides: None,
            issued_at: 0,
        }
    }

    fn rule(pkg: &str, blocked: bool, schedule: Option<GrantSchedule>) -> AppRule {
        AppRule {
            pkg: pkg.into(),
            label: None,
            blocked,
            schedule,
        }
    }

    /// Serialize a rule set to the on-wire JSON the runtime would read back.
    fn clause_json(rules: Vec<AppRule>) -> String {
        serde_json::to_string(&GrantAppRules {
            v: APP_RULES_VERSION,
            rules,
            issued_at: 10,
        })
        .unwrap()
    }

    #[test]
    fn blocked_and_allowed_and_scheduled_pkgs_resolve_correctly() {
        let json = clause_json(vec![
            // Blocked outright — always in the set.
            rule("/usr/games/blockme", true, None),
            // Allowed (no schedule) — never in the set.
            rule("/usr/bin/allowme", false, None),
            // Scheduled 07:00–08:00 — in the set only OUTSIDE the window.
            rule(
                "org.example.Scheduled",
                false,
                Some(every_day_window("07:00", "08:00")),
            ),
        ]);

        // 12:00 (outside the window): blocked app + out-of-window scheduled app.
        let out = blocked_pkgs_now(Some(&json), SAT_NOON);
        assert!(out.contains(&"/usr/games/blockme".to_string()));
        assert!(out.contains(&"org.example.Scheduled".to_string()));
        assert!(!out.contains(&"/usr/bin/allowme".to_string()));
        assert_eq!(out.len(), 2);

        // 07:30 (inside the window): only the outright-blocked app remains.
        let inside = blocked_pkgs_now(Some(&json), SAT_0730);
        assert_eq!(inside, vec!["/usr/games/blockme".to_string()]);
    }

    #[test]
    fn absent_clause_blocks_nothing() {
        assert!(blocked_pkgs_now(None, SAT_NOON).is_empty());
    }

    #[test]
    fn garbage_clause_fails_safe_to_empty() {
        assert!(blocked_pkgs_now(Some("not json at all"), SAT_NOON).is_empty());
        // Well-formed JSON of the wrong shape also fails safe.
        assert!(blocked_pkgs_now(Some(r#"{"unexpected":true}"#), SAT_NOON).is_empty());
    }

    #[test]
    fn native_pkg_matches_exact_path_and_binary_name() {
        // Exact absolute-path match.
        assert!(pkg_matches_process(
            "/usr/games/supertuxkart",
            Some("/usr/games/supertuxkart"),
            None,
            &[],
            "",
            None
        ));
        // Same binary launched from a different path (symlink/wrapper).
        assert!(pkg_matches_process(
            "/usr/games/supertuxkart",
            Some("/opt/bin/supertuxkart"),
            None,
            &[],
            "",
            None
        ));
        // Bare command name matches on the binary name.
        assert!(pkg_matches_process(
            "chromium",
            Some("/usr/bin/chromium"),
            None,
            &[],
            "",
            None
        ));
        // A different binary never matches.
        assert!(!pkg_matches_process(
            "/usr/games/supertuxkart",
            Some("/usr/bin/firefox"),
            None,
            &[],
            "",
            None
        ));
        // No exe and no argv0 available → no native match.
        assert!(!pkg_matches_process(
            "/usr/bin/chromium",
            None,
            None,
            &[],
            "",
            None
        ));
    }

    /// Google Chrome, exactly as it ships. `/usr/bin/google-chrome-stable` is a
    /// shell wrapper ending in `exec -a "$0" "$HERE/chrome" "$@"`, so the
    /// resolved exe is `/opt/google/chrome/chrome` while the identity the app
    /// inventory reports (from the `.desktop` Exec) is the `/usr/bin` path.
    /// Before argv0 was consulted, neither path equality nor basename equality
    /// (`chrome` vs `google-chrome-stable`) fired: a guardian could tick
    /// "blocked" on Chrome and get complete silence.
    #[test]
    fn google_chrome_matches_through_its_exec_a_wrapper() {
        assert!(pkg_matches_process(
            "/usr/bin/google-chrome-stable",
            Some("/opt/google/chrome/chrome"),
            Some("/usr/bin/google-chrome-stable"),
            &[],
            "",
            None
        ));
        // The exe alone still does not match — argv0 is what rescues it.
        assert!(!pkg_matches_process(
            "/usr/bin/google-chrome-stable",
            Some("/opt/google/chrome/chrome"),
            None,
            &[],
            "",
            None
        ));
    }

    #[test]
    fn argv0_matches_by_bare_name_too() {
        // A wrapper that sets argv0 to the plain command name.
        assert!(pkg_matches_process(
            "/usr/bin/minecraft-launcher",
            Some("/bin/sh"),
            Some("minecraft-launcher"),
            &[],
            "",
            None
        ));
    }

    /// argv0 is additive: it can only ever match MORE processes. A ward who
    /// forges a misleading argv0 gets their own process killed, never spares
    /// one, so the exe path is still honoured on its own.
    #[test]
    fn argv0_never_overrides_a_matching_exe() {
        assert!(pkg_matches_process(
            "/usr/games/supertuxkart",
            Some("/usr/games/supertuxkart"),
            Some("/usr/bin/totally-innocent"),
            &[],
            "",
            None
        ));
    }

    /// A kernel thread has an empty cmdline. Treating "" as an identity would
    /// make `basename("") == basename("")` match every bare-name pkg — every
    /// kernel thread swept on the first bucket rule.
    #[test]
    fn an_empty_argv0_is_not_an_identity() {
        assert!(!pkg_matches_process(
            "",
            Some("/x"),
            Some(""),
            &[],
            "",
            None
        ));
        assert!(!pkg_matches_process(
            "kthreadd",
            None,
            Some(""),
            &[],
            "",
            None
        ));
    }

    #[test]
    fn flatpak_pkg_matches_its_cgroup_scope_only() {
        let cg = "0::/user.slice/user-1002.slice/user@1002.service/app.slice/\
                  app-flatpak-org.mozilla.firefox-2731.scope\n";
        assert!(pkg_matches_process(
            "org.mozilla.firefox",
            None,
            None,
            &[],
            "",
            Some(cg)
        ));
        // A prefix app-id must NOT match a longer scope id.
        let cg_longer = "app-flatpak-org.mozilla.firefoxNightly-99.scope";
        assert!(!pkg_matches_process(
            "org.mozilla.firefox",
            None,
            None,
            &[],
            "",
            Some(cg_longer)
        ));
        // A different flatpak app never matches.
        assert!(!pkg_matches_process(
            "org.videolan.VLC",
            None,
            None,
            &[],
            "",
            Some(cg)
        ));
        // No cgroup → no flatpak match.
        assert!(!pkg_matches_process(
            "org.mozilla.firefox",
            None,
            None,
            &[],
            "",
            None
        ));
        // A flatpak id is not matched by an exe path or argv0 (cgroup is the
        // signal) — otherwise a ward could dodge a flatpak rule by launching
        // the same binary outside the sandbox under a different identity.
        assert!(!pkg_matches_process(
            "org.mozilla.firefox",
            Some("/app/bin/firefox"),
            Some("org.mozilla.firefox"),
            &[],
            "",
            None
        ));
    }

    #[test]
    fn empty_pkg_never_matches() {
        assert!(!pkg_matches_process(
            "",
            Some("/usr/bin/anything"),
            Some("/usr/bin/anything"),
            &[],
            "",
            Some("x")
        ));
    }

    // ---- cmdline: identity (honest attribution) ---------------------------

    /// decented's actual question: can a ward dodge the launcher? A JVM started
    /// with Minecraft's real main class matches `cmdline:` regardless of what
    /// started it or what its resolved exe is — and does NOT match the
    /// official launcher's path, because it isn't that process.
    fn minecraft_jvm_cmdline() -> Vec<String> {
        vec![
            "java".into(),
            "-cp".into(),
            "/home/kid/.minecraft/libraries/x.jar".into(),
            "net.minecraft.client.main.Main".into(),
            "--gameDir".into(),
            "/home/kid/.minecraft".into(),
        ]
    }

    #[test]
    fn a_jvm_matches_its_cmdline_identity_but_not_the_launcher_path() {
        let cmdline = minecraft_jvm_cmdline();
        let joined = cmdline.join(" ");
        assert!(pkg_matches_process(
            "cmdline:net.minecraft.client.main.Main",
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some("java"),
            &cmdline,
            &joined,
            None
        ));
        // The SAME process is not an instance of the launcher's path identity
        // — it genuinely is a different binary.
        assert!(!pkg_matches_process(
            "/usr/bin/minecraft-launcher",
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some("java"),
            &cmdline,
            &joined,
            None
        ));
    }

    /// The whole reason `cmdline:` names the game rather than `java` itself:
    /// an unrelated Java program (an IDE, say) must not match at all.
    #[test]
    fn an_unrelated_java_process_matches_no_cmdline_identity() {
        let cmdline = vec![
            "java".to_string(),
            "-jar".to_string(),
            "/opt/foo/foo.jar".to_string(),
        ];
        let joined = cmdline.join(" ");
        assert!(!pkg_matches_process(
            "cmdline:net.minecraft.client.main.Main",
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some("java"),
            &cmdline,
            &joined,
            None
        ));
    }

    /// `cmdline:short` — 5 bytes after the prefix, below `MIN_CMDLINE_NEEDLE`
    /// — must match nothing, even a process whose cmdline literally contains
    /// the word "short". A short needle is "too thin to mean anything" and
    /// the fail direction here is fail-OPEN (matches nothing), never a
    /// blanket rule nobody authored.
    #[test]
    fn a_malformed_short_needle_matches_nothing() {
        let cmdline = vec!["anything".to_string(), "short".to_string()];
        let joined = cmdline.join(" ");
        assert!(!pkg_matches_process(
            "cmdline:short",
            Some("/usr/bin/anything"),
            Some("anything"),
            &cmdline,
            &joined,
            None
        ));
    }

    /// A sanctioned YouTube site-app window, as `/proc/<pid>/cmdline` reads
    /// it back. Rendered by the one function that also defines what the
    /// checker accepts, so this can never drift from a real launch.
    fn youtube_site_cmdline() -> Vec<String> {
        let app = charter_proto::LearningApp {
            id: "youtube".into(),
            label: "YouTube".into(),
            kind: charter_proto::LearningAppKind::Site,
            domains: vec!["youtube.com".into()],
            url: Some("https://www.youtube.com/".into()),
            exec: None,
            trusted: false,
            // A costing site: the window is still pinned, the seconds are not
            // free. Exactly the shape this identity form exists to serve.
            free: Some(false),
        };
        crate::site_app::sanctioned_argv("/usr/bin/chromium", &app, "/home/kid")
    }

    /// The whole point of the `site:` form: "YouTube is half an hour a day"
    /// is unsayable without it, because every site app runs as the same
    /// Chromium binary and the path form cannot tell two of them apart.
    #[test]
    fn a_site_identity_matches_its_own_window_and_not_another_site_app() {
        let cmdline = youtube_site_cmdline();
        let joined = cmdline.join(" ");
        assert!(pkg_matches_process(
            "site:youtube",
            Some("/usr/bin/chromium"),
            Some("/usr/bin/chromium"),
            &cmdline,
            &joined,
            None
        ));
        // The Khan window is the SAME binary. Only the marker separates them,
        // which is precisely why the path form is useless here.
        assert!(!pkg_matches_process(
            "site:khan-academy",
            Some("/usr/bin/chromium"),
            Some("/usr/bin/chromium"),
            &cmdline,
            &joined,
            None
        ));
    }

    /// The marker is matched as a WHOLE argv token, never as a substring.
    /// Otherwise `site:youtube` would also match a window marked
    /// `--class=charter-youtube-kids`, quietly metering one site against
    /// another site's allowance.
    #[test]
    fn a_site_identity_does_not_match_a_longer_marker_by_prefix() {
        let mut cmdline = youtube_site_cmdline();
        for tok in cmdline.iter_mut() {
            if tok.starts_with("--class=") {
                *tok = "--class=charter-youtube-kids".to_string();
            }
        }
        let joined = cmdline.join(" ");
        assert!(!pkg_matches_process(
            "site:youtube",
            Some("/usr/bin/chromium"),
            Some("/usr/bin/chromium"),
            &cmdline,
            &joined,
            None
        ));
    }

    /// THE SAME ORDERING HAZARD as `cmdline:`, for the same reason: a site id
    /// may legitimately contain a dot (`site:my.school`) and no slash, which
    /// is exactly `is_flatpak_id`. Checked with no cgroup at all, so a
    /// flatpak-routed check could only fail.
    #[test]
    fn site_id_is_never_treated_as_a_flatpak_id() {
        let cmdline = vec![
            "/usr/bin/chromium".to_string(),
            "--class=charter-my.school".to_string(),
        ];
        let joined = cmdline.join(" ");
        assert!(pkg_matches_process(
            "site:my.school",
            Some("/usr/bin/chromium"),
            Some("/usr/bin/chromium"),
            &cmdline,
            &joined,
            None,
        ));
    }

    /// The SPARING direction is not this matcher's business. A ward-owned
    /// Chromium wearing a forged marker still MATCHES here — and that is
    /// correct, because every use of this matcher restricts: it meters a
    /// bucket, blocks an app, or decides a kill. Forging your way into being
    /// metered is self-harm. Free time goes through
    /// `site_app::sanctioned_app_id`, which compares the whole argv multiset
    /// AND demands a root-owned runtime.
    #[test]
    fn a_forged_marker_still_matches_because_this_matcher_only_restricts() {
        let cmdline = vec![
            "/home/kid/my-chromium".to_string(),
            "--class=charter-youtube".to_string(),
        ];
        let joined = cmdline.join(" ");
        assert!(pkg_matches_process(
            "site:youtube",
            Some("/home/kid/my-chromium"),
            Some("/home/kid/my-chromium"),
            &cmdline,
            &joined,
            None,
        ));
    }

    /// A malformed `site:` (empty id) matches nothing rather than everything
    /// — same fail-open direction a short `cmdline:` needle takes.
    #[test]
    fn an_empty_site_id_matches_nothing() {
        let cmdline = youtube_site_cmdline();
        let joined = cmdline.join(" ");
        assert!(!pkg_matches_process(
            "site:",
            Some("/usr/bin/chromium"),
            Some("/usr/bin/chromium"),
            &cmdline,
            &joined,
            None,
        ));
    }

    /// THE ORDERING HAZARD: `is_flatpak_id` is `!contains('/') &&
    /// contains('.')`, which is ALSO true of
    /// `cmdline:net.minecraft.client.main.Main` (no slash, has dots). If the
    /// flatpak branch ran first, this identity would be silently checked
    /// against the cgroup instead of the cmdline and could never match
    /// anything — a rule the guardian believes is armed, failing open in
    /// total silence. Proven here with NO flatpak cgroup present at all: the
    /// match must still succeed via the cmdline path.
    #[test]
    fn cmdline_id_is_never_treated_as_a_flatpak_id() {
        let cmdline = minecraft_jvm_cmdline();
        let joined = cmdline.join(" ");
        assert!(pkg_matches_process(
            "cmdline:net.minecraft.client.main.Main",
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some("java"),
            &cmdline,
            &joined,
            None, // no cgroup at all — a flatpak-routed check could only fail
        ));
        // Even a cgroup that carries some OTHER flatpak scope must not
        // interfere — the cmdline form never consults cgroup.
        assert!(pkg_matches_process(
            "cmdline:net.minecraft.client.main.Main",
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some("java"),
            &cmdline,
            &joined,
            Some("0::/user.slice/app-flatpak-org.mozilla.firefox-1.scope"),
        ));
    }

    /// The user-visible point of the feature: a launcher renamed to defeat
    /// path/basename matching (`/home/kid/mc` instead of
    /// `/usr/bin/minecraft-launcher`) is still caught, because `cmdline:`
    /// never looks at the exe path at all — it reads what the process itself
    /// is actually running.
    #[test]
    fn a_renamed_launcher_binary_is_still_caught_by_its_cmdline() {
        let cmdline = minecraft_jvm_cmdline();
        let joined = cmdline.join(" ");
        // Path identity misses entirely — the exe/argv0 don't name the game.
        assert!(!pkg_matches_process(
            "/usr/bin/minecraft-launcher",
            Some("/home/kid/mc"),
            Some("/home/kid/mc"),
            &cmdline,
            &joined,
            None
        ));
        // cmdline identity still lands.
        assert!(pkg_matches_process(
            "cmdline:net.minecraft.client.main.Main",
            Some("/home/kid/mc"),
            Some("/home/kid/mc"),
            &cmdline,
            &joined,
            None
        ));
    }

    /// The needle may also be found only once the tokens are joined with a
    /// space — a substring that straddles a token boundary is still a
    /// substring of "the command line" in the sense a guardian would mean it.
    #[test]
    fn a_needle_spanning_two_tokens_matches_via_the_joined_line() {
        let cmdline = vec![
            "java".to_string(),
            "net.minecraft.client.main.Main".to_string(),
            "--gameDir".to_string(),
        ];
        let joined = cmdline.join(" ");
        // Spans the space between the class and the next flag.
        assert!(pkg_matches_process(
            "cmdline:Main --gameDir",
            None,
            None,
            &cmdline,
            &joined,
            None
        ));
    }

    /// Asymmetry companion (spec §2.2): capping/blocking is RESTRICTIVE, so it
    /// is self-harm only and must apply regardless of who owns the binary —
    /// `pkg_matches_process` takes no uid at all, by construction, so a
    /// user-planted binary is blocked exactly like a system one. The
    /// ownership gate lives ONLY in `focus::classify`, where a match would
    /// otherwise grant FREE time.
    #[test]
    fn a_cmdline_identity_still_blocks_regardless_of_who_owns_the_binary() {
        let cmdline = minecraft_jvm_cmdline();
        let joined = cmdline.join(" ");
        assert!(pkg_matches_process(
            "cmdline:net.minecraft.client.main.Main",
            Some("/home/kid/mc"), // a ward-owned, renamed binary
            Some("/home/kid/mc"),
            &cmdline,
            &joined,
            None
        ));
    }
}
