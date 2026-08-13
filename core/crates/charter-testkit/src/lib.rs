//! `charter-testkit` — shared mock builders, deterministic fixtures, and the
//! golden-vector loader used across every crate's test suite.

pub mod builder;
pub mod fixtures;
pub mod golden;

pub use builder::MockSystemBuilder;

#[cfg(test)]
mod tests {
    use super::*;
    use charter_sys::{Clock, SystemLayer};

    #[test]
    fn builder_makes_a_working_mock_system() {
        let sys = MockSystemBuilder::new().at(1234).build();
        assert_eq!(sys.clock().now_utc(), 1234);
    }

    #[test]
    fn fixtures_are_distinct_and_well_formed() {
        assert_ne!(fixtures::guardian_pubkey(), fixtures::machine_pubkey());
        assert_eq!(fixtures::sha256_of(b"").to_hex().len(), 64);
    }
}
