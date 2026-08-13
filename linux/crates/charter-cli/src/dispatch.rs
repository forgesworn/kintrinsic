//! Pure command dispatch over the `charter-ipc` ports. Every effect goes through
//! `SubmitRequest` (publish-only) — there is no enact/approve path here. Exit
//! codes: 0 ok, 2 usage, 3 not-paired, 4 not-found, 5 daemon-unavailable, 6
//! denied, 7 offline.

use charter_ipc::{CharterdClient, ExecProbe, IpcError, Op, PairingSink};

use crate::pairing::validate_bunker_uri;
use crate::render;

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
pub async fn dispatch(
    client: &dyn CharterdClient,
    pairing: &dyn PairingSink,
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
                return usage("usage: charter ask-for-more <minutes> [--bucket <id>]");
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
            let limit_hit = match client.time_left().await {
                Ok(t) => t.limit_hit(),
                Err(_) => "budget",
            };
            let params =
                serde_json::json!({"minutesRequested": minutes, "limitHit": limit_hit}).to_string();
            match client.submit_request(Op::TimeExtend, params).await {
                Ok(id) => ok(format!("requested {minutes}m ({id})")),
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
        "pair" => {
            let Some(uri) = positional(1) else {
                return usage("usage: charter pair <bunker uri>");
            };
            if validate_bunker_uri(&uri).is_err() {
                return usage("invalid bunker:// uri (must be bunker:// with wss:// relays)");
            }
            match pairing.pair(&uri).await {
                Ok(_) => ok("paired".into()),
                Err(IpcError::Invalid(m)) => CliOutcome {
                    code: EXIT_USAGE,
                    rendered: m,
                },
                Err(e) => err_out(e),
            }
        }
        "" => usage("usage: charter <install|run|status|time-left|ask-for-more|ask-to-open|pair>"),
        other => usage(&format!("unknown command: {other}")),
    }
}
