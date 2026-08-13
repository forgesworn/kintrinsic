//! Launcher-aware process identity: "is this process Minecraft, or something
//! Minecraft started?"
//!
//! Almost no real game is the process a parent would name. The official
//! Minecraft launcher (`minecraft-launcher`) spawns the game as a **Java**
//! process; Steam spawns its games; Prism, Lutris and every store-style app do
//! the same. So the window the ward is actually looking at is a CHILD of the
//! thing the guardian picked from the list.
//!
//! Matching only the named binary would be quietly useless in both directions:
//! the meter would count roughly nothing (the launcher window is never in
//! front), and the sweep would kill the launcher while the game carried on.
//! Matching the child's own binary instead is *worse* — that binary is `java`,
//! and putting `java` in a bucket would sweep the ward's homework tools in with
//! their games.
//!
//! So identity walks UP the process tree: a process belongs to whatever bucket
//! one of its ancestors belongs to. The guardian picks "Minecraft" by name and
//! never learns any of this exists.

/// One process's identity, as read from `/proc`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProcId {
    pub ppid: u32,
    pub exe: Option<String>,
    /// Owner uid of the resolved `exe` FILE (not the process's owner uid) —
    /// root-owned (`0`) is the enforcement-grade signal a ward cannot forge
    /// by planting or renaming their own binary. Needed so
    /// [`crate::site_app::is_sanctioned`] can require an enforcement-grade
    /// runtime even when checked against an ANCESTOR (a renderer's sanction
    /// is inherited from its browser process, which must itself be the real,
    /// root-owned Chromium — see the ordering-hazard-adjacent bug this
    /// closes in `site_app`).
    pub exe_uid: Option<u32>,
    /// NUL-split `/proc/<pid>/cmdline`. Carried in full for two reasons: argv0
    /// is the only surviving identity for a wrapper that `exec -a`s its real
    /// binary (Google Chrome does exactly this — see
    /// [`crate::app_rules::pkg_matches_process`]), and the whole line is what
    /// [`crate::site_app::is_sanctioned`] reads when deciding whether a
    /// Chromium ancestor makes this process an educational window.
    pub cmdline: Vec<String>,
    pub cgroup: Option<String>,
}

impl ProcId {
    /// First cmdline token, if any. Empty is not an identity — a kernel thread
    /// has no cmdline at all.
    pub fn argv0(&self) -> Option<&str> {
        self.cmdline
            .first()
            .map(String::as_str)
            .filter(|a| !a.is_empty())
    }
}

/// How far up the tree to look. Deep enough for launcher → shell → wrapper →
/// game (and Steam's reaper chain), shallow enough that a runaway or cyclic
/// parent chain can never spin the tick loop.
pub const MAX_DEPTH: usize = 12;

/// Does `pid`, or any of its ancestors, match `pkg`?
///
/// `lookup` returns a process's identity, or `None` when it has gone (the tree
/// is read live and racy by nature — a vanished parent just ends the walk).
/// Terminates on pid 0/1, on a missing entry, on a self-parent cycle, or at
/// [`MAX_DEPTH`].
pub fn matches_with_ancestors<F>(pid: u32, pkg: &str, lookup: F) -> bool
where
    F: Fn(u32) -> Option<ProcId>,
{
    has_ancestor_matching(pid, lookup, |id| {
        // Each ancestor node has its own cmdline, so the join is necessarily
        // per-node here (there is no single "the process" to hoist it to,
        // unlike the direct-match callers) — still computed only once per
        // node actually inspected, never more.
        let joined = id.cmdline.join(" ");
        crate::app_rules::pkg_matches_process(
            pkg,
            id.exe.as_deref(),
            id.argv0(),
            &id.cmdline,
            &joined,
            id.cgroup.as_deref(),
        )
    })
}

/// [`matches_with_ancestors`], but each node is judged by
/// [`crate::app_rules::pkg_names_process_exactly`] — an exact equality on the
/// node's kernel-resolved `exe` path, and nothing else (argv0, argv and the
/// cgroup are never consulted; they are ward-writable for every process the
/// ward starts, ROOT-OWNED ANCESTORS INCLUDED — a `bash` parent wearing a
/// forged needle in its argv and a hand-made `app-flatpak-*` scope is one
/// unprivileged `systemd-run` away). Used ONLY where a match SPARES rather
/// than restricts (the §2.3 unrecognised counter).
///
/// The walk has to be exactly as strict as the direct test, or it becomes the
/// way around it: a ward's forged process would merely have to be STARTED BY
/// something wearing the forged identity.
pub fn names_with_ancestors_exactly<F>(pid: u32, pkg: &str, lookup: F) -> bool
where
    F: Fn(u32) -> Option<ProcId>,
{
    has_ancestor_matching(pid, lookup, |id| {
        crate::app_rules::pkg_names_process_exactly(pkg, id.exe.as_deref())
    })
}

/// Does `pid`, or any of its ancestors, satisfy `pred`?
///
/// The generic form of [`matches_with_ancestors`]. Chromium's renderer and GPU
/// processes carry argv of Chromium's own devising, bearing no resemblance to a
/// launch line, so "is this an educational window?" can only be answered by
/// asking whether some ancestor is one.
///
/// Same termination guarantees: pid 0/1, a missing entry, a self-parent cycle,
/// or [`MAX_DEPTH`]. A hostile or broken tree must never spin the tick loop.
pub fn has_ancestor_matching<F, P>(pid: u32, lookup: F, pred: P) -> bool
where
    F: Fn(u32) -> Option<ProcId>,
    P: Fn(&ProcId) -> bool,
{
    let mut current = pid;
    let mut seen = Vec::with_capacity(MAX_DEPTH);
    for _ in 0..MAX_DEPTH {
        if current <= 1 || seen.contains(&current) {
            return false;
        }
        seen.push(current);
        let Some(id) = lookup(current) else {
            return false;
        };
        if pred(&id) {
            return true;
        }
        current = id.ppid;
    }
    false
}

/// Strip the kernel's `" (deleted)"` suffix a `/proc/<pid>/exe` link reads
/// back with once the file it points to has been unlinked — which an
/// ORDINARY PACKAGE UPGRADE does to a binary held open by a still-running
/// process. Pure string logic, factored out of [`read_exe_link`] so it is
/// unit-tested directly rather than only through a live `/proc` read.
///
/// Only [`read_exe_link`] (real-only) and this file's own tests call it, so a
/// plain mock/default build (no `real` feature, not `cfg(test)`) has zero
/// callers — gated the same way rather than `#[allow(dead_code)]`, so a
/// genuinely orphaned function would still warn.
#[cfg(any(feature = "real", test))]
fn strip_deleted_suffix(s: String) -> String {
    s.strip_suffix(" (deleted)")
        .map(str::to_string)
        .unwrap_or(s)
}

/// Read `/proc/<pid>/exe`'s resolved target, stripped via
/// [`strip_deleted_suffix`]. `None` when the link is unreadable (the process
/// has gone, or was never readable to us).
///
/// Shared by every reader of this link — [`read_proc`], the kill sweep
/// (`runtime.rs`), and the focus meter (`focus.rs`) — so all three always
/// read exactly the same string for the same pid. Before this existed,
/// `focus.rs` read the link WITHOUT stripping the suffix while the sweep
/// did: after an in-place binary replacement the meter's `exe` and the
/// sweep's `exe` would diverge for the exact same process, reopening
/// metered≠stopped through a path with nothing to do with `cmdline:` at all.
#[cfg(feature = "real")]
pub fn read_exe_link(pid: u32) -> Option<String> {
    let p = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(strip_deleted_suffix(p.to_string_lossy().into_owned()))
}

/// The owning uid of the process's REAL executable — read by `stat`ing the
/// kernel's own magic symlink `/proc/<pid>/exe`, never the path string that
/// link renders to. Root-owned (`0`) is the enforcement-grade signal used
/// throughout this codebase: a ward has no write access to a root-owned file,
/// so they cannot replace or rename their way into wearing its identity.
/// `None` when the process has gone or the link is unreadable.
///
/// # Why the pid, and not the rendered path (D2 — the mount-namespace forgery)
///
/// `/proc/<pid>/exe` is a MAGIC symlink: `readlink` renders a *path string*
/// reconstructed for our own mount namespace, but a `stat` THROUGH the link
/// is resolved by the kernel to the executable's real inode — the one the
/// process is actually running — with no path lookup involved at all.
///
/// Rendering the string and stat'ing THAT was forgeable, and cheaply.
/// Unprivileged user namespaces are enabled on the target distro
/// (`kernel.unprivileged_userns_clone=1`, the AppArmor restriction off), so
/// with no privilege whatsoever a user-namespace bind mount can overlay a
/// ward-owned file onto a root-owned path (the verbatim command is omitted
/// from this narrative, but retained in the executable security regression
/// test below).
///
/// The bind mount lives only inside that private namespace, but the exe link
/// still RENDERS as `/usr/bin/wc` to everyone outside — where that path is
/// the distro's genuine, root-owned binary. Verified live on this machine:
/// `stat` of the rendered path returned **uid 0** (forged root) while `stat`
/// of the magic symlink returned **uid 1000**, the true owner. Root ownership
/// is the hinge of three separate decisions — §2.2's free-time asymmetry,
/// `site_app::is_sanctioned`'s runtime gate, and the §2.3 counter — so a
/// forged root reading meant free time, a spared browser, and silence.
///
/// The rendered string is still exactly right for IDENTITY and display (it is
/// what the guardian ticked, and what the kill sweep matches); it is only
/// OWNERSHIP that must come from the link itself. Keep the two apart.
#[cfg(feature = "real")]
pub fn exe_owner_uid(pid: u32) -> Option<u32> {
    use std::os::unix::fs::MetadataExt;
    // `metadata` FOLLOWS the link — through the kernel's magic resolution to
    // the real executable inode, not through a path lookup. Do not "simplify"
    // this to stat the readlink result.
    std::fs::metadata(format!("/proc/{pid}/exe"))
        .ok()
        .map(|m| m.uid())
}

/// Read one process's identity off `/proc`. `None` when it has exited.
#[cfg(feature = "real")]
pub fn read_proc(pid: u32) -> Option<ProcId> {
    let dir = format!("/proc/{pid}");
    // Field 4 of /proc/<pid>/stat is PPid — but field 2 (comm) can itself
    // contain spaces and brackets, so parse from the LAST ')' rather than
    // splitting the whole line (a process named "(evil) 1 2 3" would otherwise
    // forge its own parentage).
    let stat = std::fs::read_to_string(format!("{dir}/stat")).ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    let ppid: u32 = after.split_whitespace().nth(1)?.parse().ok()?;
    let exe = read_exe_link(pid);
    // Ownership from the magic symlink itself, NOT from `exe` — see
    // `exe_owner_uid`'s docs (a bind mount in an unprivileged user namespace
    // makes the rendered path lie about who owns the binary).
    let exe_uid = exe_owner_uid(pid);
    let cmdline: Vec<String> = std::fs::read(format!("{dir}/cmdline"))
        .map(|c| {
            c.split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .unwrap_or_default();
    let cgroup = std::fs::read_to_string(format!("{dir}/cgroup")).ok();
    Some(ProcId {
        ppid,
        exe,
        exe_uid,
        cmdline,
        cgroup,
    })
}

/// D2's live regression tests: ownership must come from the kernel's magic
/// symlink, never from the path string it renders to. These run real
/// processes against real `/proc`, so they are `real`-gated like the readers
/// they exercise.
#[cfg(all(test, feature = "real"))]
mod exe_ownership_tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;
    use std::process::{Child, Command};

    /// Kill-on-drop, so a failing assertion never leaks a sleeping process.
    struct Reaped(Child);
    impl Drop for Reaped {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn home() -> String {
        std::env::var("HOME").expect("HOME")
    }

    /// Serialises the two tests below. Both stage a binary in `$HOME` and then
    /// fork/exec, and libtest runs them on threads of ONE process: when test
    /// B forks, the child inherits every fd test A has open — including the
    /// write fd to the binary A is about to exec — and `execve` returns
    /// **ETXTBSY** ("Text file busy") while any process holds the image open
    /// for writing. That made the suite fail 5 runs in 10. Overlap is the
    /// whole cause, so removing the overlap is the primary fix; the two
    /// layers in [`stage_executable`] and [`spawn_staged`] make each test
    /// robust on its own as well, since a future sibling test that forks
    /// would otherwise quietly reintroduce this.
    static STAGED_EXEC: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Copy `src` to `dst` and make certain the write fd is CLOSED before we
    /// return — no fork by anyone can inherit what no longer exists.
    /// `std::fs::copy` closes its own handles, but doing it explicitly (with
    /// an `fsync`) is what makes the ordering a stated property of this
    /// helper rather than an implementation detail we are leaning on.
    fn stage_executable(src: &str, dst: &str) {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        let bytes = std::fs::read(src).expect("read the source binary");
        let _ = std::fs::remove_file(dst);
        {
            let mut f = std::fs::File::create(dst).expect("create the staged binary");
            f.write_all(&bytes).expect("write the staged binary");
            f.sync_all().expect("flush the staged binary");
        } // <- the write fd is closed HERE, before any spawn below.
        std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o755))
            .expect("make the staged binary executable");
    }

    /// Spawn, retrying briefly on ETXTBSY. Even with the mutex and the closed
    /// fd there is a window in which some OTHER thread's `fork` holds a
    /// duplicate of a write fd it has not yet `exec`'d away, and that is not
    /// a failure of anything under test — it is the harness racing itself.
    fn spawn_staged(cmd: &mut Command) -> std::io::Result<Child> {
        for _ in 0..50 {
            match cmd.spawn() {
                Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                other => return other,
            }
        }
        cmd.spawn()
    }

    /// The uid the OLD implementation would have reported: render the link to
    /// a path string (stripping the kernel's `" (deleted)"` marker, exactly as
    /// `read_exe_link` does) and `stat` THAT path.
    fn uid_via_rendered_path(pid: u32) -> Option<u32> {
        let rendered = read_exe_link(pid)?;
        std::fs::metadata(&rendered).ok().map(|m| m.uid())
    }

    /// Sanity: for an ordinary process the two agree. If they did not, the
    /// divergence tests below would prove nothing.
    #[test]
    fn ownership_agrees_with_the_rendered_path_in_the_ordinary_case() {
        let me = std::process::id();
        assert_eq!(exe_owner_uid(me), uid_via_rendered_path(me));
        assert!(exe_owner_uid(me).is_some());
    }

    /// **Divergence 1 — the deleted binary.** A ward launches their own
    /// program and then unlinks it. The link now renders `<path> (deleted)`,
    /// and stat'ing the stripped path finds NOTHING — so the old lookup
    /// returned `None`, "ownership unknown", which §2.3 turns into *never
    /// accrues* and §2.2 turns into *not enforcement-grade*. One `rm` bought
    /// permanent silence. Stat'ing the magic symlink still reaches the real
    /// (unlinked but open) inode and reports its true owner.
    ///
    /// Fully deterministic and unprivileged — no namespaces needed.
    #[test]
    fn a_deleted_ward_binary_still_reports_its_true_owner() {
        let _serialised = STAGED_EXEC.lock().unwrap_or_else(|e| e.into_inner());
        let path = format!("{}/.charter-test-deleted-exe", home());
        stage_executable("/bin/sleep", &path);
        let child =
            Reaped(spawn_staged(Command::new(&path).arg("60")).expect("spawn the ward binary"));
        let pid = child.0.id();
        let me = unsafe { libc::getuid() };
        // Before the unlink both agree.
        assert_eq!(exe_owner_uid(pid), Some(me));

        std::fs::remove_file(&path).expect("unlink it out from under itself");

        assert!(
            read_exe_link(pid).is_some(),
            "the link still renders after the unlink"
        );
        assert_eq!(
            uid_via_rendered_path(pid),
            None,
            "the OLD path-based lookup loses ownership entirely once the file is gone"
        );
        assert_eq!(
            exe_owner_uid(pid),
            Some(me),
            "the magic symlink still reaches the real inode — a rm must not buy silence"
        );
    }

    /// **Divergence 2 — the mount-namespace forgery (NEW-1).** With no
    /// privilege at all, an unprivileged user-namespace bind mount over a
    /// root-owned path leaves the exe link RENDERING as `/usr/bin/wc` — the
    /// distro's genuine root-owned binary — to everyone outside the
    /// namespace, while the
    /// process actually runs the ward's file. The old lookup read uid 0 and
    /// handed over free time (§2.2), a spared browser
    /// (`site_app::is_sanctioned`) and silence (§2.3).
    ///
    /// # This test FAILS rather than skips when it cannot stage the forgery
    ///
    /// A security regression test that quietly reports `ok` because it never
    /// ran is worse than no test: libtest captures stderr for PASSING tests,
    /// so an `eprintln!("SKIP: …")` here is invisible, and on a hardened CI
    /// kernel (no unprivileged user namespaces) this would have printed a
    /// clean pass forever while asserting nothing at all.
    ///
    /// So it panics with the reason. A platform that genuinely cannot run it
    /// must say so out loud, once, by setting `CHARTER_ALLOW_NO_USERNS=1` —
    /// an explicit, greppable, deliberate opt-out rather than silence.
    #[test]
    fn a_bind_mounted_exe_path_cannot_forge_root_ownership() {
        let _serialised = STAGED_EXEC.lock().unwrap_or_else(|e| e.into_inner());
        let opted_out = std::env::var("CHARTER_ALLOW_NO_USERNS").as_deref() == Ok("1");
        // Cleans up the staged binary however this test leaves.
        struct Unstaged(String);
        impl Drop for Unstaged {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
            }
        }
        let evil = format!("{}/.charter-test-userns-evil", home());
        stage_executable("/bin/sleep", &evil);
        let _unstaged = Unstaged(evil.clone());
        let me = unsafe { libc::getuid() };

        let script = format!("mount --bind {evil} /usr/bin/wc && exec /usr/bin/wc 60");
        let spawned = spawn_staged(Command::new("unshare").args([
            "-Urm",
            "--propagation",
            "private",
            "sh",
            "-c",
            &script,
        ]));
        let Ok(child) = spawned else {
            assert!(
                opted_out,
                "cannot stage the userns forgery: `unshare` is not on PATH. This test \
                 guards a CRITICAL, verified-live privilege forgery (a bind mount over a \
                 distro path making a ward binary read as root-owned); it must not pass \
                 by default without running. Install util-linux, or set \
                 CHARTER_ALLOW_NO_USERNS=1 to acknowledge this platform cannot check it."
            );
            eprintln!("CHARTER_ALLOW_NO_USERNS=1: skipping the userns forgery check");
            return;
        };
        let child = Reaped(child);
        let pid = child.0.id();
        std::thread::sleep(std::time::Duration::from_millis(600));

        let rendered = read_exe_link(pid);
        if rendered.as_deref() != Some("/usr/bin/wc") {
            assert!(
                opted_out,
                "cannot stage the userns forgery: the exe rendered as {rendered:?}, not the \
                 bind-mounted path — unprivileged user namespaces look unavailable here \
                 (check kernel.unprivileged_userns_clone and \
                 apparmor_restrict_unprivileged_userns). This test guards a CRITICAL, \
                 verified-live privilege forgery and must not pass by default without \
                 running. Set CHARTER_ALLOW_NO_USERNS=1 to acknowledge this platform \
                 cannot check it."
            );
            eprintln!("CHARTER_ALLOW_NO_USERNS=1: skipping the userns forgery check");
            return;
        }

        let via_path = uid_via_rendered_path(pid);
        let via_link = exe_owner_uid(pid);

        assert_eq!(
            via_path,
            Some(0),
            "the forgery's whole point: the rendered path IS a root-owned distro binary"
        );
        assert_eq!(
            via_link,
            Some(me),
            "stat'ing the magic symlink must report the REAL executable's owner"
        );
        assert_ne!(
            via_link,
            Some(0),
            "a ward must never be able to forge root ownership of their own binary"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // ---- C3: the shared "(deleted)" strip --------------------------------

    /// The exact string the kernel appends to `/proc/<pid>/exe` once the
    /// file it resolved to has been unlinked — an ORDINARY PACKAGE UPGRADE
    /// does this to a binary a running process still holds open. Before
    /// this was factored into one shared function, `focus.rs` read this
    /// link WITHOUT stripping the suffix while the sweep did, so the
    /// meter's `exe` and the sweep's `exe` diverged for the SAME process
    /// after an upgrade — a real metered≠stopped gap.
    #[test]
    fn strip_deleted_suffix_removes_the_kernel_marker() {
        assert_eq!(
            strip_deleted_suffix("/usr/bin/chromium (deleted)".to_string()),
            "/usr/bin/chromium"
        );
    }

    /// An ordinary path (the common case) passes through unchanged.
    #[test]
    fn strip_deleted_suffix_is_a_no_op_on_an_ordinary_path() {
        assert_eq!(
            strip_deleted_suffix("/usr/bin/chromium".to_string()),
            "/usr/bin/chromium"
        );
    }

    /// A path that merely CONTAINS the marker mid-string (an unlikely but
    /// possible real filename) must not be truncated — only a TRAILING
    /// occurrence is the kernel's marker.
    #[test]
    fn strip_deleted_suffix_only_strips_a_trailing_occurrence() {
        assert_eq!(
            strip_deleted_suffix("/home/kid/my (deleted) files/run".to_string()),
            "/home/kid/my (deleted) files/run"
        );
    }

    fn tree(rows: &[(u32, u32, &str)]) -> impl Fn(u32) -> Option<ProcId> {
        let map: BTreeMap<u32, ProcId> = rows
            .iter()
            .map(|(pid, ppid, exe)| {
                (
                    *pid,
                    ProcId {
                        ppid: *ppid,
                        exe: Some((*exe).to_string()),
                        exe_uid: Some(0),
                        cmdline: vec![],
                        cgroup: None,
                    },
                )
            })
            .collect();
        move |pid| map.get(&pid).cloned()
    }

    /// The case that prompted this: the guardian picks "Minecraft" (the
    /// launcher), and the ward is looking at a Java window two levels down.
    #[test]
    fn the_game_a_launcher_started_counts_as_the_launcher() {
        let t = tree(&[
            (100, 1, "/usr/bin/minecraft-launcher"),
            (200, 100, "/bin/sh"),
            (300, 200, "/usr/lib/jvm/java-17-openjdk/bin/java"),
        ]);
        assert!(matches_with_ancestors(
            300,
            "/usr/bin/minecraft-launcher",
            &t
        ));
        // …and the launcher itself still matches directly.
        assert!(matches_with_ancestors(
            100,
            "/usr/bin/minecraft-launcher",
            &t
        ));
    }

    /// The reason we walk the tree instead of naming the child's binary: a
    /// Java process that ISN'T under the launcher must stay out of the bucket,
    /// or homework lands in the games allowance.
    #[test]
    fn an_unrelated_java_process_is_not_the_game() {
        let t = tree(&[
            (100, 1, "/usr/bin/minecraft-launcher"),
            (400, 1, "/usr/lib/jvm/java-17-openjdk/bin/java"), // e.g. an IDE
        ]);
        assert!(!matches_with_ancestors(
            400,
            "/usr/bin/minecraft-launcher",
            &t
        ));
    }

    #[test]
    fn an_unrelated_tree_matches_nothing() {
        let t = tree(&[(500, 1, "/usr/bin/libreoffice")]);
        assert!(!matches_with_ancestors(
            500,
            "/usr/bin/minecraft-launcher",
            &t
        ));
    }

    /// /proc is read live: a parent can exit mid-walk. That must end the walk,
    /// not panic or loop.
    #[test]
    fn a_vanished_ancestor_just_ends_the_walk() {
        let t = tree(&[(300, 999, "/usr/lib/jvm/bin/java")]); // 999 not in the map
        assert!(!matches_with_ancestors(
            300,
            "/usr/bin/minecraft-launcher",
            &t
        ));
    }

    #[test]
    fn init_and_kernel_pids_terminate_the_walk() {
        let t = tree(&[(1, 0, "/sbin/init"), (2, 0, "/sbin/init")]);
        assert!(!matches_with_ancestors(1, "/sbin/init", &t));
        assert!(!matches_with_ancestors(0, "/sbin/init", &t));
    }

    /// A hostile or broken tree must never spin the tick loop.
    #[test]
    fn cycles_and_deep_chains_terminate() {
        let cyclic = tree(&[(10, 11, "/a"), (11, 10, "/b")]);
        assert!(!matches_with_ancestors(
            10,
            "/usr/bin/minecraft-launcher",
            &cyclic
        ));

        // A chain longer than MAX_DEPTH gives up rather than walking forever.
        let rows: Vec<(u32, u32, &str)> = (2..40).map(|i| (i, i - 1, "/filler")).collect();
        let mut all = rows.clone();
        all.push((1, 0, "/usr/bin/minecraft-launcher"));
        assert!(!matches_with_ancestors(
            39,
            "/usr/bin/minecraft-launcher",
            tree(&all)
        ));
    }

    /// A `cmdline:` identity matches directly on the focused process itself —
    /// no ancestor is needed, and none is present here. This is the point of
    /// the identity form: unlike the launcher-path form (which only ever
    /// matches a PARENT the ward could rename or swap), the game's own
    /// argv is available at the process that has the window, so honest
    /// attribution does not depend on the tree at all.
    #[test]
    fn cmdline_identity_matches_the_process_itself_with_no_ancestor_in_the_tree() {
        let map: BTreeMap<u32, ProcId> = [(
            300u32,
            ProcId {
                ppid: 1, // parent is init — no launcher ancestor exists
                exe: Some("/usr/lib/jvm/java-21-openjdk/bin/java".into()),
                exe_uid: Some(0),
                cmdline: vec![
                    "java".into(),
                    "-cp".into(),
                    "/home/kid/.minecraft/libs/x.jar".into(),
                    "net.minecraft.client.main.Main".into(),
                ],
                cgroup: None,
            },
        )]
        .into_iter()
        .collect();
        let lookup = move |pid: u32| map.get(&pid).cloned();
        assert!(matches_with_ancestors(
            300,
            "cmdline:net.minecraft.client.main.Main",
            lookup
        ));
    }

    /// An ancestor's cmdline is read too, not just its exe/argv0 — a
    /// grandchild whose own argv looks nothing like the game (e.g. a helper
    /// process) still counts as long as an ancestor's actual command line
    /// carries the needle.
    #[test]
    fn cmdline_identity_is_read_off_an_ancestor_too() {
        let map: BTreeMap<u32, ProcId> = [
            (
                100u32,
                ProcId {
                    ppid: 1,
                    exe: Some("/usr/lib/jvm/java-21-openjdk/bin/java".into()),
                    exe_uid: Some(0),
                    cmdline: vec!["java".into(), "net.minecraft.client.main.Main".into()],
                    cgroup: None,
                },
            ),
            (
                200u32,
                ProcId {
                    ppid: 100,
                    exe: Some("/usr/lib/jvm/java-21-openjdk/bin/helper".into()),
                    exe_uid: Some(0),
                    cmdline: vec!["helper".into()],
                    cgroup: None,
                },
            ),
        ]
        .into_iter()
        .collect();
        let lookup = move |pid: u32| map.get(&pid).cloned();
        assert!(matches_with_ancestors(
            200,
            "cmdline:net.minecraft.client.main.Main",
            lookup
        ));
    }

    /// Flatpak identity still works through the tree (the scope is inherited by
    /// children, so this mostly matters for wrapper chains inside the sandbox).
    #[test]
    fn flatpak_identity_survives_the_walk() {
        let map: BTreeMap<u32, ProcId> = [(
            300u32,
            ProcId {
                ppid: 1,
                exe: Some("/usr/bin/bwrap".into()),
                exe_uid: Some(0),
                cmdline: vec![],
                cgroup: Some("0::/user.slice/app-flatpak-com.mojang.Minecraft-1.scope".into()),
            },
        )]
        .into_iter()
        .collect();
        let lookup = move |pid: u32| map.get(&pid).cloned();
        assert!(matches_with_ancestors(300, "com.mojang.Minecraft", lookup));
    }

    /// The case the generic walker exists for. A Chromium renderer's own argv
    /// is `--type=renderer …` and looks nothing like a launch line, so the only
    /// way to know it belongs to a sanctioned educational window is to ask
    /// whether its browser process is one. Getting this wrong sweeps every
    /// renderer of a working Khan window on the next tick — the page dies while
    /// the frame stays up.
    #[test]
    fn a_renderer_child_inherits_its_browsers_sanction() {
        let khan = charter_proto::LearningApp {
            id: "khan-academy".into(),
            label: "Khan Academy".into(),
            kind: charter_proto::LearningAppKind::Site,
            domains: vec!["khanacademy.org".into()],
            url: Some("https://www.khanacademy.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        };
        let browser_argv =
            crate::site_app::sanctioned_argv("/usr/bin/chromium", &khan, "/home/robin");
        let map: BTreeMap<u32, ProcId> = [
            (
                100u32,
                ProcId {
                    ppid: 1,
                    exe: Some("/usr/bin/chromium".into()),
                    exe_uid: Some(0),
                    cmdline: browser_argv,
                    cgroup: None,
                },
            ),
            (
                200u32,
                ProcId {
                    ppid: 100,
                    exe: Some("/usr/bin/chromium".into()),
                    exe_uid: Some(0),
                    cmdline: vec![
                        "/usr/bin/chromium".into(),
                        "--type=renderer".into(),
                        "--lang=en-GB".into(),
                    ],
                    cgroup: None,
                },
            ),
            // An unrelated bare Chromium the ward opened themselves.
            (
                300u32,
                ProcId {
                    ppid: 1,
                    exe: Some("/usr/bin/chromium".into()),
                    exe_uid: Some(0),
                    cmdline: vec!["/usr/bin/chromium".into()],
                    cgroup: None,
                },
            ),
        ]
        .into_iter()
        .collect();
        let lookup = move |pid: u32| map.get(&pid).cloned();
        let sanctioned = |pid| {
            has_ancestor_matching(pid, &lookup, |id| {
                crate::site_app::is_sanctioned(&id.cmdline, id.exe_uid, std::slice::from_ref(&khan))
            })
        };

        assert!(sanctioned(100), "the browser process itself");
        assert!(sanctioned(200), "its renderer");
        assert!(!sanctioned(300), "an unrelated bare browser is not spared");
    }

    /// C2 at the ancestor-inheritance path: a "renderer" under a FORGED
    /// browser process (byte-identical sanctioned cmdline, but a ward-owned
    /// exe wearing the runtime's argv0) must not inherit a sanction its own
    /// parent never earned — the ancestor's OWN `exe_uid` gates it, exactly
    /// like the direct-match case in `site_app`'s tests.
    #[test]
    fn a_renderer_does_not_inherit_a_forged_ancestors_sanction() {
        let khan = charter_proto::LearningApp {
            id: "khan-academy".into(),
            label: "Khan Academy".into(),
            kind: charter_proto::LearningAppKind::Site,
            domains: vec!["khanacademy.org".into()],
            url: Some("https://www.khanacademy.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        };
        let browser_argv =
            crate::site_app::sanctioned_argv("/usr/bin/chromium", &khan, "/home/robin");
        let map: BTreeMap<u32, ProcId> = [
            (
                100u32,
                ProcId {
                    ppid: 1,
                    // Ward-owned binary wearing the runtime's argv0 — the
                    // `exec -a` forgery, exactly reproducing the sanctioned
                    // cmdline.
                    exe: Some("/home/kid/patched-chromium".into()),
                    exe_uid: Some(1002),
                    cmdline: browser_argv,
                    cgroup: None,
                },
            ),
            (
                200u32,
                ProcId {
                    ppid: 100,
                    exe: Some("/home/kid/patched-chromium".into()),
                    exe_uid: Some(1002),
                    cmdline: vec![
                        "/usr/bin/chromium".into(),
                        "--type=renderer".into(),
                        "--lang=en-GB".into(),
                    ],
                    cgroup: None,
                },
            ),
        ]
        .into_iter()
        .collect();
        let lookup = move |pid: u32| map.get(&pid).cloned();
        let sanctioned = |pid| {
            has_ancestor_matching(pid, &lookup, |id| {
                crate::site_app::is_sanctioned(&id.cmdline, id.exe_uid, std::slice::from_ref(&khan))
            })
        };

        assert!(!sanctioned(100), "the forged process itself");
        assert!(!sanctioned(200), "its renderer inherits nothing to inherit");
    }
}
