//! Pure command dispatch over the `charter-ipc` ports. Every effect goes through
//! `SubmitRequest` (publish-only) — there is no enact/approve path here. Exit
//! codes: 0 ok, 2 usage, 3 not-paired, 4 not-found, 5 daemon-unavailable, 6
//! denied, 7 offline.

use charter_ipc::dto::FREE_TIME_CHOICE;
use charter_ipc::{CharterdClient, ExecProbe, IpcError, Op, PairingSink};

use crate::render;

/// `charter pair` cannot actually pair a guardian — `RealPairingSink::pair`
/// is a permanent stub (`Err(NotPaired)`), and pairing now requires an admin
/// password the CLI has no way to collect. Point at the real path instead of
/// letting the command look like it might work. (04-G7)
const PAIR_REDIRECT: &str =
    "Pairing is done from the Kintrinsic app → Connect (it needs an admin password).";

pub const EXIT_OK: i32 = 0;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NOT_PAIRED: i32 = 3;
pub const EXIT_NOT_FOUND: i32 = 4;
pub const EXIT_DAEMON_UNAVAILABLE: i32 = 5;
pub const EXIT_DENIED: i32 = 6;
pub const EXIT_OFFLINE: i32 = 7;

/// Map an IPC error to the CLI exit code.
pub fn exit_code(e: &IpcError) -> i32 {
    match e {
        IpcError::NotPaired => EXIT_NOT_PAIRED,
        IpcError::NotFound => EXIT_NOT_FOUND,
        IpcError::DaemonUnavailable => EXIT_DAEMON_UNAVAILABLE,
        IpcError::Denied => EXIT_DENIED,
        IpcError::Offline => EXIT_OFFLINE,
        IpcError::Invalid(_) => EXIT_USAGE,
        IpcError::NotImplemented | IpcError::Io(_) => 1,
    }
}

/// The result of a CLI command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutcome {
    pub code: i32,
    pub rendered: String,
}

fn ok(s: String) -> CliOutcome {
    CliOutcome {
        code: EXIT_OK,
        rendered: s,
    }
}
fn usage(s: &str) -> CliOutcome {
    CliOutcome {
        code: EXIT_USAGE,
        rendered: s.to_string(),
    }
}
fn err_out(e: IpcError) -> CliOutcome {
    CliOutcome {
        code: exit_code(&e),
        rendered: format!("error: {e}"),
    }
}

/// Dispatch a parsed argv (without the program name).
///
/// `pairing` stays in the signature (callers — `main.rs`, the mock tests —
/// still build a `PairingSink`) even though `pair` no longer calls it; see
/// `PAIR_REDIRECT`.
pub async fn dispatch(
    client: &dyn CharterdClient,
    _pairing: &dyn PairingSink,
    probe: &dyn ExecProbe,
    args: &[String],
) -> CliOutcome {
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    let json = render::json_flag(args);
    let positional = |n: usize| args.iter().filter(|a| !a.starts_with("--")).nth(n).cloned();
    // The value right after a `--name value` flag, wherever it sits.
    let flag_value = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    match cmd {
        "install" => {
            let Some(reference) = positional(1) else {
                return usage("usage: charter install <ref>");
            };
            let params = serde_json::json!({"ref": reference, "remote": "flathub"}).to_string();
            match client.submit_request(Op::InstallFlatpak, params).await {
                Ok(id) => ok(format!("submitted {id}")),
                Err(e) => err_out(e),
            }
        }
        "run" => {
            let Some(path) = positional(1) else {
                return usage("usage: charter run <path>");
            };
            // Probe for display only; charterd hashes + builds the real request.
            match probe.probe(&path).await {
                Ok(_meta) => {
                    let params = serde_json::json!({"path": path}).to_string();
                    match client.submit_request(Op::ExecAllow, params).await {
                        Ok(id) => ok(format!("submitted {id}")),
                        Err(e) => err_out(e),
                    }
                }
                Err(e) => err_out(e),
            }
        }
        "status" => {
            let id = positional(1).unwrap_or_default();
            match client.query_status(&id).await {
                Ok(rows) => ok(render::render_status(&rows, json)),
                Err(e) => err_out(e),
            }
        }
        "time-left" => match client.time_left().await {
            Ok(t) => ok(render::render_time_left(&t, json)),
            Err(e) => err_out(e),
        },
        "ask-for-more" => {
            let Some(min_s) = positional(1) else {
                return usage(
                    "usage: charter ask-for-more <minutes> [--bucket <id> | --time <id|free>]",
                );
            };
            let Ok(minutes) = min_s.parse::<u16>() else {
                return usage("minutes must be a number");
            };
            // `--bucket <id>` asks for ONE named group's own top-up — the
            // group id is explicit, so unlike the whole-device ask below it
            // needs no `time_left` read to route it (M-named-times: an
            // extension routed to the wrong pool never lifts the group's own
            // wall, same discipline as M7).
            if let Some(bucket_id) = flag_value("--bucket") {
                let params = serde_json::json!({
                    "minutesRequested": minutes,
                    "limitHit": "bucket",
                    "bucketId": bucket_id,
                })
                .to_string();
                return match client.submit_request(Op::TimeExtend, params).await {
                    Ok(id) => ok(format!("requested {minutes}m for {bucket_id} ({id})")),
                    Err(e) => err_out(e),
                };
            }
            // M7 routing is shared (TimeLeftView::limit_hit) so the tray and
            // the CLI cannot fork the rule. A failed read defaults to
            // "budget" rather than aborting the request.
            let time_left = client.time_left().await;
            let limit_hit = time_left
                .as_ref()
                .map(|t| t.limit_hit())
                .unwrap_or("budget");
            // §Times: WHICH kind of time the ask is for. `--time <id>` names it
            // (the lock card's picker passes it, `free` included); with no flag
            // the daemon's own default for this ward right now is used, the
            // same rule the tray follows (`TimeLeftView::default_ask_time`).
            // Free time is the ABSENCE of the field, so an untyped ask — and
            // every ask on a pre-times daemon — is byte-identical to before.
            let time = match flag_value("--time") {
                Some(id) if id == FREE_TIME_CHOICE => None,
                // A named id must be one the daemon offers this ward right
                // now (`TimeLeftView::check_ask_time`, the same check
                // charterd makes before it publishes). Refused here, with the
                // choices, rather than sent to the guardian as an ask for a
                // time that does not exist. An unreadable snapshot cannot
                // vouch for any id, so a typed ask fails closed.
                Some(id) => match &time_left {
                    Ok(t) => match t.check_ask_time(&id) {
                        Ok(()) => Some(id),
                        Err(why) => return usage(&format!("error: {why}")),
                    },
                    Err(e) => {
                        return CliOutcome {
                            code: exit_code(e),
                            rendered: format!(
                                "error: couldn't check which times this computer offers ({e}) \
                                 — try again, or ask without --time"
                            ),
                        }
                    }
                },
                None => time_left
                    .as_ref()
                    .ok()
                    .and_then(|t| t.default_ask_time())
                    .map(str::to_string),
            };
            let mut params =
                serde_json::json!({"minutesRequested": minutes, "limitHit": limit_hit});
            if let Some(time) = &time {
                params["time"] = serde_json::Value::String(time.clone());
            }
            match client
                .submit_request(Op::TimeExtend, params.to_string())
                .await
            {
                Ok(id) => ok(match &time {
                    Some(t) => format!("requested {minutes}m of {t} ({id})"),
                    None => format!("requested {minutes}m ({id})"),
                }),
                Err(e) => err_out(e),
            }
        }
        "ask-to-open" => {
            let Some(pkg) = positional(1) else {
                return usage("usage: charter ask-to-open <pkg> [label]");
            };
            let label = positional(2);
            let mut params = serde_json::json!({"pkg": pkg});
            if let Some(label) = &label {
                params["label"] = serde_json::Value::String(label.clone());
            }
            match client.submit_request(Op::AppOpen, params.to_string()).await {
                Ok(id) => ok(format!("asked to open {pkg} ({id})")),
                Err(e) => err_out(e),
            }
        }
        // `charter pair` cannot pair — `RealPairingSink::pair` is a permanent
        // stub, and pairing now needs an admin password only the Kintrinsic
        // app can collect. Redirect rather than validate-then-fail (04-G7).
        "pair" => usage(PAIR_REDIRECT),
        "" => usage("usage: charter <install|run|status|time-left|ask-for-more|ask-to-open>"),
        other => usage(&format!("unknown command: {other}")),
    }
}
