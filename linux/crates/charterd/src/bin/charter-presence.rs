//! `charter-presence` — show a one-time presence code so a guardian standing
//! at this computer can confirm a pending co-guardian, root removal or root
//! move (contract, *Guardian roots and working keys*, *Presence
//! confirmation*).
//!
//! Run as root, via pkexec (the Kintrinsic console's "Confirm in person") or
//! sudo: only an administrator may mint a code, so neither the child nor
//! anyone the child passes a code to can confirm anything. The code is minted
//! by `charterd`, the one writer of the guardian-keys state: this helper
//! leaves a request in the root-only state directory and shows what the
//! daemon minted.
//!
//! ```text
//! charter-presence                  list what is pending; with exactly one, show its code
//! charter-presence --list [--json]  list what is pending
//! charter-presence --mint <d|n> [--json]
//!                                   show the code for the pending statement `d`
//!                                   (or the n-th in the list)
//! ```
//!
//! Exit codes: 0 shown (or listed), 1 error, 2 usage, 3 nothing pending,
//! 4 charterd did not answer.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use charter_primitives::PubKey;
use charterd::guardians::{
    minted_for, peek_keys, pending_kind, presence_payload, presence_request_pending,
    presence_short_form, request_presence, short_npub, withdraw_presence_request, PresenceRequest,
    STATE_DIR,
};
use charterd::pairing_setup::read_device_pub;

const DEVICE_PUB: &str = "/var/lib/charter/device.pub";
const PAIRING: &str = "/var/lib/charter/pairing.json";
/// How long to wait for charterd (its fast tick is 2 s).
const WAIT: Duration = Duration::from_secs(15);

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fail(code: i32, msg: &str) -> ! {
    eprintln!("charter-presence: {msg}");
    std::process::exit(code);
}

struct Pending {
    d: String,
    kind: &'static str,
    key: String,
    takes_effect_at: Option<i64>,
    expires_at: i64,
}

fn pending(pairing: &charter_transport::Pairing) -> Vec<Pending> {
    let keys = peek_keys(Path::new(STATE_DIR), pairing, now());
    if !keys.is_available() {
        fail(
            1,
            "the guardian state on this computer cannot be read. Re-pair from the Kintrinsic \
             app to recover.",
        );
    }
    keys.pending_statements(now())
        .into_iter()
        .map(|p| Pending {
            key: charter_guardians::split_pending_d(&p.d)
                .and_then(|(_, k)| PubKey::from_hex(k).ok())
                .map(|k| short_npub(&k))
                .unwrap_or_default(),
            kind: pending_kind(p.kind),
            takes_effect_at: p.takes_effect_at,
            expires_at: p.expires_at,
            d: p.d,
        })
        .collect()
}

fn describe(p: &Pending) -> String {
    match p.kind {
        "coguardian" => format!("add {} as a guardian", p.key),
        "removal" => format!("remove the guardian {}", p.key),
        "move" => format!("move a guardian to the new identity {}", p.key),
        other => format!("{other} {}", p.key),
    }
}

fn list_json(items: &[Pending]) -> String {
    serde_json::Value::Array(
        items
            .iter()
            .map(|p| {
                serde_json::json!({
                    "d": p.d,
                    "kind": p.kind,
                    "key": p.key,
                    "takesEffectAt": p.takes_effect_at,
                    "expiresAt": p.expires_at,
                })
            })
            .collect(),
    )
    .to_string()
}

fn print_list(items: &[Pending]) {
    for (i, p) in items.iter().enumerate() {
        println!("  {}. {}\n     {}", i + 1, describe(p), p.d);
    }
}

/// Ask charterd for a code bound to `d` and wait for its answer.
fn mint(d: &str) -> charter_sys::presence_token::PresenceToken {
    let dir = Path::new(STATE_DIR);
    let request = PresenceRequest {
        d: d.to_owned(),
        requested_at: now(),
    };
    if let Err(e) = request_presence(dir, d, request.requested_at) {
        fail(1, &format!("could not ask charterd for a code: {e}"));
    }
    let started = Instant::now();
    while presence_request_pending(dir) {
        if started.elapsed() >= WAIT {
            let _ = withdraw_presence_request(dir);
            fail(
                4,
                "charterd did not answer. Is it running? (systemctl status charterd)",
            );
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    match minted_for(dir, &request, now()) {
        Some(t) => t,
        None => fail(
            1,
            "charterd refused to make a code (see journalctl -u charterd). The statement may \
             have expired or been cancelled.",
        ),
    }
}

/// A terminal QR, when `qrencode` is installed (the console draws its own).
fn terminal_qr(payload: &str) -> Option<String> {
    let out = Command::new("qrencode")
        .args(["-t", "UTF8", "-m", "2", "-o", "-", "--", payload])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let json = args.iter().any(|a| a == "--json");
    let flag_value = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } != 0 {
        fail(
            1,
            "only an administrator can show a presence code. Run it from the Kintrinsic app, \
             or with sudo.",
        );
    }
    let Some(pairing) = std::fs::read_to_string(PAIRING)
        .ok()
        .and_then(|j| serde_json::from_str::<charter_transport::Pairing>(&j).ok())
    else {
        fail(1, "this computer is not paired with a guardian.");
    };
    let Some(machine) = read_device_pub(DEVICE_PUB) else {
        fail(
            1,
            "this computer has no device key yet. Finish \"Kintrinsic Setup\" first.",
        );
    };

    let items = pending(&pairing);
    if args.iter().any(|a| a == "--list") {
        if json {
            println!("{}", list_json(&items));
        } else if items.is_empty() {
            println!("Nothing is waiting for a guardian to confirm in person.");
        } else {
            println!("Waiting for a guardian to confirm in person:");
            print_list(&items);
        }
        return;
    }
    if items.is_empty() {
        if json {
            println!("{}", serde_json::json!({ "pending": [] }));
        } else {
            println!("Nothing is waiting for a guardian to confirm in person. No code was made.");
        }
        std::process::exit(3);
    }
    let chosen = match flag_value("--mint") {
        Some(want) => items
            .iter()
            .enumerate()
            .find(|(i, p)| p.d == want || want.parse::<usize>().ok() == Some(i + 1))
            .map(|(_, p)| p)
            .unwrap_or_else(|| fail(2, "that is not one of the pending statements (see --list).")),
        None if items.len() == 1 => &items[0],
        None => {
            println!("More than one thing is waiting to be confirmed in person:");
            print_list(&items);
            println!("Run again with --mint <number> to show the code for one of them.");
            std::process::exit(2);
        }
    };

    let token = mint(&chosen.d);
    let payload = presence_payload(&machine, &token.token, &chosen.d);
    let short = presence_short_form(&token.token);
    let expires_at = token.minted_at + charter_sys::presence_token::PRESENCE_TOKEN_TTL_SECS;
    if json {
        println!(
            "{}",
            serde_json::json!({
                "payload": payload,
                "short": short,
                "d": chosen.d,
                "kind": chosen.kind,
                "key": chosen.key,
                "expiresAt": expires_at,
            })
        );
        return;
    }
    println!(
        "To {}, the guardian scans this with Kintrinsic on their phone:",
        describe(chosen)
    );
    if let Some(qr) = terminal_qr(&payload) {
        println!("{qr}");
    }
    println!("{payload}");
    println!("Or types this code in Kintrinsic: {short}");
    println!("It works once, for the next 10 minutes. Showing a new code cancels this one.");
}
