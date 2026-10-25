//! Relay recovery works without any functioning relay. Only the pinned
//! guardian can authorise a replacement list; identity and clauses survive.

use charter_primitives::{kinds, EventId, NostrEvent, PubKey};
use charter_proto::canonical_event_string;
use serde::Deserialize;

use crate::pairing::Pairing;

pub const DEFAULT_RELAYS: [&str; 3] = [
    "wss://relay.snort.social",
    "wss://nos.lol",
    "wss://relay.primal.net",
];
pub const TROTTERS_RELAY: &str = "wss://relay.trotters.cc";
pub const CONFIG_TTL_SECS: u64 = 600;

pub fn default_relays() -> Vec<String> {
    DEFAULT_RELAYS.iter().map(|r| (*r).to_string()).collect()
}

/// Canonical secure endpoints, bounded to keep network fan-out manageable.
/// Two distinct endpoints are required for every explicit relay change.
pub fn validate_relays(relays: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    if relays.len() > 10 {
        return Err("Choose between two and ten secure relays.".into());
    }
    for relay in relays {
        let url = url::Url::parse(relay.trim()).map_err(|_| "Invalid relay URL.")?;
        if url.scheme() != "wss"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
        {
            return Err(
                "Each relay must be a wss:// URL without credentials or a fragment.".into(),
            );
        }
        let mut value = url.to_string();
        if url.path() == "/" && url.query().is_none() {
            value.pop();
        }
        if !out.contains(&value) {
            out.push(value);
        }
    }
    if out.len() < 2 {
        return Err("Choose at least two different secure relays.".into());
    }
    Ok(out)
}

/// Expand a legacy Trotters pairing once on upgrade, preserving that family's
/// configured relay while adding independent routes. Later choices survive.
pub fn migrate_legacy_relays(pairing: &mut Pairing) -> bool {
    if pairing.relays_updated_at.is_some() {
        return false;
    }
    let uses_trotters = pairing.relays.iter().any(|r| {
        url::Url::parse(r.trim())
            .ok()
            .and_then(|u| u.host_str().map(str::to_owned))
            .as_deref()
            == Some("relay.trotters.cc")
    });
    if !uses_trotters {
        return false;
    }
    for relay in default_relays() {
        if !pairing.relays.contains(&relay) {
            pairing.relays.push(relay);
        }
    }
    pairing.relays_updated_at = Some(0);
    true
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayConfig {
    machine: PubKey,
    relays: Vec<String>,
}

/// Validate before mutating; callers must persist successfully before swapping
/// their live transport. The signed JSON can be pasted or read from a file.
pub fn apply_signed_config(pairing: &mut Pairing, json: &str, now: u64) -> Result<(), String> {
    if json.len() > 16_384 {
        return Err("Relay code is too long.".into());
    }
    let event: NostrEvent = serde_json::from_str(json).map_err(|_| "Invalid signed relay code.")?;
    let hash = charter_crypto::sha256(canonical_event_string(&event).as_bytes());
    if event.pubkey != pairing.guardian_pubkey
        || event.kind != kinds::CHARTER_DEVICE_RELAYS
        || !event.has_tag("t", "charter-device")
        || !event.has_tag("p", &pairing.machine.to_hex())
        || event.id != EventId::from_bytes(hash)
        || !charter_crypto::schnorr_verify(event.pubkey.as_bytes(), &hash, event.sig.as_bytes())
    {
        return Err("This relay code was not signed by this device's guardian.".into());
    }
    if event.created_at > now
        || now.saturating_sub(event.created_at) > CONFIG_TTL_SECS
        || event.created_at < pairing.paired_at
        || event.created_at <= pairing.relays_updated_at.unwrap_or(0)
    {
        return Err("This relay code has expired or was already used. Create a fresh code.".into());
    }
    let config: RelayConfig =
        serde_json::from_str(&event.content).map_err(|_| "Invalid relay configuration.")?;
    if config.machine != pairing.machine {
        return Err("This relay code is for a different device.".into());
    }
    let relays = validate_relays(&config.relays)?;
    pairing.relays = relays;
    pairing.relays_updated_at = Some(event.created_at);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{nip59::sign_event_raw, pin_from_connect};

    fn pairing() -> Pairing {
        let guardian = PubKey::from_bytes(charter_crypto::xonly_pubkey(&[1; 32]).unwrap());
        pin_from_connect(
            &format!(
                "bunker://{}?relay={TROTTERS_RELAY}&kind=charter",
                guardian.to_hex()
            ),
            PubKey::from_bytes([2; 32]),
            PubKey::from_bytes([3; 32]),
            100,
        )
        .unwrap()
    }

    fn code(pairing: &Pairing, sk: [u8; 32], ts: u64, relays: Vec<String>) -> String {
        serde_json::to_string(
            &sign_event_raw(
                &sk,
                kinds::CHARTER_DEVICE_RELAYS,
                ts,
                vec![
                    kinds::marker_tag(),
                    vec!["p".into(), pairing.machine.to_hex()],
                ],
                serde_json::json!({"machine": pairing.machine, "relays": relays}).to_string(),
            )
            .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn legacy_recovery_preserves_identity_and_never_removes_a_manual_addition() {
        let mut p = pairing();
        p.relays_updated_at = None;
        p.relays.push("wss://family.example".into());
        let before = p.clone();
        assert!(migrate_legacy_relays(&mut p));
        assert!(p.relays.iter().any(|r| r == TROTTERS_RELAY));
        assert!(p.relays.contains(&"wss://family.example".to_string()));
        assert_eq!(p.relays.len(), 5);
        assert_eq!(p.guardian_pubkey, before.guardian_pubkey);
        assert_eq!(p.machine, before.machine);
        assert_eq!(p.subject_pubkey, before.subject_pubkey);
        assert_eq!(p.paired_at, before.paired_at);
        p.relays.push(TROTTERS_RELAY.into());
        let json = serde_json::to_string(&p).unwrap();
        let mut restarted: Pairing = serde_json::from_str(&json).unwrap();
        assert!(!migrate_legacy_relays(&mut restarted));
        assert_eq!(restarted, p);
    }

    #[test]
    fn new_pairings_and_custom_legacy_relays_are_not_rewritten() {
        let mut p = pairing(); // explicit new addition of Trotters
        assert!(!migrate_legacy_relays(&mut p));
        p.relays = vec!["wss://family.example".into()];
        p.relays_updated_at = None;
        assert!(!migrate_legacy_relays(&mut p));
    }

    #[test]
    fn offline_change_is_authenticated_and_persists_its_replay_floor() {
        let mut p = pairing();
        let before = p.clone();
        let signed = code(&p, [1; 32], 200, default_relays());
        apply_signed_config(&mut p, &signed, 200).unwrap();
        assert_eq!(p.relays, default_relays());
        assert_eq!(p.guardian_pubkey, before.guardian_pubkey);
        assert_eq!(p.subject_pubkey, before.subject_pubkey);
        assert_eq!(p.paired_at, before.paired_at);
        let mut restarted: Pairing =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(apply_signed_config(&mut restarted, &signed, 201).is_err());
        let manual = code(
            &p,
            [1; 32],
            202,
            vec![TROTTERS_RELAY.into(), DEFAULT_RELAYS[0].into()],
        );
        apply_signed_config(&mut p, &manual, 202).unwrap();
        assert!(!migrate_legacy_relays(&mut p));
        assert!(p.relays.contains(&TROTTERS_RELAY.to_string()));
    }

    #[test]
    fn invalid_codes_never_mutate_the_pairing() {
        let original = pairing();
        let mut other_machine = original.clone();
        other_machine.machine = PubKey::from_bytes([9; 32]);
        let cases = [
            (code(&original, [9; 32], 200, default_relays()), 200),
            (code(&other_machine, [1; 32], 200, default_relays()), 200),
            (code(&original, [1; 32], 200, default_relays()), 801),
            (code(&original, [1; 32], 201, default_relays()), 200),
            (code(&original, [1; 32], 99, default_relays()), 200),
            (
                code(&original, [1; 32], 200, vec!["wss://one.example".into()]),
                200,
            ),
            (
                code(
                    &original,
                    [1; 32],
                    200,
                    vec!["ws://one.example".into(), "wss://two.example".into()],
                ),
                200,
            ),
        ];
        for (json, now) in cases {
            let mut p = original.clone();
            assert!(apply_signed_config(&mut p, &json, now).is_err());
            assert_eq!(p, original);
        }
        let mut tampered: serde_json::Value =
            serde_json::from_str(&code(&original, [1; 32], 200, default_relays())).unwrap();
        tampered["content"] = "{}".into();
        let mut p = original.clone();
        assert!(apply_signed_config(&mut p, &tampered.to_string(), 200).is_err());
        assert_eq!(p, original);
    }

    #[test]
    fn relay_validation_requires_distinct_secure_endpoints() {
        for relays in [
            vec![],
            vec!["wss://ONE.example".into(), "wss://one.example:443/".into()],
            vec!["wss://user@one.example".into(), "wss://two.example".into()],
            vec![
                "wss://one.example#fragment".into(),
                "wss://two.example".into(),
            ],
            vec!["wss://one.example".into(); 11],
        ] {
            assert!(validate_relays(&relays).is_err());
        }
        assert_eq!(
            validate_relays(&[
                " wss://ONE.example:443/ ".into(),
                "wss://two.example/path".into()
            ])
            .unwrap(),
            vec!["wss://one.example", "wss://two.example/path"]
        );
    }

    #[test]
    fn nostr_tools_relay_code_applies_without_any_network() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../charter-testkit/vectors/nostr/relay_config.json"
        ))
        .unwrap();
        let guardian = fixture["guardian"].as_str().unwrap();
        let machine = PubKey::from_hex(fixture["machine"].as_str().unwrap()).unwrap();
        let mut pairing = pin_from_connect(
            &format!("bunker://{guardian}?relay={TROTTERS_RELAY}&kind=charter"),
            machine,
            machine,
            1_699_999_999,
        )
        .unwrap();
        apply_signed_config(&mut pairing, &fixture["event"].to_string(), 1_700_000_000).unwrap();
        assert_eq!(pairing.relays, default_relays());
    }
}
