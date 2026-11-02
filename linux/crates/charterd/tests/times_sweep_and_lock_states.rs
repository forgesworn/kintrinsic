//! Reproductions for a field report on a Mint ward laptop with named times.
//!
//! The schedule: Free time ending 20:00, overlapped by a large named window
//! 07:00-20:15. The Learning time template was renamed "Project time", so its
//! id stayed `learning` (`TimeEditor.tsx`: a rename never touches the id).
//!
//! Report A: nothing happened at 20:00 and he carried on using whatever he was
//! using. Report B: logging in during the Project window gives the lock screen
//! with "Ask for more time".
//!
//! Tests named `red_*` state a promise the guardian's screen makes and the
//! warden does not keep. They are `#[ignore]`d until a design decision settles
//! how processes outside the app inventory are treated; run them with
//! `--ignored` to see them fail. The rest pin the facts the diagnosis rests on,
//! and pass.

use charter_schedule::{Activity, Bucket, GrantBudget, GrantSchedule, LockReason, StandDown};
use charter_spine::child_policy::{EffectivePolicy, PolicySource};
use charter_spine::multi_child::MultiChildEnforcer;
use charter_spine::times::GrantTimes;
use charterd::ancestry::{matches_with_ancestors, ProcId};
use charterd::app_rules::pkg_matches_process;
use charterd::apps_policy::blocked_pkgs_for_time;
use charterd::times_routing::{
    apply_to_decision, charge_activity, lock_copy, state_for, times_view, warning_text, TimesLock,
    TimesState, Verdict, WarnTracker,
};

/// Mon 2026-06-29 00:00:00 UTC.
const MON: i64 = 1_782_691_200;
const UID: u32 = 1001;

fn at(hh: i64, mm: i64) -> i64 {
    MON + hh * 3600 + mm * 60
}

/// As the guardian app signs it: an untagged Free window, and a tagged window
/// appended after it (`setTimeWindows`).
fn schedule_a() -> GrantSchedule {
    serde_json::from_str(
        r#"{"v":1,"tz":"Etc/UTC","issuedAt":1,"weekly":{"mon":[
            {"start":"16:00","end":"20:00"},
            {"start":"07:00","end":"20:15","time":"learning"}
        ]}}"#,
    )
    .unwrap()
}

/// After "only Project time applies at that hour": no Free window covers it.
fn schedule_b() -> GrantSchedule {
    serde_json::from_str(
        r#"{"v":1,"tz":"Etc/UTC","issuedAt":2,"weekly":{"mon":[
            {"start":"07:00","end":"20:15","time":"learning"}
        ]}}"#,
    )
    .unwrap()
}

fn project(counts: bool) -> GrantTimes {
    let extra = if counts {
        r#","countsAgainstBudget":true"#
    } else {
        ""
    };
    GrantTimes::parse_or_absent(&format!(
        r#"{{"v":1,"issuedAt":1,"times":[{{"id":"learning","label":"Project time",
            "posture":"onlyThese","apps":["/usr/bin/gimp","/usr/bin/libreoffice"]{extra}}}]}}"#
    ))
    .expect("the clause the guardian app signs parses on the warden")
}

fn budget(daily: u32) -> GrantBudget {
    serde_json::from_value(serde_json::json!({
        "v": 1, "tz": "Etc/UTC", "dailyMinutes": daily, "issuedAt": 1
    }))
    .unwrap()
}

fn enforcer(s: &GrantSchedule, b: Option<GrantBudget>, now: i64) -> MultiChildEnforcer {
    let mut multi = MultiChildEnforcer::new();
    multi.sync(
        &[(
            UID,
            EffectivePolicy {
                schedule: Some(s.clone()),
                budget: b,
                learning: None,
                source: PolicySource::Guardian,
            },
        )],
        now,
        |_| (None, None),
    );
    multi
}

fn state(
    multi: &MultiChildEnforcer,
    s: &GrantSchedule,
    t: Option<&GrantTimes>,
    now: i64,
) -> TimesState {
    state_for(multi, Some(s), None, t, true, None, UID, now).unwrap()
}

fn named_id(st: &TimesState) -> Option<&str> {
    match &st.verdict {
        Verdict::Named { id, .. } => Some(id),
        _ => None,
    }
}

// ---- Report A: which time is it, at every hour? ----------------------------

#[test]
fn a_the_free_window_never_runs_and_20_00_is_not_a_transition() {
    let (s, t) = (schedule_a(), project(false));
    let multi = enforcer(&s, None, at(7, 0));
    // Every five minutes across the whole span, the Free hours included.
    let mut now = at(7, 0);
    while now < at(20, 15) {
        let st = state(&multi, &s, Some(&t), now);
        assert_eq!(named_id(&st), Some("learning"), "at {}", (now - MON) / 60);
        assert_eq!(
            st.span_end,
            Some(at(20, 15)),
            "same span at {}",
            (now - MON) / 60
        );
        assert!(!st.remaining.locked);
        now += 300;
    }
    // Outside it: 06:59 and 20:16 are locked; the Free window opened nothing.
    assert!(state(&multi, &s, Some(&t), at(6, 59)).remaining.locked);
    let st = state(&multi, &s, Some(&t), at(20, 16));
    assert!(st.remaining.locked);
    assert_eq!(st.remaining.reason, Some(LockReason::Schedule));
}

#[test]
fn a_the_only_warnings_count_down_to_20_15_and_none_mentions_20_00() {
    let (s, t) = (schedule_a(), project(false));
    let multi = enforcer(&s, None, at(19, 30));
    let mut tracker = WarnTracker::default();
    let mut fired = Vec::new();
    let mut now = at(19, 30);
    while now < at(20, 15) {
        let st = state(&multi, &s, Some(&t), now);
        if let Some(level) = tracker.observe("learning", st.remaining.effective_secs) {
            let (_, body) = warning_text(level, "Project time", None, st.remaining.effective_secs);
            fired.push((now, body));
        }
        now += 2;
    }
    assert_eq!(fired.len(), 2, "{fired:?}");
    assert_eq!(fired[0].0, at(20, 5));
    assert_eq!(fired[0].1, "10 minutes left of Project time — then locked.");
    assert_eq!(fired[1].0, at(20, 14));
    assert_eq!(
        fired[1].1,
        "1 minute left of Project time — then locked. Save your game."
    );
}

fn inventory() -> Vec<String> {
    [
        "/usr/bin/firefox",
        "/usr/bin/gimp",
        "/usr/bin/libreoffice",
        "/usr/bin/minecraft-launcher",
    ]
    .map(String::from)
    .to_vec()
}

fn jvm() -> (String, Vec<String>) {
    let exe = "/opt/ward/.minecraft/runtime/java-runtime-delta/bin/java".to_string();
    let argv = [
        "java",
        "-Xmx2G",
        "-cp",
        "/opt/ward/.minecraft/libraries/x.jar",
        "net.minecraft.client.main.Main",
    ]
    .map(String::from)
    .to_vec();
    (exe, argv)
}

#[test]
fn a_the_sweep_list_is_the_inventory_minus_the_time_s_apps() {
    let t = project(false);
    let blocked = blocked_pkgs_for_time(None, &t.times[0], &inventory(), at(12, 0));
    assert_eq!(
        blocked,
        vec![
            "/usr/bin/firefox".to_string(),
            "/usr/bin/minecraft-launcher".to_string()
        ]
    );
    // Listing Firefox opens it, and nothing here narrows what it can reach:
    // the web half of an only-these time is per-site windows, not Firefox.
    let with_ff: GrantTimes = GrantTimes::parse_or_absent(
        r#"{"v":1,"issuedAt":1,"times":[{"id":"learning","label":"Project time",
            "posture":"onlyThese","apps":["/usr/bin/firefox"]}]}"#,
    )
    .unwrap();
    let blocked = blocked_pkgs_for_time(None, &with_ff.times[0], &inventory(), at(12, 0));
    assert!(!blocked.contains(&"/usr/bin/firefox".to_string()));
}

#[test]
fn a_a_minecraft_jvm_under_a_live_launcher_is_swept_through_its_ancestor() {
    let t = project(false);
    let blocked = blocked_pkgs_for_time(None, &t.times[0], &inventory(), at(12, 0));
    let (exe, argv) = jvm();
    let lookup = |pid: u32| match pid {
        200 => Some(ProcId {
            ppid: 100,
            exe: Some(exe.clone()),
            exe_uid: Some(1001),
            cmdline: argv.clone(),
            cgroup: None,
        }),
        100 => Some(ProcId {
            ppid: 1,
            exe: Some("/opt/minecraft-launcher/minecraft-launcher".into()),
            exe_uid: Some(0),
            cmdline: vec!["/usr/bin/minecraft-launcher".into()],
            cgroup: None,
        }),
        _ => None,
    };
    assert!(blocked
        .iter()
        .any(|pkg| matches_with_ancestors(200, pkg, lookup)));
}

/// The editor's promise (`TimeEditor.tsx`, `POSTURE_HELP.onlyThese`): "Only the
/// apps and websites you choose can be used while this time runs. Everything
/// else is closed." The launcher's "close when the game starts" setting leaves
/// the JVM orphaned; no inventory identity names it and the time's allowlist
/// has no `cmdline:` signature in it (`apps_policy::blocked_pkgs_from_effective`).
#[test]
#[ignore = "design decision pending: processes outside the app inventory survive an only-these sweep"]
fn red_an_only_these_time_closes_a_minecraft_jvm_whose_launcher_has_exited() {
    let t = project(false);
    let blocked = blocked_pkgs_for_time(None, &t.times[0], &inventory(), at(12, 0));
    let (exe, argv) = jvm();
    let joined = argv.join(" ");
    let lookup = |pid: u32| {
        (pid == 200).then(|| ProcId {
            ppid: 1,
            exe: Some(exe.clone()),
            exe_uid: Some(1001),
            cmdline: argv.clone(),
            cgroup: None,
        })
    };
    let hit = blocked.iter().any(|pkg| {
        pkg_matches_process(pkg, Some(&exe), Some("java"), &argv, &joined, None)
            || matches_with_ancestors(200, pkg, lookup)
    });
    assert!(
        hit,
        "only-these leaves this process running: blocked list {blocked:?} names nothing it matches"
    );
}

/// Same promise, the general form: a game the inventory has no launcher for.
#[test]
#[ignore = "design decision pending: processes outside the app inventory survive an only-these sweep"]
fn red_an_only_these_time_closes_a_program_with_no_desktop_launcher() {
    let t = project(false);
    let blocked = blocked_pkgs_for_time(None, &t.times[0], &inventory(), at(12, 0));
    let exe = "/opt/ward/Games/Veloren.AppImage";
    let argv = vec![exe.to_string()];
    let hit = blocked
        .iter()
        .any(|pkg| pkg_matches_process(pkg, Some(exe), Some(exe), &argv, exe, None));
    assert!(
        hit,
        "only-these leaves this process running: blocked list {blocked:?} names nothing it matches"
    );
}

// ---- Report B: which states can show a lock inside the Project window? -----

/// The candidate most reports would assume: with the clause on the device and
/// a real enforcer tick at login, schedule B OPENS. No lock comes from the
/// schedule, from session start or from the times layer.
#[test]
fn b_a_login_inside_the_project_window_opens_when_the_clauses_are_on_the_device() {
    let (s, t) = (schedule_b(), project(false));
    let mut multi = enforcer(&s, Some(budget(120)), at(12, 0));
    let mut login = at(12, 0);
    for _ in 0..3 {
        let st = state(&multi, &s, Some(&t), login);
        let charge = charge_activity(Activity::Active, &st.verdict);
        let mut ds =
            multi.tick_attributed_with(charge, Some(UID), login, 2, Bucket::Screen, None, false);
        let st = state(&multi, &s, Some(&t), login);
        for d in ds.iter_mut() {
            apply_to_decision(d, &st);
            assert!(d.active);
            assert!(!d.locked, "{d:?}");
        }
        assert_eq!(named_id(&st), Some("learning"));
        login += 2;
    }
}

/// `countsAgainstBudget` is only a candidate if the toggle was switched on: a
/// spent budget leaves a non-counting window open, and locks a counting one.
#[test]
fn b_a_spent_budget_locks_the_project_window_only_if_it_counts() {
    let s = schedule_b();
    let now = at(12, 0);
    let mut multi = enforcer(&s, Some(budget(1)), now);
    multi.tick_attributed_with(
        Activity::Active,
        Some(UID),
        now,
        120,
        Bucket::Screen,
        None,
        false,
    );
    let later = now + 120;
    assert_eq!(
        multi.remaining(UID, later).unwrap().reason,
        Some(LockReason::Budget),
        "the core is locked by the budget"
    );
    let free_of_it = project(false);
    assert!(!state(&multi, &s, Some(&free_of_it), later).remaining.locked);
    let counting = project(true);
    let st = state(&multi, &s, Some(&counting), later);
    assert!(st.remaining.locked);
    assert_eq!(st.remaining.reason, Some(LockReason::Budget));
}

/// The symptom, reproduced: the guardian's "Finish for today" outranks the
/// window until the ward's midnight, and the lock still offers an ask. A typed
/// gift cannot lift it. Lapses by itself at `expiresAt`.
#[test]
fn b_finish_for_today_locks_a_login_in_the_project_window_until_midnight() {
    let (s, t) = (schedule_b(), project(false));
    let now = at(20, 5);
    let mut multi = enforcer(&s, None, now);
    multi.set_stand_down(UID, Some(StandDown { secs_until_lock: 0 }));
    let st = state_for(&multi, Some(&s), None, Some(&t), true, Some(0), UID, now).unwrap();
    assert_eq!(st.remaining.reason, Some(LockReason::StandDown));
    assert!(st.remaining.locked);
    assert_eq!(named_id(&st), None);
    // Ordinary stand-down copy, no times headline…
    assert_eq!(lock_copy(&st, Some(&s), Some(&t), now), None);
    // …and the panel is still handed the times to ask for, defaulting to it.
    let v = times_view(&st, Some(&s), Some(&t), &[], chrono_tz::Etc::UTC, now).unwrap();
    assert!(v.ask.iter().any(|c| c.label == "Project time"));
    assert_eq!(v.ask_default.as_deref(), Some("learning"));
    // A gift of Project time does not lift it.
    assert!(multi.apply_time_extension(UID, now, "req-1", 30, "learning"));
    let st = state_for(&multi, Some(&s), None, Some(&t), true, Some(0), UID, now).unwrap();
    assert!(st.remaining.locked);

    // It does not outlive the day: the guardian app stamps expiresAt at the
    // ward's midnight, and an expired clause does not stand.
    let body: charter_proto::StandDownBody = serde_json::from_value(serde_json::json!({
        "v": 1, "issuedAt": now, "id": "x", "expiresAt": at(24, 0), "graceSecs": 60
    }))
    .unwrap();
    assert!(body.stands(at(23, 59) as u64));
    assert!(!body.stands(at(24, 0) as u64));
}

/// What a real laptop's state file shows for each candidate: the lock title
/// tells them apart ("Ask for more time" appears on all of them).
#[test]
fn b_the_lock_title_distinguishes_the_remaining_candidates() {
    let (s, t) = (schedule_b(), project(false));
    let now = at(12, 0);
    let multi = enforcer(&s, None, now);

    // No usable `times` clause on the device: the window's tag is unknown.
    let st = state_for(&multi, Some(&s), None, None, true, None, UID, now).unwrap();
    assert_eq!(
        st.verdict,
        Verdict::Locked(TimesLock::UnknownTime("learning".into()))
    );
    let (title, detail) = lock_copy(&st, Some(&s), None, now).unwrap();
    assert_eq!(title, "Locked today", "no runnable window is left to name");
    assert_eq!(
        detail.as_deref(),
        Some("Nothing is set up for this time yet.")
    );

    // The app inventory has not answered since the daemon started.
    let st = state_for(&multi, Some(&s), None, Some(&t), false, None, UID, now).unwrap();
    assert_eq!(
        st.verdict,
        Verdict::Locked(TimesLock::InventoryUnknown("learning".into()))
    );
    let (title, _) = lock_copy(&st, Some(&s), Some(&t), now).unwrap();
    assert_eq!(title, "Getting Project time ready");

    // The window is not in the device's schedule at all (the hour fell out of
    // both windows, or the schedule clause never arrived).
    let other: GrantSchedule = serde_json::from_str(
        r#"{"v":1,"tz":"Etc/UTC","issuedAt":3,"weekly":{"mon":[
            {"start":"16:00","end":"20:00"}
        ]}}"#,
    )
    .unwrap();
    let multi = enforcer(&other, None, now);
    let st = state(&multi, &other, Some(&t), now);
    assert_eq!(st.remaining.reason, Some(LockReason::Schedule));
    assert_eq!(
        lock_copy(&st, Some(&other), Some(&t), now).unwrap().0,
        "Locked until Free time at 16:00"
    );
}

/// Run by hand on a real Mint session (it only reads `/proc`):
/// `cargo test -p charterd --features real --test times_sweep_and_lock_states \
///   -- --ignored --nocapture what_only_these_would_sweep_here`
/// Lists what an only-these time allowing just `/usr/bin/gimp` would SIGTERM
/// among the calling user's own processes.
#[cfg(feature = "real")]
#[test]
#[ignore = "reads this machine's inventory and /proc; prints, asserts nothing"]
fn what_only_these_would_sweep_here() {
    use std::os::unix::fs::MetadataExt as _;
    let home = std::env::var("HOME").unwrap_or_default();
    let inv = charterd::app_inventory::scan(&[home]);
    let pkgs: Vec<String> = inv.iter().map(|a| a.pkg.clone()).collect();
    println!("inventory: {} identities", pkgs.len());
    let t = GrantTimes::parse_or_absent(
        r#"{"v":1,"issuedAt":1,"times":[{"id":"learning","label":"Project time",
            "posture":"onlyThese","apps":["/usr/bin/gimp"]}]}"#,
    )
    .unwrap();
    let blocked = blocked_pkgs_for_time(None, &t.times[0], &pkgs, MON);
    let me = std::fs::metadata("/proc/self").unwrap().uid();
    for e in std::fs::read_dir("/proc").unwrap().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        if std::fs::metadata(e.path()).map(|m| m.uid()).ok() != Some(me) {
            continue;
        }
        let Some(id) = charterd::ancestry::read_proc(pid) else {
            continue;
        };
        let joined = id.cmdline.join(" ");
        let hit = blocked.iter().find(|pkg| {
            pkg_matches_process(
                pkg,
                id.exe.as_deref(),
                id.argv0(),
                &id.cmdline,
                &joined,
                id.cgroup.as_deref(),
            ) || matches_with_ancestors(pid, pkg, charterd::ancestry::read_proc)
        });
        if let Some(pkg) = hit {
            let line: String = joined.chars().take(90).collect();
            println!("SIGTERM {pid:>7} via {pkg}: {line}");
        }
    }
}
