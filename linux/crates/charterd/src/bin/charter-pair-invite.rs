//! `charter-pair-invite` — mint the one-time token behind this computer's
//! pairing QR and print the payload the phone should scan.
//!
//! Run as root via pkexec. Privileged for a reason: the token is the sole
//! proof of physical presence, so it is written 0600. If the ward's own
//! account could read it, the child could pair the laptop to a phone THEY
//! control and hand themselves unlimited time — which is precisely the
//! escalation the admin password exists to stop.
//!
//! Prints one line to stdout: `charter://pair?m=<machine-hex>&t=<token-hex>`

use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use charterd::pair_token;
use charterd::pairing_setup::read_device_pub;

const DEVICE_PUB: &str = "/var/lib/charter/device.pub";
const TOKEN: &str = "/var/lib/charter/pair-token.json";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The scan payload. With a token this is the scan-to-pair URI; without one it
/// degrades to the bare device code, which older Kintrinsic builds still read.
pub fn pair_qr_payload(machine_hex: &str, token: Option<&str>) -> String {
    match token {
        Some(t) => format!("charter://pair?m={machine_hex}&t={t}"),
        None => machine_hex.to_string(),
    }
}

fn main() {
    let Some(machine) = read_device_pub(DEVICE_PUB) else {
        eprintln!("charter-pair-invite: finish \"Kintrinsic Setup\" first (no device key yet).");
        std::process::exit(1);
    };

    let mut random = [0u8; 16];
    match std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut random)) {
        Ok(()) => {}
        Err(e) => {
            eprintln!("charter-pair-invite: no entropy available: {e}");
            std::process::exit(1);
        }
    }

    match pair_token::mint(TOKEN, now(), random) {
        Ok(token) => println!("{}", pair_qr_payload(&machine.to_hex(), Some(&token))),
        Err(e) => {
            eprintln!("charter-pair-invite: could not mint a pairing token: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_qr_payload_carries_both_the_machine_and_the_token() {
        let got = pair_qr_payload(&"ab".repeat(32), Some(&"cd".repeat(16)));
        assert_eq!(
            got,
            format!("charter://pair?m={}&t={}", "ab".repeat(32), "cd".repeat(16))
        );
    }

    #[test]
    fn without_a_token_the_qr_stays_the_bare_device_code() {
        // Backwards compatible: older Kintrinsic builds scan raw hex, and the
        // typed 8-group fallback is the same value.
        assert_eq!(pair_qr_payload(&"ab".repeat(32), None), "ab".repeat(32));
    }
}
