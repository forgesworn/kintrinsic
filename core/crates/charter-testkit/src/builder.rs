//! A fluent builder for the mock `SystemLayer`.

use charter_sys::MockSystem;

/// Builds a [`MockSystem`] with a chosen starting time.
pub struct MockSystemBuilder {
    now: u64,
}

impl MockSystemBuilder {
    /// Default to a fixed, recent unix second.
    pub fn new() -> Self {
        MockSystemBuilder { now: 1_700_000_000 }
    }

    /// Set the starting wall-clock unix second.
    pub fn at(mut self, now: u64) -> Self {
        self.now = now;
        self
    }

    /// Build the mock system.
    pub fn build(self) -> MockSystem {
        MockSystem::new(self.now)
    }
}

impl Default for MockSystemBuilder {
    fn default() -> Self {
        Self::new()
    }
}
