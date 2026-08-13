//! `AndroidSystem` — the Android composition of the `SystemLayer` ports
//! (port-spec §2.2). The decide/verify/persist ports are the SAME file-backed
//! `Real*` impls the Linux warden ships (atomic temp+fsync+rename, in-process
//! serialization), rooted in the app's device-protected storage. The
//! Linux-only OS-effect ports get fail-closed `Unsupported*` stubs: the
//! Android enforcement loop maps decisions to Kotlin (DevicePolicyManager)
//! effects instead, so nothing ever calls them — if something does, it errors
//! rather than silently pretending.

use std::path::Path;

use async_trait::async_trait;

use charter_primitives::NostrEvent;
use charter_sys::clock::Clock;
use charter_sys::effects::{
    AccountOps, AdmitMeta, ApprovedExecStore, CgroupFreezer, DenialWatch, DnsFilterOps, ExecGate,
    FlatpakAppInfo, FlatpakOps, InspectResult, MountOps, PolkitOps, SessionControl, SystemctlOps,
    TrustDb, VtControl, WebPolicyOps,
};
use charter_sys::persistence::{
    RealChildClauseStore, RealClauseStore, RealConsumedIdStore, RealCuratorListStore,
    RealExtensionStore, RealPairingStore, RealPendingStore, RealUsageStore,
};
use charter_sys::relay::{Filter, PublishOutcome, RelayIoError, RelayTransport, RelayUrl};
use charter_sys::signer::RealMachineSigner;
use charter_sys::{SysError, SysResult, SystemLayer};

/// Wall clock from `SystemTime` (may jump — the ledger clamp handles it);
/// monotonic from `CLOCK_BOOTTIME`, which keeps counting through suspend —
/// a phone dozes constantly and budget accounting must count across it
/// (port-spec §2.2; the CLOCK_MONOTONIC-stops-in-suspend gotcha).
#[derive(Default)]
pub struct AndroidClock;

impl Clock for AndroidClock {
    fn now_utc(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn monotonic_millis(&self) -> u64 {
        let mut ts = libc::timespec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        // SAFETY: clock_gettime with a valid clock id + out-pointer.
        let rc = unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
        if rc == 0 {
            (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000
        } else {
            0
        }
    }
}

fn unsupported<T>(what: &str) -> SysResult<T> {
    Err(SysError::Io(format!(
        "{what}: unsupported on Android (Kotlin owns enforcement effects)"
    )))
}

/// The broker never touches `sys.relay()` (delivery rides the
/// `TransportFacade`), but the port must exist: a relay that fails every
/// publish and returns no events.
#[derive(Default)]
pub struct NullRelay;

#[async_trait]
impl RelayTransport for NullRelay {
    async fn publish(
        &self,
        relays: &[RelayUrl],
        _ev: NostrEvent,
    ) -> Vec<(RelayUrl, PublishOutcome)> {
        relays
            .iter()
            .map(|r| (r.clone(), PublishOutcome::Failed("null relay".into())))
            .collect()
    }
    async fn query(
        &self,
        _relays: &[RelayUrl],
        _filter: Filter,
    ) -> Result<Vec<NostrEvent>, RelayIoError> {
        Ok(Vec::new())
    }
}

macro_rules! stub {
    ($name:ident) => {
        #[derive(Default)]
        pub struct $name;
    };
}

stub!(UnsupportedFlatpak);
#[async_trait]
impl FlatpakOps for UnsupportedFlatpak {
    async fn resolve(&self, _r: &str) -> SysResult<FlatpakAppInfo> {
        unsupported("flatpak resolve")
    }
    async fn is_installed_system(&self, _r: &str) -> SysResult<bool> {
        unsupported("flatpak is_installed")
    }
    async fn install_system(&self, _r: &str) -> SysResult<()> {
        unsupported("flatpak install")
    }
    async fn app_permissions(&self, _r: &str) -> SysResult<Vec<String>> {
        unsupported("flatpak permissions")
    }
}

stub!(UnsupportedTrust);
#[async_trait]
impl TrustDb for UnsupportedTrust {
    async fn trust_hash(&self, _h: &str) -> SysResult<()> {
        unsupported("trust db")
    }
}

stub!(UnsupportedApproved);
#[async_trait]
impl ApprovedExecStore for UnsupportedApproved {
    async fn inspect(&self, _p: &str) -> SysResult<InspectResult> {
        unsupported("exec inspect")
    }
    async fn contains(&self, _h: &str) -> SysResult<bool> {
        unsupported("exec contains")
    }
    async fn admit(&self, _p: &str, _h: &str, _m: &AdmitMeta) -> SysResult<()> {
        unsupported("exec admit")
    }
    async fn launcher(&self, _h: &str, _n: &str) -> SysResult<()> {
        unsupported("exec launcher")
    }
    async fn list(&self) -> SysResult<Vec<String>> {
        unsupported("exec list")
    }
    async fn remove(&self, _h: &str) -> SysResult<()> {
        unsupported("exec remove")
    }
}

stub!(UnsupportedExecGate);
#[async_trait]
impl ExecGate for UnsupportedExecGate {
    async fn is_blocked(&self, _p: &str) -> SysResult<bool> {
        // Fail-closed: if anything ever asks, "blocked".
        Ok(true)
    }
}

stub!(UnsupportedDenials);
#[async_trait]
impl DenialWatch for UnsupportedDenials {
    async fn next_denial(&self) -> SysResult<Option<String>> {
        Ok(None)
    }
}

stub!(UnsupportedFreezer);
#[async_trait]
impl CgroupFreezer for UnsupportedFreezer {
    async fn freeze(&self, _t: &str) -> SysResult<()> {
        unsupported("cgroup freeze")
    }
    async fn thaw(&self, _t: &str) -> SysResult<()> {
        unsupported("cgroup thaw")
    }
    async fn exists(&self, _t: &str) -> bool {
        false
    }
}

stub!(UnsupportedSession);
#[async_trait]
impl SessionControl for UnsupportedSession {
    async fn show_lock(&self, _t: &str, _d: &str) -> SysResult<()> {
        unsupported("session lock")
    }
    async fn hide_lock(&self) -> SysResult<()> {
        unsupported("session unlock")
    }
}

stub!(UnsupportedVt);
#[async_trait]
impl VtControl for UnsupportedVt {
    async fn set_vt_switching(&self, _e: bool) -> SysResult<()> {
        unsupported("vt switching")
    }
    async fn activate_vt(&self, _v: u32) -> SysResult<()> {
        unsupported("vt activate")
    }
}

stub!(UnsupportedMounts);
#[async_trait]
impl MountOps for UnsupportedMounts {
    async fn read_options(&self, _t: &str) -> SysResult<Vec<String>> {
        unsupported("mount options")
    }
}

stub!(UnsupportedPolkit);
#[async_trait]
impl PolkitOps for UnsupportedPolkit {
    async fn rules_present(&self) -> SysResult<bool> {
        unsupported("polkit rules")
    }
}

stub!(UnsupportedAccount);
#[async_trait]
impl AccountOps for UnsupportedAccount {
    async fn in_admin_group(&self, _u: &str) -> SysResult<bool> {
        // Fail-closed: nobody is an admin via this port on Android.
        Ok(false)
    }
}

stub!(UnsupportedSystemctl);
#[async_trait]
impl SystemctlOps for UnsupportedSystemctl {
    async fn is_active(&self, _u: &str) -> SysResult<bool> {
        unsupported("systemctl")
    }
}

stub!(UnsupportedWebPolicy);
#[async_trait]
impl WebPolicyOps for UnsupportedWebPolicy {
    async fn write_policies(&self, _p: &str) -> SysResult<()> {
        unsupported("web policy")
    }
    async fn clear_policies(&self) -> SysResult<()> {
        unsupported("web policy clear")
    }
}

stub!(UnsupportedDnsFilter);
#[async_trait]
impl DnsFilterOps for UnsupportedDnsFilter {
    async fn apply_plan(&self, _p: &str) -> SysResult<()> {
        unsupported("dns filter")
    }
    async fn clear(&self) -> SysResult<()> {
        unsupported("dns filter clear")
    }
}

/// The Android system layer: real persistence + identity, stubbed OS effects.
pub struct AndroidSystem {
    clock: AndroidClock,
    consumed: RealConsumedIdStore,
    pending: RealPendingStore,
    clauses: RealClauseStore,
    child_clauses: RealChildClauseStore,
    curator_lists: RealCuratorListStore,
    usage: RealUsageStore,
    extension: RealExtensionStore,
    pairing: RealPairingStore,
    machine: RealMachineSigner,
    relay: NullRelay,
    flatpak: UnsupportedFlatpak,
    trust: UnsupportedTrust,
    approved: UnsupportedApproved,
    exec: UnsupportedExecGate,
    denials: UnsupportedDenials,
    freezer: UnsupportedFreezer,
    session: UnsupportedSession,
    vt: UnsupportedVt,
    mounts: UnsupportedMounts,
    polkit: UnsupportedPolkit,
    account: UnsupportedAccount,
    systemctl: UnsupportedSystemctl,
    web_policy: UnsupportedWebPolicy,
    dns_filter: UnsupportedDnsFilter,
}

impl AndroidSystem {
    /// All stores rooted under `base` (device-protected storage) — the same
    /// directory the warden's own handles use; the shared in-process store
    /// serialization keeps the two sets of handles safe.
    pub fn with_base(base: &Path) -> SysResult<Self> {
        Ok(AndroidSystem {
            clock: AndroidClock,
            consumed: RealConsumedIdStore::with_base(base),
            pending: RealPendingStore::with_base(base),
            clauses: RealClauseStore::with_base(base),
            child_clauses: RealChildClauseStore::with_base(base),
            curator_lists: RealCuratorListStore::with_base(base),
            usage: RealUsageStore::with_base(base),
            extension: RealExtensionStore::with_base(base),
            pairing: RealPairingStore::with_base(base),
            machine: RealMachineSigner::load_or_create(base.join("machine.key"))?,
            relay: NullRelay,
            flatpak: UnsupportedFlatpak,
            trust: UnsupportedTrust,
            approved: UnsupportedApproved,
            exec: UnsupportedExecGate,
            denials: UnsupportedDenials,
            freezer: UnsupportedFreezer,
            session: UnsupportedSession,
            vt: UnsupportedVt,
            mounts: UnsupportedMounts,
            polkit: UnsupportedPolkit,
            account: UnsupportedAccount,
            systemctl: UnsupportedSystemctl,
            web_policy: UnsupportedWebPolicy,
            dns_filter: UnsupportedDnsFilter,
        })
    }
}

impl SystemLayer for AndroidSystem {
    type Clock = AndroidClock;
    type ConsumedIds = RealConsumedIdStore;
    type Pending = RealPendingStore;
    type Clauses = RealClauseStore;
    type ChildClauses = RealChildClauseStore;
    type CuratorLists = RealCuratorListStore;
    type Usage = RealUsageStore;
    type Extension = RealExtensionStore;
    type Pairing = RealPairingStore;
    type Flatpak = UnsupportedFlatpak;
    type Trust = UnsupportedTrust;
    type Approved = UnsupportedApproved;
    type Exec = UnsupportedExecGate;
    type Denials = UnsupportedDenials;
    type Freezer = UnsupportedFreezer;
    type Session = UnsupportedSession;
    type Vt = UnsupportedVt;
    type Mounts = UnsupportedMounts;
    type Polkit = UnsupportedPolkit;
    type Account = UnsupportedAccount;
    type Systemctl = UnsupportedSystemctl;
    type Machine = RealMachineSigner;
    type Relay = NullRelay;
    type WebPolicy = UnsupportedWebPolicy;
    type DnsFilter = UnsupportedDnsFilter;

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
