//! `charter-console` — Kintrinsic's single on-device control panel.
//!
//! One native window (a `wry` webview rendering the bundled Kintrinsic UI) that
//! replaces the five separate zenity dialogs (setup / screen-time / device-code
//! / pair / recovery) with one professional app. It performs NO privileged work
//! itself: it reads world-readable status straight from disk, and every change
//! is delegated to the existing, hardware-proven privileged helpers via
//! `pkexec` (which prompts the parent for their admin password through the
//! desktop's polkit agent). The managed child is blocked from pkexec by
//! `49-charter.rules`, so this GUI is safe for the child to open too — it just
//! can't authorise anything.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;

use serde_json::{json, Value};
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

const HTML: &str = include_str!("../ui/app.html");

/// The live scan-to-pair payload, minted by `charter-pair-invite` when the
/// parent opens Connect. Held in memory only: the token itself stays root-only
/// on disk, and this is just the string the QR draws.
static INVITE: std::sync::Mutex<Option<(String, std::time::Instant)>> = std::sync::Mutex::new(None);

/// Must match `charterd::pair_token::TOKEN_TTL_SECS` — the daemon refuses an
/// offer past it, so a QR shown any longer is a lie.
const TOKEN_TTL_SECS: u64 = 600;

const LIMITS_DIR: &str = "/etc/charter/limits.d";
const DEVICE_PUB: &str = "/var/lib/charter/device.pub";
const PAIRING: &str = "/var/lib/charter/pairing.json";
const SBIN: &str = "/usr/sbin";
/// Where `charterd` publishes each managed child's live state. Read-only, and
/// readable by every local account on purpose: the ward is entitled to see
/// what is being enforced against them, and a guardian needs their CHILD's
/// numbers, which the per-caller D-Bus `TimeLeft` cannot give them.
const STATE_DIR: &str = "/run/charter/state";

/// Events pumped into the tao loop from the ipc handler and worker threads.
enum UserEvent {
    /// Raw JSON command string from the webview (`window.ipc.postMessage`).
    Cmd(String),
    /// JavaScript to run in the webview (status push, toast, busy toggle).
    Eval(String),
    /// Close the window (Escape, in popup mode).
    Close,
}

fn main() -> wry::Result<()> {
    // WebKitGTK's default DMABUF renderer paints a blank white page on a wide
    // range of Linux GPU/driver combinations (Nvidia, virtual GPUs, some Intel).
    // Disabling it is the standard, well-worn fix and costs nothing for a simple
    // settings UI. Must be set before the webview initialises.
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }

    // `--popup`: the tray's click target. Same binary, same UI, same reader —
    // a tall narrow window opening straight onto the ward's own day. A second
    // app would have meant two copies of the state reader and two chances for
    // the tray and the console to disagree about the same numbers.
    let popup = std::env::args().any(|a| a == "--popup");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();

    let mut builder_win = WindowBuilder::new().with_title("Kintrinsic");
    builder_win = if popup {
        builder_win
            // Tall and narrow: the content is a stack of bars, and there is a
            // lot of vertical room beside a panel to use.
            .with_inner_size(tao::dpi::LogicalSize::new(430.0, 860.0))
            .with_min_inner_size(tao::dpi::LogicalSize::new(360.0, 480.0))
            // A glance, not a workspace — it should sit above what the ward
            // was doing and be dismissed without hunting for it.
            .with_always_on_top(true)
            .with_resizable(true)
    } else {
        builder_win
            .with_inner_size(tao::dpi::LogicalSize::new(1000.0, 700.0))
            .with_min_inner_size(tao::dpi::LogicalSize::new(720.0, 560.0))
    };
    let window = builder_win
        .build(&event_loop)
        .expect("charter-console: failed to create the window");

    let ipc_proxy = proxy.clone();
    let builder = WebViewBuilder::new()
        .with_html(HTML)
        // The page reads this to open in popup mode. Set before any script
        // runs, so the first paint is already the right shape — flipping the
        // layout after the fact would show the wide app for a frame.
        .with_initialization_script(if popup {
            "window.__charterPopup = true;"
        } else {
            "window.__charterPopup = false;"
        })
        .with_ipc_handler(move |req| {
            let body = req.into_body();
            if std::env::var_os("CHARTER_CONSOLE_DEBUG").is_some() {
                eprintln!("[ipc] {body}");
            }
            let _ = ipc_proxy.send_event(UserEvent::Cmd(body));
        });
    // Attach via GTK, never the raw X11 handle: `build(&window)` on Linux
    // takes wry's foreign-window embedding path, which "works" (JS runs, IPC
    // round-trips, DOM lays out) while painting NOTHING on many stacks — the
    // console shipped blank because of it, and the 0.1.2 smoke test's JS/DOM
    // probes couldn't catch a pixels-only failure (2026-07-23, found on-metal
    // by decented). build_gtk into tao's vbox is the canonical wry+tao pairing.
    #[cfg(target_os = "linux")]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .expect("tao gtk window has a default vbox");
        builder.build_gtk(vbox)?
    };
    #[cfg(not(target_os = "linux"))]
    let webview = builder.build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::UserEvent(UserEvent::Cmd(body)) => {
                let proxy = proxy.clone();
                // Run every command off the UI thread: pkexec puts up a
                // password dialog and blocks until the parent answers.
                std::thread::spawn(move || handle_command(&body, &proxy));
            }
            Event::UserEvent(UserEvent::Eval(js)) => {
                let _ = webview.evaluate_script(&js);
            }
            Event::UserEvent(UserEvent::Close) => *control_flow = ControlFlow::Exit,
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => *control_flow = ControlFlow::Exit,
            _ => {}
        }
    })
}

/// Run one webview command to completion, then push fresh status (and any
/// toast) back into the page.
fn handle_command(body: &str, proxy: &tao::event_loop::EventLoopProxy<UserEvent>) {
    let msg: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let cmd = msg.get("cmd").and_then(|v| v.as_str()).unwrap_or("");

    if cmd == "close" {
        let _ = proxy.send_event(UserEvent::Close);
        return;
    }

    // Give / take back is the one command whose SUCCESS message is written by
    // the helper (it knows what it actually did), so it is handled apart from
    // the fixed-message commands below.
    if cmd == "adjust" {
        let user = msg.get("user").and_then(|v| v.as_str()).unwrap_or("");
        let minutes = msg.get("minutes").and_then(|v| v.as_i64()).unwrap_or(0);
        let (text, kind) = match run_adjust(user, minutes) {
            Ok(m) => (m, "ok"),
            Err(e) => (e, "err"),
        };
        let _ = proxy.send_event(UserEvent::Eval(format!(
            "window.__toast({}, {});",
            json!(text),
            json!(kind)
        )));
        let status = collect_status();
        let _ = proxy.send_event(UserEvent::Eval(format!("window.__charter({status});")));
        return;
    }

    // The ward's own verb. Runs as the ward — NOT through pkexec — because
    // asking is not a privileged act: `charter ask-for-more` publishes a
    // request to the guardian and enacts nothing. Routing it through polkit
    // would be the same category error as the buttons that used to fail: it
    // would demand a password the person asking is not supposed to have.
    if cmd == "ask" {
        let minutes = msg.get("minutes").and_then(|v| v.as_i64()).unwrap_or(0);
        let (text, kind) = match run_ask(minutes) {
            Ok(m) => (m, "ok"),
            Err(e) => (e, "err"),
        };
        let _ = proxy.send_event(UserEvent::Eval(format!(
            "window.__toast({}, {});",
            json!(text),
            json!(kind)
        )));
        let status = collect_status();
        let _ = proxy.send_event(UserEvent::Eval(format!("window.__charter({status});")));
        return;
    }

    // "Ask for more Play time" — a spent named allowance's own ask. Same
    // unprivileged verb as "ask" above, just routed to the group's own pool.
    if cmd == "askBucket" {
        let bucket_id = msg.get("bucketId").and_then(|v| v.as_str()).unwrap_or("");
        let label = msg.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let minutes = msg.get("minutes").and_then(|v| v.as_i64()).unwrap_or(0);
        let (text, kind) = match run_ask_bucket(bucket_id, label, minutes) {
            Ok(m) => (m, "ok"),
            Err(e) => (e, "err"),
        };
        let _ = proxy.send_event(UserEvent::Eval(format!(
            "window.__toast({}, {});",
            json!(text),
            json!(kind)
        )));
        let status = collect_status();
        let _ = proxy.send_event(UserEvent::Eval(format!("window.__charter({status});")));
        return;
    }

    // "Ask to open Minecraft" — one `askFirst` app still gated right now.
    if cmd == "askOpen" {
        let pkg = msg.get("pkg").and_then(|v| v.as_str()).unwrap_or("");
        let label = msg.get("label").and_then(|v| v.as_str()).unwrap_or("");
        let (text, kind) = match run_ask_open(pkg, label) {
            Ok(m) => (m, "ok"),
            Err(e) => (e, "err"),
        };
        let _ = proxy.send_event(UserEvent::Eval(format!(
            "window.__toast({}, {});",
            json!(text),
            json!(kind)
        )));
        let status = collect_status();
        let _ = proxy.send_event(UserEvent::Eval(format!("window.__charter({status});")));
        return;
    }

    let outcome: Option<Result<&str, String>> = match cmd {
        "refresh" => None,
        "setup" => {
            let user = msg.get("user").and_then(|v| v.as_str()).unwrap_or("");
            Some(run_setup(user).map(|_| "Kintrinsic is set up."))
        }
        "limits" => {
            let user = msg.get("user").and_then(|v| v.as_str()).unwrap_or("");
            let wake = msg.get("wake").and_then(|v| v.as_str()).unwrap_or("");
            let bedtime = msg.get("bedtime").and_then(|v| v.as_str()).unwrap_or("");
            let daily = msg.get("daily").and_then(|v| v.as_u64()).unwrap_or(0);
            Some(run_limits(user, wake, bedtime, daily).map(|_| "Limits saved."))
        }
        "pair" => {
            let link = msg.get("link").and_then(|v| v.as_str()).unwrap_or("");
            let subject = msg.get("subject").and_then(|v| v.as_str()).unwrap_or("");
            Some(run_pair(link, subject).map(|_| "Phone connected."))
        }
        // Mint a fresh scan-to-pair token and show its QR. Asks for the admin
        // password, which is the point: only the parent may invite a guardian.
        "invite" => Some(run_invite().map(|_| "Scan this with your phone.")),
        "learning" => {
            let user = msg.get("user").and_then(|v| v.as_str()).unwrap_or("");
            let khan = msg.get("khan").and_then(|v| v.as_bool()).unwrap_or(false);
            Some(run_learning(user, khan).map(|_| {
                if khan {
                    "Khan Academy is now time-free."
                } else {
                    "Learning time turned off."
                }
            }))
        }
        "recovery" => {
            // Fire-and-forget: recovery drives its own dialogs.
            open_recovery();
            None
        }
        _ => None,
    };

    // Toast on an explicit action's result (skip pure refreshes).
    if let Some(res) = outcome {
        let (msg, kind) = match res {
            Ok(m) => (m.to_string(), "ok"),
            Err(e) => (e, "err"),
        };
        let _ = proxy.send_event(UserEvent::Eval(format!(
            "window.__toast({}, {});",
            json!(msg),
            json!(kind)
        )));
    }

    // Always push fresh status (clears the busy spinner).
    let status = collect_status();
    let _ = proxy.send_event(UserEvent::Eval(format!("window.__charter({});", status)));
}

// ---------------------------------------------------------------------------
// Privileged actions — delegated to the proven helpers via pkexec.
// ---------------------------------------------------------------------------

/// What pkexec's "126" actually means for whoever is sitting here.
///
/// pkexec returns 126 both when the password dialog is DISMISSED and when the
/// caller is NOT AUTHORIZED to run the helper at all. Those are the same exit
/// code and completely different situations, and the managed child hits the
/// second one on every single button. Reporting it as "cancelled", or as the
/// old catch-all "That didn't complete. Please try again.", told a ward to
/// retry something that can never work no matter how many times they press it.
///
/// The console knows which case it is without guessing, because it knows who
/// is running it: a managed account is barred by `49-charter.rules`, so for
/// them 126 is always "not allowed", and for anyone else it is always the
/// dialog. Say the true thing, and for the ward say what they CAN do.
fn denied_message() -> String {
    if current_username().as_deref().is_some_and(is_managed_user) {
        "Only an admin can change this. You can see everything here, but changes \
         need the grown-up who set this computer up."
            .into()
    } else {
        "Cancelled — no admin password given.".into()
    }
}

/// pkexec the helper, forwarding the parent's X session so its dialogs (and the
/// polkit password prompt) can draw. Returns a friendly error on failure.
fn pkexec(bin: &str, args: &[&str]) -> Result<(), String> {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let xauth = std::env::var("XAUTHORITY").unwrap_or_default();
    let mut cmd = Command::new("pkexec");
    cmd.arg("env");
    if !display.is_empty() {
        cmd.arg(format!("DISPLAY={display}"));
    }
    if !xauth.is_empty() {
        cmd.arg(format!("XAUTHORITY={xauth}"));
    }
    cmd.arg(format!("{SBIN}/{bin}")).args(args);

    match cmd.status() {
        Ok(s) if s.success() => Ok(()),
        // 126 = dismissed OR not authorized; only the caller's identity tells
        // the two apart (see `denied_message`).
        Ok(s) if s.code() == Some(126) => Err(denied_message()),
        Ok(_) => Err("That didn't complete. Please try again.".into()),
        Err(_) => Err("Couldn't start the helper. Is Kintrinsic installed correctly?".into()),
    }
}

/// pkexec a helper and capture its stdout (the pairing invite payload).
fn pkexec_capture(bin: &str, args: &[&str]) -> Result<String, String> {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let xauth = std::env::var("XAUTHORITY").unwrap_or_default();
    let mut cmd = Command::new("pkexec");
    cmd.arg("env");
    if !display.is_empty() {
        cmd.arg(format!("DISPLAY={display}"));
    }
    if !xauth.is_empty() {
        cmd.arg(format!("XAUTHORITY={xauth}"));
    }
    cmd.arg(format!("{SBIN}/{bin}")).args(args);
    match cmd.output() {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Ok(o) if o.status.code() == Some(126) => Err(denied_message()),
        Ok(_) => Err("That didn't complete. Please try again.".into()),
        Err(_) => Err("Couldn't start the helper. Is Kintrinsic installed correctly?".into()),
    }
}

/// Mint a fresh pairing invite and hold its payload for the QR.
fn run_invite() -> Result<(), String> {
    let payload = pkexec_capture("charter-pair-invite", &[])?;
    if !payload.starts_with("charter://pair?") {
        return Err("Couldn't prepare a pairing code. Please try again.".into());
    }
    *INVITE.lock().expect("invite lock") = Some((payload, std::time::Instant::now()));
    Ok(())
}

/// The Khan Academy catalogue entry, verbatim from Kintrinsic's
/// `apps/charter-app/src/data/learning_catalogue.ts` — the closure was
/// MEASURED there (2026-07-17); keep the two in sync when it changes.
const KHAN_LEARNING: &str = r#"{
  "v": 1,
  "issuedAt": 0,
  "apps": [{
    "id": "khan-academy",
    "label": "Khan Academy",
    "kind": "site",
    "url": "https://www.khanacademy.org/",
    "domains": ["khanacademy.org", "kastatic.org", "kasandbox.org",
                 "youtube-nocookie.com", "ytimg.com", "googlevideo.com"]
  }]
}"#;

/// pkexec a helper with `stdin_body` piped to it (the learning body).
fn pkexec_stdin(bin: &str, args: &[&str], stdin_body: &str) -> Result<(), String> {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let xauth = std::env::var("XAUTHORITY").unwrap_or_default();
    let mut cmd = Command::new("pkexec");
    cmd.arg("env");
    if !display.is_empty() {
        cmd.arg(format!("DISPLAY={display}"));
    }
    if !xauth.is_empty() {
        cmd.arg(format!("XAUTHORITY={xauth}"));
    }
    cmd.arg(format!("{SBIN}/{bin}")).args(args);
    cmd.stdin(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|_| "Couldn't start the helper. Is Kintrinsic installed correctly?".to_string())?;
    {
        use std::io::Write as _;
        let Some(sin) = child.stdin.as_mut() else {
            return Err("Couldn't reach the helper.".into());
        };
        let _ = sin.write_all(stdin_body.as_bytes());
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(()),
        Ok(s) if s.code() == Some(126) => Err(denied_message()),
        Ok(_) => Err("That didn't complete. Please try again.".into()),
        Err(_) => Err("Couldn't finish the helper. Please try again.".into()),
    }
}

/// Save (or clear) the child's device-only learning apps. v1 offers the
/// verified Khan Academy entry; richer editing (caps, native apps, own
/// projects) lives in Kintrinsic on the phone.
fn run_learning(user: &str, khan: bool) -> Result<(), String> {
    if user.is_empty() {
        return Err("Choose an account first.".into());
    }
    if !khan {
        return pkexec("charter-settings", &["--clear-learning", user]);
    }
    // Stamp issuedAt now so a later phone clause (fresher issuedAt) supersedes.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let body = KHAN_LEARNING.replace("\"issuedAt\": 0", &format!("\"issuedAt\": {now}"));
    pkexec_stdin("charter-settings", &["--set-learning", user], &body)
}

/// Give (`minutes > 0`) or take back (`minutes < 0`) a ward's time at this
/// computer, via the root helper. Costs an admin password, like every other
/// change here.
///
/// The helper's own stderr is surfaced verbatim on refusal rather than being
/// flattened to a generic failure: it is the only thing that can explain WHY
/// (no limits set at all, unknown account), and a guardian told merely that
/// something "didn't complete" has no way to act on it.
fn run_adjust(user: &str, minutes: i64) -> Result<String, String> {
    if user.is_empty() {
        return Err("Choose an account first.".into());
    }
    if minutes == 0 {
        return Err("Choose an amount first.".into());
    }
    let minutes = minutes.to_string();
    let out = pkexec_output("charter-time", &["--user", user, "--minutes", &minutes])?;
    Ok(out)
}

/// The ward asking their guardian for more time, through the ordinary
/// unprivileged CLI. Exit code 3 is the daemon's "not paired" — on a
/// standalone computer there is no guardian on the other end of the relay to
/// receive it, and saying so plainly beats a request that vanishes.
fn run_ask(minutes: i64) -> Result<String, String> {
    if !(1..=240).contains(&minutes) {
        return Err("Choose how many minutes to ask for.".into());
    }
    let out = Command::new("charter")
        .args(["ask-for-more", &minutes.to_string()])
        .output()
        .map_err(|_| "Couldn't reach Kintrinsic on this computer.".to_string())?;
    match out.status.code() {
        Some(0) => Ok(format!("Asked for {minutes} more minutes.")),
        Some(3) => Err(
            "No phone is connected to this computer, so there's nobody to \
                        ask. Tell whoever set it up."
                .into(),
        ),
        Some(5) => Err("Kintrinsic isn't running on this computer right now.".into()),
        _ => {
            let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if text.is_empty() {
                "Couldn't send that ask. Try again in a moment.".into()
            } else {
                text
            })
        }
    }
}

/// "Ask for more Play time" — a spent named allowance's own ask, through the
/// same unprivileged `charter` CLI as [`run_ask`] (asking is never a
/// privileged act), routed to that group's own pool with `--bucket`.
fn run_ask_bucket(bucket_id: &str, label: &str, minutes: i64) -> Result<String, String> {
    if bucket_id.is_empty() {
        return Err("Couldn't tell which allowance to ask for.".into());
    }
    if !(1..=240).contains(&minutes) {
        return Err("Choose how many minutes to ask for.".into());
    }
    let out = Command::new("charter")
        .args(["ask-for-more", &minutes.to_string(), "--bucket", bucket_id])
        .output()
        .map_err(|_| "Couldn't reach Kintrinsic on this computer.".to_string())?;
    let what = if label.is_empty() { bucket_id } else { label };
    match out.status.code() {
        Some(0) => Ok(format!("Asked for more {what} time.")),
        Some(3) => Err(
            "No phone is connected to this computer, so there's nobody to \
                        ask. Tell whoever set it up."
                .into(),
        ),
        Some(5) => Err("Kintrinsic isn't running on this computer right now.".into()),
        _ => {
            let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if text.is_empty() {
                "Couldn't send that ask. Try again in a moment.".into()
            } else {
                text
            })
        }
    }
}

/// "Ask to open Minecraft" — one `askFirst` app still gated right now.
fn run_ask_open(pkg: &str, label: &str) -> Result<String, String> {
    if pkg.is_empty() {
        return Err("Couldn't tell which app to ask about.".into());
    }
    let mut args = vec!["ask-to-open".to_string(), pkg.to_string()];
    if !label.is_empty() {
        args.push(label.to_string());
    }
    let out = Command::new("charter")
        .args(&args)
        .output()
        .map_err(|_| "Couldn't reach Kintrinsic on this computer.".to_string())?;
    let what = if label.is_empty() { pkg } else { label };
    match out.status.code() {
        Some(0) => Ok(format!("Asked to open {what}.")),
        Some(3) => Err(
            "No phone is connected to this computer, so there's nobody to \
                        ask. Tell whoever set it up."
                .into(),
        ),
        Some(5) => Err("Kintrinsic isn't running on this computer right now.".into()),
        _ => {
            let text = String::from_utf8_lossy(&out.stderr).trim().to_string();
            Err(if text.is_empty() {
                "Couldn't send that ask. Try again in a moment.".into()
            } else {
                text
            })
        }
    }
}

/// pkexec a helper, returning its stdout on success and its stderr on a
/// non-zero exit that isn't the polkit refusal.
fn pkexec_output(bin: &str, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("pkexec");
    cmd.arg("env");
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let xauth = std::env::var("XAUTHORITY").unwrap_or_default();
    if !display.is_empty() {
        cmd.arg(format!("DISPLAY={display}"));
    }
    if !xauth.is_empty() {
        cmd.arg(format!("XAUTHORITY={xauth}"));
    }
    cmd.arg(format!("{SBIN}/{bin}")).args(args);
    match cmd.output() {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout).trim().to_string();
            Ok(if text.is_empty() {
                "Done.".to_string()
            } else {
                text
            })
        }
        Ok(o) if o.status.code() == Some(126) => Err(denied_message()),
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stderr);
            // The helper prefixes its own name; strip it so the toast reads as
            // a sentence rather than a log line.
            let text = text
                .trim()
                .trim_start_matches(&format!("{bin}: "))
                .replace('\n', " ");
            Err(if text.is_empty() {
                "That didn't complete. Please try again.".into()
            } else {
                text
            })
        }
        Err(_) => Err("Couldn't start the helper. Is Kintrinsic installed correctly?".into()),
    }
}

fn run_setup(user: &str) -> Result<(), String> {
    if user.is_empty() {
        return Err("Choose an account first.".into());
    }
    pkexec("charter-setup", &[user])
}

fn run_limits(user: &str, wake: &str, bedtime: &str, daily: u64) -> Result<(), String> {
    if user.is_empty() || wake.is_empty() || bedtime.is_empty() || daily == 0 {
        return Err("Some values are missing.".into());
    }
    let daily = daily.to_string();
    pkexec(
        "charter-settings",
        &[
            "--set",
            user,
            "--wake",
            wake,
            "--bedtime",
            bedtime,
            "--daily",
            &daily,
        ],
    )
}

fn run_pair(link: &str, subject: &str) -> Result<(), String> {
    if !link.starts_with("bunker://") {
        return Err("That link should start with bunker://".into());
    }
    if subject.len() != 64 || !subject.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("The child's ID should be 64 letters and numbers.".into());
    }
    pkexec("charter-pair", &["--link", link, "--subject", subject])
}

fn open_recovery() {
    let display = std::env::var("DISPLAY").unwrap_or_default();
    let xauth = std::env::var("XAUTHORITY").unwrap_or_default();
    let child = Command::new("pkexec")
        .arg("env")
        .arg(format!("DISPLAY={display}"))
        .arg(format!("XAUTHORITY={xauth}"))
        .arg(format!("{SBIN}/charter-recovery"))
        .spawn();
    // Reap the child so it doesn't linger as a zombie for the life of the
    // console's (indefinite) event loop — same detached-wait pattern the
    // other fire-and-forget spawns in this codebase use.
    if let Ok(mut child) = child {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

// ---------------------------------------------------------------------------
// Status — read straight from world-readable state on disk.
// ---------------------------------------------------------------------------

fn collect_status() -> String {
    let children = read_children();
    if std::env::var_os("CHARTER_CONSOLE_DEBUG").is_some() {
        eprintln!(
            "[status] limits_dir readable: {:?}; children found: {}",
            std::fs::read_dir(LIMITS_DIR).map(|d| d.count()),
            children.len()
        );
    }
    let configured = !children.is_empty();
    let managed: Vec<String> = children
        .iter()
        .map(|c| c["user"].as_str().unwrap_or("").to_string())
        .collect();
    let paired = children.iter().any(|c| c["paired"].as_bool() == Some(true)) || pairing_exists();
    let device_code = read_device_code();
    // ONLY the minted invite is ever drawn as a QR. Falling back to the bare
    // device code here shipped a decoy: a scannable-looking code with no token
    // in it, sitting on screen before the parent had pressed anything. Scanning
    // it appears to work — the phone adds the computer and waits forever —
    // while the laptop never learns the guardian's key at all. The bare code
    // stays available as the typed 8-group chip for the offline path.
    // An EXPIRED invite is dropped outright rather than shown greyed out: the
    // daemon will refuse it, so leaving it on screen invites a scan that
    // silently cannot work — the same trap as the token-less decoy.
    let mut guard = INVITE.lock().expect("invite lock");
    if let Some((_, minted)) = guard.as_ref() {
        if minted.elapsed().as_secs() >= TOKEN_TTL_SECS {
            *guard = None;
        }
    }
    let invite = guard.clone();
    drop(guard);
    let secs_left = invite
        .as_ref()
        .map(|(_, m)| TOKEN_TTL_SECS.saturating_sub(m.elapsed().as_secs()));
    let qr = invite.as_ref().and_then(|(p, _)| qr_svg(p));

    let me = current_username();
    let viewer_is_ward = me.as_deref().is_some_and(is_managed_user);

    json!({
        "configured": configured,
        "paired": paired,
        "children": children,
        "candidates": candidate_accounts(&managed),
        "deviceCode": device_code.as_ref().map(|c| grouped(c)),
        "qr": qr,
        "invited": invite.is_some(),
        "secsLeft": secs_left,
        // Who is sitting here. The ward gets the SAME numbers as the guardian
        // — Kintrinsic's standing rule is that a ward can always see what is
        // being enforced — and a different set of verbs: they can ask, not
        // grant. Every write still goes through polkit regardless of what this
        // says, so a tampered page buys nothing.
        "viewer": {
            "user": me,
            "isWard": viewer_is_ward,
        },
    })
    .to_string()
}

/// Is this local account one Kintrinsic manages?
fn is_managed_user(user: &str) -> bool {
    !user.is_empty() && std::fs::metadata(format!("{LIMITS_DIR}/{user}.json")).is_ok()
}

/// One child's live state, as published by the daemon each tick. `None` when
/// charterd isn't running or hasn't ticked yet — the page says so rather than
/// drawing a confident zero, which would read as "no time left".
///
/// Flattened to one camelCase object here rather than in the page. The wire
/// shape is genuinely mixed — `TimeLeftView` is snake_case and its frozen
/// field names are asserted by the CLI's own tests, while `BucketView` is
/// camelCase — and teaching the UI both spellings is how a renamed field
/// becomes a silently blank panel later.
fn read_live(user: &str) -> Option<Value> {
    let text = std::fs::read_to_string(format!("{STATE_DIR}/{user}.json")).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let t = v.get("timeLeft")?;
    let num = |k: &str| t.get(k).and_then(|x| x.as_i64());

    // How old the snapshot is. The daemon rewrites it every tick, so anything
    // beyond a couple of minutes means charterd has stopped — and stale
    // numbers presented as live are worse than no numbers at all.
    let age = unix_now().saturating_sub(v.get("at").and_then(|x| x.as_i64()).unwrap_or(0));
    let stale = (age > 120).then(|| match age {
        a if a < 3600 => format!("{} minutes", a / 60),
        a if a < 86400 => format!("{} hours", a / 3600),
        a => format!("{} days", a / 86400),
    });

    Some(json!({
        "effectiveSeconds": num("effective_seconds").unwrap_or(0),
        "scheduleSeconds": num("schedule_seconds").unwrap_or(-1),
        "budgetSeconds": num("budget_seconds").unwrap_or(-1),
        // The two caps separately, so the popup can say WHICH one is running
        // out. Absent on a pre-0.6 daemon's file: -1 = "that cap isn't set",
        // never 0, which would read as "your week is gone".
        "budgetDaySeconds": num("budget_day_seconds").unwrap_or(-1),
        "budgetWeekSeconds": num("budget_week_seconds").unwrap_or(-1),
        "extensionSeconds": num("extension_seconds").unwrap_or(0),
        "locked": t.get("locked").and_then(|x| x.as_bool()).unwrap_or(false),
        "reason": t.get("reason").and_then(|x| x.as_str()),
        "nextOpen": num("next_open"),
        "usedTodaySeconds": num("used_today_seconds"),
        "buckets": t.get("buckets").cloned().unwrap_or(Value::Array(vec![])),
        // Apps still gated by `blocked`/`askFirst` — labelled by the daemon
        // from its own inventory, so the popup can offer "Ask to open" with
        // no policy read of its own. Absent on a pre-askFirst daemon's file.
        "askFirst": t.get("ask_first").cloned().unwrap_or(Value::Array(vec![])),
        "__adjust": v.get("localAdjustMinutes").and_then(|x| x.as_i64()).unwrap_or(0),
        "__stale": stale,
    }))
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Each `/etc/charter/limits.d/<user>.json` → a child card. Parses the current
/// `{ "limits": { tz, wake, bedtime, dailyMinutes }, "subject"? }` shape and
/// tolerates a bare `{ tz, wake, bedtime, dailyMinutes }` (older writes).
fn read_children() -> Vec<Value> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(LIMITS_DIR) else {
        return out;
    };
    let mut files: Vec<_> = entries.flatten().collect();
    files.sort_by_key(|e| e.file_name());
    for entry in files {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(user) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        let limits = v.get("limits").unwrap_or(&v);
        let wake = limits
            .get("wake")
            .and_then(|x| x.as_str())
            .unwrap_or("07:00");
        let bedtime = limits
            .get("bedtime")
            .and_then(|x| x.as_str())
            .unwrap_or("20:00");
        let daily = limits
            .get("dailyMinutes")
            .and_then(|x| x.as_u64())
            .unwrap_or(120);
        let subject = v.get("subject").and_then(|x| x.as_str());
        // Device-only learning: is the Khan entry currently in force?
        let learning_khan = v
            .get("learning")
            .and_then(|l| l.get("apps"))
            .and_then(|a| a.as_array())
            .is_some_and(|apps| {
                apps.iter()
                    .any(|a| a.get("id").and_then(|i| i.as_str()) == Some("khan-academy"))
            });
        out.push(json!({
            "user": user,
            "wake": wake,
            "bedtime": bedtime,
            "daily": daily,
            "paired": subject.is_some(),
            "learningKhan": learning_khan,
            // The rule above is what was AGREED; this is what is actually
            // happening right now. Showing only the first is how the console
            // came to display settings a ward could not act on.
            "live": read_live(user),
        }));
    }
    out
}

/// Ordinary local accounts (uid 1000..65000, a real login shell) that are NOT
/// the current admin and NOT already managed — the setup dropdown.
fn candidate_accounts(managed: &[String]) -> Vec<String> {
    let me = current_username();
    let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let mut out = Vec::new();
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 7 {
            continue;
        }
        let (name, uid, shell) = (f[0], f[2].parse::<u32>().unwrap_or(0), f[6]);
        if !(1000..65000).contains(&uid) {
            continue;
        }
        if shell.ends_with("nologin") || shell.ends_with("false") || shell.ends_with("sync") {
            continue;
        }
        if Some(name) == me.as_deref() || managed.iter().any(|m| m == name) {
            continue;
        }
        out.push(name.to_string());
    }
    out
}

fn current_username() -> Option<String> {
    std::env::var("PKEXEC_UID")
        .ok()
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("LOGNAME").ok())
        // USER/LOGNAME are the human name; PKEXEC_UID won't apply here (we run as
        // the user), so USER is the real answer.
        .and_then(|v| {
            if v.chars().all(|c| c.is_ascii_digit()) {
                None
            } else {
                Some(v)
            }
        })
}

fn read_device_code() -> Option<String> {
    let text = std::fs::read_to_string(DEVICE_PUB).ok()?;
    let token: String = text
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(64)
        .collect();
    (token.len() == 64).then_some(token)
}

fn pairing_exists() -> bool {
    std::fs::metadata(PAIRING).is_ok()
}

/// Group a 64-hex code into eight space-separated blocks of 8 for display.
fn grouped(code: &str) -> String {
    code.as_bytes()
        .chunks(8)
        .map(|c| String::from_utf8_lossy(c).to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A crisp SVG QR of the raw device code, via the `qrencode` dependency. The
/// phone's camera decodes the raw 64-hex, exactly what Kintrinsic expects.
fn qr_svg(code: &str) -> Option<String> {
    let out = Command::new("qrencode")
        .args(["-t", "SVG", "-m", "0", "-o", "-", "--", code])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let svg = String::from_utf8_lossy(&out.stdout);
    // Strip the XML/doctype prologue so it drops straight into the page as an
    // inline <svg>…</svg>.
    let start = svg.find("<svg")?;
    Some(svg[start..].to_string())
}
