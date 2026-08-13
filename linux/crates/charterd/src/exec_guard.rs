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

/// Validate an exec candidate before hashing it as root.
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
    // dir sharing the prefix (e.g. "/home/managed-evil") pass when managed_root
    // has no trailing slash; require the match to end on a path boundary.
    let root = managed_root.strip_suffix('/').unwrap_or(managed_root);
    if !path
        .strip_prefix(root)
        .is_some_and(|rest| rest.starts_with('/'))
    {
        return Err(ExecPathError::OutsideManagedTree);
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

    const ROOT: &str = "/home/managed/";

    #[test]
    fn accepts_readable_regular_in_tree() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: true,
        };
        assert!(validate_exec_candidate("/home/managed/game.AppImage", ROOT, 1000, &p).is_ok());
    }

    #[test]
    fn rejects_unreadable_root_path() {
        let p = Probe {
            kind: FileKind::Regular,
            readable: false,
        };
        assert_eq!(
            validate_exec_candidate("/home/managed/x", ROOT, 1000, &p),
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
            validate_exec_candidate("/home/managed-evil/x.AppImage", "/home/managed", 1000, &p),
            Err(ExecPathError::OutsideManagedTree)
        );
        assert!(
            validate_exec_candidate("/home/managed/game.AppImage", "/home/managed", 1000, &p)
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
            validate_exec_candidate("/home/managed/../../etc/x", ROOT, 1000, &p),
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
                validate_exec_candidate("/home/managed/weird", ROOT, 1000, &p),
                Err(ExecPathError::SpecialFile)
            );
        }
    }
}
