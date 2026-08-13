//! Domain normalization for set comparison. v1 is intentionally simple
//! (lowercase, strip scheme / `www.` / path / port) — full Public-Suffix-List
//! handling is later work.

/// Whether a normalized host is one Charter will carry (S11, review
/// 2026-08-07).
///
/// **Rejects, never sanitizes.** A domain arriving off the wire flows into
/// `DnsFilterPlan` and into Firefox `WebsiteFilter` patterns — line-oriented
/// and newline-delimited config surfaces — and `normalize_domain` was happy to
/// carry a control character straight through. Every in-crate sink is
/// serde-escaped today, so this was never demonstrably exploitable; it was one
/// naive `writeln!` in a config applier away from being an injection, and the
/// applier is not even in this tree.
///
/// Sanitising would be the wrong repair. A domain we had to *edit* to make
/// safe is a domain we no longer know the meaning of, and quietly turning
/// `evil.example\nallow: *` into `evil.exampleallow: *` invents a rule nobody
/// wrote. Refusing the entry loses one row of a curator list and keeps every
/// other row honest.
///
/// The charset is deliberately the LDH set plus dots — the characters a DNS
/// hostname is made of. Punycode is already `xn--…` ASCII by the time it gets
/// here; a raw Unicode host is refused rather than guessed at, because two
/// different Unicode strings can render identically and a homograph is exactly
/// the thing an allow/deny set must not confuse.
pub fn is_valid_domain(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        // No empty labels: a leading dot, a trailing dot (normalization already
        // strips the DNS root dot), or `a..b` are all malformed rather than
        // merely unusual, and `..` in a path-like sink is its own hazard.
        && !host.split('.').any(|label| label.is_empty() || label.len() > 63)
}

/// [`normalize_domain`], refusing anything that is not a plain DNS host.
///
/// The entry point for domains that came off the WIRE — a curator list, a
/// guardian clause — as opposed to ones this codebase built itself.
pub fn parse_domain(raw: &str) -> Option<String> {
    let host = normalize_domain(raw);
    is_valid_domain(&host).then_some(host)
}

/// Normalize a domain or URL to a lowercase bare host.
pub fn normalize_domain(raw: &str) -> String {
    let mut s = raw.trim().to_ascii_lowercase();
    for scheme in ["https://", "http://"] {
        if let Some(rest) = s.strip_prefix(scheme) {
            s = rest.to_string();
        }
    }
    // Cut the host off at the first path / query / port separator.
    let host_end = s.find(['/', '?', ':']).unwrap_or(s.len());
    // Trim the DNS root dot: `evil.example.` is the same host as `evil.example`,
    // so they must normalize identically or parentDeny could be bypassed with a
    // trailing-dot variant in allowlist posture.
    let host = s[..host_end].trim_end_matches('.');
    host.strip_prefix("www.").unwrap_or(host).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_scheme_www_path_port_and_lowercases() {
        assert_eq!(
            normalize_domain("https://www.Example.com/foo?x=1"),
            "example.com"
        );
        assert_eq!(normalize_domain("http://Example.COM:8080"), "example.com");
        assert_eq!(normalize_domain("  example.com  "), "example.com");
        assert_eq!(normalize_domain("sub.example.com"), "sub.example.com");
        assert_eq!(normalize_domain("www.example.com"), "example.com");
    }

    #[test]
    fn strips_dns_root_dot_so_variants_collapse() {
        // A trailing root dot is DNS-identical to the bare host; both must map
        // to the same normalized form for allow/deny set comparison.
        assert_eq!(normalize_domain("evil.example."), "evil.example");
        assert_eq!(normalize_domain("evil.example"), "evil.example");
        assert_eq!(
            normalize_domain("https://www.Example.com./foo"),
            "example.com"
        );
    }
}

/// Domain validation at the wire boundary (S11, review 2026-08-07).
#[cfg(test)]
mod validation_tests {
    use super::*;

    #[test]
    fn ordinary_hosts_pass() {
        for d in [
            "example.com",
            "sub.example.com",
            "a.b.c.d.example.co.uk",
            "xn--80ak6aa92e.com", // punycode is already plain ASCII here
            "my-site1.example",
            "localhost",
        ] {
            assert_eq!(parse_domain(d), Some(d.to_string()), "{d}");
        }
    }

    #[test]
    fn normalization_still_happens_before_validation() {
        assert_eq!(
            parse_domain("HTTPS://www.Example.COM:8080/path?q=1"),
            Some("example.com".into())
        );
    }

    /*
     * THE case this exists for. `DnsFilterPlan` and Firefox `WebsiteFilter`
     * patterns are line-oriented sinks; a "host" carrying a newline is a
     * second line of config that nobody authored.
     */
    #[test]
    fn a_host_with_a_control_character_is_refused_not_repaired() {
        for d in [
            "evil.example\nallow: *",
            "evil.example\r\nblock",
            "evil.example\u{0}",
            "evil.example\ttab",
        ] {
            assert_eq!(parse_domain(d), None, "{d:?} must be rejected outright");
        }
    }

    #[test]
    fn other_out_of_charset_hosts_are_refused() {
        for d in [
            "евил.example", // raw Unicode — a homograph must never be guessed at
            "evil example", // space
            "evil.example;rm -rf /",
            "evil_example", // underscore is not an LDH character
            "evil.example\"",
            "*.example.com", // wildcards are not hosts; a pattern layer's job
        ] {
            assert_eq!(parse_domain(d), None, "{d:?}");
        }
    }

    #[test]
    fn empty_and_oversized_are_refused() {
        assert_eq!(parse_domain(""), None);
        assert_eq!(parse_domain("   "), None);
        assert_eq!(parse_domain("."), None);
        assert_eq!(parse_domain("a..b"), None, "an empty label");
        assert_eq!(parse_domain(".example.com"), None, "a leading dot");
        let long_label = "a".repeat(64);
        assert_eq!(parse_domain(&format!("{long_label}.com")), None);
        let too_long = format!("{}.com", vec!["abcdefghij"; 26].join("."));
        assert_eq!(parse_domain(&too_long), None, "over 253 bytes");
    }

    // The trailing DNS root dot is stripped by normalization, so it is valid —
    // that behaviour predates this and must not regress (it is what closes the
    // trailing-dot parentDeny bypass).
    #[test]
    fn a_trailing_root_dot_still_normalizes_rather_than_failing() {
        assert_eq!(parse_domain("evil.example."), Some("evil.example".into()));
    }
}
