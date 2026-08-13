//! Diagnostic: raw relay queries/publishes for on-metal bring-up.
//!   cargo run --example relay_probe --features mock -- wraps <recipient_pk_hex>
use charter_primitives::kinds;
use charter_sys::relay::{Filter, RealRelayTransport, RelayTransport};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let relay = RealRelayTransport::default();
    match args.first().map(String::as_str) {
        Some("wraps") => {
            let pk = charter_primitives::PubKey::from_hex(args.get(1).expect("pk")).expect("hex");
            let wraps = relay
                .query(
                    &["wss://relay.trotters.cc".to_string()],
                    Filter {
                        kinds: vec![kinds::GIFT_WRAP],
                        p_tags: vec![pk],
                        since: args.get(2).and_then(|s| s.parse().ok()),
                        ..Default::default()
                    },
                )
                .await
                .expect("query");
            println!("{} wraps p-tagged to {}", wraps.len(), pk.to_hex());
            for w in wraps.iter().take(5) {
                println!(
                    "  id={} created_at={} tags={:?}",
                    w.id.to_hex(),
                    w.created_at,
                    w.tags
                );
            }
        }
        _ => eprintln!("usage: relay_probe wraps <pk_hex>"),
    }
}
