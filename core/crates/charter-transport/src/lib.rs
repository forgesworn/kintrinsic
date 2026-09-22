//! `charter-transport` — NIP-44 v2 + NIP-59 gift-wrap delivery over the single
//! `charter-crypto` backend, the `bunker://` pairing pin, the clause
//! subscription, gift-wrapped audit emit, and rate-limiting. Delivery-only:
//! returns *unverified* events and never enacts.

pub mod curator;
pub mod nip44;
pub mod nip59;
pub mod pair_offer;
pub mod pairing;
pub mod transport;

pub use curator::{list_id, parse_curator_list};
pub use pairing::{pin_from_connect, validate_and_normalize, Pairing, PairingError};
pub use transport::{
    CharterTransport, Entropy, FetchedCuratorList, ReceivedClause, ReceivedGrant, ScriptedEntropy,
    TransportError,
};
