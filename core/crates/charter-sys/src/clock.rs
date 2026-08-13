//! The injected clock seam. Separate wall-clock (`now_utc`, unix seconds) and
//! monotonic (`monotonic_millis`) sources so freshness/expiry and usage
//! accounting are deterministic and testable.

/// A clock. Sync (reading the time is not IO).
pub trait Clock: Send + Sync {
    /// Wall-clock unix seconds (UTC). May jump (clock set, DST).
    fn now_utc(&self) -> u64;
    /// Monotonic milliseconds since an arbitrary epoch. Never goes backwards.
    fn monotonic_millis(&self) -> u64;
    /// Cumulative seconds this boot spent suspended (sleep AND hibernate):
    /// `CLOCK_BOOTTIME - CLOCK_MONOTONIC`. Never decreases. Usage accounting
    /// subtracts its between-tick delta so a ward is charged awake time only.
    /// Default 0 = "no suspend observed" (safe: at worst the pre-existing
    /// per-tick clamp bounds the overcharge).
    fn suspended_secs(&self) -> u64 {
        0
    }
}

/// A controllable in-memory clock for tests.
#[cfg(feature = "mock")]
#[derive(Clone)]
pub struct MockClock {
    /// (wall unix secs, monotonic ms, cumulative suspended secs)
    inner: std::sync::Arc<std::sync::Mutex<(u64, u64, u64)>>,
}

#[cfg(feature = "mock")]
impl MockClock {
    /// Construct at a given unix second with monotonic 0.
    pub fn at(now_utc: u64) -> Self {
        MockClock {
            inner: std::sync::Arc::new(std::sync::Mutex::new((now_utc, 0, 0))),
        }
    }

    /// Set the wall-clock unix seconds.
    pub fn set_utc(&self, now_utc: u64) {
        self.inner.lock().expect("clock lock").0 = now_utc;
    }

    /// Advance both wall-clock and monotonic by `secs` seconds.
    pub fn advance_secs(&self, secs: u64) {
        let mut g = self.inner.lock().expect("clock lock");
        g.0 += secs;
        g.1 += secs * 1000;
    }

    /// Advance only the monotonic clock (e.g. simulating a backwards wall jump).
    pub fn advance_monotonic_millis(&self, ms: u64) {
        self.inner.lock().expect("clock lock").1 += ms;
    }

    /// Suspend for `secs`: wall time and the suspended counter advance,
    /// monotonic does not — exactly what S3 sleep / hibernate does.
    pub fn sleep_secs(&self, secs: u64) {
        let mut g = self.inner.lock().expect("clock lock");
        g.0 += secs;
        g.2 += secs;
    }
}

#[cfg(feature = "mock")]
impl Clock for MockClock {
    fn now_utc(&self) -> u64 {
        self.inner.lock().expect("clock lock").0
    }
    fn monotonic_millis(&self) -> u64 {
        self.inner.lock().expect("clock lock").1
    }
    fn suspended_secs(&self) -> u64 {
        self.inner.lock().expect("clock lock").2
    }
}

/// The real system clock (compile-only in the headless gate).
#[cfg(any(feature = "real-relay", feature = "real-os"))]
pub struct RealClock {
    start: std::time::Instant,
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
impl RealClock {
    /// Construct anchored at the current instant.
    pub fn new() -> Self {
        RealClock {
            start: std::time::Instant::now(),
        }
    }
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
impl Clock for RealClock {
    fn now_utc(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    fn monotonic_millis(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    // Explicit clockids, not `Instant`: std's `Instant` has changed clock
    // source across Rust releases (CLOCK_MONOTONIC vs CLOCK_BOOTTIME on
    // Linux), and this difference IS the quantity we measure. Computed in ns
    // so second-truncation of the two reads can't fabricate phantom ±1s
    // suspend deltas between ticks.
    fn suspended_secs(&self) -> u64 {
        #[cfg(all(feature = "real-os", any(target_os = "linux", target_os = "android")))]
        {
            fn nanos(id: libc::clockid_t) -> Option<i128> {
                let mut ts = libc::timespec {
                    tv_sec: 0,
                    tv_nsec: 0,
                };
                // SAFETY: valid out-pointer; clock_gettime only writes `ts`.
                if unsafe { libc::clock_gettime(id, &mut ts) } != 0 {
                    return None;
                }
                Some(ts.tv_sec as i128 * 1_000_000_000 + ts.tv_nsec as i128)
            }
            if let (Some(boot), Some(mono)) =
                (nanos(libc::CLOCK_BOOTTIME), nanos(libc::CLOCK_MONOTONIC))
            {
                return ((boot - mono).max(0) / 1_000_000_000) as u64;
            }
        }
        0
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;

    #[test]
    fn mock_clock_advances() {
        let c = MockClock::at(1000);
        assert_eq!(c.now_utc(), 1000);
        assert_eq!(c.monotonic_millis(), 0);
        c.advance_secs(5);
        assert_eq!(c.now_utc(), 1005);
        assert_eq!(c.monotonic_millis(), 5000);
        c.set_utc(50); // wall clock can jump backwards
        assert_eq!(c.now_utc(), 50);
        assert_eq!(c.monotonic_millis(), 5000); // monotonic unaffected
    }

    #[test]
    fn mock_sleep_advances_wall_and_suspended_but_not_monotonic() {
        let c = MockClock::at(1000);
        c.advance_secs(10);
        assert_eq!(c.suspended_secs(), 0);
        c.sleep_secs(3600);
        assert_eq!(c.now_utc(), 1000 + 10 + 3600);
        assert_eq!(c.monotonic_millis(), 10_000);
        assert_eq!(c.suspended_secs(), 3600);
    }
}
