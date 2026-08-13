//! Pure renderers: turn an `EffectiveWebPolicy` (from `charter-content`) into
//! vendor config artifacts. Firefox `policies.json` now; AdGuard Home + DNS
//! pinning later. No I/O, no privilege — the enactor writes what these return.

pub mod dns;
pub mod firefox;

pub use dns::{render_dns_filter, DnsFilterPlan, DnsMode, DnsRewrite};
pub use firefox::render_firefox_policies;
