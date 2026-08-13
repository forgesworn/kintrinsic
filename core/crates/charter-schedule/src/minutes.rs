//! `MinuteSet` — a 1440-bit bitmap of a local day's active minutes (bit i =
//! minute i of the day saw activity). This is the unit the union rule is
//! computed over: a child's pooled screen time is |own ∪ elsewhere| minutes,
//! so simultaneous use across devices counts once (`spec/contract.md`
//! §USAGE_SYNC union-rule extension). Wire form is base64url (no padding) of
//! the 180 raw bytes — exactly 240 chars — LSB-first within each byte.
//! Hand-rolled codec on purpose: core crates stay dependency-austere, and the
//! byte layout is frozen by cross-stack vectors either way.

use chrono::Timelike;
use chrono_tz::Tz;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

const BYTES: usize = 180;
const B64_CHARS: usize = 240;
const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// A day's active minutes. `Default` is the empty set.
#[derive(Clone, PartialEq, Eq)]
pub struct MinuteSet {
    bits: [u8; BYTES],
}

impl Default for MinuteSet {
    fn default() -> Self {
        MinuteSet { bits: [0u8; BYTES] }
    }
}

impl std::fmt::Debug for MinuteSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "MinuteSet({} minutes)", self.count())
    }
}

impl MinuteSet {
    pub const MINUTES: usize = 1440;

    /// Mark one minute-of-day. Out-of-range is a no-op (never panics on wire
    /// data or clock oddities).
    pub fn set(&mut self, minute: usize) {
        if minute < Self::MINUTES {
            self.bits[minute / 8] |= 1 << (minute % 8);
        }
    }

    pub fn contains(&self, minute: usize) -> bool {
        minute < Self::MINUTES && self.bits[minute / 8] & (1 << (minute % 8)) != 0
    }

    pub fn count(&self) -> u32 {
        self.bits.iter().map(|b| b.count_ones()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&b| b == 0)
    }

    pub fn union(&self, other: &MinuteSet) -> MinuteSet {
        let mut out = [0u8; BYTES];
        for (i, o) in out.iter_mut().enumerate() {
            *o = self.bits[i] | other.bits[i];
        }
        MinuteSet { bits: out }
    }

    /// Mark every local minute-of-day touched by the active span that ran for
    /// `elapsed_secs` ending at `now_unix`. The span's last active second is
    /// `now - 1` (a span ending exactly on a minute boundary does not mark
    /// that minute — zero seconds were spent in it). Clamped to the local day
    /// start: the pre-midnight tail of a span is dropped, mirroring how
    /// `UsageLedger::credit` day-rolls before crediting.
    pub fn mark_span(&mut self, tz: Tz, now_unix: i64, elapsed_secs: u64) {
        if elapsed_secs == 0 {
            return;
        }
        let dt = match chrono::DateTime::from_timestamp(now_unix, 0) {
            Some(dt) => dt.with_timezone(&tz),
            None => return,
        };
        let ssm = u64::from(dt.num_seconds_from_midnight());
        if ssm == 0 {
            return; // span ended exactly at local midnight: nothing today
        }
        let end_sec = ssm - 1;
        let start_sec = ssm.saturating_sub(elapsed_secs);
        for minute in (start_sec / 60)..=(end_sec / 60) {
            self.set(minute as usize);
        }
    }

    /// Encode as base64url, no padding: always exactly 240 chars.
    pub fn to_b64url(&self) -> String {
        let mut out = String::with_capacity(B64_CHARS);
        for chunk in self.bits.chunks(3) {
            let n = (u32::from(chunk[0]) << 16) | (u32::from(chunk[1]) << 8) | u32::from(chunk[2]);
            out.push(ALPHABET[(n >> 18) as usize & 63] as char);
            out.push(ALPHABET[(n >> 12) as usize & 63] as char);
            out.push(ALPHABET[(n >> 6) as usize & 63] as char);
            out.push(ALPHABET[n as usize & 63] as char);
        }
        out
    }

    /// Strict decode: exactly 240 base64url chars, standard alphabet only
    /// (`-`/`_`, no `+`/`/`, no padding). Anything else is `None` — malformed
    /// wire bitmaps fail closed at the parse boundary.
    pub fn from_b64url(s: &str) -> Option<MinuteSet> {
        if s.len() != B64_CHARS || !s.is_ascii() {
            return None;
        }
        fn val(c: u8) -> Option<u32> {
            match c {
                b'A'..=b'Z' => Some(u32::from(c - b'A')),
                b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
                b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
                b'-' => Some(62),
                b'_' => Some(63),
                _ => None,
            }
        }
        let mut bits = [0u8; BYTES];
        for (i, quad) in s.as_bytes().chunks(4).enumerate() {
            let n = (val(quad[0])? << 18)
                | (val(quad[1])? << 12)
                | (val(quad[2])? << 6)
                | val(quad[3])?;
            bits[i * 3] = (n >> 16) as u8;
            bits[i * 3 + 1] = (n >> 8) as u8;
            bits[i * 3 + 2] = n as u8;
        }
        Some(MinuteSet { bits })
    }
}

impl Serialize for MinuteSet {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_b64url())
    }
}

impl<'de> Deserialize<'de> for MinuteSet {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        MinuteSet::from_b64url(&s)
            .ok_or_else(|| serde::de::Error::custom("malformed MinuteSet base64url"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-06-29 12:00:00 UTC — exactly noon UTC, so with tz=UTC the
    // seconds-since-midnight arithmetic is transparent in assertions.
    const NOON_UTC: i64 = 1_782_734_400;

    #[test]
    fn set_count_contains_roundtrip() {
        let mut m = MinuteSet::default();
        assert!(m.is_empty());
        m.set(0);
        m.set(719);
        m.set(1439);
        m.set(1440); // out of range: no-op
        m.set(9999);
        assert_eq!(m.count(), 3);
        assert!(m.contains(0) && m.contains(719) && m.contains(1439));
        assert!(!m.contains(1) && !m.contains(1440));
    }

    #[test]
    fn b64url_roundtrip_240_chars_no_pad() {
        let mut m = MinuteSet::default();
        for i in (0..1440).step_by(7) {
            m.set(i);
        }
        let s = m.to_b64url();
        assert_eq!(s.len(), 240);
        assert!(!s.contains('='));
        assert!(s.bytes().all(|c| ALPHABET.contains(&c)));
        assert_eq!(MinuteSet::from_b64url(&s), Some(m));
        // Empty set is all-'A' (all zero bytes).
        assert_eq!(MinuteSet::default().to_b64url(), "A".repeat(240));
    }

    #[test]
    fn from_b64url_rejects_bad_length_and_alphabet() {
        let good = MinuteSet::default().to_b64url();
        assert!(MinuteSet::from_b64url(&good[..239]).is_none());
        assert!(MinuteSet::from_b64url(&format!("{good}A")).is_none());
        assert!(MinuteSet::from_b64url(&format!("+{}", &good[1..])).is_none());
        assert!(MinuteSet::from_b64url(&format!("/{}", &good[1..])).is_none());
        assert!(MinuteSet::from_b64url(&format!("={}", &good[1..])).is_none());
        assert!(MinuteSet::from_b64url("").is_none());
    }

    #[test]
    fn union_counts_overlap_once() {
        let mut a = MinuteSet::default();
        let mut b = MinuteSet::default();
        for i in [0usize, 1, 2] {
            a.set(i);
        }
        for i in [2usize, 3] {
            b.set(i);
        }
        let u = a.union(&b);
        assert_eq!(u.count(), 4);
        assert!(u.contains(0) && u.contains(3));
    }

    #[test]
    fn mark_span_marks_inclusive_minutes() {
        // 90s ending at 12:00:00 → active seconds 11:58:30..=11:59:59 →
        // minutes 718 and 719; minute 720 saw zero active seconds.
        let mut m = MinuteSet::default();
        m.mark_span(chrono_tz::UTC, NOON_UTC, 90);
        assert!(m.contains(718) && m.contains(719));
        assert!(!m.contains(720) && !m.contains(717));
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn mark_span_clamps_to_day_start() {
        // 12:00:10 with a 2h span: only 00:00:00..12:00:09 falls today —
        // clamped at midnight, so minutes 600..=720 from the span's tail.
        let mut m = MinuteSet::default();
        m.mark_span(chrono_tz::UTC, NOON_UTC + 10, 2 * 3600 + 86_400);
        assert!(m.contains(0) && m.contains(720));
        assert_eq!(m.count(), 721);
        // Span ending exactly at midnight marks nothing of the new day.
        let mut n = MinuteSet::default();
        n.mark_span(chrono_tz::UTC, NOON_UTC - 43_200, 600);
        assert!(n.is_empty());
    }

    #[test]
    fn mark_span_zero_elapsed_is_noop() {
        let mut m = MinuteSet::default();
        m.mark_span(chrono_tz::UTC, NOON_UTC, 0);
        assert!(m.is_empty());
    }

    #[test]
    fn serde_roundtrips_as_b64url_string_and_rejects_junk() {
        let mut m = MinuteSet::default();
        m.set(42);
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json.len(), 242); // 240 chars + quotes
        let back: MinuteSet = serde_json::from_str(&json).unwrap();
        assert_eq!(back, m);
        assert!(serde_json::from_str::<MinuteSet>("\"short\"").is_err());
    }

    #[test]
    fn non_utc_tz_uses_local_wall_clock() {
        // Europe/London in June is BST (UTC+1): noon UTC is 13:00 local, so
        // a 60s span ending then marks local minute 779 (12:59 local).
        let mut m = MinuteSet::default();
        m.mark_span(chrono_tz::Europe::London, NOON_UTC, 60);
        assert!(m.contains(779));
        assert_eq!(m.count(), 1);
    }
}
