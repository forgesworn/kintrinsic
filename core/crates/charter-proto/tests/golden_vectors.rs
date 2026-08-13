//! Proves `charter-proto` round-trips REAL nostr-tools golden vectors: the
//! NIP-01 canonical serialization byte-matches, and the GRANT/CLAUSE payloads
//! parse from the signed event content.

use charter_primitives::{NostrEvent, PubKey};
use charter_proto::{
    canonical_event_string, ClauseKind, ClausePayload, GrantParams, GrantPayload, OpType,
};

#[derive(serde::Deserialize)]
struct NipVector {
    event: NostrEvent,
    canonical: String,
}

#[derive(serde::Deserialize)]
struct GrantVector {
    event: NostrEvent,
    #[allow(dead_code)]
    guardian_pubkey: PubKey,
    canonical: String,
}

#[derive(serde::Deserialize)]
struct ClauseVector {
    event: NostrEvent,
    #[allow(dead_code)]
    guardian_pubkey: PubKey,
    canonical: String,
}

#[test]
fn nip01_canonical_byte_matches_nostr_tools() {
    let v: NipVector = charter_testkit::golden::load_json("nostr/nip01_event.json");
    assert_eq!(canonical_event_string(&v.event), v.canonical);
}

#[test]
fn grant_canonical_byte_matches_and_payload_parses() {
    let v: GrantVector = charter_testkit::golden::load_json("nostr/grant_install_allow.json");
    assert_eq!(canonical_event_string(&v.event), v.canonical);

    let g = GrantPayload::from_json(&v.event.content).unwrap();
    assert_eq!(g.op, OpType::InstallFlatpak);
    match g.grant_params().unwrap() {
        GrantParams::InstallFlatpak(p) => assert_eq!(p.reference, "org.videolan.VLC"),
        _ => panic!("wrong params variant"),
    }
}

#[test]
fn exec_and_time_extend_grants_parse() {
    let v: GrantVector = charter_testkit::golden::load_json("nostr/grant_exec_allow.json");
    let g = GrantPayload::from_json(&v.event.content).unwrap();
    assert!(matches!(
        g.grant_params().unwrap(),
        GrantParams::ExecAllow(_)
    ));

    let v: GrantVector = charter_testkit::golden::load_json("nostr/grant_time_extend_allow.json");
    let g = GrantPayload::from_json(&v.event.content).unwrap();
    match g.grant_params().unwrap() {
        GrantParams::TimeExtend(p) => assert_eq!(p.minutes_granted, 30),
        _ => panic!("wrong params variant"),
    }
}

#[test]
fn clause_canonical_byte_matches_and_payload_parses() {
    let v: ClauseVector = charter_testkit::golden::load_json("nostr/clause_schedule.json");
    assert_eq!(canonical_event_string(&v.event), v.canonical);

    let c = ClausePayload::from_json(&v.event.content).unwrap();
    assert_eq!(c.kind, ClauseKind::Schedule);
    assert_eq!(c.issued_at, 1_700_000_000);
}
