//! `charter-settings` — the parent's screen-time settings dialog (multi-child).
//!
//! Lists the managed children (one per `/etc/charter/limits.d/<user>.json`), lets
//! the parent pick one, and sets that child's allowed-hours window + daily cap —
//! saved back to their per-child file, which `charterd` applies within a tick.
//! Launched from the menu via pkexec (runs as root to write the config). The
//! parse/save logic lives in `charterd::device_limits` and is unit-tested; this
//! binary is the dialog glue, run on a real desktop (VM).

use std::process::Command;

use charterd::device_limits::{
    detect_tz, load_child_limits_dir, parse_form_fields_over, set_child_limits, DeviceLimits,
};

const LIMITS_DIR: &str = "/etc/charter/limits.d";

fn zenity(args: &[String]) -> std::io::Result<std::process::Output> {
    Command::new("zenity").args(args).output()
}

fn notify(flag: &str, text: &str) {
    let _ = zenity(&[flag.into(), format!("--text={text}")]);
}

/// Ask which child to manage (auto-selects when there's only one).
fn pick_child(children: &[(String, DeviceLimits)]) -> Option<String> {
    if children.len() == 1 {
        return Some(children[0].0.clone());
    }
    let mut args = vec![
        "--list".to_string(),
        "--title=Kintrinsic — Screen Time".into(),
        "--text=Whose screen-time limits do you want to set?".into(),
        "--column=Child".into(),
    ];
    args.extend(children.iter().map(|(u, _)| u.clone()));
    let out = zenity(&args).ok()?;
    if !out.status.success() {
        return None; // cancelled
    }
    let pick = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!pick.is_empty()).then_some(pick)
}

/// Headless setter used by the unified `charter-console` app:
/// `charter-settings --set <user> --wake HH:MM --bedtime HH:MM --daily N`.
/// Preserves the child's timezone, weekend window, and guardian `subject`
/// binding (only the three fields given are changed). No dialogs.
fn headless_set(args: &[String]) -> ! {
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let (Some(user), Some(wake), Some(bedtime), Some(daily)) = (
        get("--set"),
        get("--wake"),
        get("--bedtime"),
        get("--daily"),
    ) else {
        eprintln!("usage: charter-settings --set <user> --wake HH:MM --bedtime HH:MM --daily N");
        std::process::exit(2);
    };
    let Ok(daily_minutes) = daily.parse::<u32>() else {
        eprintln!("charter-settings: --daily must be a number of minutes");
        std::process::exit(2);
    };
    // Preserve tz + weekend from the child's current file; set_child_limits
    // preserves the guardian `subject` binding.
    let cur = load_child_limits_dir(LIMITS_DIR)
        .into_iter()
        .find(|(u, _)| *u == user)
        .map(|(_, l)| l);
    let limits = DeviceLimits {
        tz: cur.as_ref().map(|c| c.tz.clone()).unwrap_or_else(detect_tz),
        wake,
        bedtime,
        daily_minutes,
        weekend: cur.and_then(|c| c.weekend),
    };
    match set_child_limits(LIMITS_DIR, &user, &limits) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("charter-settings: could not save: {e}");
            std::process::exit(1);
        }
    }
}

/// `charter-settings --set-learning <user>` — reads a `GrantLearning` JSON
/// body from STDIN (validated fail-closed), stores it as the child's
/// device-only learning apps. `--clear-learning <user>` removes it. Both
/// preserve the child's limits and guardian binding. No dialogs — the
/// console app (and scripts) drive this via pkexec.
fn headless_set_learning(args: &[String]) -> ! {
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if let Some(user) = get("--clear-learning") {
        match charterd::device_limits::set_child_learning(LIMITS_DIR, &user, None) {
            Ok(()) => std::process::exit(0),
            Err(e) => {
                eprintln!("charter-settings: could not save: {e}");
                std::process::exit(1);
            }
        }
    }
    let Some(user) = get("--set-learning") else {
        eprintln!("usage: charter-settings --set-learning <user>  (GrantLearning JSON on stdin)");
        std::process::exit(2);
    };
    let mut body = String::new();
    use std::io::Read as _;
    if std::io::stdin().read_to_string(&mut body).is_err() || body.trim().is_empty() {
        eprintln!("charter-settings: expected GrantLearning JSON on stdin");
        std::process::exit(2);
    }
    let value: serde_json::Value = match serde_json::from_str(body.trim()) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("charter-settings: invalid JSON: {e}");
            std::process::exit(2);
        }
    };
    let learning = match charter_proto::GrantLearning::from_value(&value) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("charter-settings: invalid learning body: {e:?}");
            std::process::exit(2);
        }
    };
    match charterd::device_limits::set_child_learning(LIMITS_DIR, &user, Some(&learning)) {
        Ok(()) => std::process::exit(0),
        Err(e) => {
            eprintln!("charter-settings: could not save: {e}");
            std::process::exit(1);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args
        .iter()
        .any(|a| a == "--set-learning" || a == "--clear-learning")
    {
        headless_set_learning(&args);
    }
    if args.iter().any(|a| a == "--set") {
        headless_set(&args);
    }

    let children = load_child_limits_dir(LIMITS_DIR);
    if children.is_empty() {
        notify(
            "--error",
            "No managed children yet. Run \"Kintrinsic Setup\" first to add a child.",
        );
        std::process::exit(1);
    }

    let Some(user) = pick_child(&children) else {
        return; // cancelled
    };
    let cur = children
        .iter()
        .find(|(u, _)| *u == user)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| DeviceLimits {
            tz: detect_tz(),
            wake: "07:00".into(),
            bedtime: "20:00".into(),
            daily_minutes: 120,
            weekend: None,
        });

    let weekend = cur
        .weekend
        .as_ref()
        .map(|w| format!("; weekend {}–{}", w.wake, w.bedtime))
        .unwrap_or_default();
    let text = format!(
        "Screen-time limits for {user}.\n\nCurrent: allowed {}–{}, {} minutes/day{}",
        cur.wake, cur.bedtime, cur.daily_minutes, weekend
    );

    // zenity --forms can't pre-fill entries, so each label carries the current
    // value and a blank field means "keep current" (see parse_form_fields_over).
    let form: Vec<String> = vec![
        "--forms".into(),
        format!("--title=Kintrinsic — {user}"),
        format!(
            "--text={text}\n\nEdit only what you want to change — leave a field blank to keep it."
        ),
        format!("--add-entry=Allowed from — now {} (blank = keep)", cur.wake),
        format!(
            "--add-entry=Allowed until — now {} (blank = keep)",
            cur.bedtime
        ),
        format!(
            "--add-entry=Daily minutes — now {} (blank = keep)",
            cur.daily_minutes
        ),
        "--add-entry=Weekend from (optional)".into(),
        "--add-entry=Weekend until (optional)".into(),
        "--separator=|".into(),
    ];

    let out = match zenity(&form) {
        Ok(o) if o.status.success() => o,
        Ok(_) => return, // cancelled
        Err(e) => {
            eprintln!("charter-settings: zenity is required for the dialog: {e}");
            std::process::exit(1);
        }
    };

    let stdout = String::from_utf8_lossy(&out.stdout);
    let fields: Vec<&str> = stdout.trim_end_matches('\n').split('|').collect();
    match parse_form_fields_over(&fields, &cur) {
        // Preserve any guardian `subject` binding — a device-only limits edit must
        // never silently revert a phone-paired child to local-only control.
        Ok(limits) => match set_child_limits(LIMITS_DIR, &user, &limits) {
            Ok(()) => notify(
                "--info",
                &format!("Saved {user}'s limits. Kintrinsic applies them within a few seconds."),
            ),
            Err(e) => {
                notify("--error", &format!("Could not save: {e}"));
                std::process::exit(1);
            }
        },
        Err(e) => {
            notify("--error", &e);
            std::process::exit(2);
        }
    }
}
