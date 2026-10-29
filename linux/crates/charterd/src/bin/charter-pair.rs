//! `charter-pair` — connect this computer to a guardian on the parent's phone.
//!
//! The parent pastes the `bunker://…` link + their child's Signet ID from
//! Kintrinsic; this pins the guardian (writes the daemon's `pairing.json`) and
//! binds the child's subject locally so the guardian's clauses actually take
//! effect. Run as root via pkexec (writes root-owned state). The validate/build
//! logic lives in `charterd::pairing_setup` and is unit-tested; this is the
//! zenity glue (mirrors charter-settings.rs), run on a real desktop (VM).
//!
//! Replacing an existing pairing is always explicit: the interactive flow
//! confirms with a dialog, and the headless flow (`--link`, what
//! `charter-console` calls) refuses outright unless `--replace` is passed —
//! the console is expected to have confirmed with the parent itself before
//! ever passing it. Either way, binding the child's subject and writing the
//! pin are treated as one transaction (a failed pin write rolls the bind
//! back), and a re-pair to a NEW subject purges the OLD subject's now-orphaned
//! cached clauses.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use charter_primitives::PubKey;
use charterd::device_limits::{
    load_child_configs, purge_subject_store, set_child_subject, valid_subject_hex, ChildConfig,
    CHILD_CLAUSE_STORE_BASE,
};
use charterd::pairing_setup::{build_pairing_json, map_pairing_error, read_device_pub};

const DEVICE_PUB: &str = "/var/lib/charter/device.pub";
const PAIRING: &str = "/var/lib/charter/pairing.json";
const STATE_DIR: &str = charterd::guardians::STATE_DIR;
const LIMITS_DIR: &str = "/etc/charter/limits.d";

/// The headless flags that consume the following token as a value.
const VALUE_FLAGS: [&str; 2] = ["--link", "--subject"];

/// argv, parsed ONCE, the same way for every question asked of it.
///
/// There used to be two readers with two different ideas of what a token is.
/// `wants_replace` walked the args skipping each value-taking flag's value
/// slot, while the value lookup was `args.iter().position(|a| a == flag)` —
/// a search with no notion of slots at all. So `--link --subject --replace`
/// read as: `--link`'s value is `--subject`, and then the `position` search
/// found `--subject` anyway (as `--link`'s VALUE) and handed back `--replace`
/// as the subject. Two readers, two answers, from one argv — which is the
/// shape of bug that eventually lets one of them see consent the other does
/// not. One pass, one answer.
///
/// Position, not search: the token after a value-taking flag is that flag's
/// value and nothing else, even when it is spelled like a flag.
struct Flags {
    values: std::collections::BTreeMap<String, String>,
    present: std::collections::BTreeSet<String>,
}

fn parse_flags(args: &[String]) -> Flags {
    let mut values = std::collections::BTreeMap::new();
    let mut present = std::collections::BTreeSet::new();
    let mut i = 0;
    while i < args.len() {
        if VALUE_FLAGS.contains(&args[i].as_str()) {
            // First wins, as the old `position` lookup did.
            if let Some(v) = args.get(i + 1) {
                values.entry(args[i].clone()).or_insert_with(|| v.clone());
            }
            i += 2; // the flag AND the value slot that follows it
        } else {
            present.insert(args[i].clone());
            i += 1;
        }
    }
    Flags { values, present }
}

impl Flags {
    /// The value given to `flag`, or `None` when it was not passed (or was
    /// passed with nothing after it).
    fn get(&self, flag: &str) -> Option<String> {
        self.values.get(flag).cloned()
    }
    /// Whether `flag` appeared as a STANDALONE token — never as some other
    /// flag's value.
    fn has(&self, flag: &str) -> bool {
        self.present.contains(flag)
    }
}

/// Whether `--replace` was passed as a STANDALONE flag — the explicit ask
/// headless pairing requires before it will overwrite an existing guardian.
/// A bare `.any(|a| a == "--replace")` over-matches: if `--replace` happens
/// to be the *value* of `--link` or `--subject` (a mis-typed
/// `charter-console` invocation, a subject hex that collided with the literal
/// string), that token is data, not a flag, and must not be read as consent
/// to overwrite an existing pairing.
fn wants_replace(args: &[String]) -> bool {
    parse_flags(args).has("--replace")
}

/// A re-pair to a NEW subject orphans the old subject's cached clauses —
/// purge them, the same directory a guardian RELEASE purges. Best effort and
/// silent-on-success: the pairing already landed by the time this runs, so a
/// purge failure must not read as the pairing having failed.
fn purge_if_repaired_to_a_new_subject(
    pairing_existed: bool,
    previous_subject: Option<&str>,
    new_subject_hex: &str,
) {
    if !pairing_existed {
        return;
    }
    let Some(prev) = previous_subject else {
        return;
    };
    if prev.eq_ignore_ascii_case(new_subject_hex) {
        return;
    }
    if let Err(e) = purge_subject_store(CHILD_CLAUSE_STORE_BASE, prev) {
        eprintln!("charter-pair: could not clear the old guardian's cached rules: {e}");
    }
}

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

/// The lock the warden also takes around its own read-modify-write of the
/// pairing record (a gift-wrapped RELAYS), so neither loses the other's
/// change (review 2a, Info 2).
fn lock_pairing() -> std::io::Result<charter_sys::file_lock::FileLock> {
    charter_sys::file_lock::lock_exclusive(
        &Path::new(STATE_DIR).join(charter_sys::file_lock::PAIRING_LOCK_FILE),
    )
}

/// A fresh pin is a local unpair by an administrator followed by a new
/// pairing (rule 11): the new guardian inherits no root, working key,
/// pending statement or presence token from the old one. Run after the new
/// pairing is written, so a failure here leaves the old state bound to the
/// OLD pairing's `paired_at`, which the warden then discards on its own.
fn forget_previous_guardians() {
    if let Err(e) = charterd::guardians::clear_state(Path::new(STATE_DIR)) {
        eprintln!("charter-pair: could not clear the previous guardian state: {e}");
    }
}

fn write_pairing(json: &str) -> std::io::Result<()> {
    let _lock = lock_pairing()?;
    write_pairing_locked(json)
}

fn write_pairing_locked(json: &str) -> std::io::Result<()> {
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
/// `charter-pair --link <bunker://…> [--subject <hex>] [--replace]`. Same
/// validate → build → bind-subject → pin → reload path as the interactive
/// flow, without dialogs. Binds the subject to the single managed child (the
/// common family case); errors clearly when there is more than one so nothing
/// is bound wrongly. Refuses to overwrite an existing pairing unless
/// `--replace` is given — the console is expected to have already confirmed
/// with the parent before ever passing it.
fn headless_pair(args: &[String]) -> ! {
    // ONE parse, shared with `wants_replace` below: the token that is
    // `--link`'s value must not also be readable as a flag in its own right.
    let flags = parse_flags(args);
    let get = |flag: &str| -> Option<String> { flags.get(flag) };
    let Some(uri) = get("--link") else {
        eprintln!("usage: charter-pair --link <bunker://…> [--subject <hex>] [--replace]");
        std::process::exit(2);
    };
    let pairing_existed = Path::new(PAIRING).exists();
    // Same parser as `get` above — that is the whole point: one reading of
    // argv, so the token that is `--link`'s value can never also be consent.
    if pairing_existed && !wants_replace(args) {
        eprintln!(
            "charter-pair: this computer is already paired with a guardian — pass --replace \
             to replace it."
        );
        std::process::exit(3);
    }
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
    let previous_subject = children
        .iter()
        .find(|(u, _)| u == &child)
        .and_then(|(_, c)| c.subject.clone());
    if let Err(e) = set_child_subject(LIMITS_DIR, &child, Some(&subject_hex)) {
        eprintln!("charter-pair: could not link {child}: {e}");
        std::process::exit(1);
    }
    if let Err(e) = write_pairing(&json) {
        // Bind-then-write must not leave the child bound to a subject no
        // guardian holds — restore what was there before (best effort).
        if let Err(re) = set_child_subject(LIMITS_DIR, &child, previous_subject.as_deref()) {
            eprintln!(
                "charter-pair: could not roll back {child}'s subject link after a failed \
                 save: {re}"
            );
        }
        eprintln!("charter-pair: could not save the pairing: {e}");
        std::process::exit(1);
    }
    forget_previous_guardians();
    purge_if_repaired_to_a_new_subject(pairing_existed, previous_subject.as_deref(), &subject_hex);
    let _ = Command::new("systemctl")
        .args(["try-restart", "charterd.service"])
        .status();
    std::process::exit(0);
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if parse_flags(&args).has("--relay-code") {
        // Root/polkit remains the local write boundary; the signature also
        // proves the guardian approved this exact device and relay list.
        use std::io::Read;
        let result: Result<(), String> = (|| {
            let mut code = String::new();
            std::io::stdin()
                .take(16_385)
                .read_to_string(&mut code)
                .map_err(|e| e.to_string())?;
            // Held across the read and the write: the warden rewrites this
            // record too (review 2a, Info 2).
            let _lock = lock_pairing().map_err(|e| format!("lock pairing: {e}"))?;
            let json =
                std::fs::read_to_string(PAIRING).map_err(|e| format!("read pairing: {e}"))?;
            let mut pairing: charter_transport::Pairing =
                serde_json::from_str(&json).map_err(|e| format!("read pairing: {e}"))?;
            // Any key in the guardian-keys signer set may sign it (rule 11;
            // review 2a, Info 6), read without writing: the warden is the
            // state's one writer. An unreadable state has no signers, so the
            // code is refused (fail closed).
            let signers = {
                use charter_spine::GuardianStatements as _;
                charterd::guardians::peek_keys(Path::new(STATE_DIR), &pairing, now()).signers()
            };
            charter_transport::relay_config::apply_signed_config_from(
                &mut pairing,
                code.trim(),
                &*signers,
                now(),
            )?;
            let json = serde_json::to_string_pretty(&pairing).map_err(|e| e.to_string())?;
            write_pairing_locked(&json).map_err(|e| format!("save relay change: {e}"))?;
            let status = Command::new("systemctl")
                .args(["try-restart", "charterd.service"])
                .status()
                .map_err(|e| format!("Relays saved; restart charterd: {e}"))?;
            if !status.success() {
                return Err("Relays saved, but charterd could not restart. Restart the service to apply them.".into());
            }
            Ok(())
        })();
        match result {
            Ok(()) => println!("Relays changed; the guardian and charter are unchanged."),
            Err(e) => {
                eprintln!("charter-pair: {e}");
                std::process::exit(1);
            }
        }
        return;
    }
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
    let pairing_existed = Path::new(PAIRING).exists();
    if pairing_existed
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
    let previous_subject = children
        .iter()
        .find(|(u, _)| u == &child)
        .and_then(|(_, c)| c.subject.clone());
    if let Err(e) = set_child_subject(LIMITS_DIR, &child, Some(&subject_hex)) {
        err(&format!("Could not link {child} to this guardian: {e}"));
        std::process::exit(1);
    }

    // 7) Pin + reload.
    if let Err(e) = write_pairing(&json) {
        // Bind-then-write must not leave the child bound to a subject no
        // guardian holds — restore what was there before (best effort).
        if let Err(re) = set_child_subject(LIMITS_DIR, &child, previous_subject.as_deref()) {
            eprintln!(
                "charter-pair: could not roll back {child}'s subject link after a failed \
                 save: {re}"
            );
        }
        err(&format!("Could not save the pairing: {e}"));
        std::process::exit(1);
    }
    forget_previous_guardians();
    purge_if_repaired_to_a_new_subject(pairing_existed, previous_subject.as_deref(), &subject_hex);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn wants_replace_only_when_the_flag_is_present() {
        assert!(!wants_replace(&s(&["--link", "bunker://x"])));
        assert!(wants_replace(&s(&["--link", "bunker://x", "--replace"])));
        assert!(wants_replace(&s(&[
            "--replace",
            "--link",
            "bunker://x",
            "--subject",
            "ab"
        ])));
        // Not a value to some other flag — a bare token still counts, same as
        // every other boolean flag this binary parses.
        assert!(!wants_replace(&s(&["--subject", "--replace-not-quite"])));
    }

    #[test]
    fn wants_replace_ignores_the_token_when_it_is_a_value_not_a_flag() {
        // `--replace` sitting exactly where `--link`'s value belongs is that
        // value, not the flag — must not be read as consent to overwrite.
        assert!(!wants_replace(&s(&["--link", "--replace"])));
        assert!(!wants_replace(&s(&["--subject", "--replace"])));
        // A genuine standalone `--replace` AFTER a value slot still counts.
        assert!(wants_replace(&s(&["--link", "--replace", "--replace"])));
    }

    /// The two readers agreed on nothing. `["--subject","--link","--replace"]`:
    /// `--subject` takes `--link` as its value, which leaves `--replace` a
    /// standalone flag — and the OLD value lookup, a bare `position` search
    /// with no notion of value slots, found `--link` anyway and read
    /// `--replace` as the bunker URI. One parse now answers both questions the
    /// same way.
    #[test]
    fn one_parse_answers_both_questions_about_the_same_argv() {
        let a = s(&["--subject", "--link", "--replace"]);
        let f = parse_flags(&a);
        assert_eq!(f.get("--subject").as_deref(), Some("--link"));
        assert_eq!(f.get("--link"), None, "`--link` was consumed as a VALUE");
        assert!(f.has("--replace"), "and `--replace` is then a real flag");
        assert_eq!(f.has("--replace"), wants_replace(&a));

        // The mirror: `--replace` in `--link`'s value slot is data, and the
        // value lookup and the flag lookup say so together.
        let a = s(&["--link", "--replace", "--subject", "ab"]);
        let f = parse_flags(&a);
        assert_eq!(f.get("--link").as_deref(), Some("--replace"));
        assert_eq!(f.get("--subject").as_deref(), Some("ab"));
        assert!(!f.has("--replace"));
        assert!(!wants_replace(&a));

        // A value-taking flag with nothing after it consumes nothing and
        // yields nothing — it must not read the flag itself as its own value.
        let f = parse_flags(&s(&["--replace", "--link"]));
        assert!(f.has("--replace"));
        assert_eq!(f.get("--link"), None);
    }

    #[test]
    fn purge_if_repaired_only_fires_on_a_genuine_re_pair_to_a_new_subject() {
        // No prior pairing at all: never purges, whatever the subjects say.
        purge_if_repaired_to_a_new_subject(false, Some("aa"), "bb");
        // Same subject (case-insensitively): nothing to purge.
        purge_if_repaired_to_a_new_subject(true, Some("AA"), "aa");
        // No previous subject bound: nothing to purge.
        purge_if_repaired_to_a_new_subject(true, None, "bb");
        // These three must not touch the filesystem at all — nothing to
        // assert beyond "did not panic"; the real purge path (base dir,
        // sanitisation, NotFound-is-ok) is covered by
        // `device_limits::purge_subject_store`'s own tests.
    }
}
