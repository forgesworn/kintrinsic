//! The `SystemLayer` aggregate: one associated type + accessor per port, so
//! `charterd` can be generic over `S: SystemLayer` (static dispatch) and the
//! whole daemon builds, unit-tests, and integration-tests with zero privilege.

use crate::clock::Clock;
use crate::effects::{
    AccountOps, ApprovedExecStore, CgroupFreezer, DenialWatch, DnsFilterOps, ExecGate, FlatpakOps,
    MountOps, PolkitOps, SessionControl, SystemctlOps, TrustDb, VtControl, WebPolicyOps,
};
use crate::persistence::{
    ChildClauseStore, ClauseStore, ConsumedIdStore, CuratorListStore, ExtensionStore, PairingStore,
    PendingStore, UsageStore,
};
use crate::relay::RelayTransport;
use crate::signer::MachineSigner;

/// The full set of system ports, one associated type each.
pub trait SystemLayer: Send + Sync {
    type Clock: Clock;
    type ConsumedIds: ConsumedIdStore;
    type Pending: PendingStore;
    type Clauses: ClauseStore;
    type ChildClauses: ChildClauseStore;
    type CuratorLists: CuratorListStore;
    type Usage: UsageStore;
    type Extension: ExtensionStore;
    type Pairing: PairingStore;
    type Flatpak: FlatpakOps;
    type Trust: TrustDb;
    type Approved: ApprovedExecStore;
    type Exec: ExecGate;
    type Denials: DenialWatch;
    type Freezer: CgroupFreezer;
    type Session: SessionControl;
    type Vt: VtControl;
    type Mounts: MountOps;
    type Polkit: PolkitOps;
    type Account: AccountOps;
    type Systemctl: SystemctlOps;
    type Machine: MachineSigner;
    type Relay: RelayTransport;
    type WebPolicy: WebPolicyOps;
    type DnsFilter: DnsFilterOps;

    fn clock(&self) -> &Self::Clock;
    fn consumed_ids(&self) -> &Self::ConsumedIds;
    fn pending(&self) -> &Self::Pending;
    fn clauses(&self) -> &Self::Clauses;
    fn child_clauses(&self) -> &Self::ChildClauses;
    fn curator_lists(&self) -> &Self::CuratorLists;
    fn usage(&self) -> &Self::Usage;
    fn extension(&self) -> &Self::Extension;
    fn pairing(&self) -> &Self::Pairing;
    fn flatpak(&self) -> &Self::Flatpak;
    fn trust(&self) -> &Self::Trust;
    fn approved(&self) -> &Self::Approved;
    fn exec(&self) -> &Self::Exec;
    fn denials(&self) -> &Self::Denials;
    fn freezer(&self) -> &Self::Freezer;
    fn session(&self) -> &Self::Session;
    fn vt(&self) -> &Self::Vt;
    fn mounts(&self) -> &Self::Mounts;
    fn polkit(&self) -> &Self::Polkit;
    fn account(&self) -> &Self::Account;
    fn systemctl(&self) -> &Self::Systemctl;
    fn machine(&self) -> &Self::Machine;
    fn relay(&self) -> &Self::Relay;
    fn web_policy(&self) -> &Self::WebPolicy;
    fn dns_filter(&self) -> &Self::DnsFilter;
}

/// Proof that `charterd` can be written generically over any `SystemLayer`.
pub fn run<S: SystemLayer>(_sys: &S) {}

// ---------------------------------------------------------------------------
// MockSystem.
// ---------------------------------------------------------------------------

#[cfg(feature = "mock")]
mod mock {
    use super::*;
    use crate::clock::MockClock;
    use crate::effects::{
        MockAccountOps, MockApprovedExecStore, MockCgroupFreezer, MockDenialWatch,
        MockDnsFilterOps, MockExecGate, MockFlatpakOps, MockMountOps, MockPolkitOps,
        MockSessionControl, MockSystemctlOps, MockTrustDb, MockVtControl, MockWebPolicyOps,
    };
    use crate::persistence::{
        MockChildClauseStore, MockClauseStore, MockConsumedIdStore, MockCuratorListStore, MockDisk,
        MockExtensionStore, MockPairingStore, MockPendingStore, MockUsageStore,
    };
    use crate::relay::MockRelayTransport;
    use crate::signer::SeedSigner;

    /// A fully in-memory `SystemLayer` for headless tests. The persistence
    /// ports share one [`MockDisk`] so a re-created `MockSystem` over the same
    /// disk handle sees prior writes (restart durability).
    pub struct MockSystem {
        clock: MockClock,
        disk: MockDisk,
        consumed: MockConsumedIdStore,
        pending: MockPendingStore,
        clauses: MockClauseStore,
        child_clauses: MockChildClauseStore,
        curator_lists: MockCuratorListStore,
        usage: MockUsageStore,
        extension: MockExtensionStore,
        pairing: MockPairingStore,
        flatpak: MockFlatpakOps,
        trust: MockTrustDb,
        approved: MockApprovedExecStore,
        exec: MockExecGate,
        denials: MockDenialWatch,
        freezer: MockCgroupFreezer,
        session: MockSessionControl,
        vt: MockVtControl,
        mounts: MockMountOps,
        polkit: MockPolkitOps,
        account: MockAccountOps,
        systemctl: MockSystemctlOps,
        machine: SeedSigner,
        relay: MockRelayTransport,
        web_policy: MockWebPolicyOps,
        dns_filter: MockDnsFilterOps,
    }

    impl MockSystem {
        /// Build over a fresh disk at unix second `now`.
        pub fn new(now: u64) -> Self {
            Self::over_disk(now, MockDisk::new())
        }

        /// Build over an existing disk (simulates a daemon restart).
        pub fn over_disk(now: u64, disk: MockDisk) -> Self {
            MockSystem {
                clock: MockClock::at(now),
                consumed: MockConsumedIdStore::new(disk.clone()),
                pending: MockPendingStore::new(disk.clone()),
                clauses: MockClauseStore::new(disk.clone()),
                child_clauses: MockChildClauseStore::new(disk.clone()),
                curator_lists: MockCuratorListStore::new(disk.clone()),
                usage: MockUsageStore::new(disk.clone()),
                extension: MockExtensionStore::new(disk.clone()),
                pairing: MockPairingStore::new(disk.clone()),
                flatpak: MockFlatpakOps::new(),
                trust: MockTrustDb::new(),
                approved: MockApprovedExecStore::new(),
                exec: MockExecGate,
                denials: MockDenialWatch,
                freezer: MockCgroupFreezer::new(),
                session: MockSessionControl::new(),
                vt: MockVtControl::new(),
                mounts: MockMountOps,
                polkit: MockPolkitOps,
                account: MockAccountOps,
                systemctl: MockSystemctlOps,
                machine: SeedSigner::from_seed(0xAA),
                relay: MockRelayTransport::new(),
                web_policy: MockWebPolicyOps::new(),
                dns_filter: MockDnsFilterOps::new(),
                disk,
            }
        }

        /// The shared backing disk (clone to re-create a `MockSystem` after a
        /// simulated restart).
        pub fn disk(&self) -> MockDisk {
            self.disk.clone()
        }

        /// The controllable mock clock.
        pub fn mock_clock(&self) -> &MockClock {
            &self.clock
        }
    }

    impl SystemLayer for MockSystem {
        type Clock = MockClock;
        type ConsumedIds = MockConsumedIdStore;
        type Pending = MockPendingStore;
        type Clauses = MockClauseStore;
        type ChildClauses = MockChildClauseStore;
        type CuratorLists = MockCuratorListStore;
        type Usage = MockUsageStore;
        type Extension = MockExtensionStore;
        type Pairing = MockPairingStore;
        type Flatpak = MockFlatpakOps;
        type Trust = MockTrustDb;
        type Approved = MockApprovedExecStore;
        type Exec = MockExecGate;
        type Denials = MockDenialWatch;
        type Freezer = MockCgroupFreezer;
        type Session = MockSessionControl;
        type Vt = MockVtControl;
        type Mounts = MockMountOps;
        type Polkit = MockPolkitOps;
        type Account = MockAccountOps;
        type Systemctl = MockSystemctlOps;
        type Machine = SeedSigner;
        type Relay = MockRelayTransport;
        type WebPolicy = MockWebPolicyOps;
        type DnsFilter = MockDnsFilterOps;

        fn clock(&self) -> &Self::Clock {
            &self.clock
        }
        fn consumed_ids(&self) -> &Self::ConsumedIds {
            &self.consumed
        }
        fn pending(&self) -> &Self::Pending {
            &self.pending
        }
        fn clauses(&self) -> &Self::Clauses {
            &self.clauses
        }
        fn child_clauses(&self) -> &Self::ChildClauses {
            &self.child_clauses
        }
        fn curator_lists(&self) -> &Self::CuratorLists {
            &self.curator_lists
        }
        fn usage(&self) -> &Self::Usage {
            &self.usage
        }
        fn extension(&self) -> &Self::Extension {
            &self.extension
        }
        fn pairing(&self) -> &Self::Pairing {
            &self.pairing
        }
        fn flatpak(&self) -> &Self::Flatpak {
            &self.flatpak
        }
        fn trust(&self) -> &Self::Trust {
            &self.trust
        }
        fn approved(&self) -> &Self::Approved {
            &self.approved
        }
        fn exec(&self) -> &Self::Exec {
            &self.exec
        }
        fn denials(&self) -> &Self::Denials {
            &self.denials
        }
        fn freezer(&self) -> &Self::Freezer {
            &self.freezer
        }
        fn session(&self) -> &Self::Session {
            &self.session
        }
        fn vt(&self) -> &Self::Vt {
            &self.vt
        }
        fn mounts(&self) -> &Self::Mounts {
            &self.mounts
        }
        fn polkit(&self) -> &Self::Polkit {
            &self.polkit
        }
        fn account(&self) -> &Self::Account {
            &self.account
        }
        fn systemctl(&self) -> &Self::Systemctl {
            &self.systemctl
        }
        fn machine(&self) -> &Self::Machine {
            &self.machine
        }
        fn relay(&self) -> &Self::Relay {
            &self.relay
        }
        fn web_policy(&self) -> &Self::WebPolicy {
            &self.web_policy
        }
        fn dns_filter(&self) -> &Self::DnsFilter {
            &self.dns_filter
        }
    }
}

#[cfg(feature = "mock")]
pub use mock::MockSystem;

// ---------------------------------------------------------------------------
// RealSystem (compile-only).
// ---------------------------------------------------------------------------

#[cfg(all(feature = "real-relay", feature = "real-os"))]
mod real {
    use super::*;
    use crate::clock::RealClock;
    use crate::effects::{
        RealAccountOps, RealApprovedExecStore, RealCgroupFreezer, RealDenialWatch,
        RealDnsFilterOps, RealExecGate, RealFlatpakOps, RealMountOps, RealPolkitOps,
        RealSessionControl, RealSystemctlOps, RealTrustDb, RealVtControl, RealWebPolicyOps,
    };
    use crate::persistence::{
        RealChildClauseStore, RealClauseStore, RealConsumedIdStore, RealCuratorListStore,
        RealExtensionStore, RealPairingStore, RealPendingStore, RealUsageStore,
    };
    use crate::relay::RealRelayTransport;
    use crate::signer::RealMachineSigner;

    /// Compile-only real system aggregate. Every port is a `Real*` stub; the
    /// owning phases wire the live shell-out/zbus/cgroup behaviour later. Never
    /// run in the headless gate.
    #[derive(Default)]
    pub struct RealSystem {
        clock: RealClock,
        consumed: RealConsumedIdStore,
        pending: RealPendingStore,
        clauses: RealClauseStore,
        child_clauses: RealChildClauseStore,
        curator_lists: RealCuratorListStore,
        usage: RealUsageStore,
        extension: RealExtensionStore,
        pairing: RealPairingStore,
        flatpak: RealFlatpakOps,
        trust: RealTrustDb,
        approved: RealApprovedExecStore,
        exec: RealExecGate,
        denials: RealDenialWatch,
        freezer: RealCgroupFreezer,
        session: RealSessionControl,
        vt: RealVtControl,
        mounts: RealMountOps,
        polkit: RealPolkitOps,
        account: RealAccountOps,
        systemctl: RealSystemctlOps,
        machine: RealMachineSigner,
        relay: RealRelayTransport,
        web_policy: RealWebPolicyOps,
        dns_filter: RealDnsFilterOps,
    }

    impl SystemLayer for RealSystem {
        type Clock = RealClock;
        type ConsumedIds = RealConsumedIdStore;
        type Pending = RealPendingStore;
        type Clauses = RealClauseStore;
        type ChildClauses = RealChildClauseStore;
        type CuratorLists = RealCuratorListStore;
        type Usage = RealUsageStore;
        type Extension = RealExtensionStore;
        type Pairing = RealPairingStore;
        type Flatpak = RealFlatpakOps;
        type Trust = RealTrustDb;
        type Approved = RealApprovedExecStore;
        type Exec = RealExecGate;
        type Denials = RealDenialWatch;
        type Freezer = RealCgroupFreezer;
        type Session = RealSessionControl;
        type Vt = RealVtControl;
        type Mounts = RealMountOps;
        type Polkit = RealPolkitOps;
        type Account = RealAccountOps;
        type Systemctl = RealSystemctlOps;
        type Machine = RealMachineSigner;
        type Relay = RealRelayTransport;
        type WebPolicy = RealWebPolicyOps;
        type DnsFilter = RealDnsFilterOps;

        fn clock(&self) -> &Self::Clock {
            &self.clock
        }
        fn consumed_ids(&self) -> &Self::ConsumedIds {
            &self.consumed
        }
        fn pending(&self) -> &Self::Pending {
            &self.pending
        }
        fn clauses(&self) -> &Self::Clauses {
            &self.clauses
        }
        fn child_clauses(&self) -> &Self::ChildClauses {
            &self.child_clauses
        }
        fn curator_lists(&self) -> &Self::CuratorLists {
            &self.curator_lists
        }
        fn usage(&self) -> &Self::Usage {
            &self.usage
        }
        fn extension(&self) -> &Self::Extension {
            &self.extension
        }
        fn pairing(&self) -> &Self::Pairing {
            &self.pairing
        }
        fn flatpak(&self) -> &Self::Flatpak {
            &self.flatpak
        }
        fn trust(&self) -> &Self::Trust {
            &self.trust
        }
        fn approved(&self) -> &Self::Approved {
            &self.approved
        }
        fn exec(&self) -> &Self::Exec {
            &self.exec
        }
        fn denials(&self) -> &Self::Denials {
            &self.denials
        }
        fn freezer(&self) -> &Self::Freezer {
            &self.freezer
        }
        fn session(&self) -> &Self::Session {
            &self.session
        }
        fn vt(&self) -> &Self::Vt {
            &self.vt
        }
        fn mounts(&self) -> &Self::Mounts {
            &self.mounts
        }
        fn polkit(&self) -> &Self::Polkit {
            &self.polkit
        }
        fn account(&self) -> &Self::Account {
            &self.account
        }
        fn systemctl(&self) -> &Self::Systemctl {
            &self.systemctl
        }
        fn machine(&self) -> &Self::Machine {
            &self.machine
        }
        fn relay(&self) -> &Self::Relay {
            &self.relay
        }
        fn web_policy(&self) -> &Self::WebPolicy {
            &self.web_policy
        }
        fn dns_filter(&self) -> &Self::DnsFilter {
            &self.dns_filter
        }
    }
}

#[cfg(all(feature = "real-relay", feature = "real-os"))]
pub use real::RealSystem;

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;
    use crate::clock::Clock;

    #[test]
    fn generic_run_over_system_layer_compiles() {
        let sys = MockSystem::new(1000);
        run(&sys);
        assert_eq!(sys.clock().now_utc(), 1000);
    }

    #[test]
    fn mock_system_exposes_every_port() {
        let sys = MockSystem::new(0);
        // Touch every accessor so a missing port fails to compile.
        let _ = sys.clock();
        let _ = sys.consumed_ids();
        let _ = sys.pending();
        let _ = sys.clauses();
        let _ = sys.child_clauses();
        let _ = sys.curator_lists();
        let _ = sys.usage();
        let _ = sys.extension();
        let _ = sys.pairing();
        let _ = sys.flatpak();
        let _ = sys.trust();
        let _ = sys.approved();
        let _ = sys.exec();
        let _ = sys.denials();
        let _ = sys.freezer();
        let _ = sys.session();
        let _ = sys.vt();
        let _ = sys.mounts();
        let _ = sys.polkit();
        let _ = sys.account();
        let _ = sys.systemctl();
        let _ = sys.machine();
        let _ = sys.relay();
        let _ = sys.web_policy();
        let _ = sys.dns_filter();
    }
}
