//! NIP-01 canonical event serialization — `[0, pubkey, created_at, kind, tags,
//! content]` as compact JSON. This byte-matches `nostr-tools`' `serializeEvent`
//! so the Rust-computed event id equals the guardian app's.

use charter_primitives::NostrEvent;

/// The canonical NIP-01 serialization string of `ev`.
pub fn canonical_event_string(ev: &NostrEvent) -> String {
    let value = serde_json::json!([
        0,
        ev.pubkey.to_hex(),
        ev.created_at,
        ev.kind,
        ev.tags,
        ev.content,
    ]);
    serde_json::to_string(&value).expect("canonical array serializes")
}
