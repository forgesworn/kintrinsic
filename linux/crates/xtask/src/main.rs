//! `xtask` — build / package automation. Subcommands land with the phases that
//! need them. `deb` (Phase 9) assembles the installable Kintrinsic package:
//! builds the `real` binaries, stages the Debian tree from `packaging/`, and
//! emits a `dpkg-deb`-built `.deb`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    let task = env::args().nth(1).unwrap_or_default();
    let result = match task.as_str() {
        "deb" => build_deb(),
        "" => {
            eprintln!("usage: xtask <deb>");
            return;
        }
        other => {
            eprintln!("xtask: unknown task '{other}'");
            std::process::exit(2);
        }
    };
    if let Err(e) = result {
        eprintln!("xtask deb: error: {e}");
        std::process::exit(1);
    }
}

/// Workspace root (`linux/`) — two levels above this crate's manifest dir.
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("crates/xtask -> workspace root")
        .to_path_buf()
}

/// dpkg architecture (`amd64`, `arm64`, …); falls back to `amd64`.
fn dpkg_arch() -> String {
    Command::new("dpkg")
        .arg("--print-architecture")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "amd64".to_string())
}

/// The Debian `control` file. Pure so it can be unit-tested without packaging.
fn control_file(version: &str, arch: &str) -> String {
    format!(
        "Package: kintrinsic\n\
         Version: {version}\n\
         Architecture: {arch}\n\
         Maintainer: Forgesworn\n\
         Section: admin\n\
         Priority: optional\n\
         Conflicts: charter\n\
         Replaces: charter\n\
         Provides: charter\n\
         Homepage: https://kintrinsic.app\n\
         Depends: systemd, dbus, policykit-1, zenity, qrencode, libwebkit2gtk-4.1-0, x11-utils\n\
         Suggests: chromium | chromium-browser | google-chrome-stable\n\
         Recommends: fapolicyd, flatpak\n\
         Description: Kintrinsic for Linux — guardian-chartered device warden\n\
         \x20charterd is the privileged broker spine: it verifies guardian-signed\n\
         \x20grants and enforces schedule/budget limits, app-install brokering, and\n\
         \x20execution lockdown. Works standalone via device-only limits (set in\n\
         \x20the Screen Time settings app, no phone needed). Ships the systemd\n\
         \x20service, D-Bus policy, polkit rules, fapolicyd policy, noexec mount\n\
         \x20units, the charter CLI, charter-lock, charter-xclients,\n\
         \x20charter-settings, and charter-setup. Run 'charter-setup <user>' to\n\
         \x20lock down an account.\n"
    )
}

/// Copy `src` -> `dst`, creating parent dirs. Optionally mark executable.
fn stage(src: &Path, dst: &Path, exec: bool) -> Result<(), String> {
    let parent = dst.parent().ok_or("dst has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    fs::copy(src, dst).map_err(|e| format!("copy {} -> {}: {e}", src.display(), dst.display()))?;
    let mode = if exec { 0o755 } else { 0o644 };
    fs::set_permissions(dst, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", dst.display()))?;
    Ok(())
}

/// Write `contents` to `dst`, creating parent dirs and setting `mode`.
fn write_mode(dst: &Path, contents: &str, mode: u32) -> Result<(), String> {
    let parent = dst.parent().ok_or("dst has no parent")?;
    fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    fs::write(dst, contents).map_err(|e| format!("write {}: {e}", dst.display()))?;
    fs::set_permissions(dst, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("chmod {}: {e}", dst.display()))?;
    Ok(())
}

fn build_deb() -> Result<(), String> {
    let root = workspace_root();
    let pkg = root.join("packaging");
    let target = root.join("target");
    let arch = dpkg_arch();

    // 1. Build the real binaries: charterd runs the live subscribe -> verify ->
    //    enact -> enforce loop, charter is the live D-Bus CLI, charter-lock is the
    //    on-display lock. The OS-touching tail (cgroup/fapolicyd/VT/grab) is
    //    exercised on the Mint VM; everything compiles + the cores are tested.
    eprintln!("xtask deb: building real release binaries…");
    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(&root)
        .args([
            "build",
            "--release",
            "--workspace",
            "--no-default-features",
            "--features",
            "real",
        ])
        .status()
        .map_err(|e| format!("spawn cargo: {e}"))?;
    if !status.success() {
        return Err("cargo build failed".into());
    }

    // 1b. Build the unified desktop app (charter-console), a standalone crate at
    //     repo-root apps/ — it needs WebKitGTK dev libs, so it lives OUTSIDE the
    //     linux/ workspace and is never touched by the headless CI gate. The
    //     webkit toolchain must be on PATH/PKG_CONFIG when running `xtask deb`.
    let console_dir = root
        .parent()
        .ok_or("repo root above linux/")?
        .join("apps/charter-console");
    eprintln!("xtask deb: building charter-console (the unified app)…");
    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(&console_dir)
        .args(["build", "--release"])
        .status()
        .map_err(|e| format!("spawn cargo (charter-console): {e}"))?;
    if !status.success() {
        return Err(
            "charter-console build failed — is the WebKitGTK toolchain available? \
             (PKG_CONFIG_PATH / libwebkit2gtk-4.1-dev)"
                .into(),
        );
    }

    // 1c. Build the Mint tray shell — same story as charter-console: it needs
    //     GTK dev libs the headless gate host doesn't have, so it lives outside
    //     the linux/ workspace and is only ever built here. `charter-tray`
    //     execs it on Cinnamon/MATE/XFCE, where the panel can only open a
    //     WINDOW from a generic tray icon; through Mint's own icon API a left
    //     click opens a menu the desktop draws itself.
    let xapp_dir = root
        .parent()
        .ok_or("repo root above linux/")?
        .join("apps/charter-tray-xapp");
    eprintln!("xtask deb: building charter-tray-xapp (the Mint tray shell)…");
    let status = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(&xapp_dir)
        .args(["build", "--release"])
        .status()
        .map_err(|e| format!("spawn cargo (charter-tray-xapp): {e}"))?;
    if !status.success() {
        return Err(
            "charter-tray-xapp build failed — is the GTK3 toolchain available? \
             (PKG_CONFIG_PATH / libgtk-3-dev)"
                .into(),
        );
    }

    // 2. Fresh staging tree.
    let stage_dir = target.join(format!("deb/kintrinsic_{VERSION}_{arch}"));
    if stage_dir.exists() {
        fs::remove_dir_all(&stage_dir).map_err(|e| format!("clean staging: {e}"))?;
    }

    // 3. Binaries.
    let rel = target.join("release");
    stage(
        &rel.join("charterd"),
        &stage_dir.join("usr/sbin/charterd"),
        true,
    )?;
    stage(
        &rel.join("charter"),
        &stage_dir.join("usr/bin/charter"),
        true,
    )?;
    stage(
        &rel.join("charter-lock"),
        &stage_dir.join("usr/bin/charter-lock"),
        true,
    )?;
    // The window→process probe the named time model meters from. Needs no
    // privilege (it only reads an X display charterd hands it the auth for),
    // so it installs beside charter-lock in usr/bin; charterd spawns it under
    // `timeout 2` because the display belongs to the ward.
    stage(
        &rel.join("charter-xclients"),
        &stage_dir.join("usr/bin/charter-xclients"),
        true,
    )?;
    stage(
        &rel.join("charter-tray"),
        &stage_dir.join("usr/bin/charter-tray"),
        true,
    )?;
    // The Mint shell. `charter-tray` is still the single autostart entry on
    // every desktop; it hands over to this one where it applies.
    stage(
        &xapp_dir.join("target/release/charter-tray-xapp"),
        &stage_dir.join("usr/bin/charter-tray-xapp"),
        true,
    )?;
    stage(
        &rel.join("charter-settings"),
        &stage_dir.join("usr/sbin/charter-settings"),
        true,
    )?;
    stage(
        &pkg.join("setup/charter-settings-launch"),
        &stage_dir.join("usr/bin/charter-settings-launch"),
        true,
    )?;
    stage(
        &pkg.join("setup/charter-setup"),
        &stage_dir.join("usr/sbin/charter-setup"),
        true,
    )?;
    stage(
        &pkg.join("setup/charter-setup-launch"),
        &stage_dir.join("usr/bin/charter-setup-launch"),
        true,
    )?;
    // On-screen device code — unprivileged (reads the public device.pub), so it
    // installs to usr/bin and runs directly (no pkexec launcher).
    stage(
        &rel.join("charter-device-code"),
        &stage_dir.join("usr/bin/charter-device-code"),
        true,
    )?;
    // Lock-out recovery — privileged (thaw/VT/disable), pkexec'd via its launcher.
    stage(
        &rel.join("charter-recovery"),
        &stage_dir.join("usr/sbin/charter-recovery"),
        true,
    )?;
    stage(
        &pkg.join("setup/charter-recovery-launch"),
        &stage_dir.join("usr/bin/charter-recovery-launch"),
        true,
    )?;
    // Graphical guardian pairing — privileged (writes pairing.json), pkexec'd.
    stage(
        &rel.join("charter-pair"),
        &stage_dir.join("usr/sbin/charter-pair"),
        true,
    )?;
    stage(
        &pkg.join("setup/charter-pair-launch"),
        &stage_dir.join("usr/bin/charter-pair-launch"),
        true,
    )?;
    // Scan-to-pair: mints the one-time token behind the pairing QR. Privileged
    // because that token is the proof of physical presence — if the ward's own
    // account could read it, the child could pair the laptop to their own
    // phone. 49-charter.rules denies them pkexec, so only a parent can run it.
    stage(
        &rel.join("charter-pair-invite"),
        &stage_dir.join("usr/sbin/charter-pair-invite"),
        true,
    )?;
    // Give / take back a ward's time at the computer itself. Privileged
    // because it moves what is enforced: 49-charter.rules denies the managed
    // account pkexec, so a ward cannot hand themselves an extra hour. This is
    // the only give/take path that works with no guardian phone paired.
    stage(
        &rel.join("charter-time"),
        &stage_dir.join("usr/sbin/charter-time"),
        true,
    )?;
    // Launches one educational site app. NOT privileged and NOT setuid: it
    // runs as whoever clicked the menu entry and does nothing they could not do
    // themselves. It exists because a .desktop Exec is not run through a shell,
    // so `$HOME` never expands — and the Chromium profile must be per-child,
    // since ONE launcher is written per app across every managed child and a
    // shared profile would mean two children sharing a Khan login. usr/lib
    // rather than usr/bin: it is Kintrinsic's own plumbing, not a command anyone
    // types.
    stage(
        &rel.join("charter-learn"),
        &stage_dir.join("usr/lib/charter/charter-learn"),
        true,
    )?;
    // The unified app — the single menu entry. Runs as the user; delegates the
    // privileged work to the helpers above via pkexec.
    stage(
        &console_dir.join("target/release/charter-console"),
        &stage_dir.join("usr/bin/charter-console"),
        true,
    )?;

    // 4. Static assets (src under packaging/, dest under the install root).
    let assets: &[(&str, &str)] = &[
        (
            "systemd/charterd.service",
            "lib/systemd/system/charterd.service",
        ),
        ("mounts/tmp.mount", "lib/systemd/system/tmp.mount"),
        ("mounts/dev-shm.mount", "lib/systemd/system/dev-shm.mount"),
        (
            "mounts/noexec.fstab.example",
            "usr/share/charter/mounts/noexec.fstab.example",
        ),
        (
            "env/charterd.env.example",
            "usr/share/charter/charterd.env.example",
        ),
        // ONE menu entry — the unified Kintrinsic app. (The old five separate
        // launchers — setup/settings/device-code/recovery/pair — are no longer
        // shown; their binaries stay, driven by charter-console via pkexec.)
        (
            "desktop/charter.desktop",
            "usr/share/applications/charter.desktop",
        ),
        // The ward tray autostarts in EVERY session: it self-presents as
        // "not watching this account" for unmanaged users and is a companion
        // surface (killing it changes nothing about enforcement).
        (
            "desktop/charter-tray.desktop",
            "etc/xdg/autostart/charter-tray.desktop",
        ),
        (
            "desktop/charter.svg",
            "usr/share/icons/hicolor/scalable/apps/charter.svg",
        ),
        (
            "dbus/org.forgesworn.charterd.conf",
            "usr/share/dbus-1/system.d/org.forgesworn.charterd.conf",
        ),
        (
            "polkit/49-charter.rules",
            "usr/share/polkit-1/rules.d/49-charter.rules",
        ),
        (
            // Shipped as a REFERENCE, not into /etc/fapolicyd/rules.d: the
            // fragment ends in a default-deny, and the safety model says the
            // .deb never ships one live. Nothing here loads or enables
            // fapolicyd, but it is a Recommends (apt installs it), so a file in
            // rules.d sat one `fagenrules --load` away from a box that cannot
            // start a desktop. Arming is charter-setup's job, opt-in, after
            // the VM round (HANDOFF Phase 11). dpkg removes the old rules.d
            // copy on upgrade (it was never a conffile).
            "fapolicyd/charter.rules",
            "usr/share/charter/fapolicyd/72-charter.rules",
        ),
    ];
    for (src, dst) in assets {
        stage(&pkg.join(src), &stage_dir.join(dst), false)?;
    }

    // 5. DEBIAN control + maintainer scripts.
    write_mode(
        &stage_dir.join("DEBIAN/control"),
        &control_file(VERSION, &arch),
        0o644,
    )?;
    for script in ["postinst", "prerm", "postrm"] {
        stage(
            &pkg.join("debian").join(script),
            &stage_dir.join("DEBIAN").join(script),
            true,
        )?;
    }

    // 6. Build the .deb with deterministic root ownership (no fakeroot needed).
    let out = target.join(format!("deb/kintrinsic_{VERSION}_{arch}.deb"));
    eprintln!("xtask deb: packing {}…", out.display());
    let status = Command::new("dpkg-deb")
        .args(["--root-owner-group", "--build"])
        .arg(&stage_dir)
        .arg(&out)
        .status()
        .map_err(|e| format!("spawn dpkg-deb: {e}"))?;
    if !status.success() {
        return Err("dpkg-deb --build failed".into());
    }

    eprintln!("xtask deb: built {}", out.display());
    println!("{}", out.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn control_has_required_fields() {
        let c = control_file("0.1.0", "amd64");
        for field in [
            "Package: kintrinsic",
            "Version: 0.1.0",
            "Architecture: amd64",
        ] {
            assert!(c.contains(field), "control missing `{field}`:\n{c}");
        }
        // The rename trio: a laptop with the old `charter` package installed
        // must upgrade to `kintrinsic` in one `apt install` with state kept
        // (postrm only clears /var/lib/charter on purge, which apt's
        // conflict-removal never runs).
        for field in [
            "Conflicts: charter",
            "Replaces: charter",
            "Provides: charter",
        ] {
            assert!(
                c.contains(field),
                "control missing rename field `{field}`:\n{c}"
            );
        }
        // Continuation lines for the multi-line Description must start with a space.
        for line in c
            .lines()
            .skip_while(|l| !l.starts_with("Description:"))
            .skip(1)
        {
            assert!(
                line.starts_with(' '),
                "description continuation must be space-prefixed: {line:?}"
            );
        }
        // Trailing newline (dpkg-deb requires it).
        assert!(c.ends_with('\n'));
    }

    #[test]
    fn workspace_root_holds_packaging() {
        assert!(workspace_root().join("packaging").is_dir());
    }
}
