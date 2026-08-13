//! THE canonical no-on-device-DOB guard. This is the **only** birthDate guard
//! in the tree. It lives in charter-testkit so its coverage moves with the
//! shared core, not with any one substrate (port-spec §1.4).
//!
//! It scans every `core/crates`, `linux/crates`, and `android` source —
//! Rust (`.rs`) and Kotlin (`.kt`/`.kts`) — for the token set
//! `{birthDate, birth_date, birthdate, dob, dateOfBirth}` and fails on any
//! occurrence in *code* (comments and docs that merely discuss the rule are
//! stripped first, since they neither set, populate, nor read the field).
//!
//! Exactly two things are allowlisted:
//!   1. The future `charter-setup` clear-path module (it may name the field
//!      only in defensive unset/clear ops — Phase 9).
//!   2. This guard's own source file (it must name the tokens to scan for).

use std::fs;
use std::path::{Path, PathBuf};

/// Lowercased forbidden tokens. `birthdate` also covers `birthDate`;
/// `dateofbirth` covers `dateOfBirth`.
const FORBIDDEN: &[&str] = &["birthdate", "birth_date", "dob", "dateofbirth"];

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Whether `token` appears in `haystack` (already lowercased) as a whole word,
/// not as a substring of a larger identifier.
fn contains_token(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let tb = token.as_bytes();
    let mut i = 0;
    while let Some(pos) = haystack[i..].find(token) {
        let start = i + pos;
        let end = start + tb.len();
        let before_ok = start == 0 || !is_word_char(bytes[start - 1] as char);
        let after_ok = end >= bytes.len() || !is_word_char(bytes[end] as char);
        if before_ok && after_ok {
            return true;
        }
        i = start + 1;
    }
    false
}

/// Strip line comments (`// ...`) so prose that discusses the rule does not
/// count as a set/populate/read. Block comments are not used with these tokens
/// anywhere in the tree.
fn strip_comment(line: &str) -> &str {
    match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    }
}

fn collect_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            // Build outputs and vendored trees only.
            if matches!(
                name,
                "target" | "build" | ".gradle" | "node_modules" | ".kotlin"
            ) {
                continue;
            }
            collect_sources(&path, out);
        } else if matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("rs") | Some("kt") | Some("kts")
        ) {
            out.push(path);
        }
    }
}

fn is_allowlisted(path: &Path) -> bool {
    let p = path.to_string_lossy();
    // The guard's own file.
    if p.ends_with("privacy_birthdate_guard.rs") {
        return true;
    }
    // The single future charter-setup clear-path module (Phase 9).
    p.contains("charter-setup") && (p.ends_with("birthdate.rs") || p.ends_with("account.rs"))
}

#[test]
fn no_source_references_dob() {
    // charter-testkit lives at <repo>/core/crates/charter-testkit.
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .expect("repo root")
        .to_path_buf();
    let scan_roots = [
        repo_root.join("core/crates"),
        repo_root.join("linux/crates"),
        repo_root.join("android"),
    ];
    for root in &scan_roots {
        assert!(root.is_dir(), "guard scan root missing: {root:?}");
    }

    let mut files = Vec::new();
    for root in &scan_roots {
        collect_sources(root, &mut files);
    }
    assert!(!files.is_empty(), "guard found no source files to scan");

    let mut violations = Vec::new();
    for file in &files {
        if is_allowlisted(file) {
            continue;
        }
        let text = fs::read_to_string(file).unwrap_or_default();
        for (lineno, line) in text.lines().enumerate() {
            let code = strip_comment(line).to_lowercase();
            for token in FORBIDDEN {
                if contains_token(&code, token) {
                    violations.push(format!("{}:{} -> {token}", file.display(), lineno + 1));
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "no source may set/populate/read systemd birthDate; found:\n{}",
        violations.join("\n")
    );
}

#[test]
fn guard_actually_detects_a_violation() {
    // Self-test: the scanner must catch a real set, but ignore a comment.
    assert!(contains_token("user.birthdate = x", "birthdate"));
    assert!(!contains_token("my_birthdates_list", "birthdate")); // substring, not a word
    assert_eq!(
        strip_comment("let x = 1; // birthdate note").trim(),
        "let x = 1;"
    );
}
