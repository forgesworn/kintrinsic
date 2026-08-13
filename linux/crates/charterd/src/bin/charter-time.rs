//! `charter-time` — give or take back a ward's time **at the computer**.
//!
//! The privileged half of the console's "Give time" / "Take time back"
//! buttons. The console runs this through `pkexec`, so it costs an admin
//! password every time — the sudo model, and the reason the console itself can
//! stay open to the ward: they can read everything and authorise nothing.
//!
//! It writes one entry to the child's local-adjustment file and returns; the
//! daemon picks it up on its next tick (seconds) and routes it to whichever
//! limit the ward is actually up against. See `charterd::local_adjust` for why
//! this is an unsigned local record rather than a clause.
//!
//! ```text
//! charter-time --user robin --minutes 30     # give half an hour
//! charter-time --user robin --minutes -20    # take twenty minutes back
//! ```
//!
//! Exit codes: 0 ok, 2 usage, 4 no such managed child, 6 refused.

use charterd::local_adjust::{adjust_path, AdjustEntry, AdjustRecord, ADJUST_VERSION};
use charterd::state_file::{state_dir, PublishedState};

const LIMITS_DIR: &str = "/etc/charter/limits.d";

/// The same ceiling a remote `gift` respects — the local door must never be
/// the wider one just because the guardian is standing at the keyboard.
const MAX_ADJUST_MINUTES: i32 = charter_proto::MAX_GIFT_MINUTES as i32;

fn die(code: i32, msg: &str) -> ! {
    eprintln!("charter-time: {msg}");
    std::process::exit(code);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let get = |flag: &str| -> Option<String> {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };

    let (Some(user), Some(minutes)) = (get("--user"), get("--minutes")) else {
        die(
            2,
            "usage: charter-time --user <name> --minutes <+N|-N>\n\
             (positive gives time, negative takes it back)",
        );
    };

    let Ok(minutes) = minutes.parse::<i32>() else {
        die(2, "--minutes must be a whole number, positive or negative");
    };
    if minutes == 0 {
        die(2, "--minutes must not be zero");
    }
    if minutes.abs() > MAX_ADJUST_MINUTES {
        die(
            2,
            &format!("--minutes must be between -{MAX_ADJUST_MINUTES} and {MAX_ADJUST_MINUTES}"),
        );
    }

    // Only a child this machine actually manages. Without this the file would
    // be written for any name at all and simply never read — a change the
    // guardian would be told had worked.
    if !std::path::Path::new(&format!("{LIMITS_DIR}/{user}.json")).exists() {
        die(
            4,
            &format!("'{user}' isn't set up with Kintrinsic on this computer"),
        );
    }
    let Some(uid) = uid_for_user(&user) else {
        die(4, &format!("no local account called '{user}'"));
    };

    // Refuse when there is no bounded limit to move. A give that does nothing
    // merely disappoints; a take-back that silently does nothing leaves the
    // guardian believing they have acted. Both are refused out loud instead.
    //
    // Unknown state (the daemon has not published yet) is NOT treated as
    // "nothing to adjust": that would refuse a legitimate change during the
    // first seconds after a restart. The entry is harmless if it turns out
    // there is nothing to move.
    if let Some(state) = published_state(&user) {
        let t = &state.time_left;
        if t.schedule_seconds < 0 && t.budget_seconds < 0 {
            die(
                6,
                &format!(
                    "{user} has no daily limit or bedtime set, so there's no time to \
                     give or take. Set their limits first."
                ),
            );
        }
    }

    let now = unix_now();
    let mut record = AdjustRecord::load(uid).pruned(now);
    record.v = ADJUST_VERSION;
    record.entries.push(AdjustEntry {
        // Unique per entry: the timestamp alone collides when a guardian taps
        // twice inside one second, and a collision would silently swallow the
        // second tap (the ledger is idempotent by exactly this id).
        id: format!("{now}-{}-{}", std::process::id(), record.entries.len()),
        minutes,
        at: now,
        by: admin_username(),
    });

    write_record(uid, &record);

    if minutes > 0 {
        println!("Gave {user} {} more minutes today.", minutes);
    } else {
        println!("Took {} minutes off {user}'s day.", -minutes);
    }
}

/// Persist the record 0644 under root ownership: the ward's own console reads
/// it to show what was given or taken, and only root may write it.
fn write_record(uid: u32, record: &AdjustRecord) {
    let path = adjust_path(uid);
    if let Some(dir) = std::path::Path::new(&path).parent() {
        if std::fs::create_dir_all(dir).is_err() {
            die(1, "couldn't create Kintrinsic's state directory");
        }
    }
    let Ok(body) = serde_json::to_string(record) else {
        die(1, "couldn't encode the change");
    };
    // Temp-then-rename so the daemon mid-tick reads either the old file or the
    // new one, never a half-written one it would parse as empty and discard.
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, &body).is_err() {
        die(1, "couldn't save the change");
    }
    set_mode(&tmp, 0o644);
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
        die(1, "couldn't save the change");
    }
}

#[cfg(unix)]
fn set_mode(path: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

#[cfg(not(unix))]
fn set_mode(_path: &str, _mode: u32) {}

/// The child's freshest published state, if the daemon has written one.
fn published_state(user: &str) -> Option<PublishedState> {
    let text = std::fs::read_to_string(format!("{}/{user}.json", state_dir())).ok()?;
    serde_json::from_str(&text).ok()
}

fn uid_for_user(user: &str) -> Option<u32> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        (f.len() >= 3 && f[0] == user).then(|| f[2].parse().ok())?
    })
}

/// Who authorised this. Under `pkexec` the original caller is `PKEXEC_UID`;
/// `SUDO_USER` covers a plain `sudo charter-time`. Recorded so the on-screen
/// account of the day can name them — an unattributed adjustment is exactly
/// the silent change Kintrinsic exists not to make.
fn admin_username() -> Option<String> {
    if let Ok(uid) = std::env::var("PKEXEC_UID") {
        if let Ok(uid) = uid.parse::<u32>() {
            if let Some(name) = username_for_uid(uid) {
                return Some(name);
            }
        }
    }
    std::env::var("SUDO_USER").ok()
}

fn username_for_uid(uid: u32) -> Option<String> {
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    passwd.lines().find_map(|line| {
        let f: Vec<&str> = line.split(':').collect();
        (f.len() >= 3 && f[2].parse::<u32>().ok() == Some(uid)).then(|| f[0].to_string())
    })
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
