//! Render an `EffectiveWebPolicy` into a Firefox ESR `policies.json` document.
//!
//! Pure: returns a `serde_json::Value` shaped `{ "policies": { ... } }`. The
//! `web_policy` enactor writes it root-owned + immutable. Machine policy is
//! profile-independent and highest-precedence, so a child cannot escape it by
//! resetting the profile or passing `--user-data-dir`.
//!
//! SafeSearch is deliberately absent — Firefox has no SafeSearch policy; it is
//! enforced at the DNS layer (forced-SafeSearch CNAMEs), so it never appears
//! here. Categories likewise expand to domains only at the DNS layer.

use serde_json::{json, Value};

use charter_content::{EffectiveWebPolicy, Posture};

/// Match patterns covering a bare host *and* its subdomains.
fn host_patterns(domain: &str) -> Vec<String> {
    vec![format!("*://{domain}/*"), format!("*://*.{domain}/*")]
}

/// Render the full Firefox policy document for an effective web policy.
pub fn render_firefox_policies(policy: &EffectiveWebPolicy) -> Value {
    // Static hardening, applied in every state: close the escape hatches that
    // would otherwise let the child route around the network filter.
    let mut policies = json!({
        "BlockAboutConfig": true,
        "DisablePrivateBrowsing": true,
        "DisableDeveloperTools": true,
        "DisableSafeMode": true,
        "DisableTelemetry": true,
        "DisableFirefoxStudies": true,
        // No DoH escape: a child resolver bypass is the highest-value lock.
        "DNSOverHTTPS": { "Enabled": false, "Locked": true },
        // ECH encrypts SNI; off so SNI-visible filtering keeps working.
        "DisableEncryptedClientHello": true,
        "Preferences": {
            // QUIC/HTTP3 off (locked) so UDP-443 can't bypass the TCP path.
            "network.http.http3.enabled": { "Value": false, "Status": "locked" },
            // TRR (DoH) off by choice, belt-and-braces with DNSOverHTTPS above.
            "network.trr.mode": { "Value": 5, "Status": "locked" }
        }
    });

    // WebsiteFilter expresses the allow/deny posture (or a full lock).
    let website_filter = if policy.locked {
        // Fail-closed / paused: block everything, no exceptions.
        json!({ "Block": ["<all_urls>"] })
    } else {
        match policy.posture {
            Some(Posture::Allowlist) => {
                let exceptions: Vec<String> = policy
                    .allow_domains
                    .iter()
                    .flat_map(|d| host_patterns(d))
                    .collect();
                json!({ "Block": ["<all_urls>"], "Exceptions": exceptions })
            }
            Some(Posture::Blocklist) => {
                let block: Vec<String> = policy
                    .block_domains
                    .iter()
                    .flat_map(|d| host_patterns(d))
                    .collect();
                json!({ "Block": block })
            }
            // Revoked / no constraint: no WebsiteFilter (hardening still applies).
            None => Value::Null,
        }
    };

    if !website_filter.is_null() {
        policies["WebsiteFilter"] = website_filter;
    }

    json!({ "policies": policies })
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_content::{evaluate_content, AgeTier, GrantContent, Posture};

    fn clause(posture: Posture) -> GrantContent {
        GrantContent {
            v: 1,
            tz: None,
            posture,
            age_tier: AgeTier::Young,
            curators: vec![],
            quorum_n: None,
            block_categories: vec![],
            safe_search: None,
            youtube_restrict: None,
            parent_allow: vec![],
            parent_deny: vec![],
            paused: None,
            revoked: None,
            issued_at: 1,
        }
    }

    fn strings(v: &Value) -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn hardening_present_in_every_state() {
        let p = evaluate_content(&clause(Posture::Allowlist), &[]);
        let doc = render_firefox_policies(&p);
        let pol = &doc["policies"];
        assert_eq!(pol["DNSOverHTTPS"]["Enabled"], json!(false));
        assert_eq!(pol["DNSOverHTTPS"]["Locked"], json!(true));
        assert_eq!(pol["DisableDeveloperTools"], json!(true));
        assert_eq!(pol["DisablePrivateBrowsing"], json!(true));
        assert_eq!(pol["DisableEncryptedClientHello"], json!(true));
        assert_eq!(
            pol["Preferences"]["network.http.http3.enabled"]["Value"],
            json!(false)
        );
    }

    #[test]
    fn allowlist_blocks_all_and_excepts_each_domain() {
        let mut c = clause(Posture::Allowlist);
        c.parent_allow = vec!["kids.example".into(), "school.example".into()];
        let p = evaluate_content(&c, &[]);
        let wf = &render_firefox_policies(&p)["policies"]["WebsiteFilter"];
        assert_eq!(wf["Block"], json!(["<all_urls>"]));
        let exc = strings(&wf["Exceptions"]);
        assert!(exc.contains(&"*://kids.example/*".to_string()));
        assert!(exc.contains(&"*://*.kids.example/*".to_string()));
        assert!(exc.contains(&"*://school.example/*".to_string()));
    }

    #[test]
    fn blocklist_blocks_each_domain_with_no_exceptions() {
        let mut c = clause(Posture::Blocklist);
        c.parent_deny = vec!["bad.example".into()];
        let p = evaluate_content(&c, &[]);
        let wf = &render_firefox_policies(&p)["policies"]["WebsiteFilter"];
        let block = strings(&wf["Block"]);
        assert!(block.contains(&"*://bad.example/*".to_string()));
        assert!(wf.get("Exceptions").is_none());
    }

    #[test]
    fn locked_blocks_everything_no_exceptions() {
        let mut c = clause(Posture::Allowlist);
        c.parent_allow = vec!["kids.example".into()]; // would be allowed if not locked
        c.paused = Some(true);
        let p = evaluate_content(&c, &[]);
        let wf = &render_firefox_policies(&p)["policies"]["WebsiteFilter"];
        assert_eq!(wf["Block"], json!(["<all_urls>"]));
        assert!(wf.get("Exceptions").is_none());
    }

    #[test]
    fn revoked_has_hardening_but_no_website_filter() {
        let mut c = clause(Posture::Blocklist);
        c.revoked = Some(true);
        let p = evaluate_content(&c, &[]);
        let doc = render_firefox_policies(&p);
        assert!(doc["policies"].get("WebsiteFilter").is_none());
        assert_eq!(doc["policies"]["DisableDeveloperTools"], json!(true));
    }
}
