//! `charter-pair` — connect this computer to a guardian on the parent's phone.
//!
//! The parent pastes the `bunker://…` link + their child's Signet ID from
//! Kintrinsic; this pins the guardian (writes the daemon's `pairing.json`) and
//! binds the child's subject locally so the guardian's clauses actually take
//! effect. Run as root via pkexec (writes root-owned state). The validate/build
//! logic lives in `charterd::pairing_setup` and is unit-tested; this is the
//! zenity glue (mirrors charter-settings.rs), run on a real desktop (VM).

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use charter_primitives::PubKey;
use charterd::device_limits::{
    load_child_configs, set_child_subject, valid_subject_hex, ChildConfig,
};
use charterd::pairing_setup::{build_pairing_json, map_pairing_error, read_device_pub};

const DEVICE_PUB: &str = "/var/lib/charter/device.pub";
const PAIRING: &str = "/var/lib/charter/pairing.json";
const LIMITS_DIR: &str = "/etc/charter/limits.d";

fn zenity(args: &[&str]) -> Option<String> {
    let out = Command::new("zenity").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn entry(text: &str) -> Option<String> {
    zenity(&[
        "--entry",
        "--title=Kintrinsic — Pair Guardian",
        &format!("--text={text}"),
    ])
}

fn err(text: &str) {
    let _ = zenity(&[
        "--error",
        "--title=Kintrinsic — Pair Guardian",
        &format!("--text={text}"),
    ]);
}

fn confirm(text: &str) -> bool {
    zenity(&[
        "--question",
        "--title=Kintrinsic — Pair Guardian",
        &format!("--text={text}"),
    ])
    .is_some()
}

/// Draw a fresh subject locally when Kintrinsic has none to offer.
fn mint_local_subject() -> Option<String> {
    use std::io::Read;
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf).ok()?;
    Some(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Which child to bind the guardian subject to (auto when there's one).
fn pick_child(configs: &[(String, ChildConfig)]) -> Option<String> {
    match configs.len() {
        0 => None,
        1 => Some(configs[0].0.clone()),
        _ => {
            let mut args = vec![
                "--list".to_string(),
                "--title=Kintrinsic — Pair Guardian".into(),
                "--text=Which child is this guardian for?".into(),
                "--column=Child".into(),
            ];
            args.extend(configs.iter().map(|(u, _)| u.clone()));
            let argrefs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            zenity(&argrefs).filter(|s| !s.is_empty())
        }
    }
}

fn write_pairing(json: &str) -> std::io::Result<()> {
    if let Some(dir) = Path::new(PAIRING).parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = format!("{PAIRING}.tmp");
    std::fs::write(&tmp, json)?;
    // 0644 — the link is PUBLIC data; the unprivileged read-side (`charter`
    // status) reads the guardian's short fingerprint from it.
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    std::fs::rename(&tmp, PAIRING)
}

/// Headless pairing used by the unified `charter-console` app:
/// `charter-pair --link <bunker://…> --subject <hex>`. Same validate → build →
/// bind-subject → pin → reload path as the interactive flow, without dialogs.
/// Binds the subject to the single managed child (the common family case);
/// errors clearly when there is more than one so nothing is bound wrongly.
fn headless_pair(args: &[String]) -> ! {
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let Some(uri) = get("--link") else {
        eprintln!("usage: charter-pair --link <bunker://…> [--subject <hex>]");
        std::process::exit(2);
    };
    // `--subject` is OPTIONAL. Kintrinsic has no dependant pubkey to give (a
    // child's `dependantPubkey` is null and never assigned), and the broker
    // routes a subject-less clause to the pairing's sole subject — so when the
    // parent has nothing to paste we mint one locally, exactly as the scan
    // path does. Requiring it here is what made the typed flow unfinishable.
    let subject_hex = match get("--subject") {
        Some(s) if !s.trim().is_empty() => s.trim().to_lowercase(),
        _ => match mint_local_subject() {
            Some(s) => s,
            None => {
                eprintln!("charter-pair: no entropy available to create a child ID.");
                std::process::exit(1);
            }
        },
    };

    let Some(machine) = read_device_pub(DEVICE_PUB) else {
        eprintln!("charter-pair: finish \"Kintrinsic Setup\" first (no device key yet).");
        std::process::exit(1);
    };
    let subject = match (
        valid_subject_hex(&subject_hex),
        PubKey::from_hex(&subject_hex),
    ) {
        (true, Ok(p)) => p,
        _ => {
            eprintln!("charter-pair: the child ID must be 64 hex characters.");
            std::process::exit(2);
        }
    };
    let json = match build_pairing_json(&uri, machine, subject, now()) {
        Ok(j) => j,
        Err(e) => {
            eprintln!("charter-pair: {}", map_pairing_error(&e));
            std::process::exit(2);
        }
    };
    let children = load_child_configs(LIMITS_DIR);
    let child = match children.as_slice() {
        [] => {
            eprintln!("charter-pair: set up a child in \"Kintrinsic Setup\" first.");
            std::process::exit(1);
        }
        [(user, _)] => user.clone(),
        _ => {
            eprintln!(
                "charter-pair: more than one child on this computer — pair from \
                 the Kintrinsic app so you can choose which one."
            );
            std::process::exit(1);
        }
    };
    if let Err(e) = set_child_subject(LIMITS_DIR, &child, Some(&subject_hex)) {
        eprintln!("charter-pair: could not link {child}: {e}");
        std::process::exit(1);
    }
    if let Err(e) = write_pairing(&json) {
        eprintln!("charter-pair: could not save the pairing: {e}");
        std::process::exit(1);
    }
    let _ = Command::new("systemctl")
        .args(["try-restart", "charterd.service"])
        .status();
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--link") {
        headless_pair(&args);
    }

    // 1) This device's machine pubkey (written by the daemon at first boot).
    let Some(machine) = read_device_pub(DEVICE_PUB) else {
        err(
            "Finish \"Kintrinsic Setup\" first — it starts the background service that \
             creates this computer's key.",
        );
        std::process::exit(1);
    };

    // 2) Replacing an existing pairing is explicit.
    if Path::new(PAIRING).exists()
        && !confirm(
            "This computer is already connected to a guardian.\n\nReplace it with a new one?",
        )
    {
        return;
    }

    // 3) The guardian link.
    let Some(uri) =
        entry("Paste the pairing link from Kintrinsic on your phone\n(it starts with bunker://):")
    else {
        return;
    };

    // 4) The child's Signet ID — OPTIONAL. Most parents have none to paste
    //    (Kintrinsic never mints one), so an empty answer means "make one".
    let subject_hex = match entry(
        "If Kintrinsic gave you a child ID, paste it here.\n\
         Most parents can leave this blank — Kintrinsic will create one.",
    ) {
        None => return, // cancelled
        Some(s) if s.trim().is_empty() => match mint_local_subject() {
            Some(s) => s,
            None => {
                err("Couldn't create a child ID on this computer. Please try again.");
                std::process::exit(1);
            }
        },
        Some(s) => s.trim().to_lowercase(),
    };
    let subject = match (
        valid_subject_hex(&subject_hex),
        PubKey::from_hex(&subject_hex),
    ) {
        (true, Ok(p)) => p,
        _ => {
            err(
                "That child ID doesn't look right — it should be 64 letters and numbers. \
                 Leave it blank and Kintrinsic will create one for you.",
            );
            std::process::exit(2);
        }
    };

    // 5) Validate + build the pinned pairing.
    let json = match build_pairing_json(&uri, machine, subject, now()) {
        Ok(j) => j,
        Err(e) => {
            err(map_pairing_error(&e));
            std::process::exit(2);
        }
    };

    // 6) Bind the subject to the local child so the guardian's clauses RESOLVE
    //    (not just cache). This is REQUIRED — without a binding the guardian's
    //    per-child limits are inert, so we must not write the pairing or claim
    //    success unless a child was actually linked.
    let children = load_child_configs(LIMITS_DIR);
    if children.is_empty() {
        err("Set up your child's account first in \"Kintrinsic Setup\", then pair.");
        std::process::exit(1);
    }
    let Some(child) = pick_child(&children) else {
        return; // picker cancelled — link nothing, write nothing, claim nothing
    };
    if let Err(e) = set_child_subject(LIMITS_DIR, &child, Some(&subject_hex)) {
        err(&format!("Could not link {child} to this guardian: {e}"));
        std::process::exit(1);
    }

    // 7) Pin + reload.
    if let Err(e) = write_pairing(&json) {
        err(&format!("Could not save the pairing: {e}"));
        std::process::exit(1);
    }
    let _ = Command::new("systemctl")
        .args(["try-restart", "charterd.service"])
        .status();

    let _ = zenity(&[
        "--info",
        "--title=Kintrinsic — Pair Guardian",
        "--text=Paired with your guardian. Kintrinsic will now accept limits and \
         approvals from your phone.",
    ]);
}
