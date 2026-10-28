//! The **signer set**: who a ward accepts signed guardian events from, and who
//! it encrypts its own events to (contract, *Guardian roots and working keys*).
//!
//! Every author check (GRANT, CLAUSE, USAGE_SYNC, RELEASE, RELAYS, the local
//! link's guardian proof) asks a [`SignerSet`] instead of comparing against
//! one pinned key. The guardian-keys state machine supplies the real set; a
//! ward that has only ever pinned one guardian uses [`SinglePinned`], which
//! reproduces the single-guardian behaviour exactly. A bare [`PubKey`] is
//! itself a `SinglePinned` set, so `&guardian` can be passed wherever a
//! `&dyn SignerSet` is wanted.

use crate::PubKey;

/// Who may sign for, and who receives from, one ward.
///
/// Implementations must be cheap to query: the broker asks once per inbound
/// event. A set is a snapshot; callers do not cache answers across events.
pub trait SignerSet: Send + Sync {
    /// Whether `key` is currently in the signer set: a pinned root, or an
    /// approved, unwithdrawn working key bound to a pinned root.
    fn is_signer(&self, key: &PubKey) -> bool;

    /// The pinned root `key` belongs to: itself for a root, its binding root
    /// for a working key. `None` for a key the set does not hold (including a
    /// departed one).
    fn root_of(&self, key: &PubKey) -> Option<PubKey>;

    /// Whether `key` has left the signer set (rule 4): a withdrawn working
    /// key, a removed or moved-away root, or a moved root's key nobody
    /// re-approved. `false` for a current signer and for a key that was never
    /// one. (The clause floor a departure leaves behind lives in the
    /// guardian-keys state, not here.)
    fn is_departed(&self, key: &PubKey) -> bool;

    /// Who the ward encrypts REQUEST, STATUS, USAGE, AUDIT and break-glass
    /// events to (rule 9): every working key in the set, plus each pinned root
    /// with no working key in the set. Never empty for a paired ward whose
    /// set could be read; empty when it could not (send nothing rather than
    /// guess).
    fn recipients(&self) -> Vec<PubKey>;

    /// The pinned roots, in pinning order.
    fn roots(&self) -> Vec<PubKey>;
}

/// The signer set of a ward that pins exactly one guardian and nothing else:
/// that key signs, receives, and is the only root. Nothing ever departs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SinglePinned(pub PubKey);

impl SignerSet for SinglePinned {
    fn is_signer(&self, key: &PubKey) -> bool {
        *key == self.0
    }
    fn root_of(&self, key: &PubKey) -> Option<PubKey> {
        (*key == self.0).then_some(self.0)
    }
    fn is_departed(&self, _key: &PubKey) -> bool {
        false
    }
    fn recipients(&self) -> Vec<PubKey> {
        vec![self.0]
    }
    fn roots(&self) -> Vec<PubKey> {
        vec![self.0]
    }
}

/// A bare pinned key is a [`SinglePinned`] set.
impl SignerSet for PubKey {
    fn is_signer(&self, key: &PubKey) -> bool {
        SinglePinned(*self).is_signer(key)
    }
    fn root_of(&self, key: &PubKey) -> Option<PubKey> {
        SinglePinned(*self).root_of(key)
    }
    fn is_departed(&self, _key: &PubKey) -> bool {
        false
    }
    fn recipients(&self) -> Vec<PubKey> {
        vec![*self]
    }
    fn roots(&self) -> Vec<PubKey> {
        vec![*self]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pinned_is_the_one_guardian_and_nothing_else() {
        let g = PubKey::from_bytes([1; 32]);
        let other = PubKey::from_bytes([2; 32]);
        for set in [&SinglePinned(g) as &dyn SignerSet, &g as &dyn SignerSet] {
            assert!(set.is_signer(&g));
            assert!(!set.is_signer(&other));
            assert_eq!(set.root_of(&g), Some(g));
            assert_eq!(set.root_of(&other), None);
            assert!(!set.is_departed(&g));
            assert_eq!(set.recipients(), vec![g]);
            assert_eq!(set.roots(), vec![g]);
        }
    }
}
