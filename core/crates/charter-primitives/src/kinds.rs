//! The **single source of truth** for every Nostr kind number and the
//! charter-device marker tag. No other crate may define divergent kind
//! numbers — they all import these constants.

/// NIP-59 gift-wrap outer event.
pub const GIFT_WRAP: u16 = 1059;
/// NIP-59 seal event.
pub const SEAL: u16 = 13;

/// Charter device REQUEST (machine -> guardian, inner rumor).
pub const CHARTER_DEVICE_REQUEST: u16 = 31111;
/// Charter device GRANT (guardian -> machine, signed inner rumor).
pub const CHARTER_DEVICE_GRANT: u16 = 31112;
/// Charter device standing CLAUSE (guardian -> machine, schedule/budget).
pub const CHARTER_DEVICE_CLAUSE: u16 = 31113;
/// Charter device STATUS (machine -> guardian, gift-wrapped; one per child per
/// device). Numbers + enums only — the MyCharter PWA renders it and the
/// multi-device consolidation aggregator sums it.
pub const CHARTER_DEVICE_STATUS: u16 = 31114;
/// Charter device AUDIT (machine -> guardian, content always empty).
pub const CHARTER_DEVICE_AUDIT: u16 = 31000;
/// Charter device USAGE_SYNC (guardian -> machine, signed inner rumor): the
/// consolidated cross-device usage view for one child — scalar
/// spent-elsewhere totals plus the union-rule minute bitmap. Verified against
/// the pinned guardian exactly like a CLAUSE; monotonic by `ts`.
pub const CHARTER_DEVICE_USAGE_SYNC: u16 = 31115;
/// Charter device RELEASE (guardian -> machine, signed inner rumor): a
/// parent-gated unpair. The device drops its pairing, forgets every clause,
/// lifts all enforcement, and returns to "waiting to pair". Only the pinned
/// guardian's key can produce it, so a child can never release their own
/// device.
pub const CHARTER_DEVICE_RELEASE: u16 = 31116;

/// Charter device PAIR_OFFER (guardian -> machine, inner rumor): an offer to
/// pin this guardian, sent after the guardian's phone scans the ward's
/// on-screen pairing QR. Carries the one-time token from that QR — proof the
/// sender physically saw the screen. The device pins the SEAL AUTHOR, never a
/// key named in the payload.
pub const CHARTER_DEVICE_PAIR_OFFER: u16 = 31117;

/// Addressable (NIP-51-style) curator web-list: a curator's signed allow/deny
/// entries; parents subscribe by pubkey. PROVISIONAL — final value is design
/// open-question §10.1 (`docs/superpowers/specs/2026-06-28-charter-web-content-control-design.md`).
pub const CHARTER_CURATOR_WEB_LIST: u16 = 30100;

/// Addressable SOFTWARE RELEASE (release key -> everyone): announces the
/// latest artifact for one release channel (`d` = `charter-apk` |
/// `mycharter-apk` | `charter-deb`) — version, sha256, size, Blossom mirror
/// URLs, and (for APKs) the signing-cert digest. Verified against the
/// compiled-in release pubkey, NOT the pinned guardian. 30063 aligns with the
/// Zapstore software-release convention. NOTE: 31116 (`CHARTER_DEVICE_RELEASE`)
/// is the *unpair* event — an unrelated concept that happens to share the
/// word "release".
pub const SOFTWARE_RELEASE: u16 = 30063;

/// NIP-32 namespace label (`["L", ...]` tag value) scoping a curator web-list
/// event. A kind-30100 event lacking this label is **not** a Charter curator
/// web list and is ignored by the parser.
pub const CURATOR_WEB_NAMESPACE: &str = "app.charter.web";
/// Rating token (3rd element of an `["r", <domain>, <token>]` tag) marking a
/// domain kid-safe.
pub const CURATOR_RATING_KID_SAFE: &str = "kid-safe";
/// Rating token marking a domain blocked.
pub const CURATOR_RATING_BLOCK: &str = "block";
/// Rating-token prefix marking a domain blocked under a category, e.g.
/// `category:porn`. The suffix is the (non-empty) category key.
pub const CURATOR_RATING_CATEGORY_PREFIX: &str = "category:";

/// Marker tag stamped on charter-device events: `["t", "charter-device"]`.
pub const CHARTER_DEVICE_MARKER: (&str, &str) = ("t", "charter-device");

/// Convenience: the marker tag as an owned `Vec<String>`.
pub fn marker_tag() -> Vec<String> {
    vec![
        CHARTER_DEVICE_MARKER.0.to_string(),
        CHARTER_DEVICE_MARKER.1.to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_frozen() {
        assert_eq!(GIFT_WRAP, 1059);
        assert_eq!(SEAL, 13);
        assert_eq!(CHARTER_DEVICE_REQUEST, 31111);
        assert_eq!(CHARTER_DEVICE_GRANT, 31112);
        assert_eq!(CHARTER_DEVICE_CLAUSE, 31113);
        assert_eq!(CHARTER_DEVICE_STATUS, 31114);
        assert_eq!(CHARTER_DEVICE_AUDIT, 31000);
        assert_eq!(CHARTER_DEVICE_USAGE_SYNC, 31115);
        assert_eq!(CHARTER_DEVICE_RELEASE, 31116);
        assert_eq!(CHARTER_DEVICE_PAIR_OFFER, 31117);
        assert_eq!(CHARTER_CURATOR_WEB_LIST, 30100);
        assert_eq!(SOFTWARE_RELEASE, 30063);
    }

    #[test]
    fn curator_web_vocabulary_is_frozen() {
        assert_eq!(CURATOR_WEB_NAMESPACE, "app.charter.web");
        assert_eq!(CURATOR_RATING_KID_SAFE, "kid-safe");
        assert_eq!(CURATOR_RATING_BLOCK, "block");
        assert_eq!(CURATOR_RATING_CATEGORY_PREFIX, "category:");
    }

    #[test]
    fn marker_tag_shape() {
        assert_eq!(
            marker_tag(),
            vec!["t".to_string(), "charter-device".to_string()]
        );
    }
}
