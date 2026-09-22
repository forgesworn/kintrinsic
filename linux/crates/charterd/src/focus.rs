//! Foreground-window bucket attribution: is the ward, RIGHT NOW, inside a
//! guardian-designated learning app?
//!
//! The identity ladder (weakest → strongest) is deliberate: window titles are
//! page-controlled and are NOT consulted; matching is on OS-side identities
//! only — the launcher cmdline markers for site apps, the executable path /
//! flatpak id for native apps. The probe shells out to `charter-xclients` with
//! the active session's DISPLAY/XAUTHORITY (per-command env — charterd is
//! multi-threaded, so no process-global env mutation) under the same
//! hard-timeout discipline as `loginctl`: a wedged X server degrades
//! attribution, never the tick loop.
//!
//! **Window→process attribution comes from the X server, never from a
//! property.** This module used to read `_NET_WM_PID` and `_NET_CLIENT_LIST`
//! with `xprop`, and X lets every client on a display rewrite every other
//! client's properties: one `xprop -remove _NET_WM_PID` made a metered app
//! unattributable, and under `TimeModel::Named` an unattributable window
//! charged nothing at all. `charter-xclients` asks XRes `QueryClientIds`
//! instead — the server derives the pid from the owning client's socket peer
//! credentials — and takes the window set from the real window tree as well as
//! the property, so the property can only ever ADD windows now.
//!
//! Fail-closed: ANY failure (no display, no active window, no PID, unreadable
//! /proc) attributes `Bucket::Screen` — an error can never make time free.

use std::process::Command;

use charter_proto::{LearningApp, LearningAppKind};
use charter_schedule::Bucket;

/// What the X server says is on the display right now, as one
/// `charter-xclients` run reports it.
///
/// Every pid here came from XRes `QueryClientIds` — the server's own record of
/// which connection created the window, taken from that socket's peer
/// credentials. Nothing in this struct is readable, writable or deletable by a
/// client on the display, which is the whole point of it: the properties this
/// module used to read (`_NET_WM_PID`, `_NET_CLIENT_LIST`) are all three.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XSnapshot {
    /// `GetInputFocus`, resolved to an owning pid. `None` when the focus is
    /// `None`/`PointerRoot`/the root, or when the server would not name a pid.
    focus: Option<u32>,
    /// The root's `_NET_ACTIVE_WINDOW`, resolved the same way. Advisory — the
    /// window manager writes it and the ward can rewrite it, which is exactly
    /// why it is kept SEPARATE from `focus` rather than merged into it.
    active: Option<u32>,
    /// Every distinct window found, and its owning pid. `None` is a window the
    /// server would not attribute (a TCP / forwarded client, or one that closed
    /// mid-probe) — evidence of a window, never evidence of no window.
    windows: Vec<(String, Option<u32>)>,
    /// One of the helper's budgets truncated this answer, so `windows` is a
    /// PREFIX of what is on the display rather than the whole of it.
    ///
    /// This is the flood defence's other half. A cap that quietly returned a
    /// short list would be a bypass in its own right: open more windows than
    /// the helper will walk, delete `_NET_CLIENT_LIST`, and the snapshot comes
    /// back with no app in it and no `-` either — which reads as "nothing
    /// costing is open" and charges nothing. Treated everywhere below exactly
    /// like an unattributable window: it widens the process set and withdraws
    /// the free-learning credit, never the reverse.
    capped: bool,
    /// The display's DPMS power level as the helper read it off the X server
    /// (`on`/`standby`/`suspend`/`off`/`unknown`), or `None` when the helper
    /// said nothing at all — an older build against a newer daemon. Both
    /// unknowns mean the same thing here, and [`activity_from_dpms`] says what
    /// that is.
    dpms: Option<String>,
    /// Milliseconds since the X server last saw real user input, from the
    /// XScreenSaver extension (`QueryInfo.ms_since_user_input`), or `None` when
    /// the helper said nothing — no extension, an older helper, a failed probe.
    /// `None` is charged, like every other unanswered question here.
    idle_ms: Option<u64>,
}

impl XSnapshot {
    /// The display's DPMS power level as the helper read it, verbatim.
    pub fn dpms_level(&self) -> Option<&str> {
        self.dpms.as_deref()
    }
    /// Milliseconds since the X server last saw user input, if known.
    pub fn idle_ms(&self) -> Option<u64> {
        self.idle_ms
    }
}

/// How many CONSECUTIVE ticks must read the same powered-down DPMS level
/// before this daemon will stop charging. At the 2 s tick, three ticks is
/// 4–6 s of the monitor genuinely being off.
pub const IDLE_CONSECUTIVE_TICKS: u32 = 3;

/// How long the X server must have seen no input at all, on the tick that
/// declares Idle. Comfortably longer than the tick, so a single sample landing
/// in a gap between keystrokes cannot clear it.
pub const IDLE_MIN_MS: u64 = 6_000;

/// The per-tick history behind the Idle decision — the DPMS-toggle fix.
///
/// # Why one sample was not enough
///
/// [`activity_from_dpms`] reads one instantaneous DPMS level per 2 s tick, and
/// the ward can move that level whenever they like:
///
/// ```text
/// while :; do xset dpms force off; sleep .05; xset dpms force on; done
/// ```
///
/// The monitor is blanked for 50 ms at a time — imperceptible, the screen stays
/// entirely usable — and roughly half the daemon's samples land in an `off`
/// window. Half the ward's screen time simply is not charged, from a one-line
/// shell loop, which is precisely the class of bypass the move off logind's
/// `IdleHint` was meant to end.
///
/// # The shape of the fix
///
/// Two independent facts, both from the X server, both required:
///
/// * **the level must be STEADY.** A tick whose DPMS level differs from the
///   previous tick's is Active, whatever it reads, and the streak restarts. A
///   toggle loop never produces [`IDLE_CONSECUTIVE_TICKS`] identical
///   powered-down samples in a row, so it never stops the clock — while a
///   monitor that is actually off reads `off` every tick for as long as it is
///   off.
/// * **the server must have seen no INPUT.** `ms_since_user_input` is X server
///   state, reset by real keyboard and pointer events; a ward driving a toggle
///   loop is by definition at the machine using it, so their idle time keeps
///   resetting. An unknown or missing idle time is Active — the charging
///   direction, as everywhere else in this file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ActivityHistory {
    /// The normalised DPMS level the previous tick read, or `None` for "no
    /// display / the helper said nothing", which is itself a level for the
    /// purposes of the "did it change?" test.
    last_level: Option<String>,
    /// Consecutive ticks that have read the SAME powered-down level, this one
    /// included. Zeroed by any powered-on or unknown tick.
    powered_down_ticks: u32,
}

impl ActivityHistory {
    /// Fold one tick's display evidence in and answer whether this tick is
    /// charged. Pure apart from the history it carries, so the whole rule is
    /// unit-testable without a display.
    pub fn observe(
        &mut self,
        dpms: Option<&str>,
        idle_ms: Option<u64>,
    ) -> charter_schedule::Activity {
        let level = dpms.map(|s| s.trim().to_ascii_lowercase());
        let powered_down = activity_from_dpms(level.as_deref()) == charter_schedule::Activity::Idle;
        let changed = level != self.last_level;
        self.last_level = level;
        if !powered_down {
            self.powered_down_ticks = 0;
            return charter_schedule::Activity::Active;
        }
        self.powered_down_ticks = if changed {
            1
        } else {
            self.powered_down_ticks.saturating_add(1)
        };
        if self.powered_down_ticks >= IDLE_CONSECUTIVE_TICKS
            && idle_ms.is_some_and(|ms| ms >= IDLE_MIN_MS)
        {
            charter_schedule::Activity::Idle
        } else {
            charter_schedule::Activity::Active
        }
    }
}

/// G1 (03b-linux-charterd-bins-matching): **screen time is time the screen is
/// powered on.** A ward cannot use a monitor that is off, and a monitor that is
/// off is the one thing about a desktop session the ward cannot fake: turning
/// it off means actually turning it off.
///
/// This replaces logind's `IdleHint`/`LockedHint`, which the meter used to
/// trust and which are **ward-settable**. Any process in the session can set
/// them over the system bus with no polkit prompt — verified:
///
/// ```text
/// busctl --system call org.freedesktop.login1 \
///     /org/freedesktop/login1/session/self \
///     org.freedesktop.login1.Session SetIdleHint b true
/// ```
///
/// A ward looping that was never charged a second. logind's hints are not used
/// anywhere in this daemon any more, and must not come back.
///
/// The residual is the same one [`crate::focus`] already carries for
/// attribution (03b-B2): a ward who puts an X **proxy** between their apps and
/// the real server can lie to this probe about DPMS exactly as they can lie
/// about window ownership. That is one hole, already named, not a new one.
///
/// Anything that is not positively a powered-down monitor — `on`, `unknown`, a
/// helper that failed, a Wayland seat with no X server to ask — is `Active`.
/// Failing toward charging is the direction every probe in this daemon fails.
///
/// # This answers ONE sample; the decision needs [`ActivityHistory`]
///
/// On its own this mapping was gameable by a `xset dpms force off; sleep .05;
/// xset dpms force on` loop — see [`ActivityHistory`], which is what the
/// daemon actually charges from. This function survives as the single-sample
/// predicate that fold uses, and as the definition of which levels count as
/// powered down.
///
/// # The residual the pair still leaves
///
/// A ward who views their **own X display remotely** — `x11vnc` against this
/// session from another machine, with the local panel powered off and no local
/// input — reads as a monitor that is off with no user input, and is not
/// charged. It is the same class as the X-proxy residual (03b-B2): everything
/// this daemon knows about "is someone using this seat" is asked of the X
/// server, and a remote viewer is a use of the session the X server does not
/// distinguish from nobody being there. Closing it means moving to the
/// process-table model of activity (what is running and doing work, not what
/// the display says), which is a product call, not a patch — it changes what
/// screen time MEANS for every family, including the honest ones.
pub fn activity_from_dpms(level: Option<&str>) -> charter_schedule::Activity {
    match level.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("standby") | Some("suspend") | Some("off") => charter_schedule::Activity::Idle,
        _ => charter_schedule::Activity::Active,
    }
}

/// Parse `charter-xclients` stdout. Pure, so the whole wire format is tested
/// without a display.
///
/// Strict on the version line and forgiving about everything after it: a line
/// kind this build does not know is skipped rather than failing the snapshot,
/// so a newer helper against an older daemon degrades to "fewer facts" instead
/// of "no display". The version line itself is not negotiable — a `v2` that
/// reused `win` for something else would otherwise be silently mis-metered.
pub fn parse_xsnapshot(out: &str) -> Option<XSnapshot> {
    let mut lines = out.lines();
    if lines.next()?.trim() != "v1" {
        return None;
    }
    let mut snap = XSnapshot::default();
    for line in lines {
        let mut f = line.split_whitespace();
        match (f.next(), f.next(), f.next(), f.next()) {
            (Some("capped"), None, _, _) => snap.capped = true,
            (Some("dpms"), Some(level), None, _) => snap.dpms = Some(level.to_string()),
            (Some("idle_ms"), Some(ms), None, _) => snap.idle_ms = ms.parse().ok(),
            (Some("focus"), Some(pid), None, _) => snap.focus = parse_pid_field(pid),
            (Some("active"), Some(pid), None, _) => snap.active = parse_pid_field(pid),
            (Some("win"), Some(id), Some(pid), None) => {
                snap.windows.push((id.to_string(), parse_pid_field(pid)));
            }
            _ => {}
        }
    }
    Some(snap)
}

/// One `<pid|->` field. `-` and anything unparseable are the same answer: the
/// server did not name a pid for this window.
fn parse_pid_field(field: &str) -> Option<u32> {
    field.parse().ok()
}

/// Every named allowance represented by the processes currently holding a
/// window, deduplicated — what [`charter_spine::multi_child::MultiChildEnforcer::tick_open`]
/// charges.
///
/// **Deduplication is the count-once rule.** Two members of Play open together
/// spend Play once, because wall-clock time is not duplicable. Returned sorted
/// so a tick's result depends on what is open and never on window order, which
/// the window manager is free to change under us.
///
/// Pure: the `/proc` reading and the X probe live in [`open_bucket_ids_tick`];
/// the decision lives here so the count-once rule is testable without a live
/// display, which is exactly what the focused-window path never was.
pub fn open_bucket_ids<F>(
    procs: &[FocusedProcess],
    buckets: &charter_schedule::GrantBuckets,
    lookup: F,
) -> Vec<String>
where
    F: Fn(u32) -> Option<crate::ancestry::ProcId> + Copy,
{
    let mut ids: Vec<String> = procs
        .iter()
        .filter_map(|p| bucket_id_for_with(p, buckets, lookup))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
}

/// The process identity attribution matches on.
#[derive(Debug, Clone, Default)]
pub struct FocusedProcess {
    /// NUL-split /proc/<pid>/cmdline.
    pub cmdline: Vec<String>,
    /// readlink /proc/<pid>/exe (target path).
    pub exe: Option<String>,
    /// Owner uid of the exe file (root-owned = enforcement-grade identity).
    pub exe_uid: Option<u32>,
    /// /proc/<pid>/cgroup — carries the flatpak systemd scope, which is how a
    /// sandboxed app is identified. Needed so bucket attribution can use the
    /// SAME matcher as the kill path (see `bucket_id_for`).
    pub cgroup: Option<String>,
    /// The focused window's pid, so bucket attribution can walk UP the process
    /// tree: the game a launcher started is what the ward is looking at.
    pub pid: u32,
}

/// Pure matcher: which bucket does this focused process credit, given the
/// child's learning apps? Empty `apps` (no clause / paused) always → Screen.
///
/// `user_installed` is the device inventory's own set of identities flagged
/// `userInstalled` (§2.4) — flatpak/bare-name ids that came from a managed
/// user's OWN writable dirs rather than a root-owned launcher dir. A learning
/// identity in that set never credits `Bucket::Learning`: `exe_uid` cannot
/// gate the flatpak/bare-name arm at all (a real flatpak and a `flatpak
/// install --user` impostor both land on the same
/// `app-flatpak-<id>-<pid>.scope` cgroup and the same root-owned
/// `/usr/bin/bwrap`), so the inventory's own knowledge is the only signal
/// that can tell them apart. Capping/blocking (self-harm only) are
/// unaffected — this only ever refuses the FREE-time grant.
///
/// `None` means the inventory is UNKNOWN — the startup scan has not answered
/// yet (a ward-mountable hung FUSE at `~/.local/share/applications` can hold
/// it there deliberately, no privilege needed). An unknown inventory is NOT
/// an empty one: empty would read "nothing is user-installed" and hand the
/// flatpak/bare-name arm's free time straight back to the `flatpak install
/// --user` impostor §2.4 exists to refuse. While unknown, the arm that
/// DEPENDS on the inventory fails CLOSED to Screen (learning's documented
/// fail direction); a guardian-vouched `trusted` identity never depended on
/// the inventory, so it is unaffected, as are the path and `cmdline:` arms.
pub fn classify(
    p: &FocusedProcess,
    apps: &[LearningApp],
    user_installed: Option<&std::collections::BTreeSet<String>>,
) -> Bucket {
    // Site apps: the whole command line must be a sanctioned launch, not merely
    // one that carries the right-looking markers. Checking for a `--class` plus
    // SOME `--host-resolver-rules` token — which is what this used to do —
    // never compared the pin's VALUE, so `--host-resolver-rules=MAP nothing`
    // beside a forged class read as free learning time in an unpinned browser.
    // `site_app` owns the predicate and the sweep uses the very same one, so
    // what is metered is exactly what is spared.
    // A sanctioned window is not automatically a FREE one. `learning` defines
    // the pinned site apps AND, historically, granted them free time — one
    // clause doing two jobs, which is what made "YouTube in its own locked-down
    // window, but half an hour a day" impossible to say. `LearningApp::free`
    // splits them: the launcher still materialises and the resolver pin still
    // holds, but a `free: Some(false)` entry is charged like anything else and
    // earns its allowance from a `site:<id>` bucket instead.
    //
    // Note this checks the SANCTIONED id, never the class marker — a costing
    // site must not be able to buy free time by wearing a free site's marker,
    // and `sanctioned_app_id` is the predicate that compares the whole argv
    // multiset against a root-owned runtime.
    if let Some(id) = crate::site_app::sanctioned_app_id(&p.cmdline, p.exe_uid, apps) {
        let free = apps
            .iter()
            .find(|a| a.id == id)
            .is_some_and(charter_proto::LearningApp::is_free);
        if free {
            return Bucket::Learning;
        }
        // Sanctioned, but costing. Settled here rather than falling through to
        // the per-app loop below, which would test this Chromium against every
        // NATIVE identity in the clause and could only produce a wrong answer.
        return Bucket::Screen;
    }
    // Computed once per process, not once per app tested against it: the
    // tick loop checks one focused process against every learning app in the
    // clause, so this allocation must not be repeated per app.
    let cmdline_joined = p.cmdline.join(" ");
    for app in apps {
        // Each arm below decides ONLY whether the process's identity matches
        // — `hit` — never whether a match may be free. THE ASYMMETRY (spec
        // §2.2) is applied ONCE, structurally, below the match: gating each
        // arm with its own `&& enforcement_grade` is exactly how the flatpak
        // arm shipped ungated in the first place (a ward could `cp
        // any-gui-program ~/bwrap && ~/bwrap org.kde.gcompris` and get the
        // WHOLE session free) — an ok-looking future arm can forget an inline
        // `&&` just as easily as this one did. One gate, applied to every
        // arm by construction, cannot be forgotten per-arm again.
        //
        // A guardian-vouched (`trusted`) app is exempt — the guardian vouched
        // for a ward-writable path/identity themselves. A root-owned exe is
        // exempt because the ward could not have written or replaced it
        // (enforcement-grade). Capping (buckets) and blocking (appRules) have
        // NO such gate: `pkg_matches_process` (which those paths use) takes
        // no uid at all, because restricting a user-owned binary is
        // self-harm only, never a way to make time free.
        // `(hit, user_installed_hit)` — the second element is set ONLY by the
        // flatpak/bare-name arm (the one §2.4 actually concerns; the path and
        // `cmdline:` arms already have adequate ownership gates and are
        // unaffected). Kept OUT of `hit` itself and applied structurally
        // below, alongside the trusted exemption, rather than inline in the
        // arm — I7: inlining it as `hit = matches && !user_installed` (as a
        // first cut of this did) zeroes `hit` before the trusted check below
        // ever runs, so a guardian who deliberately `flatpak install --user`s
        // an app FOR their child and vouches for it (`trusted: true`) would
        // have it silently charged to the screen budget — the exact "learning
        // never worked and said nothing" failure class this feature exists to
        // close, not reproduce.
        let (hit, user_installed_hit) = match app.kind {
            // Already decided above — a site app is sanctioned or it is not.
            LearningAppKind::Site => (false, false),
            LearningAppKind::Native => match app.exec.as_deref() {
                Some(exec) if exec.starts_with('/') => {
                    // Path identity: the running exe IS that file. A
                    // guardian-vouched `trusted` app (ward-writable project)
                    // additionally matches by argv — scripts run under an
                    // interpreter, so /proc/exe is /bin/sh or python, and the
                    // script path is an argument.
                    let h = (p.exe.as_deref() == Some(exec))
                        || (app.trusted && p.cmdline.iter().any(|a| a == exec));
                    (h, false)
                }
                // MUST be checked before the flatpak arm below: `is_flatpak_id`
                // in `app_rules` is `!contains('/') && contains('.')`, which is
                // ALSO true of `cmdline:net.minecraft.client.main.Main` — the
                // same ordering hazard `pkg_matches_process` guards against.
                Some(exec) if charter_schedule::is_cmdline_id(exec) => {
                    let h = charter_schedule::cmdline_needle(exec).is_some_and(|needle| {
                        p.cmdline.iter().any(|t| t.contains(needle))
                            || cmdline_joined.contains(needle)
                    });
                    (h, false)
                }
                Some(flatpak_id) => {
                    // Flatpak identity: the sandbox wrapper carries the app id
                    // verbatim in its argv (best-effort v1). §2.4: `exe_uid`
                    // alone cannot tell a real flatpak from a `flatpak
                    // install --user` impostor of the SAME app id (identical
                    // cgroup scope, identical root-owned bwrap) — the
                    // inventory's own knowledge is the only signal that can,
                    // hence the second element here.
                    let h = p
                        .exe
                        .as_deref()
                        .is_some_and(|e| e.ends_with("bwrap") || e.ends_with("flatpak"))
                        && p.cmdline.iter().any(|a| a == flatpak_id);
                    // An UNKNOWN inventory (None) must answer as if the
                    // identity were user-installed — fail closed. `None` and
                    // "empty set" are different answers here by design.
                    let ui = user_installed.is_none_or(|set| set.contains(flatpak_id));
                    (h, ui)
                }
                None => (false, false),
            },
        };
        // A guardian-vouched (`trusted`) app is exempt from BOTH gates below —
        // the ownership one (pre-existing) and the user-installed one (I7) —
        // for the same reason: the guardian named this exact identity
        // themselves, so Kintrinsic has independent confirmation it isn't a
        // ward's own forgery, wherever it happened to be installed from.
        if hit && (app.trusted || (p.exe_uid == Some(0) && !user_installed_hit)) {
            return Bucket::Learning;
        }
    }
    Bucket::Screen
}

/// Every `pkg`-style identity the child's clauses currently name, across the
/// four kinds sharing this vocabulary (native learning apps, buckets,
/// appRules, the standing `apps` clause) — used ONLY to decide whether
/// [`is_unrecognised`] should stay silent about a process the guardian
/// already named somewhere, regardless of the enforcement/free-time
/// semantics that ID carries elsewhere. Site learning apps are excluded on
/// purpose: they only ever run as root-owned Chromium, which is not an
/// root-owned Chromium, launched by Kintrinsic's own shim — so
/// [`is_unrecognised`]'s "not vouched for" test cannot fire on them at all,
/// and they are settled before this list is consulted.
pub fn governed_pkgs(
    learning: &[LearningApp],
    buckets: &charter_schedule::GrantBuckets,
    app_rule_pkgs: &[String],
    apps_clause_pkgs: &[String],
) -> Vec<String> {
    let mut out: Vec<String> = learning
        .iter()
        .filter(|a| a.kind == LearningAppKind::Native)
        .filter_map(|a| a.exec.clone())
        .collect();
    if !buckets.is_paused() {
        for b in &buckets.buckets {
            out.extend(b.apps.iter().cloned());
        }
    }
    out.extend(app_rule_pkgs.iter().cloned());
    out.extend(apps_clause_pkgs.iter().cloned());
    out
}

/// §2.3, REDEFINED: unrecognised time is foreground time that is **not
/// vouched for** and **not matched by any identity the child's clauses
/// currently name**.
///
/// **"Not vouched for" is ONE thing:** the exe's owner is a KNOWN, non-root
/// uid — the ward could have written or replaced that binary. That is the
/// whole condition.
///
/// # What this counter does NOT cover, and why (D1, round 5)
///
/// A **ward-authored payload run by a root-owned interpreter** — `java -jar
/// ~/x.jar`, `python3 ~/game.py` — is **deliberately not counted**. It was
/// built twice and withdrawn twice, because it is not decidable from the
/// outside: *"a root-owned program with the ward's own file in its argv"* is
/// the shape of a repacked jar AND the shape of a system app opening the
/// child's own document, and every rule tried to separate them produced a
/// FALSE ACCUSATION on a real machine:
///
/// - a **suffix + exec-bit** test flagged `xed ~/homework.py` and `evince
///   ~/essay.pdf` — an editor opening a child's homework;
/// - an **interpreter allowlist** keyed on the exe basename flagged
///   `drawing ~/art.png`: this distro ships **130 `#!/usr/bin/python*`
///   launchers in `/usr/bin` alone** (581 shebang launchers in total,
///   including Mint's own image editor and Cinnamon's tools), every one of
///   which resolves `/proc/<pid>/exe` to `/usr/bin/python3.12` with the
///   script and the child's document side by side in argv. Verified live.
///
/// Its only prize was the repacked jar — the least likely bypass and the most
/// skilled — and the governing principle is not negotiable: **absence of
/// evidence must never become a finding.** A false *"Kintrinsic couldn't
/// identify 3h"* about a child is worse than missing a bypass. So the counter
/// is scoped to what the kernel can actually settle: **software running from
/// a ward-owned EXECUTABLE.** That still catches the realistic routes — a
/// renamed binary, a home-installed launcher (Prism/MultiMC and its bundled
/// JRE), an AppImage — all of which report a ward-owned `/proc/<pid>/exe`.
///
/// This is deliberate UNDER-reporting, stated plainly rather than papered
/// over: a ward who runs their own jar through the system JVM is not counted,
/// and Kintrinsic says so instead of guessing.
///
/// **UNKNOWN ownership never accrues.** `exe_uid == None` means the kernel
/// could not tell us who owns the running image at all — the process exited
/// between the probe and the stat, or its `/proc/<pid>/exe` was never
/// readable to us. Absence of evidence is not a finding: the same fail
/// direction `attribute_tick` already takes when the probe itself fails.
///
/// **SANDBOXED apps are NOT in that category any more (corrected round 5).**
/// An earlier revision of this doc said a flatpak's exe "stats to nothing"
/// and therefore never accrues. That was true only while ownership was read
/// by rendering the link to a path string: a flatpak's exe renders
/// `/app/bin/<foo>`, which exists inside the sandbox's mount namespace and
/// not on the host, so the stat failed. Stat'ing the magic symlink (D2)
/// resolves the real inode regardless of any namespace, so ownership is now
/// KNOWN for these processes. Measured live on a real system flatpak
/// (`io.github.input_leap.input-leap`): rendered `/app/bin/input-leap`, absent
/// on the host, old lookup `None`, new lookup `Some(0)`. The verdicts:
///
/// - a **system** flatpak (`/var/lib/flatpak`, root-owned) reads `Some(0)` —
///   still silent, by the ownership rule rather than by a failed stat;
/// - a **`flatpak install --user`** app (under the ward's own home,
///   ward-owned) reads `Some(<ward uid>)` and **ACCRUES** unless a clause
///   names it. That is a CHANGE, and it is correct: it is ward-installed
///   software the guardian never named — the "ward installed Prism" case this
///   counter exists for. Confirmed live with a ward-owned binary behind a
///   namespace-only path: old `None`, new `Some(1000)`.
///
/// **The suppression test trusts KERNEL-RESOLVED IDENTITY ONLY**: a ward-owned
/// exe is spared only by an exact governed match on **the kernel-resolved
/// `exe` path** — [`crate::app_rules::pkg_names_process_exactly`], on this
/// process or on an ancestor's own exe — never the wide matcher enforcement
/// uses.
///
/// Of the fields a matcher can read, only `/proc/<pid>/exe` comes from the
/// kernel; argv0, the rest of argv, and the cgroup path are written by the
/// ward, for every process the ward starts, root-owned exe or not. Where a
/// match RESTRICTS, believing them is free (you can only get your own process
/// killed). Where a match SPARES, each one is an off switch the ward holds:
/// `cp game ~/.local/bin/gcompris-qt` (basename), `exec -a
/// /usr/bin/gcompris-qt ~/.local/bin/evil` (argv0), `systemd-run --user
/// --scope --unit=app-flatpak-org.kde.gcompris-9999` (cgroup, unprivileged),
/// a `cmdline:` needle pasted into your own argv — each verified live, each
/// earning no free time and stopped by nothing, each once buying silence; and
/// each again when laundered through a root-owned ANCESTOR (`bash` wearing the
/// forged argv inside the forged scope), which is why the ancestor walk reads
/// nothing but each node's own exe path. Not "closed by construction" —
/// closed by refusing to read what the ward can write.
///
/// **And `exe_uid` itself is read from the kernel's own magic symlink, not
/// from the rendered path.** `stat`ing the *string* `/proc/<pid>/exe` resolves
/// to reveals only whatever is mounted at that path right now, and an
/// unprivileged user namespace can bind-mount a ward binary over
/// `/usr/bin/<anything>` so the rendered path reads root-owned.
/// [`crate::ancestry::exe_owner_uid`]
/// stats the magic symlink itself, which the kernel resolves to the process's
/// real executable inode regardless of any mount. See its docs.
///
/// The one legitimate shape this declines to suppress: a governed `cmdline:`
/// identity (the system JVM running Minecraft) is indistinguishable BY
/// CONSTRUCTION from the forgery, because argv is argv. That case is kept
/// honest one level up: a live bucket/appRules meter hit or a learning credit
/// settles the same seconds in [`attribute`], so the supported
/// Minecraft-in-a-Play-bucket path never double-reports.
///
/// This is deliberately NOT "absent from the installed-app inventory" (the
/// definition that first shipped): that read as a false accusation on
/// ordinary use — Kintrinsic's OWN lock shade, a shell-wrapper browser (`exec`s
/// a different real binary than its `.desktop` `Exec=`), a multi-process
/// suite (`soffice.bin`, `nemo-desktop`), or a snap/`/usr/local` app not
/// scanned at all (03b-G3 has since added both to the scan set, but the
/// argument does not depend on any particular gap in it — `XDG_DATA_DIRS` on
/// somebody's machine will always name a dir we did not think of) would ALL
/// have accrued hours of "Kintrinsic couldn't identify
/// this" about a child's completely ordinary afternoon — and it was forgeable
/// in the SUPPRESSING direction (a ward-writable `.desktop` naming
/// `~/.local/bin/java` zeroed the counter forever). This definition is a
/// POSITIVE signal instead: "your child spent time in software Kintrinsic can't
/// vouch for".
pub fn is_unrecognised(p: &FocusedProcess, governed: &[String]) -> bool {
    is_unrecognised_with(p, governed, crate::ancestry::read_proc)
}

/// [`is_unrecognised`] with the process-tree reader injected, so the rule is
/// unit-tested without a live `/proc` (mirrors [`bucket_id_for_with`]).
pub fn is_unrecognised_with<F>(p: &FocusedProcess, governed: &[String], lookup: F) -> bool
where
    F: Fn(u32) -> Option<crate::ancestry::ProcId> + Copy,
{
    let Some(exe_uid) = p.exe_uid else {
        // Ownership UNKNOWN — not evidence of anything. Never a claim.
        return false;
    };
    if exe_uid == 0 {
        // Root-owned: the ward could not have written or replaced this
        // binary, so there is nothing to be unable to vouch for. What it may
        // be RUNNING is deliberately out of scope — see the D1 note above.
        return false;
    }
    // A ward-owned exe. Spared only by an exact governed match on a
    // kernel-resolved exe path — this process's own, or an ancestor's.
    let matched = governed.iter().any(|pkg| {
        crate::app_rules::pkg_names_process_exactly(pkg, p.exe.as_deref())
            || crate::ancestry::names_with_ancestors_exactly(p.pid, pkg, lookup)
    });
    !matched
}

/// The device inventory's own `userInstalled` identities, as [`classify`]
/// needs for the §2.4 asymmetry. Computed once per call, not once per app.
/// (The inventory keeps this ONE job on the §2.3 path now — see
/// [`governed_pkgs`]/[`is_unrecognised`], which no longer consult it at all.)
pub fn user_installed_ids(
    inventory: &[charter_proto::status::AppRef],
) -> std::collections::BTreeSet<String> {
    inventory
        .iter()
        .filter(|a| a.user_installed == Some(true))
        .map(|a| a.pkg.clone())
        .collect()
}

/// Where the helper lives once installed. Overridable the same way the lock
/// binary's path is (`CHARTER_LOCK_BIN`), so a dev tree can point at
/// `target/debug/charter-xclients` without installing the `.deb`.
const XCLIENTS_BIN: &str = "/usr/bin/charter-xclients";

/// One `charter-xclients` run against the active session's display, hard-capped
/// at 2s (same rationale as `loginctl_value`: a wedged X server must degrade a
/// tick, never stall the loop — and the display belongs to the ward, so "wedged
/// on purpose" is a case, not an accident).
///
/// `None` means **the display could not be read**: the helper is missing, it
/// could not connect, the server has no XRes 1.2, or it was killed by the
/// timeout. That is a different fact from "nothing is open", and every caller
/// here keeps the two apart.
/// **The one probe a tick is allowed.** `runtime.rs` takes this once per tick
/// and hands the result to everything that needs it: the activity decision
/// (DPMS), the focused-window attribution ([`attribute_snapshot`]) and the
/// named model's open-window set ([`open_bucket_ids_snapshot`]).
///
/// Sharing it is not only an optimisation. Two probes of the same display in
/// one tick can straddle an app switch and disagree about what was on screen —
/// and once the activity decision reads off the same snapshot as attribution,
/// a disagreement would mean charging a bucket for a second the meter had
/// already decided was screen-off. One probe, one answer, one tick.
pub fn snapshot_tick(display: &str, xauth: Option<&str>) -> Option<XSnapshot> {
    xsnapshot(display, xauth)
}

fn xsnapshot(display: &str, xauth: Option<&str>) -> Option<XSnapshot> {
    let bin = std::env::var("CHARTER_XCLIENTS_BIN").unwrap_or_else(|_| XCLIENTS_BIN.to_string());
    let mut cmd = Command::new("timeout");
    cmd.arg("2").arg(bin);
    cmd.env("DISPLAY", display);
    if let Some(a) = xauth {
        cmd.env("XAUTHORITY", a);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    parse_xsnapshot(&String::from_utf8_lossy(&out.stdout))
}

/// Read the focused process's identity off /proc.
fn focused_process(pid: u32) -> Option<FocusedProcess> {
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
    let cmdline: Vec<String> = cmdline
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    // Shared with the kill sweep (`runtime.rs`) and `ancestry::read_proc` —
    // C3: this used to read the link WITHOUT stripping the kernel's
    // `" (deleted)"` suffix while the sweep did, so after an ordinary
    // in-place package upgrade the meter's `exe` string and the sweep's
    // diverged for the SAME process — a real metered≠stopped gap, and one
    // that also fed the wrong string into the C2 ownership gate below.
    let exe = crate::ancestry::read_exe_link(pid);
    // D2: ownership comes from stat'ing the magic symlink, never from the
    // rendered path — an unprivileged user-namespace bind mount makes the
    // rendered path report a root-owned distro binary while the process runs
    // the ward's own. See `ancestry::exe_owner_uid`.
    let exe_uid = crate::ancestry::exe_owner_uid(pid);
    let cgroup = std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok();
    Some(FocusedProcess {
        cmdline,
        exe,
        exe_uid,
        cgroup,
        pid,
    })
}

/// Which guardian-named app bucket ("Play") the focused process belongs to.
///
/// Deliberately reuses [`crate::app_rules::pkg_matches_process`] — the very
/// matcher the SIGTERM sweep uses. If attribution and enforcement disagreed we
/// would meter one app and stop another: an allowance that never runs down, or
/// a game killed on someone else's clock. One identity rule, both jobs.
pub fn bucket_id_for(p: &FocusedProcess, body: &charter_schedule::GrantBuckets) -> Option<String> {
    bucket_id_for_with(p, body, crate::ancestry::read_proc)
}

/// [`bucket_id_for`] with the process-tree reader injected, so the match rule
/// is unit-tested without a live `/proc`.
pub fn bucket_id_for_with<F>(
    p: &FocusedProcess,
    body: &charter_schedule::GrantBuckets,
    lookup: F,
) -> Option<String>
where
    F: Fn(u32) -> Option<crate::ancestry::ProcId> + Copy,
{
    if body.is_paused() || !body.is_valid() {
        return None;
    }
    // Computed once per process, not once per bucket app tested against it —
    // a ward can have several bucket apps, several of which may be
    // `cmdline:` identities, and this allocation must not repeat per app.
    let cmdline_joined = p.cmdline.join(" ");
    for b in &body.buckets {
        for app in &b.apps {
            // The focused process itself…
            if crate::app_rules::pkg_matches_process(
                app,
                p.exe.as_deref(),
                p.cmdline.first().map(String::as_str),
                &p.cmdline,
                &cmdline_joined,
                p.cgroup.as_deref(),
            ) {
                return Some(b.id.clone());
            }
            // …or anything it was started BY. The guardian picked "Minecraft"
            // (the launcher); the window in front is the Java game it spawned,
            // so direct matching alone would meter almost nothing.
            if crate::ancestry::matches_with_ancestors(p.pid, app, lookup) {
                return Some(b.id.clone());
            }
        }
    }
    None
}

/// The PURE per-tick attribution core: given ONE already-resolved focused
/// process, decide all three questions §2.2-§2.4 need. Fully unit-tested
/// without touching X at all — `attribute_tick` below is a thin, impure
/// wrapper that probes, then calls this. (A prior version of this module
/// inlined the equivalent of this logic directly in the probe closure, where
/// it was effectively untested as a COMPOSITION — exactly how the §2.3
/// counter's original inventory-membership definition shipped without anyone
/// exercising the false-positive cases a review later found on a real
/// desktop.)
pub fn attribute(
    p: &FocusedProcess,
    apps: &[LearningApp],
    buckets: &charter_schedule::GrantBuckets,
    user_installed: Option<&std::collections::BTreeSet<String>>,
    governed: &[String],
) -> (Bucket, Option<String>, bool) {
    let bucket = classify(p, apps, user_installed);
    let bucket_id = bucket_id_for(p, buckets);
    // NEVER both "credited" and "unrecognised" about the same seconds — in
    // EITHER credit's direction.
    //
    // `is_unrecognised`'s suppression test is deliberately EXACT while
    // `bucket_id_for` (which also decides what the kill sweep stops) matches
    // by basename too. That asymmetry is right — an exact rule is what stops
    // `cp game ~/.local/bin/gcompris-qt` silencing the counter off a governed
    // LEARNING path that grants it nothing and stops it with nothing — but on
    // its own it lets the two disagree: a bucket whose app basename-matches
    // this process is METERING and CAPPING it right now, and reporting the
    // very same seconds as "Kintrinsic can't vouch for this" alongside "Play: 22
    // of 60" is the exact contradiction §2.3 exists to avoid. A live bucket
    // hit is proof the process is accounted for, so it settles the question.
    // (This does not reopen the rename hole: that case has no bucket at all,
    // which is precisely why nothing was stopping it.)
    //
    // The SAME holds for a learning credit: a guardian-vouched `trusted` app
    // being CREDITED as learning this very second, with "Learning: 22m"
    // beside "Kintrinsic couldn't vouch for 22m", is the identical
    // contradiction. The ownership rule already settles the common shape (a
    // trusted script runs under a root-owned interpreter, which is out of
    // scope) — this gate is belt and braces for the rest, mirroring the
    // bucket one, so no future drift between `classify` and the suppression
    // matcher can accuse a credited second.
    let unrecognised =
        bucket != Bucket::Learning && bucket_id.is_none() && is_unrecognised(p, governed);
    (bucket, bucket_id, unrecognised)
}

/// Which process is in FRONT, per the snapshot: the `GetInputFocus` pid, which
/// is the server's own answer and cannot be written by a client. `active`
/// (`_NET_ACTIVE_WINDOW`, written by the window manager) is the fallback for
/// the servers and desktops where focus legitimately reads as nothing.
fn foreground_pid(snap: &XSnapshot) -> Option<u32> {
    snap.focus.or(snap.active)
}

/// The one place `_NET_ACTIVE_WINDOW` is allowed to influence a verdict, and it
/// can only ever make time COST more.
///
/// The server's focus and the window manager's `_NET_ACTIVE_WINDOW` normally
/// name the same client. When they disagree, one of two things is true: the
/// desktop is mid app-switch (the WM has moved the property, the server has not
/// yet moved focus, or vice versa), or somebody rewrote the property to point
/// the meter at a free app while using another. The second is a bypass and the
/// first costs exactly one tick, so the disagreement is resolved in the only
/// safe direction: a free-learning credit is withdrawn for that tick and the
/// time is charged as screen time. `Screen` is already the costing answer and
/// is left alone — this rule only ever removes a waiver, never adds one.
///
/// **Only a genuine disagreement counts — BOTH sources present, and different.**
/// A missing one is not a contradiction, and treating it as one made free
/// learning unreachable on whole classes of honest desktop: a session with no
/// EWMH window manager has no `_NET_ACTIVE_WINDOW` to compare against, and
/// focus-follows-mouse (or a pointer over the root) legitimately reports no
/// focus at all, so a first cut of this rule charged every single tick on
/// those machines and never said why.
///
/// A `capped` snapshot IS a reason to withdraw the credit, and a different
/// one: the display was not read in full, so "the only thing in front is a
/// learning app" is a claim this tick has no standing to make.
pub fn downgrade_on_focus_mismatch(bucket: Bucket, snap: &XSnapshot) -> Bucket {
    if bucket != Bucket::Learning {
        return bucket;
    }
    let disagree = match (snap.focus, snap.active) {
        (Some(focus), Some(active)) => focus != active,
        _ => false,
    };
    if disagree || snap.capped {
        return Bucket::Screen;
    }
    bucket
}

/// Probe the foreground window ONCE and answer three questions about it: which
/// meter it feeds (learning vs screen), which named bucket it spends, and
/// whether it is unrecognised (§2.3 — see [`is_unrecognised`] for the
/// definition). Returns `(Bucket, Option<bucket_id>, unrecognised)`.
///
/// All three are read off a single X probe because two probes could straddle
/// an app switch and credit one app's second to another app's allowance. The
/// actual decision is [`attribute`], the pure/tested core; this is only the
/// impure probe wrapped around it, plus
/// [`downgrade_on_focus_mismatch`].
///
/// `unrecognised` is deliberately computed BEFORE the downgrade: a waiver
/// withdrawn because two sources disagreed about focus is not evidence of
/// software Kintrinsic cannot vouch for, and this counter never claims a
/// finding it has no basis for.
#[allow(clippy::too_many_arguments)]
pub fn attribute_tick(
    display: &str,
    xauth: Option<&str>,
    apps: &[LearningApp],
    buckets: &charter_schedule::GrantBuckets,
    user_installed: Option<&std::collections::BTreeSet<String>>,
    governed: &[String],
) -> (Bucket, Option<String>, bool) {
    match xsnapshot(display, xauth) {
        // Fail-safe on ALL THREE counts: no probe means Screen (an error must
        // never make time free), no bucket (an error must never spend an
        // allowance the ward wasn't using), and NOT unrecognised — an error
        // reading the window is not evidence of unrecognised software, and
        // this counter must never claim a finding it has no basis for.
        None => (Bucket::Screen, None, false),
        Some(snap) => attribute_snapshot(&snap, apps, buckets, user_installed, governed),
    }
}

/// [`attribute_tick`] against a snapshot the caller already has, so the tick
/// loop spends ONE `charter-xclients` run on activity and attribution together
/// rather than one each. The `/proc` read of the focused process still happens
/// here — it is cheap, local, and must be as fresh as the decision it feeds.
pub fn attribute_snapshot(
    snap: &XSnapshot,
    apps: &[LearningApp],
    buckets: &charter_schedule::GrantBuckets,
    user_installed: Option<&std::collections::BTreeSet<String>>,
    governed: &[String],
) -> (Bucket, Option<String>, bool) {
    // No foreground window, or a foreground pid whose `/proc` entry has
    // already gone: the same fail-safe triple as an unreadable display.
    let Some(p) = foreground_pid(snap).and_then(focused_process) else {
        return (Bucket::Screen, None, false);
    };
    let (bucket, id, unrecognised) = attribute(&p, apps, buckets, user_installed, governed);
    (downgrade_on_focus_mismatch(bucket, snap), id, unrecognised)
}

/// Probe + classify the current foreground window. `apps` empty → `Screen`
/// without touching X at all (inert-until-clause: non-learning families never
/// pay the probe). Any probe failure → `Screen`.
pub fn bucket_for_tick(
    display: &str,
    xauth: Option<&str>,
    apps: &[LearningApp],
    user_installed: Option<&std::collections::BTreeSet<String>>,
) -> Bucket {
    if apps.is_empty() {
        return Bucket::Screen;
    }
    let probe = || -> Option<(XSnapshot, FocusedProcess)> {
        let snap = xsnapshot(display, xauth)?;
        let p = focused_process(foreground_pid(&snap)?)?;
        Some((snap, p))
    };
    match probe() {
        Some((snap, p)) => downgrade_on_focus_mismatch(classify(&p, apps, user_installed), &snap),
        None => Bucket::Screen,
    }
}

/// Probe every open window ONCE and answer which named allowances are being
/// spent right now — the [`charter_schedule::TimeModel::Named`] counterpart to
/// [`attribute_tick`].
///
/// # Why the window set, and not the process table
///
/// Under [`charter_schedule::TimeModel::Named`] a costing app is charged while
/// it has a window OPEN — not while it is focused, and not while its process
/// merely exists.
///
/// Focus is the wrong test on a machine with two monitors: a game full-screen
/// on one and YouTube on the other are both plainly being used, and only one
/// of them can hold focus. That asymmetry is the entire bug this model exists
/// to fix.
///
/// Process liveness is the wrong test because chat clients are resident BY
/// DESIGN — that is how messages arrive. WhatsApp, Discord, Slack, Steam and
/// Spotify all keep running after you "close" them, and charging a child all
/// day for an app they opened once at breakfast, with no way to see why, is
/// indefensible.
///
/// The window set separates those two cleanly, and does it by a property of
/// how tray apps actually behave rather than by a list Kintrinsic would have to
/// maintain: an app that minimises to the tray **destroys** its window, so it
/// stops being on the display while its process lives on. Tray-resident
/// software falls out of this test for free — no allowlist of background apps,
/// nothing to keep up to date.
///
/// A MINIMISED window is still counted, and that is deliberate: audio keeps
/// playing when a video is minimised, it is one click from being back, and
/// "hide it to stop the clock" is the wrong lesson. Close it, don't hide it.
///
/// # The fail direction, restated
///
/// **`None` means the display could not be read**; `Some(vec![])` means it was
/// read fine and nothing costing is open. They are charged differently (see
/// [`charter_spine::multi_child::MultiChildEnforcer::tick_open`]) and the
/// distinction is the point.
///
/// The doc this replaces argued that under-charging is always the right
/// direction for a probe failure. That is true of a FLAKY probe — one window
/// that vanished mid-probe, one tick lost to load — and it is what `Some`
/// still expresses. It is not true of a display that cannot be read at all: a
/// Wayland seat, a killed X server, a helper the ward managed to make fail
/// every time. "Charge nothing, forever" is not under-charging by a tick, it
/// is an uncapped day, and an unreadable display is not a flaky probe.
pub fn open_bucket_ids_tick(
    display: &str,
    xauth: Option<&str>,
    buckets: &charter_schedule::GrantBuckets,
    ward_uid: u32,
) -> Option<Vec<String>> {
    if buckets.is_paused() || !buckets.is_valid() || buckets.buckets.is_empty() {
        // Nothing to meter, so nothing to be unable to read. This must stay
        // `Some`: a family with no buckets clause is not a family whose
        // display is broken, and reporting it as one would charge them the
        // screen baseline under a model that has no baseline.
        return Some(Vec::new());
    }
    let snap = xsnapshot(display, xauth)?;
    Some(open_bucket_ids_for_snapshot(
        &snap,
        buckets,
        focused_process,
        || ward_pids(ward_uid),
        crate::ancestry::read_proc,
    ))
}

/// [`open_bucket_ids_tick`] against a snapshot the caller already has — the
/// named model's half of the one-probe-per-tick rule (see [`snapshot_tick`]).
///
/// `snap` is `None` when the display could not be read at all, and that stays
/// `None` here: the whole point of the distinction is that "unreadable" and
/// "read fine, nothing costing open" are charged differently.
pub fn open_bucket_ids_snapshot(
    snap: Option<&XSnapshot>,
    buckets: &charter_schedule::GrantBuckets,
    ward_uid: u32,
) -> Option<Vec<String>> {
    if buckets.is_paused() || !buckets.is_valid() || buckets.buckets.is_empty() {
        // Same reasoning as `open_bucket_ids_tick`, and it holds even when the
        // display was unreadable: a family with no buckets clause is not a
        // family whose display is broken.
        return Some(Vec::new());
    }
    Some(open_bucket_ids_for_snapshot(
        snap?,
        buckets,
        focused_process,
        || ward_pids(ward_uid),
        crate::ancestry::read_proc,
    ))
}

/// The pure core of [`open_bucket_ids_tick`]: snapshot in, bucket ids out, with
/// `/proc` injected. Split out so the unidentified-window rule below is tested
/// against a synthetic window set rather than only ever on a live desktop —
/// which is how the focus-only path went untested as a COMPOSITION for so long.
///
/// # The unidentified-window fallback
///
/// A window the server will not attribute (`-`) is a real window with a real
/// owner: a TCP or X-forwarded client, whose pid lives on another machine or
/// behind a socket with no peer credentials to read. Treating it as "no app" is
/// the free pass this whole change exists to close — `ssh -X` into the family
/// NAS and run the game there, and the meter sees a window belonging to nobody.
///
/// So with evidence of even one such window, "running" is taken as "open" for
/// that tick: every process the ward owns is fed to the matcher alongside the
/// windows that DID resolve. That deliberately re-admits the tray-resident
/// over-charge the window test exists to avoid — but only for a ward who has an
/// unattributable window on screen, only while it is there, and only ever in
/// the costing direction. The ordinary desktop never touches this path.
///
/// A `capped` snapshot triggers the same fallback for the same reason. A
/// truncated window set is a set that may be missing the very window the ward
/// is using — indistinguishable, from here, from a window we could see and
/// could not name.
pub fn open_bucket_ids_for_snapshot<R, W, F>(
    snap: &XSnapshot,
    buckets: &charter_schedule::GrantBuckets,
    read: R,
    ward_procs: W,
    lookup: F,
) -> Vec<String>
where
    R: Fn(u32) -> Option<FocusedProcess>,
    W: Fn() -> Vec<u32>,
    F: Fn(u32) -> Option<crate::ancestry::ProcId> + Copy,
{
    let mut pids: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    let mut unattributed = false;
    for (_, pid) in &snap.windows {
        match pid {
            Some(pid) => {
                pids.insert(*pid);
            }
            None => unattributed = true,
        }
    }
    if unattributed || snap.capped {
        pids.extend(ward_procs());
    }
    // One process, many windows (a browser's several windows, an app's
    // dialogs) is read from `/proc` once. Only an optimisation —
    // `open_bucket_ids` dedupes by bucket regardless — but it keeps a
    // window-heavy desktop from re-reading the same process repeatedly.
    let procs: Vec<FocusedProcess> = pids.iter().filter_map(|pid| read(*pid)).collect();
    open_bucket_ids(&procs, buckets, lookup)
}

/// How many ward-owned processes the fallback will consider, and how long it
/// will spend finding them.
///
/// The fallback runs inside a tick the daemon repeats every two seconds, and
/// each pid it returns costs a `/proc` read plus an ancestry walk downstream.
/// A ward with more than [`WARD_PROC_MAX`] live processes is already at their
/// `RLIMIT_NPROC`/`pids.max` ceiling — that is a cgroup's job to answer, not a
/// meter's — and spending the whole tick enumerating them would turn the
/// fallback into its own denial of service against the display probe.
const WARD_PROC_MAX: usize = 2048;
const WARD_PROC_BUDGET: std::time::Duration = std::time::Duration::from_millis(500);

/// Every pid the ward owns, from `/proc`. Feeds the unidentified-window
/// fallback above and nothing else — this is the expensive answer, and it is
/// only ever asked for when the display could not be fully attributed.
///
/// Ownership is the owning uid of `/proc/<pid>` itself, which the kernel writes
/// from the process's own credentials. An unreadable `/proc`, a pid that exited
/// between the readdir and the stat, and a non-numeric entry all simply drop
/// out: this set can only ever ADD candidate processes, so missing one
/// under-charges a tick and inventing one is impossible.
///
/// Hitting either bound **keeps** everything found so far rather than giving
/// up — the fallback's job is to raise the charge, and a partial answer still
/// does that. Returning nothing on a flood would hand the ward the free pass
/// back by making the expensive path the one that fails open.
fn ward_pids(uid: u32) -> Vec<u32> {
    use std::os::unix::fs::MetadataExt as _;
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let started = std::time::Instant::now();
    let mut out = Vec::new();
    for entry in dir.flatten() {
        if out.len() >= WARD_PROC_MAX || started.elapsed() >= WARD_PROC_BUDGET {
            break;
        }
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if entry.metadata().is_ok_and(|md| md.uid() == uid) {
            out.push(pid);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(id: &str) -> LearningApp {
        LearningApp {
            id: id.into(),
            label: id.into(),
            kind: LearningAppKind::Site,
            domains: vec!["khanacademy.org".into()],
            url: Some("https://www.khanacademy.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        }
    }

    /// A whole desktop as `charter-xclients` reports it, including the two
    /// shapes the old property-based probe could not express at all: a window
    /// the SERVER declined to attribute, and "read fine, nothing here".
    #[test]
    fn a_snapshot_parses_every_line_kind() {
        let snap = parse_xsnapshot(
            "v1\n\
             focus 4242\n\
             active 4242\n\
             win 0x2200003 4242\n\
             win 0x2400003 91\n\
             win 0x3c00007 -\n",
        )
        .expect("a v1 snapshot parses");
        assert!(!snap.capped);
        assert_eq!(snap.focus, Some(4242));
        assert_eq!(snap.active, Some(4242));
        assert_eq!(
            snap.windows,
            vec![
                ("0x2200003".to_string(), Some(4242)),
                ("0x2400003".to_string(), Some(91)),
                // `-` is a window we know about and cannot attribute. It must
                // survive parsing as a distinct fact, because it is what
                // triggers the ward-process fallback downstream.
                ("0x3c00007".to_string(), None),
            ]
        );
    }

    /// G1: the mapping from the helper's `dpms` field to whether this tick is
    /// charged at all. The whole point of the change is that the ONLY answer
    /// that stops the clock is a monitor the server says is powered down —
    /// every other answer, including every way of not answering, keeps
    /// charging. If this table ever drifts toward "silence means idle", a ward
    /// gets free time by breaking the probe.
    #[test]
    fn the_dpms_field_maps_to_activity() {
        use charter_schedule::Activity;
        // Powered down, in any of the three ways a monitor can be.
        assert_eq!(super::activity_from_dpms(Some("standby")), Activity::Idle);
        assert_eq!(super::activity_from_dpms(Some("suspend")), Activity::Idle);
        assert_eq!(super::activity_from_dpms(Some("off")), Activity::Idle);
        // On, and every flavour of "we do not know".
        assert_eq!(super::activity_from_dpms(Some("on")), Activity::Active);
        assert_eq!(super::activity_from_dpms(Some("unknown")), Activity::Active);
        // A helper that never printed the line (an older build), a value this
        // daemon has never heard of, and an empty value are all "we do not
        // know" and therefore all charge.
        assert_eq!(super::activity_from_dpms(None), Activity::Active);
        assert_eq!(super::activity_from_dpms(Some("dozing")), Activity::Active);
        assert_eq!(super::activity_from_dpms(Some("")), Activity::Active);
        // Case and stray whitespace come from the helper's output, not from us.
        assert_eq!(super::activity_from_dpms(Some(" OFF ")), Activity::Idle);
    }

    /// The `dpms` line rides in the same snapshot as everything else, and a
    /// snapshot without one still parses — an older helper against a newer
    /// daemon degrades to "we do not know", i.e. to charging.
    #[test]
    fn the_dpms_line_rides_in_the_snapshot() {
        let snap =
            parse_xsnapshot("v1\ndpms off\nidle_ms 9000\nfocus 7\nactive 7\n").expect("parses");
        assert_eq!(snap.dpms_level(), Some("off"));
        assert_eq!(snap.idle_ms(), Some(9000));
        // No `dpms` / `idle_ms` line at all: both unknown, and unknown charges.
        let snap = parse_xsnapshot("v1\nfocus 7\nactive 7\n").expect("parses");
        assert_eq!(snap.dpms_level(), None);
        assert_eq!(snap.idle_ms(), None);
        // `dpms` takes exactly one argument; a line carrying two is an unknown
        // line kind, not a power level — same discipline as `capped`.
        assert_eq!(
            parse_xsnapshot("v1\ndpms off now\n").expect("parses").dpms,
            None
        );
        // And the same for `idle_ms`, including a value that is not a number:
        // an unparseable idle time is an unknown one, never a zero.
        assert_eq!(
            parse_xsnapshot("v1\nidle_ms 1 2\n")
                .expect("parses")
                .idle_ms,
            None
        );
        assert_eq!(
            parse_xsnapshot("v1\nidle_ms soon\n")
                .expect("parses")
                .idle_ms,
            None
        );
    }

    // ---- G1 (DPMS toggle): the Idle decision is folded over ticks ----

    /// Feed a run of ticks through one history and collect the verdicts.
    fn run_ticks(ticks: &[(Option<&str>, Option<u64>)]) -> Vec<charter_schedule::Activity> {
        let mut h = ActivityHistory::default();
        ticks.iter().map(|(d, i)| h.observe(*d, *i)).collect()
    }

    /// THE BYPASS. `xset dpms force off; sleep .05; xset dpms force on` in a
    /// loop: the screen is usable throughout and about half the samples read
    /// `off`. Every tick's level differs from the last, so the streak never
    /// reaches three and NOTHING here is ever free.
    #[test]
    fn a_dpms_toggle_loop_stays_charged() {
        use charter_schedule::Activity;
        let ticks: Vec<(Option<&str>, Option<u64>)> = (0..12)
            .map(|i| (Some(if i % 2 == 0 { "off" } else { "on" }), Some(60_000)))
            .collect();
        assert!(
            run_ticks(&ticks).iter().all(|a| *a == Activity::Active),
            "a toggling display is a display in use"
        );
        // Even a lopsided toggle — two off, one on — never gets three in a row.
        let ticks: Vec<(Option<&str>, Option<u64>)> = (0..12)
            .map(|i| (Some(if i % 3 == 2 { "on" } else { "off" }), Some(60_000)))
            .collect();
        assert!(run_ticks(&ticks).iter().all(|a| *a == Activity::Active));
    }

    /// A monitor that is genuinely off, with an X server that has seen no
    /// input: charged for the first two ticks (the evidence is not in yet),
    /// free from the third.
    #[test]
    fn a_steady_off_display_with_a_real_idle_time_goes_idle_on_the_third_tick() {
        use charter_schedule::Activity;
        let ticks = vec![(Some("off"), Some(6_000)); 5];
        assert_eq!(
            run_ticks(&ticks),
            vec![
                Activity::Active,
                Activity::Active,
                Activity::Idle,
                Activity::Idle,
                Activity::Idle,
            ]
        );
        // `standby` and `suspend` are powered-down levels too — but the level
        // must be the SAME one three times; drifting between them is a change.
        let drift = vec![
            (Some("standby"), Some(60_000)),
            (Some("suspend"), Some(60_000)),
            (Some("off"), Some(60_000)),
        ];
        assert!(run_ticks(&drift).iter().all(|a| *a == Activity::Active));
    }

    /// The other half of the pair. A ward who blanks the monitor and keeps
    /// typing — a script pressing a key, a held-down modifier, a wiggling
    /// mouse — has a steady `off` level and an idle time that keeps resetting.
    /// Input is use; it is charged.
    #[test]
    fn a_steady_off_display_with_fresh_input_stays_charged() {
        use charter_schedule::Activity;
        let ticks = vec![(Some("off"), Some(0)); 6];
        assert!(run_ticks(&ticks).iter().all(|a| *a == Activity::Active));
        // Just under the floor is still input, not idleness.
        let ticks = vec![(Some("off"), Some(IDLE_MIN_MS - 1)); 6];
        assert!(run_ticks(&ticks).iter().all(|a| *a == Activity::Active));
    }

    /// An unanswered idle time is never taken as idleness, however long the
    /// display has been off — an older helper, a server with no XScreenSaver
    /// extension, a probe that failed. Fail toward charging.
    #[test]
    fn a_missing_idle_time_is_charged_however_steady_the_display() {
        use charter_schedule::Activity;
        let ticks = vec![(Some("off"), None); 8];
        assert!(run_ticks(&ticks).iter().all(|a| *a == Activity::Active));
    }

    /// A powered-on tick, an unknown level, or no snapshot at all resets the
    /// streak: three off ticks have to be three IN A ROW.
    #[test]
    fn any_powered_on_or_unknown_tick_resets_the_streak() {
        use charter_schedule::Activity;
        for interrupt in [Some("on"), Some("unknown"), None] {
            let ticks = vec![
                (Some("off"), Some(60_000)),
                (Some("off"), Some(60_000)),
                (interrupt, Some(60_000)),
                (Some("off"), Some(60_000)),
                (Some("off"), Some(60_000)),
            ];
            assert!(
                run_ticks(&ticks).iter().all(|a| *a == Activity::Active),
                "interrupted by {interrupt:?}"
            );
        }
    }

    /// The helper says when one of its budgets bit. Without this line a flood
    /// of windows would come back as a short, confident, EMPTY-looking answer
    /// — which is the fail-open the cap was supposed to prevent.
    #[test]
    fn a_truncated_answer_says_so() {
        let snap = parse_xsnapshot("v1\ncapped\nfocus -\nactive -\nwin 0x1 4242\n")
            .expect("a capped snapshot is still a snapshot");
        assert!(snap.capped);
        assert_eq!(snap.windows, vec![("0x1".to_string(), Some(4242))]);
        // `capped` takes no argument; a line that looks like it but carries
        // one is an unknown line kind, not a cap.
        assert!(
            !parse_xsnapshot("v1\ncapped 1\n")
                .expect("still parses")
                .capped
        );
    }

    /// An empty desktop is a SUCCESSFUL read that found nothing — `Some` with
    /// no windows, never `None`. `None` is reserved for "could not read the
    /// display", and conflating the two is the bug this all exists to close.
    #[test]
    fn an_empty_desktop_is_a_successful_snapshot() {
        let snap = parse_xsnapshot("v1\nfocus -\nactive -\n").expect("still a valid snapshot");
        assert_eq!(snap.focus, None);
        assert_eq!(snap.active, None);
        assert!(snap.windows.is_empty());
        // Trailing whitespace on the version line is tolerated.
        assert!(parse_xsnapshot("v1  \n").is_some());
        // And a snapshot with no lines at all after the version.
        assert_eq!(parse_xsnapshot("v1"), Some(XSnapshot::default()));
    }

    /// Anything that is not a `v1` snapshot is NOT a snapshot. The helper exits
    /// non-zero on every failure it knows about, so this is the belt to that
    /// braces: a truncated pipe, a shell error page, a future `v2` that reused
    /// `win` for something else must all read as "display unreadable" rather
    /// than as an empty desktop.
    #[test]
    fn anything_that_is_not_v1_is_not_a_snapshot() {
        assert_eq!(parse_xsnapshot(""), None);
        assert_eq!(parse_xsnapshot("garbage\nwin 0x1 5\n"), None);
        assert_eq!(parse_xsnapshot("v2\nwin 0x1 5\n"), None);
        // Unknown line kinds and malformed lines inside a v1 snapshot are
        // skipped, not fatal — a newer helper must degrade to fewer facts.
        let snap = parse_xsnapshot(
            "v1\n\
             \n\
             screens 2\n\
             win\n\
             win 0x1\n\
             win 0x1 5 extra\n\
             focus notanumber\n\
             win 0x2200003 4242\n",
        )
        .expect("unknown lines are skipped");
        assert_eq!(snap.focus, None);
        assert_eq!(snap.windows, vec![("0x2200003".to_string(), Some(4242))]);
    }

    /// THE COUNT-ONCE RULE. Two members of the same allowance open at the same
    /// time spend it ONCE — wall-clock time is not duplicable, and a meter that
    /// charged an hour twice because a child had two windows up would be
    /// impossible to defend to them.
    #[test]
    fn two_members_of_one_allowance_yield_that_allowance_once() {
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["/usr/games/supertux2".into(), "/usr/games/wesnoth".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            tz: "UTC".into(),
            week_start: None,
            issued_at: 1,
        };
        let procs = vec![
            FocusedProcess {
                cmdline: vec!["/usr/games/supertux2".into()],
                exe: Some("/usr/games/supertux2".into()),
                exe_uid: Some(0),
                cgroup: None,
                pid: 10,
            },
            FocusedProcess {
                cmdline: vec!["/usr/games/wesnoth".into()],
                exe: Some("/usr/games/wesnoth".into()),
                exe_uid: Some(0),
                cgroup: None,
                pid: 11,
            },
        ];
        assert_eq!(open_bucket_ids(&procs, &buckets, |_| None), vec!["play"]);
    }

    /// decented's actual scene, at the probe: a game on one monitor and a video
    /// site on the other. TWO allowances, each spent once — and neither
    /// depends on which of them happens to hold focus, which is the whole
    /// reason this replaced the focused-window probe.
    #[test]
    fn two_different_allowances_are_both_returned_regardless_of_focus() {
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![
                charter_schedule::AppBucket {
                    id: "play".into(),
                    label: "Play".into(),
                    apps: vec!["/usr/games/supertux2".into()],
                    daily_minutes: Some(60),
                    weekly_minutes: None,
                },
                charter_schedule::AppBucket {
                    id: "video".into(),
                    label: "Video".into(),
                    apps: vec!["site:youtube".into()],
                    daily_minutes: Some(30),
                    weekly_minutes: None,
                },
            ],
            paused: None,
            tz: "UTC".into(),
            week_start: None,
            issued_at: 1,
        };
        let youtube = costing_site("youtube");
        let procs = vec![
            FocusedProcess {
                cmdline: vec!["/usr/games/supertux2".into()],
                exe: Some("/usr/games/supertux2".into()),
                exe_uid: Some(0),
                cgroup: None,
                pid: 10,
            },
            launched(&youtube),
        ];
        assert_eq!(
            open_bucket_ids(&procs, &buckets, |_| None),
            vec!["play", "video"]
        );
    }

    /// Nothing costing open means an EMPTY result, which in the named model
    /// means nothing is charged. The free-by-absence case, at the probe.
    #[test]
    fn windows_that_name_no_allowance_yield_nothing() {
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["/usr/games/supertux2".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            tz: "UTC".into(),
            week_start: None,
            issued_at: 1,
        };
        // A text editor and a file manager: open, on screen, costing nothing.
        let procs = vec![FocusedProcess {
            cmdline: vec!["/usr/bin/xed".into()],
            exe: Some("/usr/bin/xed".into()),
            exe_uid: Some(0),
            cgroup: None,
            pid: 12,
        }];
        assert!(open_bucket_ids(&procs, &buckets, |_| None).is_empty());
        // And an empty desktop.
        assert!(open_bucket_ids(&[], &buckets, |_| None).is_empty());
    }

    /// One bucket, one member, for the snapshot-level tests below.
    fn play_bucket() -> charter_schedule::GrantBuckets {
        charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["/usr/games/supertux2".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            tz: "UTC".into(),
            week_start: None,
            issued_at: 1,
        }
    }

    /// A `/proc` that knows exactly one thing: pid 10 is the game.
    fn fake_proc(pid: u32) -> Option<FocusedProcess> {
        (pid == 10).then(|| FocusedProcess {
            cmdline: vec!["/usr/games/supertux2".into()],
            exe: Some("/usr/games/supertux2".into()),
            exe_uid: Some(0),
            cgroup: None,
            pid,
        })
    }

    fn win(id: &str, pid: Option<u32>) -> (String, Option<u32>) {
        (id.to_string(), pid)
    }

    /// The ordinary desktop: every window has a server-attributed owner, so
    /// the answer is what the window set says and nothing else is consulted.
    #[test]
    fn attributed_windows_name_their_allowances_and_nothing_more() {
        let calls = std::cell::Cell::new(0u32);
        let ids = open_bucket_ids_for_snapshot(
            &XSnapshot {
                focus: Some(10),
                active: Some(10),
                windows: vec![win("0x1", Some(10)), win("0x2", Some(99))],
                capped: false,
                dpms: None,
                idle_ms: None,
            },
            &play_bucket(),
            fake_proc,
            || {
                calls.set(calls.get() + 1);
                vec![10]
            },
            |_| None,
        );
        assert_eq!(ids, vec!["play"]);
        // THE POINT: the expensive, over-charging fallback is not a default.
        // A desktop the server can fully attribute never touches it, so the
        // tray-resident over-charge the window test exists to avoid stays
        // avoided.
        assert_eq!(calls.get(), 0);
    }

    /// A window the SERVER will not attribute — an `ssh -X` / TCP client,
    /// whose pid is on another machine — must not be a free pass. With
    /// evidence of one on screen, "running" counts as "open" for that tick.
    #[test]
    fn an_unattributable_window_falls_back_to_every_ward_process() {
        let ids = open_bucket_ids_for_snapshot(
            &XSnapshot {
                focus: None,
                active: None,
                // The only window on screen belongs to nobody we can name,
                // and the game's own window is not in the set at all.
                windows: vec![win("0x1", None)],
                capped: false,
                dpms: None,
                idle_ms: None,
            },
            &play_bucket(),
            fake_proc,
            || vec![10, 77],
            |_| None,
        );
        assert_eq!(ids, vec!["play"]);
    }

    /// THE FLOOD BYPASS, closed. Open more windows than the helper will walk,
    /// delete `_NET_CLIENT_LIST`, focus something worthless: every window in
    /// the snapshot resolves, so there is no `-` to trigger the fallback, and
    /// the app actually being used is not in the set at all. `capped` is the
    /// only thing standing between that and a free afternoon.
    #[test]
    fn a_capped_snapshot_falls_back_even_with_no_unattributed_window() {
        let ids = open_bucket_ids_for_snapshot(
            &XSnapshot {
                focus: Some(99),
                active: Some(99),
                // Every window here is attributed — to junk the ward opened
                // to push the real one out of the walk.
                windows: vec![win("0x1", Some(99)), win("0x2", Some(99))],
                capped: true,
                dpms: None,
                idle_ms: None,
            },
            &play_bucket(),
            fake_proc,
            || vec![10, 99],
            |_| None,
        );
        assert_eq!(ids, vec!["play"]);
    }

    /// And the fallback is scoped to the evidence: one unattributable window
    /// among several attributed ones still widens the set, but a set with no
    /// `-` in it never does.
    #[test]
    fn the_fallback_is_triggered_by_evidence_not_by_emptiness() {
        let empty_but_readable = open_bucket_ids_for_snapshot(
            &XSnapshot::default(),
            &play_bucket(),
            fake_proc,
            || vec![10],
            |_| None,
        );
        // Nothing open, nothing unattributable: free by absence, which is the
        // whole promise of the named model.
        assert!(empty_but_readable.is_empty());

        let mixed = open_bucket_ids_for_snapshot(
            &XSnapshot {
                focus: Some(99),
                active: Some(99),
                windows: vec![win("0x1", Some(99)), win("0x2", None)],
                capped: false,
                dpms: None,
                idle_ms: None,
            },
            &play_bucket(),
            fake_proc,
            || vec![10],
            |_| None,
        );
        assert_eq!(mixed, vec!["play"]);
    }

    /// The same pinned window, but one the guardian put in a costing group:
    /// it still materialises and is still resolver-pinned, and its seconds are
    /// charged like any other app.
    fn costing_site(id: &str) -> LearningApp {
        LearningApp {
            free: Some(false),
            ..site(id)
        }
    }

    fn native(exec: &str, trusted: bool) -> LearningApp {
        LearningApp {
            id: "app".into(),
            label: "App".into(),
            kind: LearningAppKind::Native,
            domains: vec![],
            url: None,
            exec: Some(exec.into()),
            trusted,
            free: None,
        }
    }

    fn proc_of(cmdline: &[&str], exe: Option<&str>, exe_uid: Option<u32>) -> FocusedProcess {
        FocusedProcess {
            cmdline: cmdline.iter().map(|s| s.to_string()).collect(),
            exe: exe.map(String::from),
            exe_uid,
            cgroup: None,
            pid: 0,
        }
    }

    /// The `user_installed` set for every `classify` call in this module that
    /// isn't specifically about §2.4 — an empty set behaves exactly like the
    /// pre-§2.4 code (nothing is flagged, so only `exe_uid`/`trusted` gate).
    fn none() -> std::collections::BTreeSet<String> {
        std::collections::BTreeSet::new()
    }

    /// `_NET_ACTIVE_WINDOW` contradicting the server's own focus is either an
    /// app switch caught mid-flight or somebody aiming the meter at a free app
    /// while using another. Both are settled the same way and in one
    /// direction: the free credit goes, the time costs.
    #[test]
    fn a_focus_active_disagreement_withdraws_the_free_credit_only() {
        let agree = XSnapshot {
            focus: Some(10),
            active: Some(10),
            windows: vec![],
            capped: false,
            dpms: None,
            idle_ms: None,
        };
        let disagree = XSnapshot {
            focus: Some(10),
            active: Some(11),
            windows: vec![],
            capped: false,
            dpms: None,
            idle_ms: None,
        };
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Learning, &agree),
            Bucket::Learning
        );
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Learning, &disagree),
            Bucket::Screen
        );
        // Screen is already the costing answer — this rule never touches it,
        // in either direction. It can only ever REMOVE a waiver.
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Screen, &disagree),
            Bucket::Screen
        );
        // A desktop with no EWMH window manager has no `_NET_ACTIVE_WINDOW` to
        // contradict the server with, so there is no disagreement to resolve —
        // charging it every tick would punish an honest minimal session.
        let no_wm = XSnapshot {
            focus: Some(10),
            active: None,
            windows: vec![],
            capped: false,
            dpms: None,
            idle_ms: None,
        };
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Learning, &no_wm),
            Bucket::Learning
        );
        // Focus-follows-mouse with the pointer over the root, or a server that
        // reports PointerRoot, legitimately has NO focus window. There is
        // nothing for the property to contradict, so there is no
        // disagreement — an earlier cut of this rule fired here and made free
        // learning unreachable on every such desktop, silently.
        let advisory_only = XSnapshot {
            focus: None,
            active: Some(11),
            windows: vec![],
            capped: false,
            dpms: None,
            idle_ms: None,
        };
        assert_eq!(foreground_pid(&advisory_only), Some(11));
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Learning, &advisory_only),
            Bucket::Learning
        );
        // A truncated snapshot is its own reason to withdraw the credit: "the
        // only thing in front is a learning app" is a claim a partial read of
        // the display has no standing to make.
        let capped = XSnapshot {
            focus: Some(10),
            active: Some(10),
            windows: vec![],
            capped: true,
            dpms: None,
            idle_ms: None,
        };
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Learning, &capped),
            Bucket::Screen
        );
        assert_eq!(
            downgrade_on_focus_mismatch(Bucket::Screen, &capped),
            Bucket::Screen
        );
    }

    /// Built from the very renderer the shim launches with, not a hand-written
    /// approximation — the point of `site_app` is that there is one definition
    /// of a sanctioned window and both the meter and the sweep read it.
    fn launched(app: &LearningApp) -> FocusedProcess {
        let argv = crate::site_app::sanctioned_argv("/usr/bin/chromium", app, "/home/robin");
        FocusedProcess {
            cmdline: argv,
            exe: Some("/usr/bin/chromium".into()),
            exe_uid: Some(0),
            cgroup: None,
            pid: 0,
        }
    }

    #[test]
    fn pinned_launcher_window_is_learning() {
        let khan = site("khan-academy");
        assert_eq!(
            classify(&launched(&khan), std::slice::from_ref(&khan), Some(&none())),
            Bucket::Learning
        );
    }

    /// The split this feature exists for: a site the guardian put in a COSTING
    /// group still gets its pinned, resolver-locked window — it is materialised
    /// from this very clause — but its seconds are charged like anything else.
    /// Without this, "YouTube in its own locked-down window, half an hour a
    /// day" is unsayable: naming it here made it free, and leaving it out meant
    /// there was no window to meter.
    #[test]
    fn a_costing_site_app_window_is_pinned_but_never_free() {
        let youtube = costing_site("youtube");
        assert_eq!(
            classify(
                &launched(&youtube),
                std::slice::from_ref(&youtube),
                Some(&none())
            ),
            Bucket::Screen
        );
        // It IS still a sanctioned launch — which is what keeps the lockdown
        // sweep from killing it, and what a `site:` bucket identity meters.
        assert!(crate::site_app::is_sanctioned(
            &launched(&youtube).cmdline,
            Some(0),
            std::slice::from_ref(&youtube)
        ));
    }

    /// A costing site must not buy free time by wearing a FREE site's window
    /// class. Attribution resolves the sanctioned id and looks up THAT app's
    /// own `free` flag, so mixing one app's marker into another's launch can
    /// only ever produce the other app's answer — never a free one it wasn't
    /// entitled to.
    #[test]
    fn a_costing_site_cannot_borrow_a_free_sites_marker() {
        let khan = site("khan-academy");
        let youtube = costing_site("youtube");
        let apps = vec![khan.clone(), youtube.clone()];
        // The YouTube window, with both apps in force: still charged.
        assert_eq!(
            classify(&launched(&youtube), &apps, Some(&none())),
            Bucket::Screen
        );
        // And Khan's own window is unaffected by YouTube's presence.
        assert_eq!(
            classify(&launched(&khan), &apps, Some(&none())),
            Bucket::Learning
        );
    }

    /// Absent `free` means free — every learning clause signed before the
    /// field existed keeps its exact meaning, so no ward changes behaviour on
    /// upgrade. Asserted on the wire shape, not just the struct default.
    #[test]
    fn a_clause_signed_before_the_free_field_existed_is_still_free() {
        let json = r#"{"id":"khan-academy","label":"Khan Academy","kind":"site",
            "domains":["khanacademy.org"],"url":"https://www.khanacademy.org/"}"#;
        let app: LearningApp = serde_json::from_str(json).expect("legacy entry parses");
        assert!(app.is_free());
        assert_eq!(
            classify(&launched(&app), std::slice::from_ref(&app), Some(&none())),
            Bucket::Learning
        );
    }

    /// C2 — THE CRITICAL FORGERY, at the meter. `exec -a /usr/bin/chromium
    /// ~/patched-chromium <exact sanctioned argv>`: the cmdline is BYTE
    /// IDENTICAL to a genuine sanctioned launch (the resolver pin, the class
    /// marker, the profile flag — everything `site_app::is_sanctioned`
    /// checks), but the resolved exe is a ward-owned binary, not the real,
    /// root-owned Chromium. Before `is_sanctioned` gated on `exe_uid`, this
    /// classified as `Bucket::Learning` — free time in a browser that
    /// ignores the resolver pin entirely (it isn't even really Chromium).
    #[test]
    fn a_forged_runtime_with_a_perfect_sanctioned_cmdline_is_not_free() {
        let khan = site("khan-academy");
        let mut forged = launched(&khan);
        forged.exe = Some("/home/kid/patched-chromium".into());
        forged.exe_uid = Some(1002);
        assert_eq!(
            classify(&forged, std::slice::from_ref(&khan), Some(&none())),
            Bucket::Screen
        );
        // The identical cmdline from the REAL, root-owned runtime is
        // unaffected — this is about WHICH BINARY runs, not how it was
        // typed (`launched()` already stands in for a hand-typed
        // reproduction, since it isn't the shim invoking it here either).
        assert_eq!(
            classify(&launched(&khan), std::slice::from_ref(&khan), Some(&none())),
            Bucket::Learning
        );
    }

    #[test]
    fn plain_browser_and_forged_class_stay_screen() {
        let plain = proc_of(&["/usr/bin/chromium"], Some("/usr/bin/chromium"), Some(0));
        assert_eq!(
            classify(&plain, &[site("khan-academy")], Some(&none())),
            Bucket::Screen
        );
        // A ward launching chromium with the class marker but WITHOUT the
        // resolver pin gets a normal browser — it must stay on the meter.
        let forged = proc_of(
            &["/usr/bin/chromium", "--class=charter-khan-academy"],
            Some("/usr/bin/chromium"),
            Some(0),
        );
        assert_eq!(
            classify(&forged, &[site("khan-academy")], Some(&none())),
            Bucket::Screen
        );
    }

    /// The forgery that used to work, at the level that matters: the marker is
    /// genuine and the pin is worthless, so the ward gets an UNPINNED browser
    /// whose time is free. `classify` only ever checked that some
    /// `--host-resolver-rules` token existed, never what it said.
    #[test]
    fn a_marker_with_a_worthless_pin_is_not_free_time() {
        let khan = site("khan-academy");
        let p = proc_of(
            &[
                "/usr/bin/chromium",
                "--app=https://www.khanacademy.org/",
                "--class=charter-khan-academy",
                "--host-resolver-rules=MAP nothing",
                "--user-data-dir=/home/robin/.local/share/charter/learn/khan-academy",
            ],
            Some("/usr/bin/chromium"),
            Some(0),
        );
        assert_eq!(classify(&p, &[khan], Some(&none())), Bucket::Screen);
    }

    /// A pin that is a PREFIX of the real one is still not the real one: this
    /// argv omits `EXCLUDE *.khanacademy.org`, and a near-miss must not be
    /// waved through just because it looks plausible.
    #[test]
    fn a_truncated_pin_is_not_free_time() {
        let p = proc_of(
            &[
                "/usr/bin/chromium",
                "--app=https://www.khanacademy.org/",
                "--class=charter-khan-academy",
                "--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE khanacademy.org",
                "--user-data-dir=/home/robin/.local/share/charter/learn/khan-academy",
            ],
            Some("/usr/bin/chromium"),
            Some(0),
        );
        assert_eq!(
            classify(&p, &[site("khan-academy")], Some(&none())),
            Bucket::Screen
        );
    }

    /// Reproducing the sanctioned command line by hand is FINE — it yields the
    /// same sandbox. The pin is the boundary; the class marker is only the
    /// meter. This is deliberate, not an oversight.
    #[test]
    fn a_hand_typed_but_identical_launch_is_still_learning() {
        let khan = site("khan-academy");
        let mut p = launched(&khan);
        // Same flags, different profile path (a ward retyping the Exec line).
        let i = p
            .cmdline
            .iter()
            .position(|t| t.starts_with("--user-data-dir="))
            .unwrap();
        p.cmdline[i] = "--user-data-dir=/tmp/mine".into();
        assert_eq!(classify(&p, &[khan], Some(&none())), Bucket::Learning);
    }

    #[test]
    fn native_identity_honours_ownership_and_trust() {
        let apps = [native("/usr/bin/gcompris-qt", false)];
        let sys = proc_of(&["gcompris-qt"], Some("/usr/bin/gcompris-qt"), Some(0));
        assert_eq!(classify(&sys, &apps, Some(&none())), Bucket::Learning);
        // Same path but ward-owned file (a planted impostor) — not root-owned,
        // not trusted → Screen.
        let planted = proc_of(&["gcompris-qt"], Some("/usr/bin/gcompris-qt"), Some(1002));
        assert_eq!(classify(&planted, &apps, Some(&none())), Bucket::Screen);
        // A guardian-vouched home-dir project counts despite ward ownership —
        // matched by argv, since a script's /proc/exe is its interpreter.
        let vouched = [native("/home/child/projects/examplegame/run.sh", true)];
        let own = proc_of(
            &["/bin/sh", "/home/child/projects/examplegame/run.sh"],
            Some("/usr/bin/dash"),
            Some(0),
        );
        assert_eq!(classify(&own, &vouched, Some(&none())), Bucket::Learning);
        // But WITHOUT the trusted vouch the same argv gains nothing.
        let unvouched = [native("/home/child/projects/examplegame/run.sh", false)];
        assert_eq!(classify(&own, &unvouched, Some(&none())), Bucket::Screen);
    }

    /// THE ASYMMETRY (spec §2.2). A `cmdline:` learning identity that matches
    /// a user-owned binary must NOT be free time — a ward could otherwise
    /// compile a program whose argv merely claims to be the sanctioned one
    /// and make their own playtime free. The same match on a root-owned exe
    /// (the ward cannot have written or replaced it) IS free, exactly like
    /// the existing path-form rule this mirrors.
    #[test]
    fn cmdline_identity_is_never_free_time_on_a_user_owned_binary() {
        let apps = [native("cmdline:net.minecraft.client.main.Main", false)];
        let cmdline = [
            "java",
            "-cp",
            "/home/kid/.minecraft/libs/x.jar",
            "net.minecraft.client.main.Main",
        ];
        // Root-owned java — enforcement-grade — IS learning time.
        let system = proc_of(
            &cmdline,
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some(0),
        );
        assert_eq!(classify(&system, &apps, Some(&none())), Bucket::Learning);
        // The SAME cmdline, but the exe is ward-owned (planted): the identity
        // still matches on paper, but must stay Screen, never Learning.
        let planted = proc_of(
            &cmdline,
            Some("/home/kid/.minecraft/libs/x.jar"),
            Some(1002),
        );
        assert_eq!(classify(&planted, &apps, Some(&none())), Bucket::Screen);
        // A guardian-vouched (`trusted`) app is exempt from the ownership
        // gate, exactly like the path form.
        let vouched = [native("cmdline:net.minecraft.client.main.Main", true)];
        assert_eq!(
            classify(&planted, &vouched, Some(&none())),
            Bucket::Learning
        );
    }

    /// C1 — THE STRUCTURAL ASYMMETRY, proven against the flatpak/bare-name
    /// catch-all arm (the one that shipped WITHOUT a gate). Reproduces the
    /// exact bypass: `cp any-gui-program ~/bwrap && ~/bwrap org.kde.gcompris`
    /// — a ward-owned binary renamed to end in `bwrap`, invoked with the
    /// flatpak id as its own argv. Before the structural gate, this arm's
    /// `hit` alone decided the bucket, so this classified the WHOLE session
    /// as free learning time. Same probe again for the bare-name form
    /// (`gcompris-qt`, which `resolve_exec` can emit) — the catch-all arm
    /// makes no distinction between a flatpak id and a bare name at all.
    #[test]
    fn a_forged_flatpak_or_bare_name_identity_on_a_planted_binary_is_not_free() {
        let flatpak_app = [native("org.kde.gcompris", false)];
        let forged = proc_of(
            &["/home/kid/bwrap", "org.kde.gcompris"],
            Some("/home/kid/bwrap"), // ward-renamed, NOT root-owned, NOT real bwrap
            Some(1002),
        );
        assert_eq!(
            classify(&forged, &flatpak_app, Some(&none())),
            Bucket::Screen
        );

        let bare_name_app = [native("gcompris-qt", false)];
        let forged_bare = proc_of(
            &["/home/kid/bwrap", "gcompris-qt"],
            Some("/home/kid/bwrap"),
            Some(1002),
        );
        assert_eq!(
            classify(&forged_bare, &bare_name_app, Some(&none())),
            Bucket::Screen
        );

        // The identical forgery against a REAL root-owned bwrap still counts
        // — this arm's coverage of genuine flatpaks is unchanged.
        let real = proc_of(
            &["/usr/bin/bwrap", "org.kde.gcompris"],
            Some("/usr/bin/bwrap"),
            Some(0),
        );
        assert_eq!(
            classify(&real, &flatpak_app, Some(&none())),
            Bucket::Learning
        );
    }

    /// §2.4 — THE CASE C1's `exe_uid` GATE CANNOT CLOSE: a `flatpak install
    /// --user` impostor of a REAL system app id. The exe is the genuine,
    /// root-owned `/usr/bin/bwrap` (no forged binary at all — this is
    /// `flatpak install --user org.kde.gcompris` run straight, exactly as the
    /// design doc describes), and the cgroup/argv are indistinguishable from
    /// the system install `real` above. Only the device's OWN inventory
    /// knowledge (`user_installed` flags this identity) can refuse it free
    /// time; the identical match with NO such flag stays Learning.
    #[test]
    fn a_user_installed_flatpak_identity_never_credits_learning() {
        let flatpak_app = [native("org.kde.gcompris", false)];
        let user_flatpak = proc_of(
            &["/usr/bin/bwrap", "org.kde.gcompris"],
            Some("/usr/bin/bwrap"), // genuine, root-owned bwrap — not a forgery
            Some(0),
        );
        let mut flagged = std::collections::BTreeSet::new();
        flagged.insert("org.kde.gcompris".to_string());
        assert_eq!(
            classify(&user_flatpak, &flatpak_app, Some(&flagged)),
            Bucket::Screen,
            "a userInstalled flatpak identity must never be free time"
        );
        // The SAME process, SAME cmdline, but the inventory has NOT flagged
        // this identity user-installed (a genuine system-wide install) —
        // unaffected, still Learning.
        assert_eq!(
            classify(&user_flatpak, &flatpak_app, Some(&none())),
            Bucket::Learning
        );
    }

    /// I7 — a guardian-vouched (`trusted`) app must stay free EVEN WHEN the
    /// inventory flags it `userInstalled`. A parent who runs `flatpak install
    /// --user org.kde.gcompris` for their child and then deliberately ticks it
    /// as learning — vouching for it themselves — must not have that homework
    /// silently charged to the screen budget, which is exactly the
    /// "learning never worked and said nothing" failure class this whole
    /// feature exists to close, not reproduce. The un-vouched identical
    /// process (previous test) still correctly stays Screen.
    #[test]
    fn a_trusted_learning_app_stays_free_even_when_user_installed() {
        let vouched_flatpak = [native("org.kde.gcompris", true)];
        let user_flatpak = proc_of(
            &["/usr/bin/bwrap", "org.kde.gcompris"],
            Some("/usr/bin/bwrap"),
            Some(0),
        );
        let mut flagged = std::collections::BTreeSet::new();
        flagged.insert("org.kde.gcompris".to_string());
        assert_eq!(
            classify(&user_flatpak, &vouched_flatpak, Some(&flagged)),
            Bucket::Learning,
            "trusted must exempt the user-installed gate, same as the ownership gate"
        );
    }

    /// C-C — an UNKNOWN inventory (`None`) is not an EMPTY one. The startup
    /// scan can be held hostage by a ward-mounted hung FUSE at
    /// `~/.local/share/applications` (no privilege needed), and if unknown
    /// answered like empty, `flatpak install --user <governed learning id>`
    /// after a reboot would be free time — the exact hole §2.4 closes. While
    /// the inventory is unknown, the arm that DEPENDS on it fails CLOSED to
    /// Screen; a guardian-vouched `trusted` identity never depended on the
    /// inventory and is unaffected, as is the path arm (gated by `exe_uid`,
    /// not the inventory).
    #[test]
    fn an_unknown_inventory_fails_the_flatpak_arm_closed_not_open() {
        let flatpak_app = [native("org.kde.gcompris", false)];
        let user_flatpak = proc_of(
            &["/usr/bin/bwrap", "org.kde.gcompris"],
            Some("/usr/bin/bwrap"),
            Some(0),
        );
        // Unknown inventory: no free time on the inventory-dependent arm.
        assert_eq!(classify(&user_flatpak, &flatpak_app, None), Bucket::Screen);
        // A KNOWN-empty inventory still grants it — the pre-existing rule.
        assert_eq!(
            classify(&user_flatpak, &flatpak_app, Some(&none())),
            Bucket::Learning
        );
        // Trusted never consulted the inventory; unaffected.
        let vouched = [native("org.kde.gcompris", true)];
        assert_eq!(classify(&user_flatpak, &vouched, None), Bucket::Learning);
        // The path arm is gated by exe_uid, not the inventory; unaffected.
        let path_app = [native("/usr/bin/gcompris-qt", false)];
        let sys = proc_of(&["gcompris-qt"], Some("/usr/bin/gcompris-qt"), Some(0));
        assert_eq!(classify(&sys, &path_app, None), Bucket::Learning);
    }

    #[test]
    fn no_apps_or_no_match_is_screen() {
        let p = proc_of(&["/usr/bin/minecraft"], Some("/usr/bin/minecraft"), Some(0));
        assert_eq!(classify(&p, &[], Some(&none())), Bucket::Screen);
        assert_eq!(
            classify(&FocusedProcess::default(), &[site("k")], Some(&none())),
            Bucket::Screen
        );
    }

    // ---- is_unrecognised / governed_pkgs (§2.3, redefined) -----------------
    //
    // The definition: unrecognised = NOT enforcement-grade (exe_uid != root)
    // AND not matched by any identity any clause currently names. Every case
    // below is a real bug a reviewer found by checking the PRIOR
    // (inventory-membership) definition against an actual Mint laptop —
    // pinned here so it can never regress back to a false-accusation
    // definition.

    fn no_governed() -> Vec<String> {
        Vec::new()
    }

    /// No process tree — every walk ends immediately.
    fn no_tree(_pid: u32) -> Option<crate::ancestry::ProcId> {
        None
    }

    /// Kintrinsic's OWN lock shade: `charter.desktop` says `Exec=charter-console`,
    /// but what actually runs when a child is locked out is
    /// `/usr/bin/charter-lock` — a root-owned binary the inventory never even
    /// scanned (it's `charter-*`, deliberately skipped). Under the OLD
    /// definition this accrued hours every single time a child ran out of
    /// time. Root-owned ⇒ zero, unconditionally, regardless of any clause.
    #[test]
    fn charters_own_lock_shade_is_never_unrecognised() {
        let p = proc_of(
            &["/usr/bin/charter-lock"],
            Some("/usr/bin/charter-lock"),
            Some(0),
        );
        assert!(!is_unrecognised(&p, &no_governed()));
    }

    /// A wrapper-script browser: `/usr/bin/brave-browser-stable` `exec`s the
    /// REAL binary at `/opt/brave.com/brave/brave`. Both paths are root-owned
    /// regardless of which one `/proc/<pid>/exe` resolves to post-exec, so
    /// this must be zero however the guardian named the browser (or didn't).
    #[test]
    fn a_wrapper_script_browser_is_never_unrecognised() {
        let p = proc_of(
            &["/opt/brave.com/brave/brave"],
            Some("/opt/brave.com/brave/brave"),
            Some(0),
        );
        assert!(!is_unrecognised(&p, &no_governed()));
    }

    /// A multi-process suite: LibreOffice's actual running binary is
    /// `soffice.bin`, not the `soffice` wrapper a `.desktop` `Exec=` would
    /// name. Root-owned either way — zero, with no clause naming either form.
    #[test]
    fn a_multi_process_suites_real_binary_is_never_unrecognised() {
        let p = proc_of(
            &["/usr/lib/libreoffice/program/soffice.bin"],
            Some("/usr/lib/libreoffice/program/soffice.bin"),
            Some(0),
        );
        assert!(!is_unrecognised(&p, &no_governed()));
    }

    /// A snap: root-owned (snaps install under `/snap`, root-owned like any
    /// other system package) and, under the OLD definition, never scanned by
    /// the `.desktop` inventory at all (snap's own mount namespace). Zero.
    #[test]
    fn a_snap_is_never_unrecognised() {
        let p = proc_of(
            &["/snap/firefox/current/usr/lib/firefox/firefox"],
            Some("/snap/firefox/current/usr/lib/firefox/firefox"),
            Some(0),
        );
        assert!(!is_unrecognised(&p, &no_governed()));
    }

    /// The SUPPORTED Minecraft path, checked end to end: the launcher window
    /// AND the *bare* JVM child it spawns are both root-owned (the launcher
    /// itself, and the SYSTEM java at `/usr/lib/jvm/...`) — zero for both,
    /// regardless of whether a Play bucket happens to name either one. This
    /// is the case that contradicted the OLD definition (`Play: 60 of 60` AND
    /// `3h unrecognised` about the very same three hours).
    ///
    /// NB "system java is never unrecognised" is NOT the rule, and must not
    /// be read out of this test: a system java running a WARD-AUTHORED jar is
    /// counted (see `system_java_running_a_ward_owned_jar_is_unrecognised`).
    /// What makes this case zero is that the JVM's argv names no ward file at
    /// all.
    #[test]
    fn the_minecraft_launcher_and_its_bare_system_jvm_child_are_never_unrecognised() {
        let launcher = proc_of(&[], Some("/usr/bin/minecraft-launcher"), Some(0));
        let jvm = proc_of(
            &["/usr/lib/jvm/java-21-openjdk/bin/java"],
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some(0),
        );
        assert!(!is_unrecognised(&launcher, &no_governed()));
        assert!(!is_unrecognised(&jvm, &no_governed()));
    }

    /// A user-owned binary with NO clause naming it (a renamed launcher, a
    /// home-dir Prism/MultiMC bundling its OWN portable JRE under the ward's
    /// home directory, a repacked jar run through that portable JRE) IS
    /// counted — the positive signal the redefinition exists to give.
    #[test]
    fn a_user_owned_unclaused_binary_is_unrecognised() {
        let p = proc_of(
            &["/home/kid/.local/share/PrismLauncher/java/bin/java"],
            Some("/home/kid/.local/share/PrismLauncher/java/bin/java"),
            Some(1002),
        );
        assert!(is_unrecognised(&p, &no_governed()));
    }

    /// The same user-owned identity, but a guardian DELIBERATELY put it in a
    /// bucket (or any other clause sharing the vocabulary) — matched, so it
    /// must NOT be flagged. This is what makes the counter compatible with a
    /// guardian's own choice to manage user-installed software directly.
    #[test]
    fn a_user_owned_binary_matched_by_a_governing_clause_is_not_unrecognised() {
        let p = proc_of(
            &["/home/kid/.local/bin/prismlauncher"],
            Some("/home/kid/.local/bin/prismlauncher"),
            Some(1002),
        );
        let governed = vec!["/home/kid/.local/bin/prismlauncher".to_string()];
        assert!(!is_unrecognised(&p, &governed));
    }

    /// The ancestor-walk parity with `bucket_id_for`: a guardian's Play
    /// bucket names the (user-owned, portable-JRE) LAUNCHER; the actual
    /// focused window is its JVM CHILD, started by it. Direct-match alone
    /// would miss this exactly the way the old inventory-membership
    /// definition did — `is_unrecognised` must walk the tree too, or a
    /// correctly-configured device still falsely flags its own supported
    /// path.
    #[test]
    fn a_governed_launchers_user_owned_jvm_child_is_not_unrecognised() {
        let tree = |pid: u32| {
            if pid == 300 {
                Some(crate::ancestry::ProcId {
                    ppid: 100,
                    exe: Some("/home/kid/.local/bin/prismlauncher".into()),
                    exe_uid: Some(1002),
                    cmdline: vec![],
                    cgroup: None,
                })
            } else {
                None
            }
        };
        let jvm_child = FocusedProcess {
            cmdline: vec![],
            exe: Some("/home/kid/.local/share/PrismLauncher/java/bin/java".into()),
            exe_uid: Some(1002),
            cgroup: None,
            pid: 300,
        };
        let governed = vec!["/home/kid/.local/bin/prismlauncher".to_string()];
        assert!(!is_unrecognised_with(&jvm_child, &governed, tree,));
    }

    // ---- N2: UNKNOWN ownership is never a finding ---------------------------

    /// The rule itself, on the shapes that STILL produce `exe_uid == None`
    /// after D2: the process exited between the probe and the stat, or its
    /// `/proc/<pid>/exe` was never readable to us. Absence of evidence is not
    /// a finding.
    ///
    /// NB this deliberately no longer claims the SANDBOX case. Before D2,
    /// ownership was read by rendering the link to a path string, so a
    /// flatpak's `/app/bin/<foo>` — real only inside the sandbox's own mount
    /// namespace — stat'd to nothing and every flatpak landed here. Stat'ing
    /// the magic symlink resolves the real inode through any namespace, so
    /// those processes now have KNOWN ownership and are decided by the
    /// ownership rule instead. Their verdicts are pinned in
    /// `flatpak_verdicts_after_d2` below rather than left implied here.
    #[test]
    fn unknown_exe_ownership_never_accrues_however_it_arose() {
        for exe in [
            None,                                  // unreadable /proc/<pid>/exe
            Some("/tmp/deleted-binary"),           // raced the process exit
            Some("/home/kid/.local/bin/whatever"), // stat raced the exit
        ] {
            let p = proc_of(&["whatever"], exe, None);
            assert!(
                !is_unrecognised(&p, &no_governed()),
                "exe={exe:?} — an error is not evidence of unrecognised software"
            );
        }
    }

    /// **The sandboxed-app verdicts, as production now actually produces
    /// them.** Measured live before these fixtures were written, on a real
    /// system flatpak (`io.github.input_leap.input-leap`, rendered
    /// `/app/bin/input-leap`, absent on the host): the old path-based lookup
    /// answered `None`, the magic-symlink lookup answers `Some(0)`. The
    /// ward-owned counterpart was reproduced with a ward-owned binary behind
    /// a namespace-only path: old `None`, new `Some(1000)`.
    ///
    /// So both flatpak kinds are now decided by ownership, not by a failed
    /// stat — and they part company, which is the point:
    #[test]
    fn flatpak_verdicts_after_d2() {
        // A SYSTEM flatpak: real files under /var/lib/flatpak, root-owned.
        // Silent — same verdict as before D2, reached by the ownership rule
        // instead of by an unreadable path.
        let system = proc_of(
            &["/app/bin/gcompris-qt"],
            Some("/app/bin/gcompris-qt"),
            Some(0),
        );
        assert!(
            !is_unrecognised(&system, &no_governed()),
            "a root-owned sandboxed app is vouched for"
        );

        // A `flatpak install --user` app: real files under the ward's own
        // home, ward-owned. This ACCRUES now where it never did before —
        // deliberately. It is ward-installed software the guardian has not
        // named: the "ward installed Prism" case this counter exists for.
        let user_installed = proc_of(
            &["/app/bin/gcompris-qt"],
            Some("/app/bin/gcompris-qt"),
            Some(1002),
        );
        assert!(
            is_unrecognised(&user_installed, &no_governed()),
            "a ward-installed sandboxed app the guardian never named must be counted"
        );

        // …and naming it in any clause still settles it, exactly as for an
        // unsandboxed ward-owned binary.
        let governed = vec!["/app/bin/gcompris-qt".to_string()];
        assert!(!is_unrecognised(&user_installed, &governed));
    }

    // ---- N3: suppression demands an EXACT identity --------------------------

    /// `cp game ~/.local/bin/gcompris-qt`. The learning path compares
    /// ABSOLUTE paths, so this copy earns no free time; learning has no kill
    /// rule, so nothing stops it. But the SUPPRESSION test used the same wide
    /// basename matcher enforcement uses, so basename-matching the governed
    /// `/usr/bin/gcompris-qt` silenced the counter for free — a real hole the
    /// prior round's docs called "closed by construction". Suppression now
    /// requires the exact identity, so the rename accrues.
    #[test]
    fn a_ward_owned_rename_of_a_governed_binarys_basename_still_accrues() {
        let p = proc_of(
            &["/home/kid/.local/bin/gcompris-qt"],
            Some("/home/kid/.local/bin/gcompris-qt"),
            Some(1002),
        );
        let governed = vec!["/usr/bin/gcompris-qt".to_string()];
        assert!(
            is_unrecognised(&p, &governed),
            "a basename-only match must not suppress the counter"
        );
        // …and the genuine, exactly-named binary still does suppress it.
        let real = proc_of(
            &["/usr/bin/gcompris-qt"],
            Some("/usr/bin/gcompris-qt"),
            Some(1002),
        );
        assert!(!is_unrecognised(&real, &governed));
    }

    /// The same exactness on the ANCESTOR walk — otherwise a ward's renamed
    /// copy merely has to be *started by* something whose basename matches.
    #[test]
    fn an_ancestor_that_only_basename_matches_does_not_suppress() {
        let tree = |pid: u32| {
            if pid == 300 {
                Some(crate::ancestry::ProcId {
                    ppid: 100,
                    exe: Some("/home/kid/.local/bin/prismlauncher".into()),
                    exe_uid: Some(1002),
                    cmdline: vec![],
                    cgroup: None,
                })
            } else {
                None
            }
        };
        let child = FocusedProcess {
            cmdline: vec![],
            exe: Some("/home/kid/.local/share/PrismLauncher/java/bin/java".into()),
            exe_uid: Some(1002),
            cgroup: None,
            pid: 300,
        };
        // The guardian governs the DISTRO's prismlauncher, not the ward's copy.
        let governed = vec!["/usr/bin/prismlauncher".to_string()];
        assert!(is_unrecognised_with(&child, &governed, tree,));
    }

    /// **FORGERY 2 — argv0.** `exec -a /usr/bin/gcompris-qt
    /// ~/.local/bin/evil`: one shell builtin, no privilege, and under the
    /// argv0 arm it was byte-for-byte as effective as the `cp` rename above.
    /// Suppression does not consult argv0 at all.
    #[test]
    fn a_forged_argv0_does_not_suppress() {
        let p = FocusedProcess {
            cmdline: vec!["/usr/bin/gcompris-qt".into()],
            exe: Some("/home/kid/.local/bin/evil".into()),
            exe_uid: Some(1002),
            cgroup: None,
            pid: 0,
        };
        let governed = vec!["/usr/bin/gcompris-qt".to_string()];
        assert!(is_unrecognised(&p, &governed));
        // Enforcement still believes argv0 — as it should: a forged argv0
        // there only gets the ward's own process killed.
        assert!(crate::app_rules::pkg_matches_process(
            "/usr/bin/gcompris-qt",
            p.exe.as_deref(),
            p.cmdline.first().map(String::as_str),
            &p.cmdline,
            &p.cmdline.join(" "),
            None,
        ));
    }

    /// **FORGERY 3 — the cgroup path.** `systemd-run --user --scope
    /// --unit=app-flatpak-org.kde.gcompris-9999 <anything>` runs
    /// unprivileged and produces a scope name satisfying the flatpak arm's
    /// `flatpak-<id>-` containment test, so ANY ward binary could wear a
    /// governed flatpak's identity. Believed only for a root-owned exe now.
    #[test]
    fn a_hand_made_flatpak_scope_does_not_suppress() {
        let p = FocusedProcess {
            cmdline: vec!["/home/kid/.local/bin/evil".into()],
            exe: Some("/home/kid/.local/bin/evil".into()),
            exe_uid: Some(1002),
            cgroup: Some(
                "0::/user.slice/user-1002.slice/user@1002.service/\
                 app.slice/app-flatpak-org.kde.gcompris-9999.scope"
                    .into(),
            ),
            pid: 0,
        };
        let governed = vec!["org.kde.gcompris".to_string()];
        assert!(is_unrecognised(&p, &governed));
    }

    /// **FORGERY 4 — the `cmdline:` substring.** That identity is containment
    /// over the process's OWN argv, so a ward need only mention the needle in
    /// their command line for the counter to go quiet. Never consulted by
    /// suppression at all now — a root-owned exe does not make argv
    /// trustworthy — the ward hands even a root-owned program its argv, and
    /// after D1 a root-owned exe is out of the counter's scope regardless.
    #[test]
    fn a_self_declared_cmdline_needle_does_not_suppress() {
        let needle = "net.minecraft.client.main.Main";
        let forged = proc_of(
            &["/home/kid/.local/bin/evil", needle],
            Some("/home/kid/.local/bin/evil"),
            Some(1002),
        );
        let governed = vec![format!("cmdline:{needle}")];
        assert!(is_unrecognised(&forged, &governed));
        // A ROOT-owned JVM is vouched for by the ownership rule alone —
        // the ward could not have written or replaced it.
        let real = proc_of(
            &["/usr/lib/jvm/java-21-openjdk/bin/java", needle],
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some(0),
        );
        assert!(!is_unrecognised(&real, &governed));
    }

    /// The ancestor walk must be exactly as strict, or a forged process
    /// merely has to be STARTED BY one. Both ward-writable ancestor fields,
    /// on a tree whose every node is ward-owned.
    #[test]
    fn a_forged_ancestor_identity_does_not_suppress_either() {
        let tree = |pid: u32| {
            (pid == 300).then(|| crate::ancestry::ProcId {
                ppid: 100,
                exe: Some("/home/kid/.local/bin/parent".into()),
                exe_uid: Some(1002),
                // Forged argv0 AND a hand-made flatpak scope, together.
                cmdline: vec![
                    "/usr/bin/gcompris-qt".into(),
                    "net.minecraft.client.main.Main".into(),
                ],
                cgroup: Some("0::/user.slice/app-flatpak-org.kde.gcompris-1.scope".into()),
            })
        };
        let child = FocusedProcess {
            cmdline: vec![],
            exe: Some("/home/kid/.local/bin/evil".into()),
            exe_uid: Some(1002),
            cgroup: None,
            pid: 300,
        };
        for pkg in [
            "/usr/bin/gcompris-qt",
            "org.kde.gcompris",
            "cmdline:net.minecraft.client.main.Main",
        ] {
            assert!(
                is_unrecognised_with(&child, &[pkg.to_string()], tree),
                "a ward-owned ancestor must not launder {pkg}"
            );
        }
    }

    /// An AppImage must not regress: its runtime IS the ward-owned exe, so
    /// the ownership condition counts it on its own. This is one of the
    /// realistic routes D1 kept — the counter's whole remaining scope is
    /// "running from a ward-owned executable".
    #[test]
    fn an_appimage_still_accrues_via_its_ward_owned_runtime() {
        let p = proc_of(
            &["/home/kid/Apps/Game.AppImage"],
            Some("/home/kid/Apps/Game.AppImage"),
            Some(1002),
        );
        assert!(is_unrecognised_with(&p, &no_governed(), no_tree));
    }

    /// **Ancestor laundering** (round 4, still live after D1): the focused
    /// process is the ward's own binary, STARTED BY a root-owned `bash`
    /// wearing a forged `cmdline:` needle in its argv inside a hand-made
    /// `app-flatpak-*` scope. An earlier round passed the ancestor's
    /// `exe_uid` into the matcher, so the root-owned parent made those
    /// ward-written fields "credible" and one unprivileged `systemd-run …
    /// bash -c` laundered both. The walk now believes an ancestor's
    /// kernel-resolved exe path and nothing else.
    #[test]
    fn a_root_owned_forged_ancestor_does_not_launder_a_ward_binary() {
        let tree = |pid: u32| {
            (pid == 300).then(|| crate::ancestry::ProcId {
                ppid: 100,
                exe: Some("/bin/bash".into()),
                exe_uid: Some(0), // ROOT-owned — the part round 3 trusted
                cmdline: vec![
                    "bash".into(),
                    "-c".into(),
                    "net.minecraft.client.main.Main; /home/kid/.local/bin/evil".into(),
                ],
                cgroup: Some("0::/user.slice/app-flatpak-org.kde.gcompris-9999.scope".into()),
            })
        };
        let child = FocusedProcess {
            cmdline: vec![],
            exe: Some("/home/kid/.local/bin/evil".into()),
            exe_uid: Some(1002),
            cgroup: None,
            pid: 300,
        };
        for pkg in ["org.kde.gcompris", "cmdline:net.minecraft.client.main.Main"] {
            assert!(
                is_unrecognised_with(&child, &[pkg.to_string()], tree),
                "a root-owned forged ancestor must not launder {pkg}"
            );
        }
    }

    // ---- D1 (round 5): the scope line, stated as a test ---------------------
    //
    // The ward-authored-payload limb is GONE. It was built twice and withdrawn
    // twice because it is not decidable from outside: "a root-owned program
    // with the ward's own file in its argv" is the shape of a repacked jar AND
    // of a system app opening the child's own document. Every separating rule
    // tried produced a false accusation on a real machine — a suffix+exec-bit
    // test flagged `xed ~/homework.py`; an interpreter allowlist flagged
    // `drawing ~/art.png`, because this distro ships 130 `#!/usr/bin/python*`
    // launchers in /usr/bin alone (581 shebang launchers overall, Mint's image
    // editor and Cinnamon's tools among them), every one of which resolves its
    // exe to /usr/bin/python3.12. Three strikes on "absence of evidence must
    // never become a finding" is the whole argument.

    /// The false accusation D1 exists to prevent, in the exact shape that was
    /// verified live: a distro app that is really a python launcher, opening
    /// the CHILD'S OWN file. Under the round-4 rule this accrued (exe
    /// `/usr/bin/python3.12`, uid 0, ward-owned document in argv); it must
    /// read zero, whether the document is `.txt`, `.png` or a script.
    #[test]
    fn a_distro_python_launcher_opening_the_wards_own_file_is_never_accused() {
        for doc in [
            "/home/kid/homework.txt",
            "/home/kid/art.png",
            "/home/kid/homework.py", // the suffix must not resurrect it either
            "/home/kid/essay.pdf",
        ] {
            // `drawing`, `hp-doctor`, and 128 others on this machine: the
            // shebang means /proc/<pid>/exe IS the interpreter.
            let p = proc_of(
                &["/usr/bin/python3", "/usr/bin/drawing", doc],
                Some("/usr/bin/python3.12"),
                Some(0),
            );
            assert!(
                !is_unrecognised_with(&p, &no_governed(), no_tree),
                "a distro launcher opening {doc} must never be a finding"
            );
        }
    }

    /// The honest cost of D1, pinned so nobody mistakes it for a bug: the
    /// repacked jar under the SYSTEM JVM is deliberately NOT counted. If this
    /// test ever flips, the false-accusation cases above will have come back
    /// with it — they are the same rule.
    #[test]
    fn a_ward_jar_under_a_root_owned_jvm_is_deliberately_not_counted() {
        let p = proc_of(
            &["/usr/bin/java", "-jar", "/home/kid/repacked.jar"],
            Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
            Some(0),
        );
        assert!(
            !is_unrecognised_with(&p, &no_governed(), no_tree),
            "documented under-reporting: a root-owned interpreter is out of scope"
        );
        // The SAME jar run by the ward's own bundled JRE — the realistic
        // route, and the one still covered.
        let portable = proc_of(
            &[
                "/home/kid/.local/share/PrismLauncher/java/bin/java",
                "-jar",
                "/home/kid/repacked.jar",
            ],
            Some("/home/kid/.local/share/PrismLauncher/java/bin/java"),
            Some(1002),
        );
        assert!(is_unrecognised_with(&portable, &no_governed(), no_tree));
    }

    /// No argv token, of any ownership, can make a root-owned exe accrue —
    /// the whole ward-payload surface (and NEW-2, where appending a governed
    /// path as an inert argument was an off switch) is gone, not narrowed.
    #[test]
    fn no_argv_token_can_make_a_root_owned_exe_accrue() {
        for argv in [
            vec!["/usr/bin/python3", "/home/kid/evil.py"],
            vec![
                "/usr/bin/python3",
                "/home/kid/evil.py",
                "/home/kid/tutor.py",
            ],
            vec!["/bin/bash", "/home/kid/x.sh"],
            vec!["/usr/bin/java", "-jar", "/home/kid/x.jar"],
        ] {
            let p = proc_of(&argv, Some(argv[0]), Some(0));
            assert!(!is_unrecognised_with(&p, &no_governed(), no_tree));
        }
    }

    // ---- governed_pkgs -------------------------------------------------------

    #[test]
    fn governed_pkgs_unions_native_learning_bucket_apprules_and_apps_clause() {
        let learning = [
            native("/usr/bin/gcompris-qt", false),
            site("khan-academy"), // excluded: site apps are always root-owned
        ];
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["com.mojang.Minecraft".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "UTC".into(),
            issued_at: 1,
        };
        let app_rules = vec!["org.chromium.vanadium".to_string()];
        let apps_clause = vec!["com.google.android.youtube".to_string()];
        let governed = governed_pkgs(&learning, &buckets, &app_rules, &apps_clause);
        assert_eq!(
            governed,
            vec![
                "/usr/bin/gcompris-qt",
                "com.mojang.Minecraft",
                "org.chromium.vanadium",
                "com.google.android.youtube",
            ]
        );
    }

    /// A paused buckets clause contributes NOTHING — it isn't currently "in
    /// force", so its apps must not silently suppress the counter either.
    #[test]
    fn a_paused_buckets_clause_contributes_nothing_to_governed_pkgs() {
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["com.mojang.Minecraft".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: Some(true),
            week_start: None,
            tz: "UTC".into(),
            issued_at: 1,
        };
        assert!(governed_pkgs(&[], &buckets, &[], &[]).is_empty());
    }

    // ---- attribute() — the extracted, tested composition ---------------------

    /// The composition itself, proven end to end on the exact scenario that
    /// slipped through under the OLD definition: a Play bucket governs the
    /// Minecraft launcher; the focused window is its root-owned JVM child.
    /// `attribute` must read `Play: <spent>` (a bucket hit) AND
    /// `unrecognised: false` about the SAME three seconds — never both
    /// "spent" and "unrecognised" about identical time, which is precisely
    /// the contradiction a reviewer caught.
    #[test]
    fn attribute_never_double_reports_a_governed_root_owned_process() {
        // The bucket names the JVM's OWN (root-owned, system) path directly,
        // so `bucket_id_for`'s direct-match arm hits with no ancestor tree
        // needed — isolating this test to what `attribute` itself composes,
        // not `bucket_id_for_with`'s separately-tested tree walk.
        let jvm_path = "/usr/lib/jvm/java-21-openjdk/bin/java";
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec![jvm_path.to_string()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "UTC".into(),
            issued_at: 1,
        };
        let jvm = proc_of(&[jvm_path], Some(jvm_path), Some(0));
        let governed = vec![jvm_path.to_string()];
        let (bucket, bucket_id, unrecognised) =
            attribute(&jvm, &[], &buckets, Some(&none()), &governed);
        assert_eq!(bucket, Bucket::Screen); // ordinary play time, not learning
        assert_eq!(bucket_id.as_deref(), Some("play")); // the SAME window spends Play…
        assert!(
            !unrecognised,
            "…and must never ALSO be unrecognised about it"
        );
    }

    /// The same no-double-report invariant where the two matchers DISAGREE:
    /// the bucket names `/usr/bin/prismlauncher`, the ward runs their own
    /// `~/.local/bin/prismlauncher`. `bucket_id_for` (basename, the same rule
    /// the kill sweep uses) meters and caps it under Play; `is_unrecognised`
    /// alone would call it unvouched, since its suppression test is exact.
    /// `attribute` must not report both about the same seconds.
    #[test]
    fn attribute_does_not_report_a_basename_metered_process_as_unrecognised() {
        let buckets = charter_schedule::GrantBuckets {
            v: 1,
            buckets: vec![charter_schedule::AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["/usr/bin/prismlauncher".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "UTC".into(),
            issued_at: 1,
        };
        let wards_copy = "/home/kid/.local/bin/prismlauncher";
        let p = proc_of(&[wards_copy], Some(wards_copy), Some(1002));
        let governed = vec!["/usr/bin/prismlauncher".to_string()];
        // The identity rule ALONE does flag it — the rename hole stays closed.
        assert!(is_unrecognised(&p, &governed));
        // …but the composition sees a live bucket hit and reports one thing.
        let (_, bucket_id, unrecognised) = attribute(&p, &[], &buckets, Some(&none()), &governed);
        assert_eq!(bucket_id.as_deref(), Some("play"));
        assert!(!unrecognised, "never both spent AND unrecognised");
    }

    /// …and with NO bucket in play — a learning clause naming the distro's
    /// binary, which grants the ward's renamed copy no free time and stops it
    /// with nothing — the rename accrues, end to end through `attribute`.
    #[test]
    fn attribute_still_flags_a_rename_that_no_bucket_meters() {
        let wards_copy = "/home/kid/.local/bin/gcompris-qt";
        let p = proc_of(&[wards_copy], Some(wards_copy), Some(1002));
        let governed = vec!["/usr/bin/gcompris-qt".to_string()];
        let learning = [native("/usr/bin/gcompris-qt", false)];
        let (bucket, bucket_id, unrecognised) = attribute(
            &p,
            &learning,
            &charter_schedule::GrantBuckets::default(),
            Some(&none()),
            &governed,
        );
        assert_eq!(bucket, Bucket::Screen, "the copy earns no free time");
        assert_eq!(bucket_id, None, "and nothing meters or stops it");
        assert!(unrecognised, "so it must be counted");
    }

    /// C-B — a guardian-vouched `trusted` learning SCRIPT was being credited
    /// as Learning AND accused as unrecognised for the very same seconds:
    /// `classify`'s path arm supports `trusted && argv contains exec`
    /// (scripts run under an interpreter), while the old suppression rule
    /// knew nothing of the payload. Both halves of the fix hold it shut now:
    /// the script runs under a ROOT-owned interpreter, which after D1 is out
    /// of the counter's scope entirely, AND `attribute`'s learning guard
    /// settles the seconds regardless.
    #[test]
    fn a_trusted_learning_script_is_credited_and_never_accused() {
        let script = "/home/kid/projects/tutor.py";
        let learning = [native(script, true)];
        let p = proc_of(
            &["/usr/bin/python3", script],
            Some("/usr/bin/python3"),
            Some(0),
        );
        // What `governed_pkgs` would build from this clause: the script path.
        let governed = vec![script.to_string()];
        let (bucket, bucket_id, unrecognised) = attribute(
            &p,
            &learning,
            &charter_schedule::GrantBuckets::default(),
            Some(&none()),
            &governed,
        );
        assert_eq!(bucket, Bucket::Learning, "the vouched script is credited");
        assert_eq!(bucket_id, None);
        assert!(
            !unrecognised,
            "learning-credited seconds must never also be reported unrecognised"
        );
        // The identity rule agrees on its own: a root-owned interpreter is
        // vouched, whatever is in its argv.
        assert!(!is_unrecognised_with(&p, &governed, |_| None));
    }

    /// The belt-and-braces half of C-B: even where `is_unrecognised` on its
    /// own WOULD flag the process (a trusted `cmdline:` identity on a
    /// ward-owned exe — suppression's exact matcher never consults argv, so
    /// the identity rule alone says "unvouched and unmatched"), a learning
    /// credit settles the same seconds, mirroring the bucket-hit gate. No
    /// future drift between `classify` and the suppression matcher can accuse
    /// a credited second.
    #[test]
    fn attribute_never_accuses_a_learning_credited_second() {
        let needle = "cmdline:net.minecraft.client.main.Main";
        let vouched = [native(needle, true)];
        let p = proc_of(
            &["/home/kid/mc", "net.minecraft.client.main.Main"],
            Some("/home/kid/mc"),
            Some(1002),
        );
        let governed = vec![needle.to_string()];
        // The identity rule ALONE flags it…
        assert!(is_unrecognised(&p, &governed));
        // …but classify credits it (trusted exemption), and one credit is one
        // story.
        let (bucket, _, unrecognised) = attribute(
            &p,
            &vouched,
            &charter_schedule::GrantBuckets::default(),
            Some(&none()),
            &governed,
        );
        assert_eq!(bucket, Bucket::Learning);
        assert!(!unrecognised, "never both credited AND unrecognised");
    }
}

#[cfg(test)]
mod bucket_attribution_tests {
    use super::*;
    use charter_schedule::{AppBucket, GrantBuckets};

    fn play() -> GrantBuckets {
        GrantBuckets {
            v: 1,
            buckets: vec![AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["/usr/games/supertux2".into(), "com.mojang.Minecraft".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        }
    }

    fn proc_with(exe: Option<&str>, cgroup: Option<&str>) -> FocusedProcess {
        FocusedProcess {
            cmdline: vec![],
            exe: exe.map(str::to_string),
            exe_uid: Some(0),
            cgroup: cgroup.map(str::to_string),
            pid: 0,
        }
    }

    #[test]
    fn a_native_game_in_the_bucket_is_attributed() {
        let p = proc_with(Some("/usr/games/supertux2"), None);
        assert_eq!(bucket_id_for(&p, &play()).as_deref(), Some("play"));
    }

    /// A wrapper/symlink launch of the same binary still spends the allowance
    /// (binary-name equality) — otherwise the cap is trivially side-stepped.
    #[test]
    fn a_wrapper_launch_still_spends_the_allowance() {
        let p = proc_with(Some("/usr/local/bin/supertux2"), None);
        assert_eq!(bucket_id_for(&p, &play()).as_deref(), Some("play"));
    }

    #[test]
    fn a_flatpak_game_is_attributed_by_its_scope() {
        let p = proc_with(
            Some("/usr/bin/bwrap"),
            Some("0::/user.slice/app-flatpak-com.mojang.Minecraft-2331.scope"),
        );
        assert_eq!(bucket_id_for(&p, &play()).as_deref(), Some("play"));
    }

    #[test]
    fn an_unrelated_app_spends_nothing() {
        let p = proc_with(Some("/usr/bin/libreoffice"), None);
        assert!(bucket_id_for(&p, &play()).is_none());
    }

    /// A cap that cannot be trusted must not quietly run someone's clock down.
    #[test]
    fn a_paused_or_malformed_clause_attributes_nothing() {
        let p = proc_with(Some("/usr/games/supertux2"), None);
        let mut paused = play();
        paused.paused = Some(true);
        assert!(bucket_id_for(&p, &paused).is_none());

        let mut broken = play();
        broken.buckets[0].daily_minutes = Some(0);
        assert!(bucket_id_for(&p, &broken).is_none());
    }

    fn cmdline_play() -> GrantBuckets {
        GrantBuckets {
            v: 1,
            buckets: vec![AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["cmdline:net.minecraft.client.main.Main".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        }
    }

    fn minecraft_jvm() -> FocusedProcess {
        FocusedProcess {
            cmdline: vec![
                "java".into(),
                "-cp".into(),
                "/home/kid/.minecraft/libs/x.jar".into(),
                "net.minecraft.client.main.Main".into(),
            ],
            exe: Some("/usr/lib/jvm/java-21-openjdk/bin/java".into()),
            exe_uid: Some(0),
            cgroup: None,
            pid: 0,
        }
    }

    /// The meter attributes a bare `java -jar`/renamed-launcher JVM to its
    /// bucket by cmdline alone — no launcher ancestor exists at all (`pid: 0`,
    /// and the injected tree in `launcher_attribution_tests` is not even
    /// consulted for a direct match). This is the fourth bypass the design
    /// doc names (`java -jar` directly, no launcher to walk up to).
    #[test]
    fn a_cmdline_identity_is_attributed_with_no_launcher_ancestor() {
        assert_eq!(
            bucket_id_for(&minecraft_jvm(), &cmdline_play()).as_deref(),
            Some("play")
        );
    }

    /// Asymmetry companion (spec §2.2): capping is RESTRICTIVE, so it is
    /// self-harm only — a user-owned binary matching a `cmdline:` identity
    /// still spends the bucket allowance exactly like a root-owned one.
    /// Unlike `classify`, `bucket_id_for`/`pkg_matches_process` take no uid at
    /// all, so there is nothing to gate here by construction.
    #[test]
    fn a_cmdline_identity_still_caps_a_user_owned_binary() {
        let mut p = minecraft_jvm();
        p.exe = Some("/home/kid/mc".into());
        p.exe_uid = Some(1002);
        assert_eq!(bucket_id_for(&p, &cmdline_play()).as_deref(), Some("play"));
    }
}

#[cfg(test)]
mod launcher_attribution_tests {
    use super::*;
    use crate::ancestry::ProcId;
    use charter_schedule::{AppBucket, GrantBuckets};
    use std::collections::BTreeMap;

    /// decented's actual laptop, 2026-07-25: Minecraft is the official Mojang
    /// launcher (`minecraft-launcher`), and the window the ward looks at is the
    /// JAVA game it spawns. The guardian picks "Minecraft" by name in
    /// Kintrinsic and must never have to know any of that.
    fn play_bucket() -> GrantBuckets {
        GrantBuckets {
            v: 1,
            buckets: vec![AppBucket {
                id: "play".into(),
                label: "Play".into(),
                apps: vec!["/usr/bin/minecraft-launcher".into()],
                daily_minutes: Some(60),
                weekly_minutes: None,
            }],
            paused: None,
            week_start: None,
            tz: "Europe/London".into(),
            issued_at: 1,
        }
    }

    fn tree() -> impl Fn(u32) -> Option<ProcId> + Copy {
        |pid: u32| {
            let rows: BTreeMap<u32, (u32, &str)> = [
                (100, (1, "/usr/bin/minecraft-launcher")),
                (300, (100, "/usr/lib/jvm/java-17-openjdk/bin/java")),
                (400, (1, "/usr/lib/jvm/java-17-openjdk/bin/java")), // homework IDE
            ]
            .into_iter()
            .collect();
            rows.get(&pid).map(|(ppid, exe)| ProcId {
                ppid: *ppid,
                exe: Some((*exe).to_string()),
                exe_uid: Some(0),
                cmdline: vec![],
                cgroup: None,
            })
        }
    }

    fn focused(pid: u32, exe: &str) -> FocusedProcess {
        FocusedProcess {
            cmdline: vec![],
            exe: Some(exe.into()),
            exe_uid: Some(0),
            cgroup: None,
            pid,
        }
    }

    #[test]
    fn the_java_game_under_the_launcher_spends_play() {
        let p = focused(300, "/usr/lib/jvm/java-17-openjdk/bin/java");
        assert_eq!(
            bucket_id_for_with(&p, &play_bucket(), tree()).as_deref(),
            Some("play")
        );
    }

    /// The whole reason for walking the tree rather than naming `java`: an
    /// unrelated Java program is NOT the game, and must not spend the games
    /// allowance (nor be killed when it runs out).
    #[test]
    fn an_unrelated_java_program_spends_nothing() {
        let p = focused(400, "/usr/lib/jvm/java-17-openjdk/bin/java");
        assert!(bucket_id_for_with(&p, &play_bucket(), tree()).is_none());
    }

    #[test]
    fn the_launcher_window_itself_also_counts() {
        let p = focused(100, "/usr/bin/minecraft-launcher");
        assert_eq!(
            bucket_id_for_with(&p, &play_bucket(), tree()).as_deref(),
            Some("play")
        );
    }
}
