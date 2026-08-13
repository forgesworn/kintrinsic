//! Emits a deterministic guardian-signed CLAUSE fixture for the on-device e2e
//! test. Run: `cargo run --example gen_fixture --features charter-verify/mock`.
//! Prints guardian pubkey, ward subject, and a signed "paused" schedule clause
//! (paused → always locked, so the emulator assertion is time-independent).

use charter_verify::test_support::{ClauseBuilder, TestGuardian};
use serde_json::json;

fn main() {
    let guardian = TestGuardian::new();
    let ward = TestGuardian::from_seed(0x44);

    // Paused → always locked (time-independent lock assertion).
    let paused = ClauseBuilder::schedule(1)
        .subject(ward.pubkey())
        .body(json!({ "v": 1, "tz": "Europe/London", "paused": true, "weekly": {}, "issuedAt": 1 }))
        .build(&guardian);

    // Empty weekly → always ALLOWED (I24) but still a charter: unlocked yet
    // configured, so restrictions must stay up (the review's critical fix).
    let open = ClauseBuilder::schedule(2)
        .subject(ward.pubkey())
        .body(json!({ "v": 1, "tz": "Europe/London", "weekly": {}, "issuedAt": 2 }))
        .build(&guardian);

    // Tethering fixtures (store_key 7): the on-device test walks the whole
    // posture ladder — raw (far-future until, active at any realistic `now`),
    // filtered (no until), revoked, then an already-expired grant (monotonic
    // issuedAt makes each supersede the last).
    let tether = |issued_at: u64, body: serde_json::Value| {
        ClauseBuilder {
            kind: charter_proto::ClauseKind::Tethering,
            issued_at,
            body: json!({}),
            created_at: issued_at,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(body)
        .build(&guardian)
    };
    let tether_raw = tether(
        3,
        json!({ "v": 1, "issuedAt": 3, "allow": "raw", "until": 4_000_000_000u64 }),
    );
    let tether_filtered = tether(5, json!({ "v": 1, "issuedAt": 5, "allow": "filtered" }));
    let tether_none = tether(6, json!({ "v": 1, "issuedAt": 6, "allow": "none" }));
    let tether_expired = tether(
        7,
        json!({ "v": 1, "issuedAt": 7, "allow": "raw", "until": 2_000u64 }),
    );

    println!("GUARDIAN={}", guardian.pubkey().to_hex());
    println!("SUBJECT={}", ward.pubkey().to_hex());
    println!("CLAUSE_PAUSED={}", serde_json::to_string(&paused).unwrap());
    println!("CLAUSE_OPEN={}", serde_json::to_string(&open).unwrap());
    println!(
        "CLAUSE_TETHER_RAW={}",
        serde_json::to_string(&tether_raw).unwrap()
    );
    println!(
        "CLAUSE_TETHER_FILTERED={}",
        serde_json::to_string(&tether_filtered).unwrap()
    );
    println!(
        "CLAUSE_TETHER_NONE={}",
        serde_json::to_string(&tether_none).unwrap()
    );
    println!(
        "CLAUSE_TETHER_EXPIRED={}",
        serde_json::to_string(&tether_expired).unwrap()
    );
}
