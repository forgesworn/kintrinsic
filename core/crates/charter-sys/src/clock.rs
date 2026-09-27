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
    /// Whether the platform vouches for the wall clock right now — on Linux,
    /// the kernel reports it NTP-synchronised (`adjtimex(2)`: not
    /// `TIME_ERROR`, `STA_UNSYNC` clear). The usage ledger drops a far-future
    /// day key back only while this holds (N1). Default `false` = untrusted,
    /// the fail-safe: the ledger keeps its pure high-water mark.
    fn wall_clock_synchronised(&self) -> bool {
        false
    }
}

/// A controllable in-memory clock for tests.
#[cfg(feature = "mock")]
#[derive(Clone)]
pub struct MockClock {
    /// (wall unix secs, monotonic ms, cumulative suspended secs)
    inner: std::sync::Arc<std::sync::Mutex<(u64, u64, u64)>>,
    /// What [`Clock::wall_clock_synchronised`] answers; `false` unless set.
    synchronised: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "mock")]
impl MockClock {
    /// Construct at a given unix second with monotonic 0.
    pub fn at(now_utc: u64) -> Self {
        MockClock {
            inner: std::sync::Arc::new(std::sync::Mutex::new((now_utc, 0, 0))),
            synchronised: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Set what [`Clock::wall_clock_synchronised`] answers.
    pub fn set_synchronised(&self, synchronised: bool) {
        self.synchronised
            .store(synchronised, std::sync::atomic::Ordering::Relaxed);
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
    fn wall_clock_synchronised(&self) -> bool {
        self.synchronised.load(std::sync::atomic::Ordering::Relaxed)
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
    // A read-only `adjtimex` (`modes = 0` changes nothing, so it needs no
    // privilege). The kernel sets `STA_UNSYNC` at boot and on every
    // `settimeofday`/`clock_settime`, and only an NTP daemon's discipline
    // clears it — so an RTC set in firmware setup, or a clock stepped by
    // hand, reads as untrusted until NTP has actually synchronised it. Any
    // failure is untrusted (fail-safe).
    fn wall_clock_synchronised(&self) -> bool {
        #[cfg(all(feature = "real-os", target_os = "linux"))]
        {
            // SAFETY: `timex` is plain old data; all-zero is a valid value
            // and `modes = 0` makes the call a pure read into it.
            let mut tx: libc::timex = unsafe { std::mem::zeroed() };
            // SAFETY: valid, exclusive out-pointer for the call's duration.
            let state = unsafe { libc::adjtimex(&mut tx) };
            return state >= 0 && state != libc::TIME_ERROR && tx.status & libc::STA_UNSYNC == 0;
        }
        #[allow(unreachable_code)]
        false
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
