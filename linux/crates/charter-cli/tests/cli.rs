//! CLI dispatch over the mock IPC client: happy paths + the named exit codes.

#![cfg(feature = "mock")]

use charter_cli::dispatch::{dispatch, exit_code};
use charter_cli::*;
use charter_ipc::mock::{MockCharterdClient, MockExecProbe, MockPairingSink, ScriptedDecision};
use charter_ipc::{ExecMeta, IpcError, TimeLeftView};

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

#[tokio::test]
async fn install_golden_submits() {
    let client = MockCharterdClient::new();
    let pairing = MockPairingSink::new(true);
    let probe = MockExecProbe::new();
    let out = dispatch(
        &client,
        &pairing,
        &probe,
        &argv(&["install", "org.videolan.VLC"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    assert!(out.rendered.starts_with("submitted "));
}

#[tokio::test]
async fn install_while_unpaired_exit3() {
    let client = MockCharterdClient::new().with_paired(false);
    let out = dispatch(
        &client,
        &MockPairingSink::new(false),
        &MockExecProbe::new(),
        &argv(&["install", "org.x.Y"]),
    )
    .await;
    assert_eq!(out.code, EXIT_NOT_PAIRED);
}

#[tokio::test]
async fn status_unknown_exit4() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["status", "deadbeef"]),
    )
    .await;
    assert_eq!(out.code, EXIT_NOT_FOUND);
}

#[tokio::test]
async fn time_left_renders_locked() {
    let client = MockCharterdClient::new().with_time_left(TimeLeftView::unknown());
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["time-left"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    assert!(out.rendered.contains("LOCKED"));
}

#[tokio::test]
async fn ask_for_more_submits() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "15"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    assert!(out.rendered.contains("15m"));
}

/// `--bucket <id>` routes the ask to that named group's own pool — explicit,
/// so it needs no `time_left` read to route it (unlike the whole-device ask).
#[tokio::test]
async fn ask_for_more_with_bucket_flag_routes_to_the_named_group() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "15", "--bucket", "play"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    assert!(out.rendered.contains("play"), "{}", out.rendered);
    let subs = client.submitted();
    assert_eq!(subs.len(), 1);
    assert!(
        subs[0].1.contains("\"limitHit\":\"bucket\""),
        "{}",
        subs[0].1
    );
    assert!(subs[0].1.contains("\"bucketId\":\"play\""), "{}", subs[0].1);
    assert!(
        subs[0].1.contains("\"minutesRequested\":15"),
        "{}",
        subs[0].1
    );
}

#[tokio::test]
async fn ask_to_open_submits_pkg_and_label() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-to-open", "com.mojang.minecraftpe", "Minecraft"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    let subs = client.submitted();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].0, charter_ipc::Op::AppOpen);
    assert!(
        subs[0].1.contains("\"pkg\":\"com.mojang.minecraftpe\""),
        "{}",
        subs[0].1
    );
    assert!(
        subs[0].1.contains("\"label\":\"Minecraft\""),
        "{}",
        subs[0].1
    );
}

#[tokio::test]
async fn ask_to_open_without_label_omits_it() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-to-open", "com.mojang.minecraftpe"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    let subs = client.submitted();
    assert!(!subs[0].1.contains("label"), "{}", subs[0].1);
}

#[tokio::test]
async fn ask_to_open_missing_pkg_is_usage() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-to-open"]),
    )
    .await;
    assert_eq!(out.code, EXIT_USAGE);
}

#[tokio::test]
async fn ask_for_more_routes_schedule_lock_to_schedule_pool() {
    // M7: a schedule (bedtime) lock must request the SCHEDULE dimension, else the
    // granted minutes land in the isolated budget pool and never lift the lock.
    let client = MockCharterdClient::new().with_time_left(TimeLeftView {
        effective_seconds: 0,
        schedule_seconds: 0, // window closed -> schedule-locked
        budget_seconds: -1,
        extension_seconds: 0,
        locked: true,
        reason: Some("schedule".into()),
        next_open: None,
        offline: false,
        learning_today_seconds: None,
        used_today_seconds: None,
        buckets: Vec::new(),
        budget_day_seconds: -1,
        budget_week_seconds: -1,
        ask_first: Vec::new(),
        time: None,
    });
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "30"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    let subs = client.submitted();
    assert_eq!(subs.len(), 1);
    assert!(
        subs[0].1.contains("\"limitHit\":\"schedule\""),
        "a schedule lock must request schedule, got {}",
        subs[0].1
    );
}

#[tokio::test]
async fn ask_for_more_degraded_snapshot_defaults_budget() {
    // A degraded both-zero snapshot (e.g. TimeLeftView::unknown(), no reason)
    // must NOT be mislabeled as a schedule lock — default to budget.
    let client = MockCharterdClient::new().with_time_left(TimeLeftView {
        effective_seconds: 0,
        schedule_seconds: 0,
        budget_seconds: 0,
        extension_seconds: 0,
        locked: true,
        reason: None,
        next_open: None,
        offline: true,
        learning_today_seconds: None,
        used_today_seconds: None,
        buckets: Vec::new(),
        budget_day_seconds: -1,
        budget_week_seconds: -1,
        ask_first: Vec::new(),
        time: None,
    });
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "10"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    let subs = client.submitted();
    assert!(
        subs[0].1.contains("\"limitHit\":\"budget\""),
        "degraded snapshot must default to budget, got {}",
        subs[0].1
    );
}

#[tokio::test]
async fn run_probes_then_submits() {
    let client = MockCharterdClient::new();
    let probe = MockExecProbe::new().with_path(
        "/home/managed/g.AppImage",
        ExecMeta {
            name: "Game".into(),
            size: 10,
            origin: None,
        },
    );
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &probe,
        &argv(&["run", "/home/managed/g.AppImage"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
}

#[tokio::test]
async fn run_missing_path_exit4() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["run", "/nope"]),
    )
    .await;
    assert_eq!(out.code, EXIT_NOT_FOUND);
}

// `charter pair` cannot pair — `RealPairingSink::pair` is a permanent stub
// (04-G7). Whatever argv follows `pair`, the CLI redirects to the app rather
// than validating a URI it can never act on.
#[tokio::test]
async fn pair_always_redirects_to_the_app() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(false),
        &MockExecProbe::new(),
        &argv(&[
            "pair",
            "bunker://aabbccddeeff00112233445566778899aabbccddeeff00112233445566778899?relay=wss://r&kind=charter",
        ]),
    )
    .await;
    assert_eq!(out.code, EXIT_USAGE);
    assert_eq!(
        out.rendered,
        "Pairing is done from the Kintrinsic app → Connect (it needs an admin password)."
    );
}

#[tokio::test]
async fn pair_with_no_args_still_redirects() {
    let client = MockCharterdClient::new();
    let out = dispatch(
        &client,
        &MockPairingSink::new(false),
        &MockExecProbe::new(),
        &argv(&["pair"]),
    )
    .await;
    assert_eq!(out.code, EXIT_USAGE);
    assert!(out.rendered.contains("Kintrinsic app"));
}

#[tokio::test]
async fn unknown_command_is_usage_no_effect() {
    let client = MockCharterdClient::new().with_decision(ScriptedDecision::Allow);
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["frobnicate"]),
    )
    .await;
    assert_eq!(out.code, EXIT_USAGE);
}

#[test]
fn exit_code_mapping_covers_all() {
    assert_eq!(exit_code(&IpcError::NotPaired), EXIT_NOT_PAIRED);
    assert_eq!(exit_code(&IpcError::NotFound), EXIT_NOT_FOUND);
    assert_eq!(
        exit_code(&IpcError::DaemonUnavailable),
        EXIT_DAEMON_UNAVAILABLE
    );
    assert_eq!(exit_code(&IpcError::Denied), EXIT_DENIED);
    assert_eq!(exit_code(&IpcError::Offline), EXIT_OFFLINE);
    assert_eq!(exit_code(&IpcError::Invalid("x".into())), EXIT_USAGE);
}

/// §Times: the lock card's picker names the kind of time with `--time`; Free
/// time is the ABSENCE of the field, so `--time free` asks exactly as before.
#[tokio::test]
async fn ask_for_more_with_a_time_flag_puts_the_time_on_the_request() {
    let client = MockCharterdClient::new().with_time_left(learning_offered());
    let out = dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "30", "--time", "learning"]),
    )
    .await;
    assert_eq!(out.code, EXIT_OK);
    let subs = client.submitted();
    assert!(subs[0].1.contains("\"time\":\"learning\""), "{}", subs[0].1);

    let client = MockCharterdClient::new();
    dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "30", "--time", "free"]),
    )
    .await;
    let subs = client.submitted();
    assert!(!subs[0].1.contains("\"time\""), "{}", subs[0].1);
}

/// A snapshot whose times view offers Free and Learning time, defaulting to
/// Learning.
fn learning_offered() -> TimeLeftView {
    let mut t = TimeLeftView::unknown();
    t.time = Some(Box::new(charter_ipc::dto::TimesView {
        ask: vec![
            charter_ipc::dto::TimeChoice {
                id: "free".into(),
                label: "Free time".into(),
            },
            charter_ipc::dto::TimeChoice {
                id: "learning".into(),
                label: "Learning time".into(),
            },
        ],
        ask_default: Some("learning".into()),
        ..Default::default()
    }));
    t
}

/// §Times: `--time` may only name a well-formed time the daemon offers this
/// ward; anything else is refused with the choices and nothing is submitted.
#[tokio::test]
async fn ask_for_more_refuses_a_time_the_daemon_does_not_offer() {
    for (view, id, needle) in [
        (learning_offered(), "chores", "free, learning"),
        (learning_offered(), "Learning Time", "not a time id"),
        (learning_offered(), &*"x".repeat(33), "not a time id"),
        // A pre-times daemon (no times view) offers nothing to name.
        (TimeLeftView::unknown(), "learning", "ask for Free time"),
    ] {
        let client = MockCharterdClient::new().with_time_left(view);
        let out = dispatch(
            &client,
            &MockPairingSink::new(true),
            &MockExecProbe::new(),
            &argv(&["ask-for-more", "30", "--time", id]),
        )
        .await;
        assert_eq!(out.code, EXIT_USAGE, "{id}: {}", out.rendered);
        assert!(out.rendered.contains(needle), "{id}: {}", out.rendered);
        assert!(client.submitted().is_empty(), "{id}: nothing is submitted");
    }
}

/// §Times: with no flag, the ask names the daemon's default for this ward.
#[tokio::test]
async fn ask_for_more_without_a_flag_uses_the_daemon_s_default_time() {
    let client = MockCharterdClient::new().with_time_left(learning_offered());
    dispatch(
        &client,
        &MockPairingSink::new(true),
        &MockExecProbe::new(),
        &argv(&["ask-for-more", "30"]),
    )
    .await;
    let subs = client.submitted();
    assert!(subs[0].1.contains("\"time\":\"learning\""), "{}", subs[0].1);
}
