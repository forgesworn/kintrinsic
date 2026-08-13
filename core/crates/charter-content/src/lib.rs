//! Pure policy core for Charter web-content control. No I/O, no clock, no
//! privilege. Mirrors the `charter-schedule` device-enforced-clause pattern.

pub mod clause;
pub mod curator;
pub mod domain;
pub mod evaluate;

pub use clause::{AgeTier, GrantContent, Posture, YoutubeRestrict};
pub use curator::{CuratorEntry, CuratorList, Rating};
pub use evaluate::{evaluate_content, evaluate_content_json, EffectiveWebPolicy};
