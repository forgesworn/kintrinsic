//! The guardian's stand-down ("finish up now"), as enforcement needs it.
//!
//! Shared by every warden: the ward's grace has to be a real minute, and the
//! rule for producing it is subtle enough that two copies would drift.
//!
//! The lock instant is deliberately NOT on the wire. Were it stamped
//! `issuedAt + graceSecs`, relay transit would eat into the warning, and a
//! device that spent an hour offline would reconnect and cut the ward off with
//! no warning at all. So the DEVICE starts the clock when it first sees a given
//! clause `id` — and pins that moment durably, because an in-memory pin would
//! restart on every boot and the lock would never land.

use charter_proto::{ClauseKind, StandDownBody, STANDDOWN_GRACE_STORE_KEY};
use charter_schedule::enforcer::StandDown;
use charter_sys::persistence::{ChildClauseStore, ClauseStore};

/// The three slot operations pinning the grace needs. Implemented over the
/// single-child `ClauseStore` and the per-child `ChildClauseStore` alike, so
/// both wardens share one implementation of a rule that is easy to get subtly
/// wrong.
pub trait GraceSlot {
    /// The standing stand-down clause JSON, if any.
    fn clause_json(&self) -> Option<String>;
    /// The pinned `{"id":…,"startedAt":…}`, if any.
    fn pin_json(&self) -> Option<String>;
    /// The highest `issued_at` the pin slot has accepted.
    fn pin_floor(&self) -> u64;
    /// Record a pin. Failure is tolerated by the caller but must be rare — see
    /// [`stand_down_now`].
    fn put_pin(&self, issued_at: u64, json: &str);
}

/// The live stand-down, or `None`.
///
/// Fails **OPEN**, the mirror of a gift's fail-CLOSED: a gift loosens
/// enforcement so doubt must mean "no extra time"; a stand-down tightens it so
/// doubt must mean "no lock". Both land on the ward's standing charter — the
/// principle is that uncertainty never invents a state harsher OR looser than
/// what the guardian signed. This direction matters more: a malformed clause
/// that locks a ward out of her own phone while her guardian believes she is
/// fine is worse than one that leaves her alone, and the guardian can see it did
/// not apply and try again.
pub fn stand_down_now(slot: &impl GraceSlot, now: u64) -> Option<StandDown> {
    let body: StandDownBody = serde_json::from_str(&slot.clause_json()?).ok()?;
    if !body.stands(now) {
        return None;
    }
    let started = grace_started_at(slot, &body.id, now);
    let left = body
        .grace_secs()
        .saturating_sub(now.saturating_sub(started));
    Some(StandDown {
        secs_until_lock: left as i64,
    })
}

/// When this device first saw the standing stand-down.
///
/// First sight of an `id` pins `now`. The store's monotonic floor would reject a
/// second write stamped in the same second as the last, so the stamp is nudged
/// past it: the value read back is the payload's `startedAt`, and losing the
/// write would fail OPEN forever — re-pinning the grace on every tick, so the
/// lock never lands.
fn grace_started_at(slot: &impl GraceSlot, id: &str, now: u64) -> u64 {
    if let Some(json) = slot.pin_json() {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
            if v.get("id").and_then(|s| s.as_str()) == Some(id) {
                if let Some(at) = v.get("startedAt").and_then(|n| n.as_u64()) {
                    return at;
                }
            }
        }
    }
    let stamp = now.max(slot.pin_floor().saturating_add(1));
    slot.put_pin(
        stamp,
        &serde_json::json!({ "id": id, "startedAt": now }).to_string(),
    );
    now
}

/// The clause store key a stand-down is filed under.
pub fn clause_key() -> u16 {
    ClauseKind::StandDown.store_key()
}

/// The device-local slot key the grace pin is filed under.
pub fn pin_key() -> u16 {
    STANDDOWN_GRACE_STORE_KEY
}

/// [`GraceSlot`] over a per-child store — a phone, or charterd's multi-child path.
pub struct ChildSlot<'a, S: ChildClauseStore + ?Sized> {
    pub store: &'a S,
    pub subject_hex: &'a str,
}

impl<S: ChildClauseStore + ?Sized> GraceSlot for ChildSlot<'_, S> {
    fn clause_json(&self) -> Option<String> {
        self.store
            .get_child_clause(self.subject_hex, clause_key())
            .ok()
            .flatten()
    }
    fn pin_json(&self) -> Option<String> {
        self.store
            .get_child_clause(self.subject_hex, pin_key())
            .ok()
            .flatten()
    }
    fn pin_floor(&self) -> u64 {
        self.store
            .highest_issued_at(self.subject_hex, pin_key())
            .ok()
            .flatten()
            .unwrap_or(0)
    }
    fn put_pin(&self, issued_at: u64, json: &str) {
        let _ = self
            .store
            .put_child_clause(self.subject_hex, pin_key(), issued_at, json);
    }
}

/// [`GraceSlot`] over the single-child store — charterd's device-only path.
pub struct DeviceSlot<'a, S: ClauseStore + ?Sized> {
    pub store: &'a S,
}

impl<S: ClauseStore + ?Sized> GraceSlot for DeviceSlot<'_, S> {
    fn clause_json(&self) -> Option<String> {
        self.store.get_clause(clause_key()).ok().flatten()
    }
    fn pin_json(&self) -> Option<String> {
        self.store.get_clause(pin_key()).ok().flatten()
    }
    fn pin_floor(&self) -> u64 {
        self.store
            .highest_issued_at(pin_key())
            .ok()
            .flatten()
            .unwrap_or(0)
    }
    fn put_pin(&self, issued_at: u64, json: &str) {
        let _ = self.store.put_clause(pin_key(), issued_at, json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeSlot {
        clause: Option<String>,
        pin: RefCell<Option<(u64, String)>>,
    }

    impl GraceSlot for FakeSlot {
        fn clause_json(&self) -> Option<String> {
            self.clause.clone()
        }
        fn pin_json(&self) -> Option<String> {
            self.pin.borrow().as_ref().map(|(_, j)| j.clone())
        }
        fn pin_floor(&self) -> u64 {
            self.pin.borrow().as_ref().map(|(t, _)| *t).unwrap_or(0)
        }
        fn put_pin(&self, issued_at: u64, json: &str) {
            // Mirror the real store's monotonic floor: a stamp at or below the
            // highest seen is REJECTED.
            let mut cur = self.pin.borrow_mut();
            let floor = cur.as_ref().map(|(t, _)| *t).unwrap_or(0);
            if cur.is_none() || issued_at > floor {
                *cur = Some((issued_at, json.to_string()));
            }
        }
    }

    fn clause(id: &str, expires_at: u64, grace: u64) -> String {
        serde_json::json!({
            "v": 1, "issuedAt": 1_000, "id": id,
            "expiresAt": expires_at, "graceSecs": grace
        })
        .to_string()
    }

    fn slot(id: &str, expires_at: u64, grace: u64) -> FakeSlot {
        FakeSlot {
            clause: Some(clause(id, expires_at, grace)),
            pin: RefCell::new(None),
        }
    }

    #[test]
    fn no_clause_means_no_stand_down() {
        assert!(stand_down_now(&FakeSlot::default(), 500).is_none());
    }

    #[test]
    fn first_sight_pins_now_and_owes_the_full_grace() {
        let s = slot("sd-1", 10_000, 60);
        let sd = stand_down_now(&s, 500).expect("stands");
        assert_eq!(sd.secs_until_lock, 60);
        assert!(s.pin_json().is_some(), "first sight must pin durably");
    }

    /// The point of pinning: the countdown runs from FIRST SIGHT, so re-reading
    /// the same clause every tick does not keep pushing the lock away.
    #[test]
    fn the_grace_counts_down_across_ticks() {
        let s = slot("sd-1", 10_000, 60);
        assert_eq!(stand_down_now(&s, 500).unwrap().secs_until_lock, 60);
        assert_eq!(stand_down_now(&s, 530).unwrap().secs_until_lock, 30);
        assert_eq!(stand_down_now(&s, 560).unwrap().secs_until_lock, 0);
        // …and stays locked, rather than going negative and reopening.
        assert_eq!(stand_down_now(&s, 9_000).unwrap().secs_until_lock, 0);
    }

    /// A reboot must not hand the ward a fresh minute — the pin is why.
    #[test]
    fn a_restart_does_not_restart_the_grace() {
        let s = slot("sd-1", 10_000, 60);
        stand_down_now(&s, 500);
        let pinned = s.pin.borrow().clone();
        // A new process, same durable slot.
        let after = FakeSlot {
            clause: Some(clause("sd-1", 10_000, 60)),
            pin: RefCell::new(pinned),
        };
        assert_eq!(stand_down_now(&after, 545).unwrap().secs_until_lock, 15);
    }

    /// A SECOND stand-down is a new id, so it earns its own fresh minute.
    #[test]
    fn a_new_stand_down_gets_its_own_grace() {
        let s = slot("sd-1", 10_000, 60);
        stand_down_now(&s, 500);
        let pinned = s.pin.borrow().clone();
        let second = FakeSlot {
            clause: Some(clause("sd-2", 10_000, 60)),
            pin: RefCell::new(pinned),
        };
        assert_eq!(second.pin_floor(), 500);
        assert_eq!(stand_down_now(&second, 500).unwrap().secs_until_lock, 60);
        // Re-pinned in the same second: the stamp must clear the floor, or the
        // write is rejected and the grace re-pins forever (fail-open).
        assert_eq!(second.pin_floor(), 501);
        assert_eq!(stand_down_now(&second, 530).unwrap().secs_until_lock, 30);
    }

    #[test]
    fn it_fails_open_on_anything_it_cannot_trust() {
        // Lapsed — the midnight lapse.
        assert!(stand_down_now(&slot("sd-1", 400, 60), 500).is_none());
        // A lift (expiry at its own issue instant) seen by a slow device
        // clock: must free, never re-lock.
        assert!(stand_down_now(&slot("sd-1", 1_000, 60), 500).is_none());
        // An expiry no honest "rest of today" could reach — a client bug.
        assert!(stand_down_now(&slot("sd-1", u64::MAX, 60), 500).is_none());
        // Empty id: nothing to pin first-sight against.
        assert!(stand_down_now(&slot("", 10_000, 60), 500).is_none());
        // Unparseable body.
        let junk = FakeSlot {
            clause: Some("{{not json".into()),
            pin: RefCell::new(None),
        };
        assert!(stand_down_now(&junk, 500).is_none());
    }

    #[test]
    fn an_absurd_grace_is_clamped_not_refused() {
        let s = slot("sd-1", 10_000, u64::MAX);
        assert_eq!(stand_down_now(&s, 500).unwrap().secs_until_lock, 600);
        let z = slot("sd-z", 10_000, 0);
        assert_eq!(stand_down_now(&z, 500).unwrap().secs_until_lock, 60);
    }
}
