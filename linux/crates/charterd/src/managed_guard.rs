//! Who the daemon may freeze/lock — and who may invoke recovery.
//!
//! A uid is **lockable** only if it is a charter-managed **child**: a member of
//! the `charter-managed` group, in the human uid range, and **not** in any admin
//! group. The parent / admin / root / system can therefore **never** be frozen
//! — admin membership always wins, so even an accidentally mis-enrolled admin is
//! safe. This is the structural "we can never brick the parent's own machine"
//! guarantee; it composes with the existing `is_valid_freeze_target` slice guard.
//!
//! Pure (parses `/etc/passwd` + a group document) — no I/O, no clock — so it is
//! fully unit-tested under mocks; `runtime.rs` does the live wiring.

use std::collections::BTreeSet;

use crate::device_limits::{parse_group_members, uid_for_user};

/// The group charter-setup enrolls managed children into (and polkit keys off).
const MANAGED_GROUP: &str = "charter-managed";

/// Admin groups — membership in ANY makes a uid non-lockable. This MUST stay a
/// **superset** of what `charter-setup` strips (never a subset), so a parent is
/// never freezable even if mis-enrolled. (charter-setup strips sudo/adm/lpadmin;
/// we additionally exclude the classic admin groups.)
const ADMIN_GROUPS: &[&str] = &["sudo", "adm", "lpadmin", "wheel", "admin", "root"];

/// Human login uid range. Below 1000 = system/root; >=60000 = nobody/overflow.
const MIN_MANAGED_UID: u32 = 1000;
const MAX_MANAGED_UID: u32 = 60000;

/// The set of uids the daemon is permitted to freeze/lock this tick.
pub struct ManagedRoster {
    lockable: BTreeSet<u32>,
}

impl ManagedRoster {
    /// Build from an `/etc/passwd` document and a `getent group` / `/etc/group`
    /// document. A uid is included iff it is a `charter-managed` member, NOT in
    /// any admin group, and in `[1000, 60000)`.
    pub fn from_system(passwd: &str, group: &str) -> Self {
        // Admins by SUPPLEMENTARY membership (the member list) AND by PRIMARY gid
        // (the gid on their passwd line) — so an admin whose privilege comes via
        // a primary group (e.g. primary group `wheel`) is excluded too.
        let admins: BTreeSet<String> = ADMIN_GROUPS
            .iter()
            .flat_map(|g| parse_group_members(group, g))
            .collect();
        let admin_gids: BTreeSet<u32> = ADMIN_GROUPS
            .iter()
            .filter_map(|g| group_gid(group, g))
            .collect();
        let mut lockable = BTreeSet::new();
        for user in parse_group_members(group, MANAGED_GROUP) {
            if admins.contains(&user) {
                continue; // supplementary admin — never lockable
            }
            if let Some(pgid) = primary_gid(passwd, &user) {
                if admin_gids.contains(&pgid) {
                    continue; // primary-gid admin — never lockable
                }
            }
            if let Some(uid) = uid_for_user(passwd, &user) {
                if (MIN_MANAGED_UID..MAX_MANAGED_UID).contains(&uid) {
                    lockable.insert(uid);
                }
            }
        }
        ManagedRoster { lockable }
    }

    /// True iff the daemon may freeze/lock `uid`.
    pub fn is_lockable(&self, uid: u32) -> bool {
        self.lockable.contains(&uid)
    }

    /// Every lockable uid (for the startup/recovery thaw sweep).
    pub fn uids(&self) -> Vec<u32> {
        self.lockable.iter().copied().collect()
    }
}

/// The gid of `name` from a group document (field 2 of `name:x:<gid>:members`).
fn group_gid(group_doc: &str, name: &str) -> Option<u32> {
    for line in group_doc.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.first() == Some(&name) {
            return f.get(2).and_then(|g| g.parse().ok());
        }
    }
    None
}

/// `username`'s primary gid from `/etc/passwd` (`name:x:uid:<gid>:…`).
fn primary_gid(passwd: &str, username: &str) -> Option<u32> {
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.first() == Some(&username) {
            return f.get(3).and_then(|g| g.parse().ok());
        }
    }
    None
}

/// Is `uid` a charter-managed child (lockable)? Convenience over [`ManagedRoster`].
pub fn is_managed_child(passwd: &str, group: &str, uid: u32) -> bool {
    ManagedRoster::from_system(passwd, group).is_lockable(uid)
}

/// Recovery self-defense: refuse if the pkexec caller is a managed child.
/// `None` (not launched via pkexec, e.g. a direct root run over SSH/TTY) is
/// **not** a managed child — but recovery's authoritative gate is `geteuid()==0`
/// in the binary; this is the belt-and-suspenders audit layer.
pub fn caller_is_managed_child(passwd: &str, group: &str, pkexec_uid: Option<u32>) -> bool {
    match pkexec_uid {
        Some(uid) => is_managed_child(passwd, group, uid),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // root(0), svc(200), dad(1000, admin), sam(1001), bob(1002), guest(1003),
    // padmin(1500, PRIMARY gid 27 = sudo), nobody(65534).
    const PASSWD: &str = "root:x:0:0::/root:/bin/bash\n\
        svc:x:200:200::/:/usr/sbin/nologin\n\
        dad:x:1000:1000::/home/dad:/bin/bash\n\
        sam:x:1001:1001::/home/sam:/bin/bash\n\
        bob:x:1002:1002::/home/bob:/bin/bash\n\
        guest:x:1003:1003::/home/guest:/bin/bash\n\
        padmin:x:1500:27::/home/padmin:/bin/bash\n\
        nobody:x:65534:65534::/:/usr/sbin/nologin\n";

    // charter-managed deliberately (mis-)includes root/svc/dad/padmin/nobody to
    // prove every exclusion fires. dad is in sudo+adm (supplementary admin);
    // padmin's PRIMARY group is sudo (gid 27).
    const GROUP: &str = "root:x:0:\n\
        sudo:x:27:dad\n\
        adm:x:4:dad\n\
        charter-managed:x:990:sam,bob,dad,root,svc,padmin,nobody\n";

    fn roster() -> ManagedRoster {
        ManagedRoster::from_system(PASSWD, GROUP)
    }

    #[test]
    fn managed_child_is_lockable() {
        assert!(roster().is_lockable(1001)); // sam
        assert!(roster().is_lockable(1002)); // bob
    }

    #[test]
    fn admin_in_both_groups_is_never_lockable() {
        // dad is in charter-managed AND sudo/adm — admin wins, never freezable.
        assert!(!roster().is_lockable(1000));
    }

    #[test]
    fn primary_gid_admin_is_never_lockable() {
        // padmin's PRIMARY group is sudo (gid 27) — admin by primary gid, not a
        // supplementary member — must still be excluded.
        assert!(!roster().is_lockable(1500));
    }

    #[test]
    fn root_and_system_uids_are_never_lockable() {
        assert!(!roster().is_lockable(0)); // root, uid < 1000
        assert!(!roster().is_lockable(200)); // svc, uid < 1000
        assert!(!roster().is_lockable(65534)); // nobody, uid >= 60000
    }

    #[test]
    fn uid_not_in_charter_managed_is_not_lockable() {
        assert!(!roster().is_lockable(1003)); // guest — has an account, not enrolled
        assert!(!roster().is_lockable(9999)); // unknown
    }

    #[test]
    fn uids_lists_exactly_the_lockable_children() {
        assert_eq!(roster().uids(), vec![1001, 1002]);
    }

    #[test]
    fn caller_managed_child_is_refused_admin_and_none_allowed() {
        assert!(caller_is_managed_child(PASSWD, GROUP, Some(1001))); // sam -> refuse recovery
        assert!(!caller_is_managed_child(PASSWD, GROUP, Some(1000))); // dad (admin) -> allow
        assert!(!caller_is_managed_child(PASSWD, GROUP, None)); // direct root run -> allow
    }
}
