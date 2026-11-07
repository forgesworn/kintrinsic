//! Relay recovery works without any functioning relay. A RELAYS, pasted as a
//! code or gift-wrapped to the ward, comes from a key in its signer set, and
//! only a root may change the relay list (rule 11): a working key's RELAYS
//! carries the guardian PIN (gift-wrapped only) or battery settings and leaves
//! the list as it is. Identity and clauses survive either way.

use charter_primitives::{kinds, EventId, NostrEvent, PubKey, SignerSet};
use charter_proto::canonical_event_string;
use serde::{Deserialize, Serialize};

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
        let value = normalise_relay(relay)?;
        if !out.contains(&value) {
            out.push(value);
        }
    }
    if out.len() < 2 {
        return Err("Choose at least two different secure relays.".into());
    }
    Ok(out)
}

/// One relay URL in the canonical form [`validate_relays`] stores.
fn normalise_relay(relay: &str) -> Result<String, String> {
    let url = url::Url::parse(relay.trim()).map_err(|_| "Invalid relay URL.")?;
    if url.scheme() != "wss"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err("Each relay must be a wss:// URL without credentials or a fragment.".into());
    }
    let mut value = url.to_string();
    if url.path() == "/" && url.query().is_none() {
        value.pop();
    }
    Ok(value)
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

/// Android network scheduling only; enforcement and LAN monitoring are independent.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BatterySettings {
    pub screen_off_secs: u64,
    pub ordinary_secs: u64,
    pub request_boost: bool,
    pub opening_boost: bool,
    pub boost_secs: u64,
}
impl BatterySettings {
    pub fn valid(&self) -> bool {
        [120, 300, 600].contains(&self.screen_off_secs)
            && [30, 60, 120].contains(&self.ordinary_secs)
            && [60, 120, 180].contains(&self.boost_secs)
    }
}

/// The ward's guardian PIN record, as a guardian sets it inside a RELAYS body
/// (contract, *Presence confirmation*): scrypt (N=2^15, r=8, p=1, 32-byte
/// output) of the PIN's UTF-8 bytes under `salt`. Never stored in the pairing
/// record, which is not a secret: [`apply_signed_relays`] hands it back, and
/// the guardian-keys state keeps it in its `0600` file (review 2a, L2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuardianPin {
    /// Format version: 1.
    pub v: u32,
    /// The scrypt salt, lowercase hex of 16 to 64 bytes.
    pub salt: String,
    /// The 32-byte scrypt output, lowercase hex.
    pub hash: String,
}

impl GuardianPin {
    /// Whether this record has the shape version 1 requires (open question
    /// "Guardian PIN salt and encoding": salt is 16 to 64 bytes of lowercase
    /// hex, hash is 32 bytes of lowercase hex).
    pub fn is_valid(&self) -> bool {
        fn lower_hex(s: &str) -> bool {
            s.len().is_multiple_of(2)
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        self.v == 1
            && lower_hex(&self.salt)
            && (32..=128).contains(&self.salt.len())
            && lower_hex(&self.hash)
            && self.hash.len() == 64
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RelayConfig {
    machine: PubKey,
    /// Absent in a RELAYS that carries only the guardian PIN or the battery
    /// settings (rule 11): it leaves the relay list as it is.
    #[serde(default)]
    relays: Option<Vec<String>>,
    #[serde(default)]
    battery: Option<BatterySettings>,
    #[serde(default)]
    guardian_pin: Option<GuardianPin>,
}

/// How long a ward with more than one root also sends STATUS on its previous
/// relays after a RELAYS change (rule 11).
pub const PREVIOUS_RELAYS_SECS: u64 = 7 * 24 * 60 * 60;

/// The refusal for a RELAYS from a working key that would change the relay
/// list (rule 11; the reference's `relays_root_only`).
pub const RELAYS_ROOT_ONLY: &str = "Only a parent's own identity can change this device's relays.";

/// The relay list a RELAYS change replaced, and until when STATUS should
/// still go out on it (rule 11). The guardian-keys port keeps it and hands
/// the lists back for STATUS emission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviousRelays {
    pub relays: Vec<String>,
    /// Ward time (unix seconds) after which the list is no longer used.
    pub until: u64,
}

/// What a gift-wrapped RELAYS change did to the pairing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaysApplied {
    /// The list it replaced (rule 11), or `None` when the relay list did not
    /// change: the body had no `relays` (a PIN-only or battery-only RELAYS),
    /// or the same set. No change starts no previous-relays window.
    pub previous: Option<PreviousRelays>,
    /// Whether the body carried a relay list at all (the same set counts).
    pub had_relays: bool,
    /// The guardian PIN record it carried, validated. NOT written into the
    /// pairing: the caller stores it with the guardian-keys state, or drops
    /// it on a ward without one.
    pub pin: Option<GuardianPin>,
}

/// How a RELAYS reached the ward.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Channel {
    /// Typed or pasted at the ward: the 600-second rule always applies, and
    /// a `guardianPin` is refused.
    Pasted,
    /// Gift-wrapped over relays or the local link.
    Wrapped,
}

/// A RELAYS body that passed every check, its relay list validated.
struct Checked {
    relays: Option<Vec<String>>,
    battery: Option<BatterySettings>,
    guardian_pin: Option<GuardianPin>,
}

/// The same relay set, whatever the order and spelling: each side in its
/// canonical form (a stored entry that is not a valid relay stays as it is,
/// and so never matches a validated one).
fn same_set(a: &[String], b: &[String]) -> bool {
    let canon = |l: &[String]| {
        let mut v: Vec<String> = l
            .iter()
            .map(|r| normalise_relay(r).unwrap_or_else(|_| r.clone()))
            .collect();
        v.sort();
        v.dedup();
        v
    };
    canon(a) == canon(b)
}

/// Validate before mutating; callers must persist successfully before swapping
/// their live transport. The signed JSON can be pasted or read from a file.
///
/// Only the pinned guardian can sign a pasted code, and a pasted code may not
/// carry a `guardianPin`: a code passes through whoever pastes it, so a PIN
/// reaches a ward only gift-wrapped from a key in its signer set (see
/// [`apply_signed_relays`] and the open question "How a guardian PIN reaches
/// an Android ward").
pub fn apply_signed_config(pairing: &mut Pairing, json: &str, now: u64) -> Result<(), String> {
    let pinned = pairing.guardian_pubkey;
    apply_signed_config_from(pairing, json, &pinned, now)
}

/// [`apply_signed_config`] with the author checked against `signers` rather
/// than the pinned guardian alone: on a ward with the guardian-keys section,
/// "a RELAYS change from any signer applies as usual" (rule 11; review 2a,
/// Info 6). The pasted-code rules are unchanged: the 600-second rule, and a
/// `guardianPin` refused. Only a root in `signers` may change the relay list
/// (rule 11): a working key's code must leave it as it is, by having no
/// `relays` (battery settings only) or the same set.
pub fn apply_signed_config_from(
    pairing: &mut Pairing,
    json: &str,
    signers: &dyn SignerSet,
    now: u64,
) -> Result<(), String> {
    if json.len() > 16_384 {
        return Err("Relay code is too long.".into());
    }
    let event: NostrEvent = serde_json::from_str(json).map_err(|_| "Invalid signed relay code.")?;
    let config = check_relays_event(pairing, &event, signers, Channel::Pasted, now)?;
    if config.guardian_pin.is_some() {
        return Err("A guardian PIN can't be set from a pasted code.".into());
    }
    if let Some(battery) = config.battery {
        pairing.battery = Some(battery);
    }
    if let Some(relays) = config.relays {
        if !same_set(&relays, &pairing.relays) {
            pairing.relays = relays;
        }
    }
    pairing.relays_updated_at = Some(event.created_at);
    Ok(())
}

/// Apply a RELAYS event that arrived gift-wrapped (over relays or the local
/// link) from a key in `signers` (rule 11). The checks are
/// [`apply_signed_config`]'s — kind, marker, `p` names this machine, id and
/// signature, not future-dated, at most 600 seconds old, not before pairing,
/// newer than the last change, a working key leaving the relay list as it is
/// — with the author checked against the signer set.
///
/// - A `guardianPin` is accepted here and returned in
///   [`RelaysApplied::pin`]; the pairing never holds it.
/// - A **PIN-only RELAYS** (a `guardianPin` and no `relays`) is checked here
///   only against the pairing time: not for its age (a ward that was asleep
///   still takes the PIN), nor for being ahead of the ward clock (a guardian
///   phone a few minutes fast still sets it), nor against the relay list's
///   floor. Its replay floor is per signer and lives in the guardian-keys
///   state, which the caller records it in (rule 11). It changes no relay
///   list, leaves `relays_updated_at` alone, so a PIN resend never makes a
///   later relay code look already used, starts no previous-relays window
///   and applies no battery settings (they would sit outside the relay
///   list's floor).
///
/// Nothing is mutated on `Err`. The caller must have checked that the seal
/// author is the event's author.
pub fn apply_signed_relays(
    pairing: &mut Pairing,
    event: &NostrEvent,
    signers: &dyn SignerSet,
    now: u64,
) -> Result<RelaysApplied, String> {
    let config = check_relays_event(pairing, event, signers, Channel::Wrapped, now)?;
    if let Some(pin) = &config.guardian_pin {
        if !pin.is_valid() {
            return Err("Invalid guardian PIN record.".into());
        }
    }
    let had_relays = config.relays.is_some();
    if !had_relays && config.guardian_pin.is_some() {
        return Ok(RelaysApplied {
            previous: None,
            had_relays,
            pin: config.guardian_pin,
        });
    }
    let previous = match config.relays {
        Some(relays) if !same_set(&relays, &pairing.relays) => Some(PreviousRelays {
            relays: std::mem::replace(&mut pairing.relays, relays),
            until: now.saturating_add(PREVIOUS_RELAYS_SECS),
        }),
        _ => None,
    };
    pairing.relays_updated_at = Some(event.created_at);
    if let Some(battery) = config.battery {
        pairing.battery = Some(battery);
    }
    Ok(RelaysApplied {
        previous,
        had_relays,
        pin: config.guardian_pin,
    })
}

/// The checks every RELAYS event passes, however it arrived, returning its
/// parsed body with the relay list validated. `authors` is the pinned
/// guardian for a pasted code, the signer set otherwise. Order as the
/// reference's `apply_relays`: author, freshness (only the pairing time for a
/// gift-wrapped PIN-only RELAYS, whose replay floor the guardian-keys state
/// keeps per signer), the relay list, root-only.
fn check_relays_event(
    pairing: &Pairing,
    event: &NostrEvent,
    authors: &dyn SignerSet,
    channel: Channel,
    now: u64,
) -> Result<Checked, String> {
    let hash = charter_crypto::sha256(canonical_event_string(event).as_bytes());
    if !authors.is_signer(&event.pubkey)
        || event.kind != kinds::CHARTER_DEVICE_RELAYS
        || !event.has_tag("t", "charter-device")
        || !event.has_tag("p", &pairing.machine.to_hex())
        || event.id != EventId::from_bytes(hash)
        || !charter_crypto::schnorr_verify(event.pubkey.as_bytes(), &hash, event.sig.as_bytes())
    {
        return Err("This relay code was not signed by this device's guardian.".into());
    }
    let config: RelayConfig =
        serde_json::from_str(&event.content).map_err(|_| "Invalid relay configuration.")?;
    let pin_only =
        channel == Channel::Wrapped && config.relays.is_none() && config.guardian_pin.is_some();
    let fresh = pin_only
        || (event.created_at <= now
            && now.saturating_sub(event.created_at) <= CONFIG_TTL_SECS
            && event.created_at > pairing.relays_updated_at.unwrap_or(0));
    if !fresh || event.created_at < pairing.paired_at {
        return Err("This relay code has expired or was already used. Create a fresh code.".into());
    }
    if config.machine != pairing.machine {
        return Err("This relay code is for a different device.".into());
    }
    if config.battery.as_ref().is_some_and(|b| !b.valid()) {
        return Err("Invalid battery settings.".into());
    }
    let relays = config.relays.as_deref().map(validate_relays).transpose()?;
    if let Some(list) = &relays {
        if !same_set(list, &pairing.relays) && !authors.roots().contains(&event.pubkey) {
            return Err(RELAYS_ROOT_ONLY.into());
        }
    }
    Ok(Checked {
        relays,
        battery: config.battery,
        guardian_pin: config.guardian_pin,
    })
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
    fn invalid_stored_battery_metadata_does_not_corrupt_pairing() {
        let mut value = serde_json::to_value(pairing()).unwrap();
        for invalid in [
            serde_json::json!("outdated"),
            serde_json::json!({"ordinarySecs":-1}),
        ] {
            value["battery"] = invalid;
            let restored: Pairing = serde_json::from_value(value.clone()).unwrap();
            assert!(restored.battery.is_none());
            assert_eq!(restored.guardian_pubkey, pairing().guardian_pubkey);
        }
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../charter-testkit/vectors/nostr/battery_config.json"
        ))
        .unwrap();
        let mut p = pairing();
        p.guardian_pubkey = PubKey::from_hex(fixture["guardian"].as_str().unwrap()).unwrap();
        p.machine = PubKey::from_hex(fixture["machine"].as_str().unwrap()).unwrap();
        apply_signed_config(&mut p, &fixture["event"].to_string(), 1_700_000_000).unwrap();
        assert_eq!(p.battery.unwrap().ordinary_secs, 60);
    }

    #[test]
    fn battery_configuration_is_signed_device_scoped_persisted_and_replay_protected() {
        let mut p = pairing();
        let mut event: NostrEvent =
            serde_json::from_str(&code(&p, [1; 32], 101, default_relays())).unwrap();
        let battery = serde_json::json!({"screenOffSecs":600,"ordinarySecs":120,"requestBoost":true,"openingBoost":false,"boostSecs":60});
        let mut content: serde_json::Value = serde_json::from_str(&event.content).unwrap();
        content["battery"] = battery;
        event = sign_event_raw(
            &[1; 32],
            event.kind,
            event.created_at,
            event.tags.clone(),
            content.to_string(),
        )
        .unwrap();
        let signed = serde_json::to_string(&event).unwrap();
        let mut wrong_device = p.clone();
        wrong_device.machine = PubKey::from_bytes([9; 32]);
        assert!(apply_signed_config(&mut wrong_device, &signed, 102).is_err());
        apply_signed_config(&mut p, &signed, 102).unwrap();
        let mut restored: Pairing =
            serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(restored.battery.as_ref().unwrap().screen_off_secs, 600);
        assert!(apply_signed_config(&mut restored, &signed, 102).is_err());
        // An ordinary relay edit leaves the device's battery choice intact.
        let next = code(&p, [1; 32], 103, default_relays());
        apply_signed_config(&mut p, &next, 104).unwrap();
        assert_eq!(p.battery, restored.battery);
        content["battery"]["ordinarySecs"] = serde_json::json!(1);
        let invalid =
            sign_event_raw(&[1; 32], event.kind, 105, event.tags, content.to_string()).unwrap();
        let before = p.clone();
        assert!(
            apply_signed_config(&mut p, &serde_json::to_string(&invalid).unwrap(), 106).is_err()
        );
        assert_eq!(p, before);
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

    fn event_with(pairing: &Pairing, sk: [u8; 32], ts: u64, body: serde_json::Value) -> NostrEvent {
        sign_event_raw(
            &sk,
            kinds::CHARTER_DEVICE_RELAYS,
            ts,
            vec![
                kinds::marker_tag(),
                vec!["p".into(), pairing.machine.to_hex()],
            ],
            body.to_string(),
        )
        .unwrap()
    }

    fn pin() -> serde_json::Value {
        serde_json::json!({"v": 1, "salt": "ab".repeat(16), "hash": "cd".repeat(32)})
    }

    #[test]
    fn a_pasted_code_may_not_set_a_guardian_pin() {
        let original = pairing();
        let ev = event_with(
            &original,
            [1; 32],
            200,
            serde_json::json!({"machine": original.machine, "relays": default_relays(), "guardianPin": pin()}),
        );
        let mut p = original.clone();
        assert!(apply_signed_config(&mut p, &serde_json::to_string(&ev).unwrap(), 200).is_err());
        assert_eq!(p, original);
    }

    /// A root and one of its working keys, as a ward's signer set sees them.
    struct Two(PubKey, PubKey);
    impl SignerSet for Two {
        fn is_signer(&self, k: &PubKey) -> bool {
            *k == self.0 || *k == self.1
        }
        fn root_of(&self, _k: &PubKey) -> Option<PubKey> {
            Some(self.0)
        }
        fn is_departed(&self, _k: &PubKey) -> bool {
            false
        }
        fn recipients(&self) -> Vec<PubKey> {
            vec![self.1]
        }
        fn roots(&self) -> Vec<PubKey> {
            vec![self.0]
        }
    }

    fn root_and_working(p: &Pairing) -> Two {
        let working = PubKey::from_bytes(charter_crypto::xonly_pubkey(&[5; 32]).unwrap());
        Two(p.guardian_pubkey, working)
    }

    /// Rule 11 (D3): a root's gift-wrapped RELAYS may change the list and set
    /// the PIN; the replaced list opens the previous-relays window.
    #[test]
    fn a_roots_gift_wrapped_change_applies_and_may_set_the_pin() {
        let mut p = pairing();
        let set = root_and_working(&p);
        let before = p.relays.clone();
        let ev = event_with(
            &p,
            [1; 32],
            200,
            serde_json::json!({"machine": p.machine, "relays": default_relays(), "guardianPin": pin()}),
        );
        let applied = apply_signed_relays(&mut p, &ev, &set, 200).unwrap();
        assert_eq!(applied.pin.as_ref().unwrap().hash, "cd".repeat(32));
        let previous = applied.previous.unwrap();
        assert_eq!(previous.relays, before);
        assert_eq!(previous.until, 200 + PREVIOUS_RELAYS_SECS);
        assert!(applied.had_relays);
        assert_eq!(p.relays, default_relays());
        // The PIN never reaches the persisted pairing (L2).
        assert!(!serde_json::to_string(&p)
            .unwrap()
            .contains("cd".repeat(32).as_str()));
        // Same event again: not newer.
        assert!(apply_signed_relays(&mut p.clone(), &ev, &set, 201).is_err());
        // A key outside the set is refused, and nothing changes.
        let original = pairing();
        let mut q = original.clone();
        let stranger = event_with(
            &q,
            [9; 32],
            200,
            serde_json::json!({"machine": q.machine, "relays": default_relays()}),
        );
        assert!(apply_signed_relays(&mut q, &stranger, &set, 200).is_err());
        assert_eq!(q, original);
        // A malformed PIN refuses the whole change.
        let bad = event_with(
            &q,
            [1; 32],
            200,
            serde_json::json!({"machine": q.machine, "relays": default_relays(),
                "guardianPin": {"v": 1, "salt": "AB", "hash": "cd"}}),
        );
        assert!(apply_signed_relays(&mut q, &bad, &set, 200).is_err());
        assert_eq!(q, original);
    }

    /// Rule 11 (D3): a working key's RELAYS may not change the relay list,
    /// gift-wrapped or pasted; the same set, in any order, or no list, is
    /// fine, and the battery settings may ride on it.
    #[test]
    fn a_working_key_may_not_change_the_relay_list() {
        let original = pairing();
        let set = root_and_working(&original);
        let battery = serde_json::json!({"screenOffSecs":600,"ordinarySecs":120,"requestBoost":true,"openingBoost":false,"boostSecs":60});
        let mut q = original.clone();
        let change = event_with(
            &q,
            [5; 32],
            200,
            serde_json::json!({"machine": q.machine, "relays": default_relays(), "guardianPin": pin()}),
        );
        assert_eq!(
            apply_signed_relays(&mut q, &change, &set, 200),
            Err(RELAYS_ROOT_ONLY.into())
        );
        let pasted = serde_json::to_string(&event_with(
            &q,
            [5; 32],
            200,
            serde_json::json!({"machine": q.machine, "relays": default_relays()}),
        ))
        .unwrap();
        assert_eq!(
            apply_signed_config_from(&mut q, &pasted, &set, 200),
            Err(RELAYS_ROOT_ONLY.into())
        );
        assert_eq!(q, original);

        // The same set, reordered and spelt differently, with battery
        // settings: applied, the list untouched, no previous-relays window.
        let mut current = original.clone();
        current.relays = default_relays();
        let mut same: Vec<String> = default_relays().into_iter().rev().collect();
        same[0].push('/');
        let ev = event_with(
            &current,
            [5; 32],
            201,
            serde_json::json!({"machine": current.machine, "relays": same, "battery": battery}),
        );
        let mut r = current.clone();
        let applied = apply_signed_relays(&mut r, &ev, &set, 201).unwrap();
        assert_eq!(applied.previous, None);
        assert!(applied.had_relays);
        assert_eq!(r.relays, default_relays());
        assert_eq!(r.battery.as_ref().unwrap().screen_off_secs, 600);
        assert_eq!(r.relays_updated_at, Some(201));
    }

    /// Rule 11 (D4): a gift-wrapped PIN-only RELAYS is taken from any signer
    /// however old, and when it is ahead of the ward clock; only the pairing
    /// time is checked here (its replay floor is per signer, in the
    /// guardian-keys state). It changes no relay list, leaves the relay
    /// list's floor alone, opens no previous-relays window and applies no
    /// battery settings. Pasted, it is refused.
    #[test]
    fn a_pin_only_relays_has_no_age_limit_and_its_own_floor() {
        let original = pairing();
        let set = root_and_working(&original);
        let pin_only = |at: u64| {
            event_with(
                &original,
                [5; 32],
                at,
                serde_json::json!({"machine": original.machine, "guardianPin": pin(),
                    "battery": {"screenOffSecs":600,"ordinarySecs":120,"requestBoost":true,"openingBoost":false,"boostSecs":60}}),
            )
        };
        // A day after it was signed: the 600-second rule does not apply.
        let mut p = original.clone();
        let applied = apply_signed_relays(&mut p, &pin_only(200), &set, 200 + 86_400).unwrap();
        assert!(applied.pin.is_some());
        assert_eq!(applied.previous, None);
        assert!(!applied.had_relays);
        assert_eq!(p, original, "nothing in the pairing record changes");
        // Ahead of the ward clock: accepted too.
        assert!(apply_signed_relays(&mut original.clone(), &pin_only(400), &set, 300).is_ok());
        // Before pairing: refused.
        assert!(apply_signed_relays(&mut original.clone(), &pin_only(99), &set, 300).is_err());
        // A relay code dated before the PIN's created_at still applies.
        let mut r = original.clone();
        apply_signed_relays(&mut r, &pin_only(400), &set, 300).unwrap();
        let later_code = code(&r, [1; 32], 290, default_relays());
        apply_signed_config_from(&mut r, &later_code, &set, 300).unwrap();
        assert_eq!(r.relays, default_relays());
        // Pasted, a PIN is refused, and a pasted code keeps the 600 s rule.
        let pasted = serde_json::to_string(&pin_only(200)).unwrap();
        assert!(apply_signed_config_from(&mut original.clone(), &pasted, &set, 200).is_err());
        let battery_only = serde_json::to_string(&event_with(
            &original,
            [5; 32],
            200,
            serde_json::json!({"machine": original.machine, "battery": {"screenOffSecs":600,"ordinarySecs":120,"requestBoost":true,"openingBoost":false,"boostSecs":60}}),
        ))
        .unwrap();
        assert!(apply_signed_config_from(&mut original.clone(), &battery_only, &set, 801).is_err());
        let mut b = original.clone();
        apply_signed_config_from(&mut b, &battery_only, &set, 800).unwrap();
        assert_eq!(b.relays, original.relays);
        assert_eq!(b.battery.unwrap().screen_off_secs, 600);
    }

    /// Review 2a, Info 6: a pasted code from a working key in the signer set
    /// applies; a stranger's does not; a PIN is refused whoever signed it.
    #[test]
    fn a_pasted_code_from_any_signer_applies_but_never_a_pin() {
        let mut p = pairing();
        let working = PubKey::from_bytes(charter_crypto::xonly_pubkey(&[5; 32]).unwrap());
        let set = charter_primitives::SinglePinned(working);
        let signed = code(&p, [5; 32], 200, default_relays());
        // Against the pinned guardian alone (the old reading) it is refused.
        assert!(apply_signed_config(&mut p.clone(), &signed, 200).is_err());
        apply_signed_config_from(&mut p, &signed, &set, 200).unwrap();
        assert_eq!(p.relays, default_relays());
        assert_eq!(p.relays_updated_at, Some(200));
        let original = pairing();
        let mut q = original.clone();
        let stranger = code(&q, [9; 32], 200, default_relays());
        assert!(apply_signed_config_from(&mut q, &stranger, &set, 200).is_err());
        let with_pin = event_with(
            &q,
            [5; 32],
            200,
            serde_json::json!({"machine": q.machine, "relays": default_relays(), "guardianPin": pin()}),
        );
        let with_pin = serde_json::to_string(&with_pin).unwrap();
        assert!(apply_signed_config_from(&mut q, &with_pin, &set, 200).is_err());
        assert_eq!(q, original);
    }

    /// The battery settings are part of the signed relay change (contract,
    /// *Android battery scheduling*), so they follow its author check: any
    /// signer, pasted or gift-wrapped, under the same replay floor, and an
    /// invalid value refuses the whole change.
    #[test]
    fn battery_settings_come_with_a_relay_change_from_any_signer() {
        let working = PubKey::from_bytes(charter_crypto::xonly_pubkey(&[5; 32]).unwrap());
        let set = charter_primitives::SinglePinned(working);
        let battery = serde_json::json!({"screenOffSecs":600,"ordinarySecs":120,"requestBoost":true,"openingBoost":false,"boostSecs":60});
        let with_battery = |p: &Pairing, seed: [u8; 32], at: u64, battery: &serde_json::Value| {
            event_with(
                p,
                seed,
                at,
                serde_json::json!({"machine": p.machine, "relays": default_relays(), "battery": battery}),
            )
        };

        // Pasted, from a working key.
        let mut pasted = pairing();
        let signed = serde_json::to_string(&with_battery(&pasted, [5; 32], 200, &battery)).unwrap();
        assert!(apply_signed_config(&mut pasted.clone(), &signed, 200).is_err());
        apply_signed_config_from(&mut pasted, &signed, &set, 200).unwrap();
        assert_eq!(pasted.battery.as_ref().unwrap().screen_off_secs, 600);
        // The same code is not taken twice.
        assert!(apply_signed_config_from(&mut pasted, &signed, &set, 201).is_err());

        // Gift-wrapped, from a working key.
        let mut wrapped = pairing();
        let ev = with_battery(&wrapped, [5; 32], 200, &battery);
        apply_signed_relays(&mut wrapped, &ev, &set, 200).unwrap();
        assert_eq!(wrapped.battery, pasted.battery);

        // A stranger's, or an invalid value from a signer, changes nothing.
        let original = pairing();
        let mut q = original.clone();
        let stranger = with_battery(&q, [9; 32], 200, &battery);
        assert!(apply_signed_relays(&mut q, &stranger, &set, 200).is_err());
        let mut invalid = battery.clone();
        invalid["ordinarySecs"] = serde_json::json!(1);
        let bad = with_battery(&q, [5; 32], 200, &invalid);
        assert!(apply_signed_relays(&mut q, &bad, &set, 200).is_err());
        let bad = serde_json::to_string(&bad).unwrap();
        assert!(apply_signed_config_from(&mut q, &bad, &set, 200).is_err());
        assert_eq!(q, original);
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
