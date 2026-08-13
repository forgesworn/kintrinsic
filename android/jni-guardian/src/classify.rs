//! Classify one gift-wrap addressed to the guardian: is it a ward's ask
//! (notify!), a status heartbeat (ignore), or something else? Pure logic —
//! no IO, no state — so the whole carrier brain is host-testable.

use charter_primitives::{kinds, NostrEvent};
use charter_proto::RequestPayload;
use charter_transport::nip59;

fn drop_json(reason: &str) -> String {
    serde_json::json!({"type": "drop", "reason": reason}).to_string()
}

/// Classify a wrap event (JSON) using the guardian secret. Total: every input
/// maps to a JSON verdict; malformed/foreign/stale input is a "drop", never an
/// error the service has to special-case.
pub fn classify_wrap(wrap_json: &str, guardian_sk: &[u8; 32], now_unix: u64) -> String {
    let wrap: NostrEvent = match serde_json::from_str(wrap_json) {
        Ok(e) => e,
        Err(_) => return drop_json("bad wrap json"),
    };
    let (rumor, author) =
        match nip59::unwrap_with_author(&wrap, guardian_sk, now_unix, nip59::MAX_JITTER_SECS) {
            Ok(pair) => pair,
            Err(_) => return drop_json("unwrap failed"),
        };
    match rumor.kind {
        k if k == kinds::CHARTER_DEVICE_REQUEST => {
            let req = match RequestPayload::from_json(&rumor.content) {
                Ok(r) => r,
                Err(_) => return drop_json("bad request payload"),
            };
            // Attribution guard: the payload's machine claim must be the seal
            // author, or a compromised relay/device could impersonate another
            // (mirror of the PWA's unwrapRequest spoof guard).
            if req.machine != author {
                return drop_json("machine/author mismatch");
            }
            serde_json::json!({
                "type": "request",
                "reqId": req.req_id.to_hex(),
                "op": req.op,
                "machine": req.machine.to_hex(),
                "subject": req.subject.to_hex(),
                "ts": req.ts,
                "params": req.params,
            })
            .to_string()
        }
        k if k == kinds::CHARTER_DEVICE_STATUS => {
            // Deliberately payload-free: the service needs "a heartbeat came
            // from this machine", never the content.
            serde_json::json!({"type": "status", "machine": author.to_hex()}).to_string()
        }
        k if k == kinds::CHARTER_DEVICE_AUDIT => {
            // Device audits are tags-only (content always empty). The one the
            // carrier must ALERT on is the break-glass override: the ward
            // opened their phone in an emergency and the guardian should know
            // now, not on next glance (design memo 2026-07-24).
            let tag = |name: &str| -> Option<String> {
                rumor
                    .tags
                    .iter()
                    .find(|t| t.first().map(String::as_str) == Some(name))
                    .and_then(|t| t.get(1).cloned())
            };
            serde_json::json!({
                "type": "audit",
                "machine": author.to_hex(),
                "outcome": tag("outcome").unwrap_or_default(),
                "op": tag("op").unwrap_or_default(),
                "scope": tag("scope").unwrap_or_default(),
                "durationSecs": tag("durationSecs").unwrap_or_default(),
            })
            .to_string()
        }
        k => serde_json::json!({"type": "other", "kind": k}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use charter_primitives::{kinds, Nonce, PubKey, ReqId};
    use charter_proto::{OpType, RequestPayload};
    use charter_transport::nip59::{self, Rumor, WrapRandomness};
    use charter_verify::test_support::{sign_event, TestGuardian};

    use super::classify_wrap;

    const NOW: u64 = 1_800_000_000;

    /// The raw secret behind `SeedSigner::from_seed(seed)` (low byte set).
    fn sk_of(seed: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = seed.max(1);
        s
    }

    fn rand32() -> [u8; 32] {
        let mut b = [0u8; 32];
        getrandom::getrandom(&mut b).expect("entropy");
        b
    }

    /// Wrap a signed event from the sender's raw secret to `recipient_pk`, as JSON.
    fn wrap_json(
        ev: &charter_primitives::NostrEvent,
        sender_sk: &[u8; 32],
        recipient_pk: &PubKey,
        now: u64,
    ) -> String {
        let wrap = nip59::wrap(
            &Rumor::from_signed_event(ev),
            sender_sk,
            recipient_pk.as_bytes(),
            &WrapRandomness {
                ephemeral_secret: rand32(),
                seal_nonce: rand32(),
                wrap_nonce: rand32(),
                seal_created_at: now,
                wrap_created_at: now,
            },
        )
        .expect("wrap");
        serde_json::to_string(&wrap).expect("wrap json")
    }

    /// A signed kind-31111 REQUEST from machine seed 0x44, wrapped to `guardian_pk`.
    fn request_wrap(guardian_pk: &PubKey, machine_seed: u8, now: u64) -> String {
        let machine = TestGuardian::from_seed(machine_seed);
        let payload = RequestPayload {
            v: 1,
            op: OpType::TimeExtend,
            req_id: ReqId::from_bytes([7; 32]),
            nonce: Nonce::from_bytes([8; 32]),
            subject: PubKey::from_bytes([9; 32]),
            machine: machine.pubkey(),
            ts: now,
            params: serde_json::json!({"minutesRequested": 30, "limitHit": "budget"}),
        };
        let ev = sign_event(
            &machine.signer,
            kinds::CHARTER_DEVICE_REQUEST,
            now,
            vec![kinds::marker_tag()],
            payload.to_json(),
        );
        wrap_json(&ev, &sk_of(machine_seed), guardian_pk, now)
    }

    #[test]
    fn classifies_a_request_wrap() {
        let guardian = TestGuardian::new(); // seed 0x11
        let machine = TestGuardian::from_seed(0x44);
        let json = request_wrap(&guardian.pubkey(), 0x44, NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, &sk_of(0x11), NOW)).unwrap();
        assert_eq!(out["type"], "request");
        assert_eq!(out["op"], "time.extend");
        assert_eq!(out["machine"], machine.pubkey().to_hex());
        assert_eq!(out["params"]["minutesRequested"], 30);
        assert_eq!(out["reqId"].as_str().unwrap().len(), 64);
    }

    #[test]
    fn classifies_a_status_wrap_without_payload_leak() {
        let guardian = TestGuardian::new();
        let machine = TestGuardian::from_seed(0x44);
        let ev = sign_event(
            &machine.signer,
            kinds::CHARTER_DEVICE_STATUS,
            NOW,
            vec![kinds::marker_tag()],
            r#"{"v":1}"#.to_string(),
        );
        let json = wrap_json(&ev, &sk_of(0x44), &guardian.pubkey(), NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, &sk_of(0x11), NOW)).unwrap();
        assert_eq!(out["type"], "status");
        assert_eq!(out["machine"], machine.pubkey().to_hex());
        assert!(out.get("params").is_none(), "status must not carry content");
    }

    #[test]
    fn drops_a_wrap_for_someone_else() {
        let stranger = TestGuardian::from_seed(0x55);
        // Wrapped to the STRANGER — our guardian key (0x11) cannot unwrap it.
        let json = request_wrap(&stranger.pubkey(), 0x44, NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, &sk_of(0x11), NOW)).unwrap();
        assert_eq!(out["type"], "drop");
    }

    #[test]
    fn drops_a_spoofed_machine_claim() {
        // Payload claims machine=X but the seal author is Y: refuse attribution
        // (mirror of the PWA's unwrapRequest spoof guard).
        let guardian = TestGuardian::new();
        let real_author = TestGuardian::from_seed(0x44);
        let claimed = TestGuardian::from_seed(0x66);
        let payload = RequestPayload {
            v: 1,
            op: OpType::TimeExtend,
            req_id: ReqId::from_bytes([7; 32]),
            nonce: Nonce::from_bytes([8; 32]),
            subject: PubKey::from_bytes([9; 32]),
            machine: claimed.pubkey(), // lie
            ts: NOW,
            params: serde_json::json!({}),
        };
        let ev = sign_event(
            &real_author.signer,
            kinds::CHARTER_DEVICE_REQUEST,
            NOW,
            vec![kinds::marker_tag()],
            payload.to_json(),
        );
        let json = wrap_json(&ev, &sk_of(0x44), &guardian.pubkey(), NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, &sk_of(0x11), NOW)).unwrap();
        assert_eq!(out["type"], "drop");
    }

    #[test]
    fn other_kinds_report_kind_only() {
        let guardian = TestGuardian::new();
        let machine = TestGuardian::from_seed(0x44);
        let ev = sign_event(
            &machine.signer,
            kinds::CHARTER_DEVICE_GRANT,
            NOW,
            vec![kinds::marker_tag()],
            r#"{"v":1}"#.to_string(),
        );
        let json = wrap_json(&ev, &sk_of(0x44), &guardian.pubkey(), NOW);
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap(&json, &sk_of(0x11), NOW)).unwrap();
        assert_eq!(out["type"], "other");
        assert_eq!(out["kind"], kinds::CHARTER_DEVICE_GRANT);
    }

    #[test]
    fn garbage_json_is_a_drop_not_a_panic() {
        let out: serde_json::Value =
            serde_json::from_str(&classify_wrap("not json at all", &sk_of(0x11), NOW)).unwrap();
        assert_eq!(out["type"], "drop");
    }
}
