//! What version of Kintrinsic this machine is running, in the two forms the
//! guardian surface needs.
//!
//! Android ships a monotonic integer `versionCode` alongside its display name;
//! Linux has only a semver string. Kintrinsic compares versions numerically (one
//! rule for both platforms) and *shows* the name, because "305" is not a
//! version to a parent. So the daemon reports both, deriving the number from
//! its own package version.
//!
//! Until this existed, `charterd` reported no version at all — Kintrinsic had
//! no idea what a paired laptop was running, so it could neither say "update
//! available" nor tell whether one had landed.

/// This build's version as humans write it (e.g. "0.3.5").
pub fn version_name() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// This build's version as a comparable integer (e.g. "0.3.5" → 305).
pub fn version_code() -> u64 {
    parse_version_code(version_name()).unwrap_or(0)
}

/// `major.minor.patch` → `major*10_000 + minor*100 + patch`.
///
/// Ordering is the only contract: a later release must always produce a bigger
/// number. Minor and patch are therefore given two digits each, and anything
/// that would overflow those lanes (a 100th patch) saturates rather than
/// carrying into the lane above — a wrong ORDER would offer a downgrade as an
/// update, which is worse than a stuck one. Pre-release/build suffixes
/// ("0.3.5-rc1") are ignored: they compare equal to the release, and we would
/// rather not offer an update than offer a bogus one.
pub fn parse_version_code(v: &str) -> Option<u64> {
    let core = v.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major: u64 = parts.next()?.trim().parse().ok()?;
    let minor: u64 = parts.next().unwrap_or("0").trim().parse().ok()?;
    let patch: u64 = parts.next().unwrap_or("0").trim().parse().ok()?;
    if parts.next().is_some() {
        return None; // 1.2.3.4 is not a version we understand
    }
    Some(major * 10_000 + minor.min(99) * 100 + patch.min(99))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_a_semver_to_a_number() {
        assert_eq!(parse_version_code("0.3.5"), Some(305));
        assert_eq!(parse_version_code("0.3.4"), Some(304));
        assert_eq!(parse_version_code("1.0.0"), Some(10_000));
        assert_eq!(parse_version_code("0.0.1"), Some(1));
    }

    /// The ONLY contract that matters: later must always be bigger. A wrong
    /// order would advertise a downgrade as an update.
    #[test]
    fn ordering_holds_across_releases() {
        let ordered = [
            "0.0.1", "0.1.0", "0.2.9", "0.3.0", "0.3.4", "0.3.5", "0.4.0", "1.0.0", "1.0.1",
            "2.0.0",
        ];
        let codes: Vec<u64> = ordered
            .iter()
            .map(|v| parse_version_code(v).unwrap())
            .collect();
        for pair in codes.windows(2) {
            assert!(pair[0] < pair[1], "{codes:?} must be strictly increasing");
        }
    }

    #[test]
    fn tolerates_short_versions() {
        assert_eq!(parse_version_code("1"), Some(10_000));
        assert_eq!(parse_version_code("1.2"), Some(10_200));
    }

    /// A pre-release compares equal to its release rather than inventing an
    /// ordering — we would rather not offer an update than offer a wrong one.
    #[test]
    fn suffixes_are_ignored() {
        assert_eq!(parse_version_code("0.3.5-rc1"), Some(305));
        assert_eq!(parse_version_code("0.3.5+build7"), Some(305));
    }

    #[test]
    fn rejects_what_it_cannot_understand() {
        assert_eq!(parse_version_code(""), None);
        assert_eq!(parse_version_code("banana"), None);
        assert_eq!(parse_version_code("1.2.3.4"), None);
        assert_eq!(parse_version_code("-1.0.0"), None);
    }

    /// Saturating rather than carrying: a 100th patch must not read as the
    /// next minor and leapfrog a real release.
    #[test]
    fn overlong_lanes_saturate_instead_of_carrying() {
        let p99 = parse_version_code("0.3.99").unwrap();
        let p120 = parse_version_code("0.3.120").unwrap();
        let next_minor = parse_version_code("0.4.0").unwrap();
        assert!(p99 <= p120);
        assert!(p120 < next_minor, "a patch must never outrank a minor");
    }

    /// The daemon's own version must be reportable — a build that can't say
    /// what it is leaves the guardian blind, which is the bug this fixes.
    #[test]
    fn this_build_reports_itself() {
        assert!(!version_name().is_empty());
        assert!(
            version_code() > 0,
            "own version must parse: {}",
            version_name()
        );
    }
}
