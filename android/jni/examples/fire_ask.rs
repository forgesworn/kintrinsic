//! Fire ONE test ask at a guardian: publish a kind-31111 time.extend REQUEST
//! (from a throwaway machine key) gift-wrapped to <guardian_pk_hex> over the
//! real relay. The carrier APK's end-to-end round: run this, watch the
//! guardian phone light up.
//!
//!   cargo run --example fire_ask --features mock -- <guardian_pk_hex> [minutes]

use std::time::{SystemTime, UNIX_EPOCH};

use charter_primitives::{kinds, Nonce, PubKey, ReqId};
use charter_proto::{OpType, RequestPayload};
use charter_sys::relay::{RealRelayTransport, RelayTransport};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_verify::test_support::sign_event;

const RELAY: &str = "wss://relay.trotters.cc";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn rand32() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("entropy");
    b
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let guardian = PubKey::from_hex(
        &std::env::args()
            .nth(1)
            .expect("usage: fire_ask <guardian_pk_hex> [minutes]"),
    )
    .expect("valid guardian pubkey hex");
    let minutes: u64 = std::env::args()
        .nth(2)
        .and_then(|m| m.parse().ok())
        .unwrap_or(10);

    // Throwaway machine identity — a fresh one per shot is fine: the carrier
    // notifies on any authenticated ask addressed to the guardian.
    let machine_sk = rand32();
    let machine_pk = PubKey::from_bytes(charter_crypto::xonly_pubkey(&machine_sk).expect("sk"));

    let ts = now();
    let payload = RequestPayload {
        v: 1,
        op: OpType::TimeExtend,
        req_id: ReqId::from_bytes(rand32()),
        nonce: Nonce::from_bytes(rand32()),
        subject: machine_pk,
        machine: machine_pk,
        ts,
        params: serde_json::json!({"minutesRequested": minutes, "limitHit": "budget"}),
    };
    let signer = charter_sys::signer::SeedSigner::from_secret(machine_sk);
    let ev = sign_event(
        &signer,
        kinds::CHARTER_DEVICE_REQUEST,
        ts,
        vec![kinds::marker_tag()],
        payload.to_json(),
    );
    let wrap = nip59::wrap(
        &Rumor::from_signed_event(&ev),
        &machine_sk,
        guardian.as_bytes(),
        &WrapRandomness {
            ephemeral_secret: rand32(),
            seal_nonce: rand32(),
            wrap_nonce: rand32(),
            seal_created_at: ts,
            wrap_created_at: ts,
        },
    )
    .expect("wrap");

    let relay = RealRelayTransport::default();
    let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
    println!(
        "fired ask reqId={} ({minutes} min) at guardian {}: {outcomes:?}",
        payload.req_id.to_hex(),
        guardian.to_hex(),
    );
}
