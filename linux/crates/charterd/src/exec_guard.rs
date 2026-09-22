//! The confused-deputy guard for `exec.allow` request building. Before charterd
//! (root) hashes an exec candidate, it verifies the *calling managed uid* can
//! actually read the path, refuses paths outside the managed tree, and rejects
//! symlinks-elsewhere / FIFOs / device nodes. Pure logic over a `ProbeExec`
//! seam (the real uid-read / stat is a charter-sys port) — plus one
//! non-negotiable syscall: the guard **opens** the candidate itself and hands
//! the descriptor on, so the thing that was checked is the thing that is hashed.

use std::fs::File;
use std::path::{Path, PathBuf};

/// What kind of filesystem object a candidate path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Regular,
    Symlink,
    Fifo,
    Device,
    Dir,
    Missing,
}

/// Why an exec candidate was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecPathError {
    /// Path contains `..` traversal.
    Traversal,
    /// Path is outside the managed user's tree.
    OutsideManagedTree,
    /// The calling managed uid cannot read the path.
    NotReadableByCaller,
    /// Not a regular file (symlink-elsewhere / FIFO / device / dir / missing).
    SpecialFile,
}

/// Seam for the privileged file probe (uid-scoped read + stat).
pub trait ProbeExec: Send + Sync {
    /// Whether `uid` can read `path` (open as that uid / access-check).
    fn can_read_as_uid(&self, path: &str, uid: u32) -> bool;
    /// The kind of object at `path` (without following a final symlink).
    fn file_kind(&self, path: &str) -> FileKind;
}

/// Is `path` strictly beneath `root`, comparing whole path COMPONENTS?
///
/// Component-wise, never a string prefix: `/managed/ward2` starts with `/managed/ward`
/// as text and is a different account on the disk. Both sides are expected to
/// be already-resolved (canonical) absolute paths.
fn is_beneath(path: &std::path::Path, root: &std::path::Path) -> bool {
    let mut p = path.components();
    for c in root.components() {
        if p.next() != Some(c) {
            return false;
        }
    }
    // Strictly beneath: the root ITSELF is a directory, never a candidate.
    p.next().is_some()
}

/// A candidate that passed the guard — and the **open descriptor** it passed on.
///
/// This is the whole point of 03-B8: the guard's answer is not "that path was
/// fine a moment ago", it is "here is the file I checked". Anything downstream
/// (hashing, admission) reads `file`, never re-opens `resolved`.
#[derive(Debug)]
pub struct ValidatedExec {
    /// The candidate, opened read-only under `RESOLVE_BENEATH` (see
    /// [`open_beneath`]). Positioned at byte 0.
    pub file: File,
    /// The fully-resolved absolute path the descriptor refers to. Kept for
    /// messages and for the rare consumer that genuinely needs a name.
    pub resolved: PathBuf,
}

/// The open flags every candidate is opened with.
///
/// `O_NOFOLLOW` refuses a final-component symlink outright; `O_NONBLOCK` is the
/// belt to the `lstat` pre-filter's braces — a FIFO that slipped in between the
/// two would otherwise park charterd's request thread forever waiting for a
/// writer. For a regular file, which is the only kind that survives the `fstat`
/// below, `O_NONBLOCK` is a no-op.
const CANDIDATE_OPEN_FLAGS: libc::c_int =
    libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;

/// `resolve` mask for [`openat2`]: never leave the managed root, never traverse
/// a symlink, never traverse a `/proc`-style magic link.
/// (`uapi/linux/openat2.h`: `RESOLVE_NO_MAGICLINKS = 0x02`,
/// `RESOLVE_NO_SYMLINKS = 0x04`, `RESOLVE_BENEATH = 0x08`.)
const CANDIDATE_RESOLVE: u64 =
    libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;

/// What one `openat2(2)` attempt came back with.
enum OpenAttempt {
    Opened(File),
    /// The kernel refused, with this errno.
    Errno(i32),
    /// The kernel has no `openat2(2)` (or rejected the `open_how` it was
    /// handed) — the caller should fall back and log once.
    Unsupported,
}

/// `openat2(dirfd, rel, O_RDONLY|O_CLOEXEC|O_NOFOLLOW|O_NONBLOCK,
/// RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS|RESOLVE_NO_MAGICLINKS)`.
///
/// Issued through `syscall(2)` because glibc exposes no wrapper. `open_how` and
/// the `RESOLVE_*` bits come from `libc`, which mirrors
/// `uapi/linux/openat2.h`; the struct is `#[non_exhaustive]`, so it is built
/// zeroed and filled field-wise rather than with a literal. The 4th argument is
/// `sizeof(open_how)` — the kernel's extensible-struct versioning.
fn openat2(dir: &File, rel: &Path) -> OpenAttempt {
    use std::os::fd::{AsRawFd as _, FromRawFd as _, RawFd};
    use std::os::unix::ffi::OsStrExt as _;

    let Ok(c_rel) = std::ffi::CString::new(rel.as_os_str().as_bytes()) else {
        // An interior NUL is not a path any kernel will ever open.
        return OpenAttempt::Errno(libc::EINVAL);
    };
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = CANDIDATE_OPEN_FLAGS as u64;
    how.resolve = CANDIDATE_RESOLVE;
    // SAFETY: `how` is a fully-initialised `open_how` whose size is passed
    // alongside it, `c_rel` outlives the call, and the returned fd is either
    // negative (an error) or owned by us and immediately wrapped in a `File`.
    let rc = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir.as_raw_fd(),
            c_rel.as_ptr(),
            std::ptr::addr_of!(how),
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if rc >= 0 {
        // SAFETY: a non-negative openat2 return is a fresh owned descriptor.
        return OpenAttempt::Opened(unsafe { File::from_raw_fd(rc as RawFd) });
    }
    match std::io::Error::last_os_error().raw_os_error() {
        // No such syscall (pre-5.6), or a kernel that knows the number but not
        // this `open_how` — either way there is nothing to retry.
        Some(libc::ENOSYS) | Some(libc::EINVAL) => OpenAttempt::Unsupported,
        Some(e) => OpenAttempt::Errno(e),
        None => OpenAttempt::Errno(libc::EIO),
    }
}

/// Logged at most once per daemon lifetime when the kernel has no `openat2(2)`.
static OPENAT2_FALLBACK_LOGGED: std::sync::Once = std::sync::Once::new();

/// Open `real` (already canonical, already known to be beneath `real_root`)
/// so that the *kernel* re-checks containment at open time.
///
/// `canonicalize` answers a question about the past tense: by the time the
/// answer is read the ward may have re-pointed a directory symlink in their own
/// home, and a second open-by-path would land somewhere else entirely. Opening
/// the resolved path *relative to a descriptor on the managed root*, under
/// `RESOLVE_BENEATH`, makes the containment rule part of the same syscall that
/// produces the descriptor — there is no window left between them.
fn open_beneath(real_root: &Path, real: &Path) -> Result<File, ExecPathError> {
    use std::os::unix::fs::OpenOptionsExt as _;

    let rel = real
        .strip_prefix(real_root)
        .map_err(|_| ExecPathError::OutsideManagedTree)?;
    let dir = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(real_root)
        .map_err(|_| ExecPathError::OutsideManagedTree)?;
    match openat2(&dir, rel) {
        OpenAttempt::Opened(f) => Ok(f),
        // EXDEV is openat2's "that would have left the root": the escape the
        // whole check exists for, so it gets the escape's error.
        OpenAttempt::Errno(libc::EXDEV) => Err(ExecPathError::OutsideManagedTree),
        OpenAttempt::Errno(_) => Err(ExecPathError::SpecialFile),
        OpenAttempt::Unsupported => {
            OPENAT2_FALLBACK_LOGGED.call_once(|| {
                eprintln!(
                    "charterd: this kernel has no openat2(2); exec candidates are opened by \
                     resolved path with O_NOFOLLOW instead (containment is checked, but not \
                     atomically with the open)"
                );
            });
            std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(CANDIDATE_OPEN_FLAGS)
                .open(real)
                .map_err(|_| ExecPathError::SpecialFile)
        }
    }
}

/// Validate an exec candidate and **open it**, before a byte of it is hashed.
///
/// # B8 — containment is RESOLVED, not textual
///
/// The `..`-rejection plus separator-anchored prefix below is a check on the
/// STRING the ward handed us, and the ward owns their own home directory: one
/// `ln -s /usr ~/x` makes `/managed/ward/x/bin/<anything>` a path with no `..`
/// segment, a matching prefix, and a perfectly ordinary regular file at the
/// end of it — `symlink_metadata` only ever looked at that FINAL component, so
/// nothing in the chain was ever resolved. "Outside your home folder" was
/// therefore not enforced at all, and an out-of-tree binary could be admitted
/// into the root-owned approved-exec store.
///
/// So the textual test is kept as a cheap first pass and BOTH sides are then
/// canonicalised (every component resolved, symlinks followed) and the
/// containment re-applied over path COMPONENTS. A candidate that will not
/// resolve at all — a dangling link, a missing file — is refused the same way
/// a FIFO is: an identity we cannot pin down is not one we hash as root.
///
/// # B8 (TOCTOU) — the checked file is the *opened* file
///
/// Canonicalising and then letting someone else open the path later is two
/// walks of a tree the ward can rewrite in between. So the last thing this
/// function does is [`open_beneath`], and what it returns is the descriptor:
/// the `fstat` that confirms "regular file" is taken on that fd, and the hash
/// downstream is taken from that fd. A managed root that will not canonicalise
/// is now an error rather than a skipped check — nothing can be beneath a root
/// that does not resolve.
pub fn validate_exec_candidate_open(
    path: &str,
    managed_root: &str,
    caller_uid: u32,
    probe: &dyn ProbeExec,
) -> Result<ValidatedExec, ExecPathError> {
    if path.split('/').any(|seg| seg == "..") {
        return Err(ExecPathError::Traversal);
    }
    // M3: separator-anchored containment. A bare `starts_with` lets a sibling
    // dir sharing the prefix (e.g. "/managed/managed-evil") pass when managed_root
    // has no trailing slash; require the match to end on a path boundary.
    let root = managed_root.strip_suffix('/').unwrap_or(managed_root);
    if !path
        .strip_prefix(root)
        .is_some_and(|rest| rest.starts_with('/'))
    {
        return Err(ExecPathError::OutsideManagedTree);
    }
    // B8: the same question again, of the filesystem rather than of the string.
    // Kept as a pre-filter so the ward gets "outside your home folder" rather
    // than the flat "that isn't a regular file" an openat2 EXDEV would give.
    let real_root = std::fs::canonicalize(root).map_err(|_| ExecPathError::OutsideManagedTree)?;
    let real = std::fs::canonicalize(path).map_err(|_| ExecPathError::SpecialFile)?;
    if !is_beneath(&real, &real_root) {
        return Err(ExecPathError::OutsideManagedTree);
    }
    let real_str = real.to_str().ok_or(ExecPathError::SpecialFile)?;
    // lstat the path as handed to us: a final-component symlink, FIFO or device
    // node is refused before anything is opened at all.
    match probe.file_kind(path) {
        FileKind::Regular => {}
        _ => return Err(ExecPathError::SpecialFile),
    }
    // Ask the kernel, as the calling child, about the RESOLVED path — the
    // readability answer and the bytes we hash must be about one file.
    if !probe.can_read_as_uid(real_str, caller_uid) {
        return Err(ExecPathError::NotReadableByCaller);
    }
    let file = open_beneath(&real_root, &real)?;
    // The authoritative kind check, on the descriptor itself. Everything above
    // it is a pre-filter; this one cannot be raced, because there is no path
    // left to swap.
    match file.metadata() {
        Ok(md) if md.is_file() => {}
        _ => return Err(ExecPathError::SpecialFile),
    }
    Ok(ValidatedExec {
        file,
        resolved: real,
    })
}

/// [`validate_exec_candidate_open`] for callers that only want the verdict.
///
/// Kept so the guard still reads as a predicate where that is all that is
/// wanted; the descriptor is opened and dropped. Anything that goes on to hash
/// the candidate must use [`validate_exec_candidate_open`] instead.
pub fn validate_exec_candidate(
    path: &str,
    managed_root: &str,
    caller_uid: u32,
    probe: &dyn ProbeExec,
) -> Result<(), ExecPathError> {
    validate_exec_candidate_open(path, managed_root, caller_uid, probe).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Probe {
        kind: FileKind,
        readable: bool,
    }
    impl ProbeExec for Probe {
        fn can_read_as_uid(&self, _path: &str, _uid: u32) -> bool {
            self.readable
        }
        fn file_kind(&self, _path: &str) -> FileKind {
            self.kind
        }
    }

    /// A root that exists nowhere. Only the two checks that answer *before* the
    /// filesystem is touched — the `..` scan and the textual prefix — may use
    /// it; everything past those needs a real tree, because a managed root that
    /// will not canonicalise is now itself a refusal.
    const UNREAL_ROOT: &str = "/managed/managed/";

    /// A throwaway tree: `<tmp>/<name>/{home,outside}`.
    fn real_tree(name: &str) -> std::path::PathBuf {
        let d =
            std::env::temp_dir().join(format!("charterd-exec-guard-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("home/bin")).unwrap();
        std::fs::create_dir_all(d.join("outside/bin")).unwrap();
        std::fs::write(d.join("home/bin/mine"), b"#!/bin/sh\n").unwrap();
        std::fs::write(d.join("outside/bin/theirs"), b"#!/bin/sh\n").unwrap();
        d
    }

    #[test]
    fn accepts_readable_regular_in_tree() {
        let d = real_tree("accepts");
        let home = d.join("home");
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert!(validate_exec_candidate(
            home.join("bin/mine").to_str().unwrap(),
            home.to_str().unwrap(),
            1000,
            &p
        )
        .is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_unreadable_root_path() {
        let d = real_tree("unreadable");
        let home = d.join("home");
        let p = Probe {
            kind: FileKind::Regular,
            readable: false,
        };
        assert_eq!(
            validate_exec_candidate(
                home.join("bin/mine").to_str().unwrap(),
                home.to_str().unwrap(),
                1000,
                &p
            ),
            Err(ExecPathError::NotReadableByCaller)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_out_of_tree() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate("/etc/shadow", UNREAL_ROOT, 1000, &p),
            Err(ExecPathError::OutsideManagedTree)
        );
    }

    #[test]
    fn rejects_sibling_dir_sharing_prefix() {
        // M3: a sibling home sharing the prefix must not pass the boundary, even
        // when the managed root has no trailing slash. Textual, so no tree needed.
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate(
                "/managed/managed-evil/x.AppImage",
                "/managed/managed",
                1000,
                &p
            ),
            Err(ExecPathError::OutsideManagedTree)
        );
    }

    #[test]
    fn an_in_tree_path_passes_with_a_slash_less_root() {
        let d = real_tree("slashless");
        let home = d.join("home");
        // No trailing slash on the root — the other half of M3.
        let root = home.to_str().unwrap().trim_end_matches('/').to_string();
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert!(
            validate_exec_candidate(home.join("bin/mine").to_str().unwrap(), &root, 1000, &p)
                .is_ok()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn rejects_traversal() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate("/managed/managed/../../etc/x", UNREAL_ROOT, 1000, &p),
            Err(ExecPathError::Traversal)
        );
    }

    #[test]
    fn a_managed_root_that_will_not_resolve_is_refused() {
        // Dropped branch: the resolved check used to be SKIPPED when the root
        // would not canonicalise, which quietly turned the whole containment
        // test off. Nothing is beneath a root that does not exist.
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate("/managed/managed/game.AppImage", UNREAL_ROOT, 1000, &p),
            Err(ExecPathError::OutsideManagedTree)
        );
    }

    #[test]
    fn rejects_symlink_or_special_file() {
        let d = real_tree("special");
        let home = d.join("home");
        for kind in [
            FileKind::Symlink,
            FileKind::Fifo,
            FileKind::Device,
            FileKind::Dir,
            FileKind::Missing,
        ] {
            let p = Probe {
                kind,
                readable: true,
            };
            assert_eq!(
                validate_exec_candidate(
                    home.join("bin/mine").to_str().unwrap(),
                    home.to_str().unwrap(),
                    1000,
                    &p
                ),
                Err(ExecPathError::SpecialFile)
            );
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- B8: containment against a REAL tree, where symlinks exist ----

    /// The finding itself. `ln -s <outside> ~/x` needs no privilege, the
    /// candidate carries no `..`, the textual prefix matches, and the FINAL
    /// component really is an ordinary readable file — every check the guard
    /// used to make says yes, and the path is not in the managed tree at all.
    #[test]
    fn an_intermediate_symlink_cannot_walk_out_of_the_managed_tree() {
        let d = real_tree("escape");
        let home = d.join("home");
        std::os::unix::fs::symlink(d.join("outside"), home.join("x")).unwrap();

        let p = Probe {
            // What `symlink_metadata` of the FULL path actually reports: the
            // last component is a regular file. Only the middle lied.
            kind: FileKind::Regular,
            readable: true,
        };
        let candidate = home.join("x/bin/theirs");
        assert_eq!(
            validate_exec_candidate(
                candidate.to_str().unwrap(),
                home.to_str().unwrap(),
                1000,
                &p
            ),
            Err(ExecPathError::OutsideManagedTree),
            "an escape through an intermediate symlink must be refused"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_genuinely_in_tree_file_still_passes_the_resolved_check() {
        let d = real_tree("ok");
        let home = d.join("home");
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert!(validate_exec_candidate(
            home.join("bin/mine").to_str().unwrap(),
            home.to_str().unwrap(),
            1000,
            &p
        )
        .is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A ward-owned link that stays inside their own tree is not an escape —
    /// the rule is containment, not "no symlinks anywhere in the chain".
    #[test]
    fn a_symlink_that_stays_inside_the_tree_is_still_contained() {
        let d = real_tree("inside");
        let home = d.join("home");
        std::fs::create_dir_all(home.join("games")).unwrap();
        std::os::unix::fs::symlink(home.join("bin"), home.join("games/link")).unwrap();
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert!(validate_exec_candidate(
            home.join("games/link/mine").to_str().unwrap(),
            home.to_str().unwrap(),
            1000,
            &p
        )
        .is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `/managed/ward2` is a string prefix match for `/managed/ward` and a different
    /// account on the disk — the resolved test compares COMPONENTS for exactly
    /// this reason. (The textual pass catches it first; this pins the resolved
    /// half so a later refactor cannot quietly drop back to `starts_with`.)
    #[test]
    fn a_resolved_sibling_home_sharing_the_prefix_is_not_beneath() {
        use std::path::Path;
        assert!(!is_beneath(
            Path::new("/managed/ward2/bin/x"),
            Path::new("/managed/ward")
        ));
        assert!(is_beneath(
            Path::new("/managed/ward/bin/x"),
            Path::new("/managed/ward")
        ));
        assert!(
            !is_beneath(Path::new("/managed/ward"), Path::new("/managed/ward")),
            "the root itself is a directory, never a candidate"
        );
    }

    /// A dangling link resolves to nothing, so there is no identity to hash.
    #[test]
    fn a_candidate_that_will_not_resolve_is_refused() {
        let d = real_tree("dangling");
        let home = d.join("home");
        std::os::unix::fs::symlink(d.join("nowhere"), home.join("ghost")).unwrap();
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate(
                home.join("ghost").to_str().unwrap(),
                home.to_str().unwrap(),
                1000,
                &p
            ),
            Err(ExecPathError::SpecialFile)
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    // ---- B8 (TOCTOU): the open itself is the check ----

    /// The guard hands back a descriptor on the very file it validated, so the
    /// hash downstream never re-walks the path.
    #[test]
    fn the_guard_returns_an_open_descriptor_on_the_candidate() {
        use std::io::Read as _;
        let d = real_tree("fd");
        let home = d.join("home");
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        let mut v = validate_exec_candidate_open(
            home.join("bin/mine").to_str().unwrap(),
            home.to_str().unwrap(),
            1000,
            &p,
        )
        .expect("an in-tree regular file is admitted");
        assert_eq!(
            v.resolved,
            std::fs::canonicalize(home.join("bin/mine")).unwrap()
        );
        let mut got = String::new();
        v.file.read_to_string(&mut got).unwrap();
        assert_eq!(got, "#!/bin/sh\n", "the fd reads the candidate's bytes");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// `openat2` refuses the escape on its own, with no canonicalize in front
    /// of it: the relative path is walked *by the kernel* from a descriptor on
    /// the managed root, under `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS`, so an
    /// intermediate symlink out of the tree cannot be followed however the
    /// caller got here.
    #[test]
    fn openat2_refuses_a_symlink_that_leaves_the_managed_root() {
        let d = real_tree("beneath");
        let home = std::fs::canonicalize(d.join("home")).unwrap();
        std::os::unix::fs::symlink(d.join("outside"), home.join("x")).unwrap();

        // Sanity: the escape target really is a readable regular file.
        assert!(d.join("outside/bin/theirs").is_file());

        // `open_beneath` takes an already-resolved path, so hand it the
        // UNRESOLVED escape directly — this is exactly the state a swap after
        // canonicalize would leave us in.
        let err = open_beneath(&home, &home.join("x/bin/theirs"))
            .expect_err("openat2 must refuse an escape through an intermediate symlink");
        assert!(
            matches!(
                err,
                ExecPathError::OutsideManagedTree | ExecPathError::SpecialFile
            ),
            "refused, not admitted: {err:?}"
        );

        // And the in-tree file through the same door still opens.
        assert!(open_beneath(&home, &home.join("bin/mine")).is_ok());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An absolute path, or one climbing out with `..`, is refused by the
    /// kernel's own `RESOLVE_BENEATH` rather than by anything we wrote.
    #[test]
    fn openat2_refuses_a_path_that_is_not_beneath_the_root() {
        let d = real_tree("escape-rel");
        let home = std::fs::canonicalize(d.join("home")).unwrap();
        assert!(
            open_beneath(&home, &d.join("outside/bin/theirs")).is_err(),
            "a sibling of the root is not beneath it"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
