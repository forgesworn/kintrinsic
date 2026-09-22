//! The device's installed-app inventory, published on STATUS (`apps`) so the
//! guardian picks learning/native apps by NAME in Kintrinsic instead of typing
//! executable paths.
//!
//! Root-owned launcher dirs (`/usr/share/applications`, the system flatpak
//! export) are scanned unflagged. The managed users' OWN dirs
//! (`~/.local/share/applications`, `~/.local/share/flatpak/exports/share/
//! applications`) are ALSO scanned — so the guardian can see a ward-installed
//! launcher (Prism, MultiMC, a `flatpak install --user`) exist at all — but
//! every entry from one is stamped `AppRef.user_installed = true`. A
//! ward-writable entry must never masquerade as an installable identity:
//! flagging it is what makes SHOWING it safe, and `focus::classify` (§2.2/
//! §2.4) is what refuses it FREE (learning) time — capping/blocking a
//! user-installed identity is still allowed (restrictive is self-harm only).
//!
//! `pkg` carries the attribution identity `focus::classify` will match:
//! the resolved absolute binary path, or the flatpak app id.

use charter_proto::status::AppRef;

/// Parse one `.desktop` file into an inventory entry. `None` = not a real
/// launchable app (hidden, not an Application, one of ours, no Exec).
/// `user_installed` is stamped on the result as-is — the caller decides it
/// from which dir the entry came from, not from anything in the file itself.
pub fn parse_desktop_entry(filename: &str, content: &str) -> Option<AppRef> {
    if !filename.ends_with(".desktop")
        || filename.starts_with("charter-")
        || filename == "charter.desktop"
    {
        return None;
    }
    let mut name = None;
    let mut exec = None;
    let mut flatpak_id = None;
    let mut in_main = false;
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            // Only the main group defines the app (not per-action groups).
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        if let Some(v) = line.strip_prefix("NoDisplay=") {
            if v.trim() == "true" {
                return None;
            }
        } else if let Some(v) = line.strip_prefix("Type=") {
            if v.trim() != "Application" {
                return None;
            }
        } else if let Some(v) = line.strip_prefix("Name=") {
            name.get_or_insert_with(|| v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("Exec=") {
            exec.get_or_insert_with(|| v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("X-Flatpak=") {
            flatpak_id = Some(v.trim().to_string());
        }
    }
    let label = name?;
    // Flatpak exports carry their app id — that IS the identity.
    if let Some(id) = flatpak_id {
        return Some(AppRef {
            pkg: id,
            label,
            user_installed: None,
            hidden: None,
        });
    }
    // Otherwise the first Exec token (with any %-field codes ignored).
    let first = exec?.split_whitespace().next()?.to_string();
    if first.starts_with('%') || first.is_empty() {
        return None;
    }
    Some(AppRef {
        pkg: first,
        label,
        user_installed: None,
        hidden: None,
    })
}

/// The binary directories a bare `Exec` name is looked up in, in order. The
/// first four are `/usr/share/applications`' own working assumption; `/bin` and
/// `/sbin` are usually symlinks to their `/usr` twins on a merged-`/usr`
/// system, and cost one `stat` each where they are not.
const BIN_DIRS: [&str; 6] = [
    "/usr/bin",
    "/usr/games",
    "/usr/local/bin",
    "/usr/local/games",
    "/bin",
    "/opt/bin",
];

/// Does `pkg` have the SHAPE of a flatpak application id — reverse-DNS, as
/// `org.mozilla.firefox` or `com.valvesoftware.Steam`?
///
/// # Why shape, and not "has a dot" (03b-B5)
///
/// `contains('.') && !contains('/')` is true of `gimp-2.10` and `lua5.4` —
/// ordinary native commands with a version in the name. A launcher carrying
/// `Exec=gimp-2.10 %U` therefore entered the inventory as the flatpak app id
/// `gimp-2.10`, and `matches_pkg` then looked for `flatpak-gimp-2.10-` in the
/// process's cgroup, which a native process can never carry. The guardian's
/// block, the app's allowed hours, its bucket membership and the allowlist's
/// implied block were all silently inert for that app: a rule the guardian
/// believes is armed, failing open in total silence.
///
/// At least three dot-separated segments, each non-empty and starting with an
/// ASCII LETTER, and no character outside `[A-Za-z0-9_-]` — which `gimp-2.10`
/// (two segments, second starts with a digit) and `lua5.4` both fail.
fn looks_like_flatpak_id(pkg: &str) -> bool {
    if pkg.contains('/') {
        return false;
    }
    let segs: Vec<&str> = pkg.split('.').collect();
    segs.len() >= 3
        && segs.iter().all(|s| {
            s.starts_with(|c: char| c.is_ascii_alphabetic())
                && s.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

/// Resolve a non-absolute Exec token to an absolute path via `exists`.
/// Absolute tokens pass through; unresolvable tokens are dropped (an identity
/// we can't verify is not offered for picking).
///
/// Order matters, and it changed (03b-B5): a bare name is looked for on disk
/// FIRST, and only a name that no binary directory answers for — and that has
/// the shape of a reverse-DNS app id — is taken as a flatpak identity. A real
/// binary on this machine is the better answer than a guess about a flatpak
/// that, in the misrouting case, does not exist at all.
pub fn resolve_exec(pkg: &str, exists: impl Fn(&str) -> bool) -> Option<String> {
    if pkg.starts_with('/') {
        return exists(pkg).then(|| pkg.to_string());
    }
    for dir in BIN_DIRS {
        let candidate = format!("{dir}/{pkg}");
        if exists(&candidate) {
            return Some(candidate);
        }
    }
    if looks_like_flatpak_id(pkg) {
        // A flatpak app id (org.foo.Bar) — identity as-is.
        return Some(pkg.to_string());
    }
    None
}

/// Build the deduped, sorted inventory from `(filename, content, user_installed)`
/// triples — `user_installed` is which SCAN DIR the entry came from, decided by
/// the caller, never by anything in the `.desktop` file itself. When the same
/// resolved identity appears from more than one dir, the FIRST one seen wins
/// (callers list root dirs before user dirs, so a genuine root-owned entry is
/// never shadowed by a ward's own copy of the same identity).
pub fn build_inventory<'a>(
    entries: impl Iterator<Item = (String, String, bool)>,
    exists: impl Fn(&str) -> bool + 'a,
) -> Vec<AppRef> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out: Vec<AppRef> = Vec::new();
    for (name, content, user_installed) in entries {
        let Some(mut app) = parse_desktop_entry(&name, &content) else {
            continue;
        };
        let Some(resolved) = resolve_exec(&app.pkg, &exists) else {
            continue;
        };
        app.pkg = resolved;
        if user_installed {
            app.user_installed = Some(true);
        }
        if seen.insert(app.pkg.clone()) {
            out.push(app);
        }
    }
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out
}

/// The root-owned launcher dirs — scanned unflagged.
///
/// 03b-G3: snap and `/usr/local` were missing, and under the allowlist posture
/// that is fail-open in the most visible way there is — Ubuntu/Mint ship
/// Firefox, Chromium, Thunderbird and Steam as snaps, and an app that is not in
/// `inventory_pkgs` is never in the blocked set, so "only these apps are
/// allowed" permits it. Worse, the guardian cannot even SEE it to tick it.
#[cfg(feature = "real")]
const ROOT_DIRS: [&str; 4] = [
    "/usr/share/applications",
    // System-installed flatpaks export here (root-owned).
    "/var/lib/flatpak/exports/share/applications",
    // Every installed snap's launchers (root-owned, and where Ubuntu's default
    // Firefox actually lives).
    "/var/lib/snapd/desktop/applications",
    // Locally installed software, by the FHS and by `XDG_DATA_DIRS`' default.
    "/usr/local/share/applications",
];

/// The ward-writable dirs, relative to a managed user's home — scanned
/// `user_installed: true`.
#[cfg(feature = "real")]
const USER_DIR_SUFFIXES: [&str; 2] = [
    ".local/share/applications",
    ".local/share/flatpak/exports/share/applications",
];

/// Is `dir` owned by root? Only root-owned launcher dirs are scanned
/// UNFLAGGED; anything a non-root account owns is a dir that account can write,
/// so its entries carry `user_installed` exactly as the per-home mirrors do.
///
/// Unreadable / missing ⇒ `false`: the cautious answer is the flagged one, and
/// a dir that cannot be stat'ed yields no entries anyway.
#[cfg(feature = "real")]
fn is_root_owned(dir: &str) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(dir)
        .map(|m| m.uid() == 0)
        .unwrap_or(false)
}

/// The `applications/` dir under each `XDG_DATA_DIRS` entry, paired with
/// whether its entries are `user_installed` (03b-G3).
///
/// `XDG_DATA_DIRS` is how a distro, a site admin or a third-party installer
/// says "there are launchers over here too" — Nix, Homebrew-on-Linux and
/// several vendor packages all use it, and none of them were being read. It is
/// also environment, i.e. whatever charterd's own unit inherited: root-owned
/// entries are trusted like the fixed roots, the rest are flagged.
#[cfg(feature = "real")]
fn xdg_data_dirs() -> Vec<(String, bool)> {
    match std::env::var("XDG_DATA_DIRS") {
        Ok(raw) => xdg_dirs_from(&raw),
        Err(_) => Vec::new(),
    }
}

/// [`xdg_data_dirs`] over an explicit value — the env var is process-global,
/// and tests that set it race each other.
#[cfg(feature = "real")]
fn xdg_dirs_from(raw: &str) -> Vec<(String, bool)> {
    raw.split(':')
        .map(str::trim)
        .filter(|p| p.starts_with('/'))
        .map(|p| {
            let dir = format!("{}/applications", p.trim_end_matches('/'));
            let root_owned = is_root_owned(&dir);
            (dir, !root_owned)
        })
        .collect()
}

/// Every dir this device's inventory scans, root dirs FIRST (so `build_inventory`'s
/// first-wins dedup lets a root identity always win over a same-identity user
/// copy), each paired with whether entries from it are `user_installed`.
///
/// Deduped by path, first spelling wins — `XDG_DATA_DIRS` routinely repeats
/// `/usr/share` and `/usr/local/share`, and scanning either twice would only
/// cost `stat`s and risk a flagged duplicate shadowing the unflagged original.
#[cfg(feature = "real")]
fn all_scan_dirs(home_dirs: &[String]) -> Vec<(String, bool)> {
    let mut dirs: Vec<(String, bool)> = ROOT_DIRS.iter().map(|d| (d.to_string(), false)).collect();
    // Root-owned XDG entries rank with the fixed roots; flagged ones rank with
    // the per-home mirrors, after them.
    let xdg = xdg_data_dirs();
    dirs.extend(xdg.iter().filter(|(_, f)| !f).cloned());
    for home in home_dirs {
        for suffix in USER_DIR_SUFFIXES {
            dirs.push((format!("{home}/{suffix}"), true));
        }
    }
    dirs.extend(xdg.into_iter().filter(|(_, f)| *f));
    let mut seen = std::collections::BTreeSet::new();
    dirs.retain(|(d, _)| seen.insert(d.clone()));
    dirs
}

/// The largest a `.desktop` file this scan will read — a real one is a few
/// hundred bytes to a few KB. Bounds a ward-writable-dir attack where the
/// "file" is actually `ln -s /dev/zero fake.desktop`: unbounded reads of that
/// are the OOM variant of C1 (see [`read_desktop_file_safely`] for the other
/// half — refusing to even open it, since it isn't a regular file at all).
#[cfg(feature = "real")]
const MAX_DESKTOP_FILE_BYTES: u64 = 64 * 1024;

/// The most `.desktop` entries read out of a SINGLE directory per scan. A
/// real launcher dir has dozens to a few hundred entries; thousands in one
/// ward-writable dir is not a real desktop environment, it is an attempt to
/// make every scan (and the fingerprint/dedup work downstream) expensive.
#[cfg(feature = "real")]
const MAX_ENTRIES_PER_DIR: usize = 4096;

/// Read one `.desktop` file's content, refusing anything that isn't an
/// ordinary, size-bounded file — C1 (CRITICAL): a ward-writable scan dir is
/// adversarial input, not merely untrusted data inside a file.
///
/// `mkfifo ~/.local/share/applications/x.desktop` makes a plain `open()` for
/// reading BLOCK FOREVER waiting for a writer — with no writer ever coming,
/// that call (and, before this, the whole enforcement tick calling it) never
/// returns: no lock, no sweep, no STATUS, indefinitely, from ONE file in ONE
/// ward-writable directory. `ln -s /dev/zero x.desktop` is the OOM variant —
/// an unbounded read of an infinite device.
///
/// Both are refused the same way: the path must resolve to an ORDINARY FILE
/// — not a FIFO, device, directory, or socket — whose size is already within
/// [`MAX_DESKTOP_FILE_BYTES`].
///
/// **That check uses `metadata` (which FOLLOWS symlinks), never
/// `symlink_metadata`.** A first cut of this used `symlink_metadata` on the
/// theory that "not even a symlink" was the stricter rule. It is not stricter,
/// it is WRONG: **flatpak exports every single `.desktop` entry as a symlink**
/// — `/var/lib/flatpak/exports/share/applications/*.desktop` and the `--user`
/// mirror under `~/.local/share/flatpak/exports/...` are both symlinks into
/// `.../app/<id>/current/active/export/...`. Refusing symlinks therefore
/// dropped EVERY flatpak from the guardian's app list, and — far worse —
/// emptied the `userInstalled` set that `focus::classify` needs, handing the
/// `flatpak install --user` impostor free learning time again (the §2.4 hole
/// closed in `ff69f8e`). Following the link loses nothing: a symlink to
/// `/dev/zero` resolves to a character device (`is_file() == false`, refused),
/// a symlink to a FIFO resolves to a FIFO (refused), and a symlink to a huge
/// file reports that file's size (refused). Verified on a real Mint box.
///
/// Three further layers, because a followed path is a TOCTOU surface:
/// - the open sets `O_NONBLOCK` — a no-op for an ordinary file, but it means
///   a race that swaps the target for a FIFO between `metadata` and `open()`
///   returns immediately instead of blocking forever;
/// - the OPENED FD is re-checked (`fstat`, not another path lookup) for BOTH
///   type and size, so whatever was actually opened — not merely whatever the
///   path named a moment ago — has to be an ordinary, bounded file;
/// - the read is capped at [`MAX_DESKTOP_FILE_BYTES`] regardless
///   (`Read::take`), so a file that grows after even the `fstat` still cannot
///   stream unbounded bytes into memory.
///
/// `None` for anything refused or unreadable — an unsafe or oversized entry
/// is simply not offered for picking, never a reason to block the scan.
#[cfg(feature = "real")]
fn read_desktop_file_safely(path: &std::path::Path) -> Option<String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    // Follows symlinks ON PURPOSE — see the doc comment: every flatpak export
    // is one, and following costs no safety (the TARGET's type/size is what
    // gets judged, which is the thing that would actually hang or OOM us).
    let meta = std::fs::metadata(path).ok()?;
    if !meta.file_type().is_file() || meta.len() > MAX_DESKTOP_FILE_BYTES {
        return None;
    }
    let f = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    // Judge what we ACTUALLY opened, not what the path named before the open
    // (`File::metadata` is `fstat` on the fd — no second path resolution, so
    // nothing can be swapped underneath it).
    let opened = f.metadata().ok()?;
    if !opened.file_type().is_file() || opened.len() > MAX_DESKTOP_FILE_BYTES {
        return None;
    }
    let mut buf = Vec::with_capacity(opened.len() as usize);
    f.take(MAX_DESKTOP_FILE_BYTES).read_to_end(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

/// Scan the root-owned launcher dirs PLUS the managed users' own dirs
/// (`home_dirs` — the daemon's caller resolves these from the managed roster,
/// since a uid is needed and this module has no identity-DB access of its
/// own). Change-detected by the caller (dir mtimes) — this does the full read.
///
/// The caller MUST run this on a blocking-pool thread (`spawn_blocking`), not
/// the async executor — [`read_desktop_file_safely`] defends against the
/// worst cases, but a scan of a real filesystem is still synchronous I/O of
/// unpredictable latency, exactly like every other `/proc`/`xprop` probe this
/// daemon runs off the executor.
#[cfg(feature = "real")]
pub fn scan(home_dirs: &[String]) -> Vec<AppRef> {
    let entries = all_scan_dirs(home_dirs)
        .into_iter()
        .flat_map(|(dir, user_installed)| {
            std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .filter_map(|e| e.ok())
                .take(MAX_ENTRIES_PER_DIR)
                .filter_map(move |e| {
                    let name = e.file_name().into_string().ok()?;
                    let content = read_desktop_file_safely(&e.path())?;
                    Some((name, content, user_installed))
                })
                .collect::<Vec<_>>()
        });
    build_inventory(entries, |p| std::path::Path::new(p).exists())
}

/// The scanned dirs' mtimes — a cheap change signal for rescan. Same
/// `home_dirs` input as [`scan`].
///
/// "Cheap" is not "instant": these are `stat`s of WARD-CONTROLLED paths, and
/// a ward can make `~/.local/share/applications` a hung FUSE mount whose
/// every `stat` blocks indefinitely. It therefore carries [`scan`]'s
/// requirement too — the caller MUST run it on a blocking-pool thread, in the
/// SAME `spawn_blocking` as the scan it gates (a fingerprint on the executor
/// and a scan off it just moves the wedge one line up).
#[cfg(feature = "real")]
pub fn dirs_fingerprint(home_dirs: &[String]) -> u64 {
    use std::time::UNIX_EPOCH;
    all_scan_dirs(home_dirs)
        .iter()
        .filter_map(|(d, _)| std::fs::metadata(d).ok()?.modified().ok())
        .filter_map(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, body: &str) -> (String, String, bool) {
        (name.to_string(), body.to_string(), false)
    }

    fn user_entry(name: &str, body: &str) -> (String, String, bool) {
        (name.to_string(), body.to_string(), true)
    }

    #[test]
    fn parses_apps_and_skips_hidden_ours_and_nonapps() {
        let fixtures = vec![
            entry(
                "gcompris-qt.desktop",
                "[Desktop Entry]\nType=Application\nName=GCompris\nExec=gcompris-qt %U\n",
            ),
            entry(
                "hidden.desktop",
                "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n",
            ),
            entry(
                "charter-learn-khan-academy.desktop",
                "[Desktop Entry]\nType=Application\nName=Khan Academy\nExec=/usr/bin/chromium\n",
            ),
            entry(
                "link.desktop",
                "[Desktop Entry]\nType=Link\nName=A link\nURL=https://x\n",
            ),
            entry(
                "flatpak-app.desktop",
                "[Desktop Entry]\nType=Application\nName=Tux Paint\nExec=/usr/bin/flatpak run org.tuxpaint.Tuxpaint\nX-Flatpak=org.tuxpaint.Tuxpaint\n",
            ),
        ];
        let inv = build_inventory(fixtures.into_iter(), |p| p == "/usr/bin/gcompris-qt");
        let pkgs: Vec<&str> = inv.iter().map(|a| a.pkg.as_str()).collect();
        assert_eq!(pkgs, vec!["/usr/bin/gcompris-qt", "org.tuxpaint.Tuxpaint"]);
        assert_eq!(inv[0].label, "GCompris");
        // Root-scan entries stay unflagged.
        assert_eq!(inv[0].user_installed, None);
        assert_eq!(inv[1].user_installed, None);
    }

    #[test]
    fn dedups_by_identity_and_drops_unresolvable() {
        let fixtures = vec![
            entry(
                "a.desktop",
                "[Desktop Entry]\nType=Application\nName=A\nExec=/usr/bin/tool\n",
            ),
            entry(
                "b.desktop",
                "[Desktop Entry]\nType=Application\nName=B copy\nExec=/usr/bin/tool --flag\n",
            ),
            entry(
                "c.desktop",
                "[Desktop Entry]\nType=Application\nName=C\nExec=not-installed-anywhere\n",
            ),
        ];
        let inv = build_inventory(fixtures.into_iter(), |p| p == "/usr/bin/tool");
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].pkg, "/usr/bin/tool");
    }

    #[test]
    fn desktop_actions_do_not_override_main_group() {
        let body = "[Desktop Entry]\nType=Application\nName=Real\nExec=/usr/bin/real\n\
                    [Desktop Action new-window]\nName=New Window\nExec=/usr/bin/other\n";
        let app = parse_desktop_entry("x.desktop", body).unwrap();
        assert_eq!(app.label, "Real");
        assert_eq!(app.pkg, "/usr/bin/real");
    }

    // ---- §2.4: user-installed flagging -------------------------------------

    /// A `~/.local/share/applications` entry (Prism Launcher, say) is flagged
    /// `userInstalled: true`; a root-owned entry alongside it is not.
    #[test]
    fn user_dir_entries_are_flagged_and_root_entries_are_not() {
        let fixtures = vec![
            entry(
                "system-app.desktop",
                "[Desktop Entry]\nType=Application\nName=System App\nExec=/usr/bin/sysapp\n",
            ),
            user_entry(
                "prismlauncher.desktop",
                "[Desktop Entry]\nType=Application\nName=Prism Launcher\nExec=/managed/ward/.local/bin/prismlauncher\n",
            ),
        ];
        let inv = build_inventory(fixtures.into_iter(), |p| {
            p == "/usr/bin/sysapp" || p == "/managed/ward/.local/bin/prismlauncher"
        });
        let sys = inv.iter().find(|a| a.label == "System App").unwrap();
        let prism = inv.iter().find(|a| a.label == "Prism Launcher").unwrap();
        assert_eq!(sys.user_installed, None);
        assert_eq!(prism.user_installed, Some(true));
    }

    /// Every existing skip rule (hidden / non-Application / one of ours / no
    /// Exec) still applies to a USER-dir entry — a ward cannot smuggle a hidden
    /// or self-authored "charter-*" entry into the inventory just because it
    /// came from their own writable dir.
    #[test]
    fn skip_rules_still_apply_to_user_dir_entries() {
        let fixtures = vec![
            user_entry(
                "hidden.desktop",
                "[Desktop Entry]\nType=Application\nName=Hidden\nExec=hidden\nNoDisplay=true\n",
            ),
            user_entry(
                "charter-fake.desktop",
                "[Desktop Entry]\nType=Application\nName=Fake\nExec=/usr/bin/fake\n",
            ),
            user_entry(
                "link.desktop",
                "[Desktop Entry]\nType=Link\nName=A link\nURL=https://x\n",
            ),
            user_entry(
                "noexec.desktop",
                "[Desktop Entry]\nType=Application\nName=NoExec\n",
            ),
        ];
        let inv = build_inventory(fixtures.into_iter(), |_| true);
        assert!(
            inv.is_empty(),
            "every fixture above must be skipped, got {inv:?}"
        );
    }

    /// A user-dir flatpak export (`flatpak install --user`) is flagged too —
    /// not just native `.desktop` entries with an absolute Exec.
    #[test]
    fn a_user_dir_flatpak_export_is_flagged() {
        let fixtures = vec![user_entry(
            "org.prismlauncher.PrismLauncher.desktop",
            "[Desktop Entry]\nType=Application\nName=Prism Launcher\nExec=/usr/bin/flatpak run org.prismlauncher.PrismLauncher\nX-Flatpak=org.prismlauncher.PrismLauncher\n",
        )];
        // A realistic `exists`: no machine has a `/usr/bin/<flatpak app id>`.
        // (`|_| true` used to pass here, but a blanket-yes probe now answers
        // the binary-dir lookup that runs FIRST — see `resolve_exec`, 03b-B5.)
        let inv = build_inventory(fixtures.into_iter(), |p| p == "/usr/bin/flatpak");
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].pkg, "org.prismlauncher.PrismLauncher");
        assert_eq!(inv[0].user_installed, Some(true));
    }

    /// When the SAME resolved identity appears from both a root dir and a
    /// user dir, the root (unflagged, first-seen) entry wins — a ward cannot
    /// launder a root-owned identity's trust by dropping their own copy of
    /// the same `.desktop` file into their writable dir.
    #[test]
    fn a_root_identity_is_not_shadowed_by_a_same_identity_user_copy() {
        let fixtures = vec![
            entry(
                "real.desktop",
                "[Desktop Entry]\nType=Application\nName=Real\nExec=/usr/bin/real\n",
            ),
            user_entry(
                "real-copy.desktop",
                "[Desktop Entry]\nType=Application\nName=Real Copy\nExec=/usr/bin/real\n",
            ),
        ];
        let inv = build_inventory(fixtures.into_iter(), |p| p == "/usr/bin/real");
        assert_eq!(inv.len(), 1);
        assert_eq!(inv[0].label, "Real");
        assert_eq!(inv[0].user_installed, None);
    }
}

/// C1 (CRITICAL): a ward-writable scan dir is ADVERSARIAL input. These run
/// against the real filesystem (mkfifo, a real symlink, a real oversized
/// file) rather than the pure `build_inventory` fixtures above, since the
/// whole point is the file-type/size checks `read_desktop_file_safely` does
/// BEFORE `build_inventory` ever sees a string.
#[cfg(all(test, feature = "real"))]
mod safety_tests {
    use super::*;
    use std::io::Write;
    use std::sync::mpsc;
    use std::time::Duration;

    fn tmp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "charterd-app-inventory-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    /// Runs `f` on a background thread and FAILS the test rather than hanging
    /// the whole suite if it doesn't return promptly — the entire point of
    /// these tests is that a regression here would otherwise block forever
    /// (C1), so the test itself must not be able to hang with it.
    fn with_timeout<F: FnOnce() -> Option<String> + Send + 'static>(f: F) -> Option<String> {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("read_desktop_file_safely must never block indefinitely")
    }

    /// `mkfifo ~/.local/share/applications/x.desktop` — a plain `open()` for
    /// reading would block FOREVER with no writer. Refused three times over
    /// without ever blocking this test: `metadata`'s file-type check (a FIFO
    /// is not `is_file()`, and `stat` on a FIFO path does not block — only
    /// `open` does), then `O_NONBLOCK`, then the `fstat` on the opened fd.
    #[test]
    fn a_fifo_is_refused_without_blocking() {
        let dir = tmp_dir("fifo");
        let path = dir.join("x.desktop");
        let cpath = std::ffi::CString::new(path.to_str().expect("utf8 path")).unwrap();
        let rc = unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        assert_eq!(with_timeout(move || read_desktop_file_safely(&path)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ln -s /dev/zero x.desktop` — the OOM variant. Following the link
    /// (which is what we now do, so real flatpak exports work) lands on a
    /// CHARACTER DEVICE: `is_file()` is false, so it is refused before any
    /// read is attempted at all. Following costs nothing here.
    #[test]
    fn a_symlink_to_dev_zero_is_refused() {
        let dir = tmp_dir("symlink");
        let path = dir.join("x.desktop");
        std::os::unix::fs::symlink("/dev/zero", &path).expect("symlink");
        assert_eq!(with_timeout(move || read_desktop_file_safely(&path)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A real file, but too large to be a genuine `.desktop` entry — refused
    /// by the metadata size check before the capped read even has to trigger.
    #[test]
    fn an_oversized_ordinary_file_is_refused() {
        let dir = tmp_dir("oversize");
        let path = dir.join("x.desktop");
        let mut f = std::fs::File::create(&path).expect("create");
        let big = vec![b'a'; (MAX_DESKTOP_FILE_BYTES + 1) as usize];
        f.write_all(&big).expect("write");
        assert_eq!(with_timeout(move || read_desktop_file_safely(&path)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A FIFO reached THROUGH a symlink — the race the `fstat`-the-opened-fd
    /// re-check exists for, expressed as a static shape. Following the link
    /// lands on a FIFO, which is refused just like a bare one, and (belt and
    /// braces) `O_NONBLOCK` means even reaching the open cannot hang.
    #[test]
    fn a_symlink_to_a_fifo_is_refused_without_blocking() {
        let dir = tmp_dir("symlink-fifo");
        let fifo = dir.join("real.fifo");
        let cpath = std::ffi::CString::new(fifo.to_str().expect("utf8 path")).unwrap();
        let rc = unsafe { libc::mkfifo(cpath.as_ptr(), 0o600) };
        assert_eq!(rc, 0, "mkfifo failed: {}", std::io::Error::last_os_error());
        let path = dir.join("x.desktop");
        std::os::unix::fs::symlink(&fifo, &path).expect("symlink");
        assert_eq!(with_timeout(move || read_desktop_file_safely(&path)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The ordinary case must keep working — these checks refuse the
    /// adversarial shapes, not real `.desktop` files.
    #[test]
    fn an_ordinary_desktop_file_reads_normally() {
        let dir = tmp_dir("ordinary");
        let path = dir.join("x.desktop");
        std::fs::write(
            &path,
            "[Desktop Entry]\nType=Application\nName=X\nExec=/usr/bin/x\n",
        )
        .expect("write");
        let content = with_timeout(move || read_desktop_file_safely(&path));
        assert!(content.is_some_and(|c| c.contains("Name=X")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// N1 (CRITICAL, round 2): **a LEGITIMATE symlink must be ingested.**
    /// Every `.desktop` entry flatpak exports — system AND `--user` — is a
    /// symlink into `.../app/<id>/current/active/export/...`. A round of
    /// hardening that refused symlinks outright passed its whole safety suite
    /// (which only ever tried adversarial shapes) while silently deleting
    /// every flatpak from the device inventory. This is the missing
    /// legitimate case.
    #[test]
    fn a_symlinked_desktop_file_is_ingested() {
        let dir = tmp_dir("symlink-ok");
        let target = dir.join("target.desktop");
        std::fs::write(
            &target,
            "[Desktop Entry]\nType=Application\nName=Linked\nExec=/usr/bin/linked\n",
        )
        .expect("write");
        let path = dir.join("x.desktop");
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        let content = with_timeout(move || read_desktop_file_safely(&path));
        assert!(
            content.is_some_and(|c| c.contains("Name=Linked")),
            "a symlinked .desktop (i.e. every flatpak export) must be read"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A symlink whose TARGET is oversized is still refused — following the
    /// link means judging the target's real size, so the OOM bound survives.
    #[test]
    fn a_symlink_to_an_oversized_file_is_refused() {
        let dir = tmp_dir("symlink-big");
        let target = dir.join("big");
        let mut f = std::fs::File::create(&target).expect("create");
        f.write_all(&vec![b'a'; (MAX_DESKTOP_FILE_BYTES + 1) as usize])
            .expect("write");
        let path = dir.join("x.desktop");
        std::os::unix::fs::symlink(&target, &path).expect("symlink");
        assert_eq!(with_timeout(move || read_desktop_file_safely(&path)), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// N1, end to end and at the level that actually matters: a `flatpak
    /// install --user` export (a SYMLINK, in the ward's own writable dir)
    /// must reach `focus::user_installed_ids`. That set is the ONLY signal
    /// separating a real flatpak from a `--user` impostor of the same app id
    /// (identical cgroup scope, identical root-owned `bwrap`), so an empty
    /// set is not a cosmetic inventory gap — it is the §2.4 free-learning-time
    /// hole reopening.
    #[test]
    fn a_user_flatpak_export_symlink_reaches_user_installed_ids() {
        let home = tmp_dir("user-flatpak-home");
        let exports = home.join(".local/share/flatpak/exports/share/applications");
        std::fs::create_dir_all(&exports).expect("create export dir");
        // The real file lives in the flatpak "repo"; the export is a symlink
        // to it, exactly as `flatpak install --user` lays it out.
        let repo = home.join(".local/share/flatpak/app/x/current/active/export");
        std::fs::create_dir_all(&repo).expect("create repo dir");
        let target = repo.join("org.forgesworn.TestImpostor.desktop");
        std::fs::write(
            &target,
            "[Desktop Entry]\nType=Application\nName=Test Impostor\n\
             Exec=/usr/bin/flatpak run org.forgesworn.TestImpostor\n\
             X-Flatpak=org.forgesworn.TestImpostor\n",
        )
        .expect("write");
        std::os::unix::fs::symlink(&target, exports.join("org.forgesworn.TestImpostor.desktop"))
            .expect("symlink");

        let home_str = home.to_str().expect("utf8 home").to_string();
        let inv = scan(&[home_str]);
        let ids = crate::focus::user_installed_ids(&inv);
        assert!(
            ids.contains("org.forgesworn.TestImpostor"),
            "a --user flatpak export (a symlink) must land in user_installed_ids, got {ids:?}"
        );
        let _ = std::fs::remove_dir_all(&home);
    }

    // ---- 03b-B5: a versioned command name is not a flatpak app id ----

    /// The finding. `Exec=gimp-2.10 %U` has a dot and no slash, so the old
    /// branch handed it back as a flatpak app id, unverified — and every rule
    /// naming that app then looked for a `flatpak-gimp-2.10-` cgroup no native
    /// process can carry.
    #[test]
    fn a_versioned_command_name_resolves_to_its_binary_not_a_flatpak_id() {
        for (name, dir) in [
            ("gimp-2.10", "/usr/bin"),
            ("lua5.4", "/usr/bin"),
            ("openttd-1.2", "/usr/games"),
        ] {
            let full = format!("{dir}/{name}");
            let got = resolve_exec(name, |p| p == full);
            assert_eq!(
                got.as_deref(),
                Some(full.as_str()),
                "{name} is a binary on this machine, not an app id"
            );
        }
    }

    /// A real flatpak app id still resolves as one — nothing on disk answers
    /// for it, and it has the reverse-DNS shape.
    #[test]
    fn a_real_flatpak_app_id_still_resolves_as_itself() {
        for id in [
            "org.tuxpaint.Tuxpaint",
            "com.valvesoftware.Steam",
            "org.gnome.gedit",
            "io.github.some_app.Thing",
        ] {
            assert_eq!(
                resolve_exec(id, |_| false).as_deref(),
                Some(id),
                "{id} has the shape of an app id"
            );
        }
    }

    /// A binary on disk WINS over the app-id reading, which is the reorder
    /// that actually closes the finding.
    #[test]
    fn a_binary_on_disk_beats_the_app_id_reading() {
        assert_eq!(
            resolve_exec("org.foo.Bar", |p| p == "/usr/bin/org.foo.Bar").as_deref(),
            Some("/usr/bin/org.foo.Bar")
        );
    }

    /// Unresolvable stays dropped — an identity we cannot verify is still not
    /// offered for picking, and the shape check must not become a way to mint
    /// one from any dotted string.
    #[test]
    fn a_dotted_name_that_is_neither_a_binary_nor_app_id_shaped_is_dropped() {
        for name in [
            "gimp-2.10", // two segments, second starts with a digit
            "lua5.4",
            "foo.bar",     // only two segments
            "2go.foo.bar", // a segment starting with a digit
            "a..b.c",      // an empty segment
            "x.y.z!",      // a character an app id may not contain
        ] {
            assert_eq!(
                resolve_exec(name, |_| false),
                None,
                "{name} must be dropped"
            );
        }
    }

    /// A bare name now also resolves out of `/usr/local/bin`, which a locally
    /// built launcher routinely points at.
    #[test]
    fn a_bare_name_resolves_from_the_wider_binary_path() {
        assert_eq!(
            resolve_exec("mything", |p| p == "/usr/local/bin/mything").as_deref(),
            Some("/usr/local/bin/mything")
        );
    }

    // ---- 03b-G3: the scan set ----

    /// snap and `/usr/local` are where Ubuntu/Mint actually keep Firefox,
    /// Chromium, Thunderbird and Steam. Missing them is fail-OPEN under the
    /// allowlist posture, and invisible to the guardian besides.
    #[test]
    fn the_scan_set_includes_snap_and_usr_local() {
        let dirs = all_scan_dirs(&[]);
        let paths: Vec<&str> = dirs.iter().map(|(d, _)| d.as_str()).collect();
        for want in [
            "/usr/share/applications",
            "/var/lib/flatpak/exports/share/applications",
            "/var/lib/snapd/desktop/applications",
            "/usr/local/share/applications",
        ] {
            assert!(
                paths.contains(&want),
                "{want} must be scanned, got {paths:?}"
            );
            assert_eq!(
                dirs.iter().filter(|(d, _)| d == want).count(),
                1,
                "{want} is scanned exactly once"
            );
            assert_eq!(
                dirs.iter().find(|(d, _)| d == want).map(|(_, f)| *f),
                Some(false),
                "{want} is a root dir, so its entries are unflagged"
            );
        }
        let home_dir = "/managed/ward/.local/share/applications";
        let with_home = all_scan_dirs(&["/managed/ward".to_string()]);
        assert_eq!(
            with_home
                .iter()
                .find(|(d, _)| d == home_dir)
                .map(|(_, f)| *f),
            Some(true),
            "a per-home mirror is still flagged"
        );
    }

    /// A non-root-owned `XDG_DATA_DIRS` entry is scanned but FLAGGED, exactly
    /// as a per-home mirror is — it is writable by somebody who is not root,
    /// so its entries are not an inventory the ward cannot have altered.
    #[test]
    fn an_xdg_data_dir_is_scanned_and_a_non_root_one_is_flagged() {
        let d = tmp_dir("xdg-scan");
        let owned = d.join("mine");
        std::fs::create_dir_all(owned.join("applications")).expect("create");
        // Read from an explicit value, not the process-global env var — this
        // suite runs in parallel and several of these would race on it.
        let dirs = xdg_dirs_from(&format!("{}:/usr/share:relative:", owned.display()));

        let mine = format!("{}/applications", owned.display());
        assert_eq!(
            dirs.iter().find(|(p, _)| *p == mine).map(|(_, f)| *f),
            Some(true),
            "a dir this test's own (non-root) user owns is flagged: {dirs:?}"
        );
        assert_eq!(
            dirs.iter()
                .find(|(p, _)| p == "/usr/share/applications")
                .map(|(_, f)| *f),
            Some(false),
            "a root-owned XDG entry ranks with the fixed roots"
        );
        assert!(
            dirs.iter().all(|(p, _)| p.starts_with('/')),
            "a relative or empty XDG entry is not a path and is dropped: {dirs:?}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An XDG entry repeating a fixed root must not be scanned twice — the
    /// default value contains `/usr/share` and `/usr/local/share`.
    #[test]
    fn an_xdg_entry_repeating_a_fixed_root_is_deduped() {
        let xdg = xdg_dirs_from("/usr/local/share:/usr/share");
        assert_eq!(xdg.len(), 2);
        // `all_scan_dirs` dedupes by path, so the fixed roots keep their
        // (unflagged, first-seen) place whatever XDG repeats.
        let dirs = all_scan_dirs(&[]);
        assert_eq!(
            dirs.iter()
                .filter(|(p, _)| p == "/usr/share/applications")
                .count(),
            1
        );
    }

    /// No `XDG_DATA_DIRS` value at all adds nothing.
    #[test]
    fn an_absent_xdg_data_dirs_adds_nothing() {
        assert!(xdg_dirs_from("").is_empty());
    }
}
