//! `charter-primitives` — zero-dependency wire primitives shared by every
//! Charter-for-Linux crate.
//!
//! This crate has **no crypto and no IO**: only fixed-width hex newtypes, the
//! NIP-01 [`NostrEvent`] envelope, and the single source of truth for all
//! Nostr [`kinds`]. Everything else in the workspace depends on it.

mod event;
mod ids;
pub mod kinds;
pub mod uri;

pub use event::NostrEvent;
pub use ids::{EventId, HexError, Nonce, PubKey, ReqId, Sha256Hex, Sig};
