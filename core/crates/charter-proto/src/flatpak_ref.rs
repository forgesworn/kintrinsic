//! `FlatpakRef` — a strict Flatpak application reference grammar that doubles
//! as a shell-injection guard. Grammar:
//! `^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*(/[A-Za-z0-9_-]*){0,2}$`. The enactor
//! re-parses the granted ref through this **before any FlatpakOps call**, so an
//! injected ref never reaches a subprocess.

use std::fmt;

/// A validated Flatpak reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatpakRef(String);

/// Why a ref was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlatpakRefError;

impl fmt::Display for FlatpakRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid flatpak ref")
    }
}

impl std::error::Error for FlatpakRefError {}

fn is_ref_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

impl FlatpakRef {
    /// Parse + validate a reference. Rejects shell metacharacters, traversal,
    /// empty/`//` segments, leading `/`, and non-ASCII.
    pub fn parse(s: &str) -> Result<FlatpakRef, FlatpakRefError> {
        if s.is_empty() || !s.is_ascii() {
            return Err(FlatpakRefError);
        }
        if s.contains("..") || s.contains("//") || s.starts_with('/') {
            return Err(FlatpakRefError);
        }
        // Defense in depth: no shell metacharacters or whitespace/control.
        if s.bytes()
            .any(|b| !(is_ref_char(b) || b == b'.' || b == b'/'))
        {
            return Err(FlatpakRefError);
        }
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() > 3 {
            return Err(FlatpakRefError); // app-id + optional arch + branch
        }
        // App id: reverse-DNS, each component non-empty + ref-chars only.
        let app = parts[0];
        let comps: Vec<&str> = app.split('.').collect();
        if comps.is_empty()
            || comps
                .iter()
                .any(|c| c.is_empty() || !c.bytes().all(is_ref_char))
        {
            return Err(FlatpakRefError);
        }
        // arch / branch parts: ref-chars only (may be empty per the grammar).
        for p in &parts[1..] {
            if !p.bytes().all(is_ref_char) {
                return Err(FlatpakRefError);
            }
        }
        Ok(FlatpakRef(s.to_string()))
    }

    /// The validated ref string (safe to pass as a single argv element).
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FlatpakRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_canonical_refs() {
        for ok in [
            "org.videolan.VLC",
            "com.valvesoftware.Steam",
            "vlc",
            "org.x.Y/x86_64/stable",
            "org.x.Y/x86_64",
        ] {
            assert!(FlatpakRef::parse(ok).is_ok(), "{ok} should parse");
        }
    }

    #[test]
    fn rejects_injection_corpus() {
        for bad in [
            "org.x; rm -rf /",
            "org.x|cat",
            "org.x && id",
            "org.x$(whoami)",
            "org.x`id`",
            "/etc/passwd",
            "org..x",
            "org//x",
            "org.x ",
            "org.x\nnext",
            "org.x\0",
            "org.café",    // non-ascii
            "org.x/a/b/c", // too many slashes
            "",
            "org.x.", // trailing dot -> empty component
        ] {
            assert!(FlatpakRef::parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }
}
