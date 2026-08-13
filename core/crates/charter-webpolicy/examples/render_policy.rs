//! Render the REAL Firefox `policies.json` for a web-content clause, via the
//! production evaluate+render path — for the on-metal Linux round.
//!   render_policy on   -> blocklist: block youtube.com, allow wikipedia.org
//!   render_policy off  -> revoked (filtering lifted; hardening only)

use charter_content::evaluate_content_json;
use charter_webpolicy::render_firefox_policies;

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "on".to_string());
    let clause = if mode == "off" {
        r#"{"v":1,"posture":"blocklist","ageTier":"older","revoked":true,"issuedAt":1}"#
    } else {
        r#"{"v":1,"posture":"blocklist","ageTier":"older","parentAllow":["wikipedia.org"],"parentDeny":["youtube.com"],"safeSearch":true,"youtubeRestrict":"strict","issuedAt":1}"#
    };
    let eff = evaluate_content_json(clause, &[]);
    let doc = render_firefox_policies(&eff);
    println!("{}", serde_json::to_string_pretty(&doc).unwrap());
}
