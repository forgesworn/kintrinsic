//! The confused-deputy guard for `exec.allow` request building. Before charterd
//! (root) hashes an exec candidate, it verifies the *calling managed uid* can
//! actually read the path, refuses paths outside the managed tree, and rejects
//! symlinks-elsewhere / FIFOs / device nodes. Pure logic over a `ProbeExec`
//! seam (the real uid-read / stat is a charter-sys port).

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

/// Validate an exec candidate before hashing it as root.
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
/// The resolved test only runs when the managed root itself resolves, so the
/// pure-logic unit tests below (whose `/managed/managed` exists nowhere) still
/// exercise the textual rules on their own.
pub fn validate_exec_candidate(
    path: &str,
    managed_root: &str,
    caller_uid: u32,
    probe: &dyn ProbeExec,
) -> Result<(), ExecPathError> {
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
    if let Ok(real_root) = std::fs::canonicalize(root) {
        match std::fs::canonicalize(path) {
            Ok(real) if is_beneath(&real, &real_root) => {}
            Ok(_) => return Err(ExecPathError::OutsideManagedTree),
            Err(_) => return Err(ExecPathError::SpecialFile),
        }
    }
    match probe.file_kind(path) {
        FileKind::Regular => {}
        _ => return Err(ExecPathError::SpecialFile),
    }
    if !probe.can_read_as_uid(path, caller_uid) {
        return Err(ExecPathError::NotReadableByCaller);
    }
    Ok(())
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

    const ROOT: &str = "/managed/managed/";

    #[test]
    fn accepts_readable_regular_in_tree() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert!(validate_exec_candidate("/managed/managed/game.AppImage", ROOT, 1000, &p).is_ok());
    }

    #[test]
    fn rejects_unreadable_root_path() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: false,
        };
        assert_eq!(
            validate_exec_candidate("/managed/managed/x", ROOT, 1000, &p),
            Err(ExecPathError::NotReadableByCaller)
        );
    }

    #[test]
    fn rejects_out_of_tree() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate("/etc/shadow", ROOT, 1000, &p),
            Err(ExecPathError::OutsideManagedTree)
        );
    }

    #[test]
    fn rejects_sibling_dir_sharing_prefix() {
        // M3: a sibling home sharing the prefix must not pass the boundary, even
        // when the managed root has no trailing slash.
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate("/managed/managed-evil/x.AppImage", "/managed/managed", 1000, &p),
            Err(ExecPathError::OutsideManagedTree)
        );
        assert!(
            validate_exec_candidate("/managed/managed/game.AppImage", "/managed/managed", 1000, &p)
                .is_ok(),
            "in-tree path with a slash-less root still passes"
        );
    }

    #[test]
    fn rejects_traversal() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert_eq!(
            validate_exec_candidate("/managed/managed/../../etc/x", ROOT, 1000, &p),
            Err(ExecPathError::Traversal)
        );
    }

    #[test]
    fn rejects_symlink_or_special_file() {
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
                validate_exec_candidate("/managed/managed/weird", ROOT, 1000, &p),
                Err(ExecPathError::SpecialFile)
            );
        }
    }

    // ---- B8: containment against a REAL tree, where symlinks exist ----

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
}
