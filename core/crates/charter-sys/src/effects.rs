//! IO effect ports. Every trait has a deterministic in-memory `Mock*` (all
//! headless tests) and a `Real*` impl. The `Real*` impls' pure cores
//! (path/argv construction, `/proc` + db parsing, the VT-lock mapping) run +
//! are tested on the headless gate; the OS-touching tail (shell-outs, sysfs/
//! ioctl writes, the lock spawn) is compile-verified here and exercised on the
//! Mint VM.

use async_trait::async_trait;

use crate::error::SysResult;

/// App metadata surfaced to the guardian at approval time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatpakAppInfo {
    pub reference: String,
    pub name: String,
    /// The app's requested permissions/portals (advisory, for the guardian).
    pub permissions: Vec<String>,
}

/// Flatpak system install/query (Phase 4). The real impl shells out with
/// args-arrays only (never a shell) and applies charterd-tightened overrides.
#[async_trait]
pub trait FlatpakOps: Send + Sync {
    /// Resolve a Flathub reference to its display metadata + permissions.
    async fn resolve(&self, reference: &str) -> SysResult<FlatpakAppInfo>;
    /// Whether `reference` is already installed system-wide.
    async fn is_installed_system(&self, reference: &str) -> SysResult<bool>;
    /// System-install `reference` (idempotent at the caller; tightened
    /// overrides applied in the real impl).
    async fn install_system(&self, reference: &str) -> SysResult<()>;
    /// The app's requested permissions/portals.
    async fn app_permissions(&self, reference: &str) -> SysResult<Vec<String>>;
}

/// Share a `FlatpakOps` behind an `Arc` (the enactor owns one; tests keep a
/// clone to inspect call records).
#[async_trait]
impl<T: FlatpakOps + ?Sized> FlatpakOps for std::sync::Arc<T> {
    async fn resolve(&self, reference: &str) -> SysResult<FlatpakAppInfo> {
        (**self).resolve(reference).await
    }
    async fn is_installed_system(&self, reference: &str) -> SysResult<bool> {
        (**self).is_installed_system(reference).await
    }
    async fn install_system(&self, reference: &str) -> SysResult<()> {
        (**self).install_system(reference).await
    }
    async fn app_permissions(&self, reference: &str) -> SysResult<Vec<String>> {
        (**self).app_permissions(reference).await
    }
}

/// fapolicyd trust-DB append/reload (Phase 5 finalizes).
#[async_trait]
pub trait TrustDb: Send + Sync {
    /// Trust an executable by its sha256 hex.
    async fn trust_hash(&self, sha256_hex: &str) -> SysResult<()>;
}

/// Share a `TrustDb` behind an `Arc`.
#[async_trait]
impl<T: TrustDb + ?Sized> TrustDb for std::sync::Arc<T> {
    async fn trust_hash(&self, sha256_hex: &str) -> SysResult<()> {
        (**self).trust_hash(sha256_hex).await
    }
}

/// Display metadata for an admitted binary. The `name` is validated and only
/// ever used as a desktop-entry-escaped `Name=` — it NEVER reaches a path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmitMeta {
    pub name: String,
    pub size: u64,
    pub origin: Option<String>,
}

/// The result of inspecting a source binary (for building the request).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectResult {
    pub sha256: String,
    pub size: u64,
}

/// Why a display name was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NameError;

/// Validate an exec display name: non-empty, length-bounded, no `/`, `..`, NUL,
/// or control characters. (Stops path traversal + desktop-entry injection.)
pub fn validate_display_name(name: &str) -> Result<(), NameError> {
    if name.is_empty() || name.len() > 128 {
        return Err(NameError);
    }
    if name.contains('/') || name.contains("..") {
        return Err(NameError);
    }
    if name.chars().any(|c| c.is_control()) {
        return Err(NameError);
    }
    Ok(())
}

/// Escape a (already-validated) name for a desktop-entry `Name=` value.
pub fn desktop_escape(name: &str) -> String {
    // Backslash escaping; control chars (incl. CR/LF) are already rejected by
    // validate_display_name, stripped here as belt-and-suspenders.
    name.replace('\\', "\\\\").replace(['\r', '\n'], "")
}

/// The root-owned approved-exec store. `admit` re-hashes the COPIED bytes (TOCTOU
/// close) and stores under the sha256 filename — never `meta.name`.
#[async_trait]
pub trait ApprovedExecStore: Send + Sync {
    /// Hash + size a source binary (request-building side).
    async fn inspect(&self, src_path: &str) -> SysResult<InspectResult>;
    /// Whether the store already holds a binary with this sha256 hex.
    async fn contains(&self, sha256_hex: &str) -> SysResult<bool>;
    /// Copy `src_path` into the store, re-hashing the copied bytes; on mismatch
    /// with `expected_sha256` nothing is stored (`Conflict`). Stores under the
    /// sha256 filename; `meta.name` is validated and never used as a path.
    async fn admit(&self, src_path: &str, expected_sha256: &str, meta: &AdmitMeta)
        -> SysResult<()>;
    /// Write a launcher `.desktop` whose `Exec=` points at the stored sha256
    /// path and whose `Name=` is the desktop-escaped display name.
    async fn launcher(&self, sha256_hex: &str, name: &str) -> SysResult<()>;
    /// List stored sha256 hexes.
    async fn list(&self) -> SysResult<Vec<String>>;
    /// Remove a stored binary + its launcher.
    async fn remove(&self, sha256_hex: &str) -> SysResult<()>;
}

/// Share an `ApprovedExecStore` behind an `Arc`.
#[async_trait]
impl<S: ApprovedExecStore + ?Sized> ApprovedExecStore for std::sync::Arc<S> {
    async fn inspect(&self, src_path: &str) -> SysResult<InspectResult> {
        (**self).inspect(src_path).await
    }
    async fn contains(&self, sha256_hex: &str) -> SysResult<bool> {
        (**self).contains(sha256_hex).await
    }
    async fn admit(&self, src: &str, expected: &str, meta: &AdmitMeta) -> SysResult<()> {
        (**self).admit(src, expected, meta).await
    }
    async fn launcher(&self, sha256_hex: &str, name: &str) -> SysResult<()> {
        (**self).launcher(sha256_hex, name).await
    }
    async fn list(&self) -> SysResult<Vec<String>> {
        (**self).list().await
    }
    async fn remove(&self, sha256_hex: &str) -> SysResult<()> {
        (**self).remove(sha256_hex).await
    }
}

/// Execution-gate query (Phase 5 finalizes).
#[async_trait]
pub trait ExecGate: Send + Sync {
    /// Whether `path` is currently blocked from execution.
    async fn is_blocked(&self, path: &str) -> SysResult<bool>;
}

/// fapolicyd denial watch (Phase 5 finalizes) — surfaces blocked execs as
/// prompts instead of silent failures.
#[async_trait]
pub trait DenialWatch: Send + Sync {
    /// The next observed denial path, if any.
    async fn next_denial(&self) -> SysResult<Option<String>>;
}

/// cgroup-v2 app-slice freeze/thaw (Phase 6 finalizes). The freeze target is
/// always the managed `app.slice` — never `charterd` or the lock UI.
#[async_trait]
pub trait CgroupFreezer: Send + Sync {
    /// Freeze the named target.
    async fn freeze(&self, target: &str) -> SysResult<()>;
    /// Thaw the named target.
    async fn thaw(&self, target: &str) -> SysResult<()>;
    /// True iff the target cgroup slice exists (i.e. the user has a live session,
    /// so there is something to freeze). The enforcement loop reconciles the
    /// freeze state level-triggered, so it must not attempt — and error on — a
    /// slice that does not exist yet (a managed child who is not logged in).
    async fn exists(&self, target: &str) -> bool;
}

/// Session control: show/hide the undismissable Charter lock (Phase 6).
#[async_trait]
pub trait SessionControl: Send + Sync {
    /// Show the fullscreen lock (acquire grab) before freezing, with a `title`
    /// + `detail` message explaining why (so the child sees "Time's up for
    /// today", not a blank screen).
    async fn show_lock(&self, title: &str, detail: &str) -> SysResult<()>;
    /// Hide the lock on thaw.
    async fn hide_lock(&self) -> SysResult<()>;
}

/// Virtual-terminal switching control (Phase 6).
#[async_trait]
pub trait VtControl: Send + Sync {
    /// Enable/disable VT switching for the managed session.
    async fn set_vt_switching(&self, enabled: bool) -> SysResult<()>;
    /// Switch the console to `vt` (VT_ACTIVATE) — un-strands a display parked
    /// on a dead VT after the locked session ends. Requires switching enabled.
    async fn activate_vt(&self, vt: u32) -> SysResult<()>;
}

/// Mount options inspection (Phase 9).
#[async_trait]
pub trait MountOps: Send + Sync {
    /// The mount options currently in effect for `target`.
    async fn read_options(&self, target: &str) -> SysResult<Vec<String>>;
}

/// polkit rule presence (Phase 9).
#[async_trait]
pub trait PolkitOps: Send + Sync {
    /// Whether the Charter polkit rules are installed.
    async fn rules_present(&self) -> SysResult<bool>;
}

/// Account administration (Phase 9). The DOB clear-path lives in the
/// `charter-setup` crate, the single module allowlisted by the privacy guard —
/// no `birthDate` token appears here.
#[async_trait]
pub trait AccountOps: Send + Sync {
    /// Whether `user` is a member of an admin group (sudo/adm/lpadmin).
    async fn in_admin_group(&self, user: &str) -> SysResult<bool>;
}

/// systemctl unit control (Phase 9).
#[async_trait]
pub trait SystemctlOps: Send + Sync {
    /// Whether `unit` is active.
    async fn is_active(&self, unit: &str) -> SysResult<bool>;
}

/// Firefox machine-policy file management (web content control). The real impl
/// writes the rendered `policies.json` root-owned + immutable and re-asserts on
/// drift. `policies_json` is the opaque rendered document (see `charter-webpolicy`).
#[async_trait]
pub trait WebPolicyOps: Send + Sync {
    /// Install/refresh the Firefox policy document (idempotent state-sync).
    async fn write_policies(&self, policies_json: &str) -> SysResult<()>;
    /// Remove the managed policy file (content control revoked).
    async fn clear_policies(&self) -> SysResult<()>;
}

/// DNS-layer filtering management (web content control): configure the local
/// resolver (AdGuard Home), pin `/etc/resolv.conf`, firewall DNS/QUIC. The real
/// impl interprets `plan_json` (the opaque rendered DNS plan).
#[async_trait]
pub trait DnsFilterOps: Send + Sync {
    /// Apply the DNS filtering plan + pin the resolver (idempotent state-sync).
    async fn apply_plan(&self, plan_json: &str) -> SysResult<()>;
    /// Tear down filtering + unpin the resolver (content control revoked).
    async fn clear(&self) -> SysResult<()>;
}

// ---------------------------------------------------------------------------
// Mock impls.
// ---------------------------------------------------------------------------

#[cfg(feature = "mock")]
mod mock {
    use std::sync::Mutex;

    use super::*;

    /// Mock flatpak: a seeded catalog, an installed set, an install-call
    /// recorder, and an offline toggle.
    #[derive(Default)]
    pub struct MockFlatpakOps {
        catalog: Mutex<Vec<FlatpakAppInfo>>,
        installed: Mutex<Vec<String>>,
        install_calls: Mutex<Vec<String>>,
        offline: Mutex<bool>,
    }
    impl MockFlatpakOps {
        pub fn new() -> Self {
            Self::default()
        }
        /// Add a resolvable app to the catalog.
        pub fn with_app(self, reference: &str, name: &str, permissions: &[&str]) -> Self {
            self.catalog.lock().expect("lock").push(FlatpakAppInfo {
                reference: reference.to_string(),
                name: name.to_string(),
                permissions: permissions.iter().map(|s| s.to_string()).collect(),
            });
            self
        }
        /// Seed an already-installed reference.
        pub fn with_installed(self, reference: &str) -> Self {
            self.installed
                .lock()
                .expect("lock")
                .push(reference.to_string());
            self
        }
        /// Force the offline state (resolve/install fail).
        pub fn set_offline(&self, offline: bool) {
            *self.offline.lock().expect("lock") = offline;
        }
        /// The references passed to `install_system`, in order.
        pub fn install_calls(&self) -> Vec<String> {
            self.install_calls.lock().expect("lock").clone()
        }
        fn lookup(&self, reference: &str) -> Option<FlatpakAppInfo> {
            self.catalog
                .lock()
                .expect("lock")
                .iter()
                .find(|a| a.reference == reference)
                .cloned()
        }
    }
    #[async_trait]
    impl FlatpakOps for MockFlatpakOps {
        async fn resolve(&self, reference: &str) -> SysResult<FlatpakAppInfo> {
            if *self.offline.lock().expect("lock") {
                return Err(crate::error::SysError::Io("offline".into()));
            }
            self.lookup(reference)
                .ok_or(crate::error::SysError::NotFound)
        }
        async fn is_installed_system(&self, reference: &str) -> SysResult<bool> {
            Ok(self
                .installed
                .lock()
                .expect("lock")
                .iter()
                .any(|r| r == reference))
        }
        async fn install_system(&self, reference: &str) -> SysResult<()> {
            if *self.offline.lock().expect("lock") {
                return Err(crate::error::SysError::Io("offline".into()));
            }
            self.install_calls
                .lock()
                .expect("lock")
                .push(reference.to_string());
            self.installed
                .lock()
                .expect("lock")
                .push(reference.to_string());
            Ok(())
        }
        async fn app_permissions(&self, reference: &str) -> SysResult<Vec<String>> {
            Ok(self
                .lookup(reference)
                .map(|a| a.permissions)
                .unwrap_or_default())
        }
    }

    /// Mock trust DB recording trusted hashes, with a failure injector.
    #[derive(Default)]
    pub struct MockTrustDb {
        trusted: Mutex<Vec<String>>,
        fail: Mutex<bool>,
        fail_times: Mutex<usize>,
    }
    impl MockTrustDb {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn trusted(&self) -> Vec<String> {
            self.trusted.lock().expect("lock").clone()
        }
        /// Make the next (and all) `trust_hash` calls fail (fail-closed test).
        pub fn set_fail(&self, fail: bool) {
            *self.fail.lock().expect("lock") = fail;
        }
        /// Fail the next `n` `trust_hash` calls, then succeed — exercises the
        /// broker's transient-retry loop (M8).
        pub fn set_fail_times(&self, n: usize) {
            *self.fail_times.lock().expect("lock") = n;
        }
    }
    #[async_trait]
    impl TrustDb for MockTrustDb {
        async fn trust_hash(&self, sha256_hex: &str) -> SysResult<()> {
            if *self.fail.lock().expect("lock") {
                return Err(crate::error::SysError::Io("fapolicyd reload failed".into()));
            }
            {
                let mut ft = self.fail_times.lock().expect("lock");
                if *ft > 0 {
                    *ft -= 1;
                    return Err(crate::error::SysError::Io(
                        "fapolicyd reload (transient)".into(),
                    ));
                }
            }
            self.trusted
                .lock()
                .expect("lock")
                .push(sha256_hex.to_string());
            Ok(())
        }
    }

    /// Mock approved-exec store: an in-memory source fs + store + launchers, a
    /// `noexec` toggle, and `mutate_source` to exercise the TOCTOU close.
    #[derive(Default)]
    pub struct MockApprovedExecStore {
        source_fs: Mutex<Vec<(String, Vec<u8>)>>,
        store: Mutex<Vec<String>>,                       // sha256 hexes
        launchers: Mutex<Vec<(String, String, String)>>, // (sha256, name, desktop)
        noexec: Mutex<bool>,
    }
    impl MockApprovedExecStore {
        pub fn new() -> Self {
            Self::default()
        }
        /// Place a source binary at `path`.
        pub fn put_source(&self, path: &str, bytes: &[u8]) {
            let mut fs = self.source_fs.lock().expect("lock");
            fs.retain(|(p, _)| p != path);
            fs.push((path.to_string(), bytes.to_vec()));
        }
        /// Mutate a source binary after inspection (TOCTOU attempt).
        pub fn mutate_source(&self, path: &str, bytes: &[u8]) {
            self.put_source(path, bytes);
        }
        /// Mark the store root as a noexec mount (admit must refuse).
        pub fn set_noexec(&self, noexec: bool) {
            *self.noexec.lock().expect("lock") = noexec;
        }
        fn read_source(&self, path: &str) -> Option<Vec<u8>> {
            self.source_fs
                .lock()
                .expect("lock")
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, b)| b.clone())
        }
        /// The launcher desktop entries written, as `(sha256, name, desktop)`.
        pub fn launchers(&self) -> Vec<(String, String, String)> {
            self.launchers.lock().expect("lock").clone()
        }
    }
    #[async_trait]
    impl ApprovedExecStore for MockApprovedExecStore {
        async fn inspect(&self, src_path: &str) -> SysResult<InspectResult> {
            let bytes = self
                .read_source(src_path)
                .ok_or(crate::error::SysError::NotFound)?;
            Ok(InspectResult {
                sha256: hex32(&charter_crypto::sha256(&bytes)),
                size: bytes.len() as u64,
            })
        }
        async fn contains(&self, sha256_hex: &str) -> SysResult<bool> {
            Ok(self
                .store
                .lock()
                .expect("lock")
                .iter()
                .any(|h| h == sha256_hex))
        }
        async fn admit(
            &self,
            src_path: &str,
            expected_sha256: &str,
            meta: &AdmitMeta,
        ) -> SysResult<()> {
            // Validate the display name before it can reach a launcher.
            validate_display_name(&meta.name)
                .map_err(|_| crate::error::SysError::Conflict("invalid display name".into()))?;
            if *self.noexec.lock().expect("lock") {
                return Err(crate::error::SysError::Unsupported("NotOnExecMount".into()));
            }
            // Copy the bytes NOW and re-hash them (TOCTOU close).
            let bytes = self
                .read_source(src_path)
                .ok_or(crate::error::SysError::NotFound)?;
            let actual = hex32(&charter_crypto::sha256(&bytes));
            if actual != expected_sha256 {
                // Nothing is stored on mismatch.
                return Err(crate::error::SysError::Conflict("hash mismatch".into()));
            }
            self.store.lock().expect("lock").push(actual);
            Ok(())
        }
        async fn launcher(&self, sha256_hex: &str, name: &str) -> SysResult<()> {
            let escaped = desktop_escape(name);
            let desktop = format!(
                "[Desktop Entry]\nType=Application\nName={escaped}\nExec=/var/lib/charter/approved/{sha256_hex}/run\n"
            );
            self.launchers
                .lock()
                .expect("lock")
                .push((sha256_hex.to_string(), escaped, desktop));
            Ok(())
        }
        async fn list(&self) -> SysResult<Vec<String>> {
            Ok(self.store.lock().expect("lock").clone())
        }
        async fn remove(&self, sha256_hex: &str) -> SysResult<()> {
            self.store.lock().expect("lock").retain(|h| h != sha256_hex);
            self.launchers
                .lock()
                .expect("lock")
                .retain(|(h, _, _)| h != sha256_hex);
            Ok(())
        }
    }

    fn hex32(bytes: &[u8; 32]) -> String {
        let mut s = String::with_capacity(64);
        for b in bytes {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }

    /// Mock exec gate.
    #[derive(Default)]
    pub struct MockExecGate;
    #[async_trait]
    impl ExecGate for MockExecGate {
        async fn is_blocked(&self, _path: &str) -> SysResult<bool> {
            Ok(true)
        }
    }

    /// Mock denial watch (always quiet).
    #[derive(Default)]
    pub struct MockDenialWatch;
    #[async_trait]
    impl DenialWatch for MockDenialWatch {
        async fn next_denial(&self) -> SysResult<Option<String>> {
            Ok(None)
        }
    }

    /// Mock cgroup freezer recording freeze/thaw calls.
    #[derive(Default)]
    pub struct MockCgroupFreezer {
        log: Mutex<Vec<(String, bool)>>, // (target, frozen)
    }
    impl MockCgroupFreezer {
        pub fn new() -> Self {
            Self::default()
        }
        /// True if `target` is currently frozen.
        pub fn is_frozen(&self, target: &str) -> bool {
            self.log
                .lock()
                .expect("lock")
                .iter()
                .rfind(|(t, _)| t == target)
                .map(|(_, f)| *f)
                .unwrap_or(false)
        }
    }
    #[async_trait]
    impl CgroupFreezer for MockCgroupFreezer {
        async fn freeze(&self, target: &str) -> SysResult<()> {
            self.log
                .lock()
                .expect("lock")
                .push((target.to_string(), true));
            Ok(())
        }
        async fn thaw(&self, target: &str) -> SysResult<()> {
            self.log
                .lock()
                .expect("lock")
                .push((target.to_string(), false));
            Ok(())
        }
        async fn exists(&self, _target: &str) -> bool {
            // The mock assumes a live session (there is always something to
            // freeze); tests that need to simulate a not-logged-in child use the
            // real freezer with a redirected root.
            true
        }
    }

    /// Mock session control recording lock visibility.
    #[derive(Default)]
    pub struct MockSessionControl {
        locked: Mutex<bool>,
        last_message: Mutex<Option<(String, String)>>,
    }
    impl MockSessionControl {
        pub fn new() -> Self {
            Self::default()
        }
        pub fn is_locked(&self) -> bool {
            *self.locked.lock().expect("lock")
        }
        /// The `(title, detail)` of the most recent `show_lock`, if any.
        pub fn last_message(&self) -> Option<(String, String)> {
            self.last_message.lock().expect("lock").clone()
        }
    }
    #[async_trait]
    impl SessionControl for MockSessionControl {
        async fn show_lock(&self, title: &str, detail: &str) -> SysResult<()> {
            *self.locked.lock().expect("lock") = true;
            *self.last_message.lock().expect("lock") =
                Some((title.to_string(), detail.to_string()));
            Ok(())
        }
        async fn hide_lock(&self) -> SysResult<()> {
            *self.locked.lock().expect("lock") = false;
            Ok(())
        }
    }

    /// Mock VT control.
    #[derive(Default)]
    pub struct MockVtControl {
        switching: Mutex<bool>,
    }
    impl MockVtControl {
        pub fn new() -> Self {
            Self {
                switching: Mutex::new(true),
            }
        }
        pub fn switching_enabled(&self) -> bool {
            *self.switching.lock().expect("lock")
        }
    }
    #[async_trait]
    impl VtControl for MockVtControl {
        async fn set_vt_switching(&self, enabled: bool) -> SysResult<()> {
            *self.switching.lock().expect("lock") = enabled;
            Ok(())
        }
        async fn activate_vt(&self, _vt: u32) -> SysResult<()> {
            Ok(())
        }
    }

    macro_rules! trivial_mock {
        ($name:ident) => {
            #[derive(Default)]
            pub struct $name;
        };
    }
    trivial_mock!(MockMountOps);
    trivial_mock!(MockPolkitOps);
    trivial_mock!(MockAccountOps);
    trivial_mock!(MockSystemctlOps);

    #[async_trait]
    impl MountOps for MockMountOps {
        async fn read_options(&self, _target: &str) -> SysResult<Vec<String>> {
            Ok(vec!["noexec".into(), "nosuid".into(), "nodev".into()])
        }
    }
    #[async_trait]
    impl PolkitOps for MockPolkitOps {
        async fn rules_present(&self) -> SysResult<bool> {
            Ok(true)
        }
    }
    #[async_trait]
    impl AccountOps for MockAccountOps {
        async fn in_admin_group(&self, _user: &str) -> SysResult<bool> {
            Ok(false)
        }
    }
    #[async_trait]
    impl SystemctlOps for MockSystemctlOps {
        async fn is_active(&self, _unit: &str) -> SysResult<bool> {
            Ok(true)
        }
    }

    /// Records the last-written Firefox policy document + whether cleared.
    #[derive(Default)]
    pub struct MockWebPolicyOps {
        last: Mutex<Option<String>>,
        cleared: Mutex<bool>,
    }
    impl MockWebPolicyOps {
        pub fn new() -> Self {
            Self::default()
        }
        /// The last policy document written (None if never).
        pub fn last_policies(&self) -> Option<String> {
            self.last.lock().expect("lock").clone()
        }
        pub fn was_cleared(&self) -> bool {
            *self.cleared.lock().expect("lock")
        }
    }
    #[async_trait]
    impl WebPolicyOps for MockWebPolicyOps {
        async fn write_policies(&self, policies_json: &str) -> SysResult<()> {
            *self.last.lock().expect("lock") = Some(policies_json.to_string());
            Ok(())
        }
        async fn clear_policies(&self) -> SysResult<()> {
            *self.cleared.lock().expect("lock") = true;
            Ok(())
        }
    }

    /// Records the last-applied DNS plan + whether cleared.
    #[derive(Default)]
    pub struct MockDnsFilterOps {
        last: Mutex<Option<String>>,
        cleared: Mutex<bool>,
    }
    impl MockDnsFilterOps {
        pub fn new() -> Self {
            Self::default()
        }
        /// The last DNS plan applied (None if never).
        pub fn last_plan(&self) -> Option<String> {
            self.last.lock().expect("lock").clone()
        }
        pub fn was_cleared(&self) -> bool {
            *self.cleared.lock().expect("lock")
        }
    }
    #[async_trait]
    impl DnsFilterOps for MockDnsFilterOps {
        async fn apply_plan(&self, plan_json: &str) -> SysResult<()> {
            *self.last.lock().expect("lock") = Some(plan_json.to_string());
            Ok(())
        }
        async fn clear(&self) -> SysResult<()> {
            *self.cleared.lock().expect("lock") = true;
            Ok(())
        }
    }
}

#[cfg(feature = "mock")]
pub use mock::{
    MockAccountOps, MockApprovedExecStore, MockCgroupFreezer, MockDenialWatch, MockDnsFilterOps,
    MockExecGate, MockFlatpakOps, MockMountOps, MockPolkitOps, MockSessionControl,
    MockSystemctlOps, MockTrustDb, MockVtControl, MockWebPolicyOps,
};

// ---------------------------------------------------------------------------
// Real stubs (compile-only).
// ---------------------------------------------------------------------------

#[cfg(feature = "real-os")]
mod real {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::sync::Mutex;

    use super::*;
    use crate::error::SysError;
    use crate::fsutil::{atomic_write, hex_bytes, io_err};

    // The two privileged state roots. `with_base` overrides them for the
    // headless filesystem tests; `Default` uses the production locations.
    const CHARTER_BASE: &str = "/var/lib/charter";
    const FIREFOX_POLICY_DIR: &str = "/etc/firefox/policies";

    /// Hard ceiling on a candidate binary handed to `inspect` / `admit`.
    ///
    /// Both used to `fs::read` the whole file to hash it, with no size check
    /// anywhere before the read — `InspectResult.size` was computed *from* the
    /// buffer, after the fact. A managed child needed only
    /// `fallocate -l 64G ~/big.bin && charter run ~/big.bin`: the file is
    /// regular, inside their own home and readable by them, so every
    /// confused-deputy guard passes and **root** charterd allocates 64 GiB on
    /// the request path, before any guardian approval is involved. The OOM
    /// killer then takes the enforcer. 512 MiB is far above any real desktop
    /// binary and is now refused outright rather than allocated.
    const MAX_CANDIDATE_BYTES: u64 = 512 * 1024 * 1024;

    /// The streaming buffer. The point of the fix: the file is never resident.
    const CANDIDATE_CHUNK_BYTES: usize = 64 * 1024;

    /// `stat` a candidate before a byte of it is read. Missing is
    /// [`SysError::NotFound`] (terminal: "the file could not be read"); over
    /// the ceiling is [`SysError::Unsupported`], which the exec enactor also
    /// treats as terminal — a 64 GiB candidate is never going to get smaller
    /// on a retry.
    fn candidate_size(path: &Path) -> SysResult<u64> {
        let meta = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SysError::NotFound),
            Err(e) => return Err(io_err("stat candidate", e)),
        };
        if meta.len() > MAX_CANDIDATE_BYTES {
            return Err(too_large(meta.len()));
        }
        Ok(meta.len())
    }

    fn too_large(len: u64) -> SysError {
        SysError::Unsupported(format!(
            "candidate is {len} bytes, over the {MAX_CANDIDATE_BYTES}-byte ceiling"
        ))
    }

    /// Stream `path` through sha256 in [`CANDIDATE_CHUNK_BYTES`] chunks,
    /// optionally writing each chunk into `sink` as it goes, and return
    /// `(sha256_hex, bytes_read)`. Peak memory is one chunk, whatever the file.
    ///
    /// The ceiling is re-checked *as we read*, not only at the `stat`: a file
    /// that grows between the two (the child appending while we copy) must not
    /// get an unbounded read through the back door.
    fn stream_file(path: &Path, mut sink: Option<&mut fs::File>) -> SysResult<(String, u64)> {
        use sha2::{Digest as _, Sha256};
        use std::io::{Read as _, Write as _};
        let mut f = match fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(SysError::NotFound),
            Err(e) => return Err(io_err("open candidate", e)),
        };
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; CANDIDATE_CHUNK_BYTES];
        let mut total: u64 = 0;
        loop {
            let n = f.read(&mut buf).map_err(|e| io_err("read candidate", e))?;
            if n == 0 {
                break;
            }
            total += n as u64;
            if total > MAX_CANDIDATE_BYTES {
                return Err(too_large(total));
            }
            hasher.update(&buf[..n]);
            if let Some(w) = sink.as_deref_mut() {
                w.write_all(&buf[..n])
                    .map_err(|e| io_err("write candidate", e))?;
            }
        }
        let digest: [u8; 32] = hasher.finalize().into();
        Ok((hex_bytes(&digest), total))
    }

    /// A staging path no concurrent admit can collide with (pid + a process-
    /// local counter). It lives in the store dir so the final `rename` stays on
    /// one filesystem, and it is removed on every failure path.
    fn staging_path(store_dir: &Path) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        store_dir.join(format!(
            ".admit-{}-{}.tmp",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ))
    }

    // ---- OS-query ports (pure parse/interpret core + thin IO) -------------

    /// Admin groups the managed user must NOT belong to (charter-setup strips
    /// these; `RealAccountOps` re-checks). Mirrors the `charter-setup` list.
    const ADMIN_GROUPS: &[&str] = &["sudo", "adm", "lpadmin"];

    /// Comma-split mount options of the LAST `/proc/mounts` line whose mount
    /// point equals `target` (the effective mount under any overmount), or
    /// `None` if `target` is not a mount point.
    fn parse_mount_options(mounts: &str, target: &str) -> Option<Vec<String>> {
        let mut found = None;
        for line in mounts.lines() {
            // fields: device mountpoint fstype options dump pass
            let mut it = line.split_whitespace();
            let (_dev, mp, _fs, opts) = (it.next(), it.next(), it.next(), it.next());
            if mp == Some(target) {
                if let Some(opts) = opts {
                    found = Some(opts.split(',').map(|s| s.to_string()).collect());
                }
            }
        }
        found
    }

    /// True if `user` is listed as a member of any `admin_groups` entry in an
    /// `/etc/group` document (`name:passwd:gid:member,member,...`).
    fn user_in_admin_group(group_file: &str, user: &str, admin_groups: &[&str]) -> bool {
        for line in group_file.lines() {
            let mut parts = line.splitn(4, ':');
            let name = parts.next().unwrap_or("");
            if !admin_groups.contains(&name) {
                continue;
            }
            let members = parts.nth(2).unwrap_or(""); // skip passwd + gid
            if members.split(',').any(|m| m == user) {
                return true;
            }
        }
        false
    }

    /// Interpret `systemctl is-active <unit>` output (only `active` is up).
    fn interpret_is_active(stdout: &str) -> bool {
        stdout.trim() == "active"
    }

    /// `/proc/mounts` reader. The core ([`parse_mount_options`]) is verified
    /// headlessly; the trait method reads the live mount table.
    #[derive(Default)]
    pub struct RealMountOps;
    #[async_trait]
    impl MountOps for RealMountOps {
        async fn read_options(&self, target: &str) -> SysResult<Vec<String>> {
            let mounts =
                fs::read_to_string("/proc/mounts").map_err(|e| io_err("read mounts", e))?;
            parse_mount_options(&mounts, target).ok_or(SysError::NotFound)
        }
    }

    /// `/etc/group` admin-membership checker (core verified headlessly). Reads
    /// the file-backed group db; NSS-only groups are out of scope (the managed
    /// admin groups are file-based on the target distro).
    #[derive(Default)]
    pub struct RealAccountOps;
    #[async_trait]
    impl AccountOps for RealAccountOps {
        async fn in_admin_group(&self, user: &str) -> SysResult<bool> {
            let group = fs::read_to_string("/etc/group").map_err(|e| io_err("read group", e))?;
            Ok(user_in_admin_group(&group, user, ADMIN_GROUPS))
        }
    }

    /// `systemctl is-active` wrapper (interpretation verified headlessly; the
    /// exec is VM-verified).
    #[derive(Default)]
    pub struct RealSystemctlOps;
    #[async_trait]
    impl SystemctlOps for RealSystemctlOps {
        async fn is_active(&self, unit: &str) -> SysResult<bool> {
            let out = Command::new("systemctl")
                .arg("is-active")
                .arg(unit)
                .output()
                .map_err(|e| io_err("systemctl is-active", e))?;
            // `is-active` exits non-zero for inactive; the word on stdout is the
            // source of truth either way.
            Ok(interpret_is_active(&String::from_utf8_lossy(&out.stdout)))
        }
    }

    /// Charter polkit-rules presence check (a file existence test, redirectable
    /// for tests via [`with_path`](RealPolkitOps::with_path)).
    pub struct RealPolkitOps {
        path: PathBuf,
    }
    impl Default for RealPolkitOps {
        fn default() -> Self {
            Self {
                path: "/etc/polkit-1/rules.d/49-charter.rules".into(),
            }
        }
    }
    impl RealPolkitOps {
        /// Test/override constructor: point at a specific rules file.
        pub fn with_path(path: impl Into<PathBuf>) -> Self {
            Self { path: path.into() }
        }
    }
    #[async_trait]
    impl PolkitOps for RealPolkitOps {
        async fn rules_present(&self) -> SysResult<bool> {
            Ok(self.path.is_file())
        }
    }

    // ---- flatpak (system install via the flatpak CLI, args-array only) ----

    /// Absolute path of an approved binary in the content-addressed store
    /// (mirrors `RealApprovedExecStore` + the fapolicyd allow-anchor).
    fn approved_bin_path(sha256_hex: &str) -> String {
        format!("{CHARTER_BASE}/approved/{sha256_hex}/run")
    }

    /// `flatpak install` argv — system scope, fully non-interactive, never a
    /// shell (no string interpolation reaches a `sh -c`).
    fn flatpak_install_argv(reference: &str) -> Vec<String> {
        vec![
            "install".into(),
            "--system".into(),
            "--noninteractive".into(),
            "--assumeyes".into(),
            "flathub".into(),
            reference.into(),
        ]
    }

    /// Parse `flatpak list --columns=application` output into app ids.
    fn parse_installed_refs(stdout: &str) -> Vec<String> {
        stdout
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .map(|l| l.to_string())
            .collect()
    }

    fn run_ok(cmd: &str, args: &[String]) -> SysResult<std::process::Output> {
        Command::new(cmd)
            .args(args)
            .output()
            .map_err(|e| io_err(cmd, e))
    }

    /// System flatpak install/query via the `flatpak` CLI. Command construction
    /// and the installed-list parse are verified headlessly; the execs (and the
    /// `remote-info`/`--show-permissions` output parsing, which is version-
    /// sensitive) are VM-verified.
    #[derive(Default)]
    pub struct RealFlatpakOps;
    #[async_trait]
    impl FlatpakOps for RealFlatpakOps {
        async fn resolve(&self, reference: &str) -> SysResult<FlatpakAppInfo> {
            let out = run_ok(
                "flatpak",
                &[
                    "remote-info".into(),
                    "--system".into(),
                    "flathub".into(),
                    reference.into(),
                ],
            )?;
            if !out.status.success() {
                return Err(SysError::NotFound);
            }
            // Name/permissions parsing is VM-refined; fall back to the reference.
            Ok(FlatpakAppInfo {
                reference: reference.to_string(),
                name: reference.to_string(),
                permissions: self.app_permissions(reference).await.unwrap_or_default(),
            })
        }
        async fn is_installed_system(&self, reference: &str) -> SysResult<bool> {
            let out = run_ok(
                "flatpak",
                &[
                    "list".into(),
                    "--system".into(),
                    "--app".into(),
                    "--columns=application".into(),
                ],
            )?;
            Ok(parse_installed_refs(&String::from_utf8_lossy(&out.stdout))
                .iter()
                .any(|r| r == reference))
        }
        async fn install_system(&self, reference: &str) -> SysResult<()> {
            let out = run_ok("flatpak", &flatpak_install_argv(reference))?;
            if out.status.success() {
                Ok(())
            } else {
                Err(SysError::Io(format!(
                    "flatpak install failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )))
            }
        }
        async fn app_permissions(&self, reference: &str) -> SysResult<Vec<String>> {
            let out = run_ok(
                "flatpak",
                &["info".into(), "--show-permissions".into(), reference.into()],
            )?;
            Ok(String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .filter(|l| !l.is_empty())
                .collect())
        }
    }

    // ---- fapolicyd (trust db + denial watch) ------------------------------

    /// `fapolicyd-cli --file add <path> --trust-file charter` argv.
    fn trust_add_argv(path: &str) -> Vec<String> {
        vec![
            "--file".into(),
            "add".into(),
            path.into(),
            "--trust-file".into(),
            "charter".into(),
        ]
    }

    /// True if `path` appears as a whitespace token in a `fapolicyd-cli
    /// --dump-db` document (i.e. it is a trusted entry).
    fn trust_db_contains(dump: &str, path: &str) -> bool {
        dump.lines()
            .any(|l| l.split_whitespace().any(|tok| tok == path))
    }

    /// Extract the `path=` value from a fapolicyd denial log line.
    fn parse_denial_path(line: &str) -> Option<String> {
        line.split_whitespace()
            .find_map(|tok| tok.strip_prefix("path="))
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    }

    /// fapolicyd trust-DB writer: trusts the approved binary at its store path by
    /// adding it to the Charter trust file, then reloads. argv + path are
    /// verified headlessly; the exec/reload is VM-verified.
    #[derive(Default)]
    pub struct RealTrustDb;
    #[async_trait]
    impl TrustDb for RealTrustDb {
        async fn trust_hash(&self, sha256_hex: &str) -> SysResult<()> {
            let path = approved_bin_path(sha256_hex);
            let add = run_ok("fapolicyd-cli", &trust_add_argv(&path))?;
            if !add.status.success() {
                return Err(SysError::Io(format!(
                    "fapolicyd trust add failed: {}",
                    String::from_utf8_lossy(&add.stderr).trim()
                )));
            }
            let upd = run_ok("fapolicyd-cli", &["--update".into()])?;
            if upd.status.success() {
                Ok(())
            } else {
                Err(SysError::Io("fapolicyd reload failed".into()))
            }
        }
    }

    /// Execution-gate query: a path is blocked unless it is a trusted entry in
    /// the fapolicyd trust db. The dump parse is verified headlessly; the
    /// `--dump-db` exec is VM-verified.
    #[derive(Default)]
    pub struct RealExecGate;
    #[async_trait]
    impl ExecGate for RealExecGate {
        async fn is_blocked(&self, path: &str) -> SysResult<bool> {
            let out = run_ok("fapolicyd-cli", &["--dump-db".into()])?;
            Ok(!trust_db_contains(
                &String::from_utf8_lossy(&out.stdout),
                path,
            ))
        }
    }

    /// fapolicyd denial watch: surfaces the next blocked-exec path from the
    /// denial log so charterd can prompt instead of failing silently. The line
    /// parse is verified headlessly; the live log cursor is VM-verified.
    const FAPOLICYD_DENY_LOG: &str = "/var/log/fapolicyd-access.log";
    pub struct RealDenialWatch {
        log: PathBuf,
    }
    impl Default for RealDenialWatch {
        fn default() -> Self {
            Self {
                log: FAPOLICYD_DENY_LOG.into(),
            }
        }
    }
    #[async_trait]
    impl DenialWatch for RealDenialWatch {
        async fn next_denial(&self) -> SysResult<Option<String>> {
            // Best-effort: scan the current log tail for the most recent denial.
            // A durable cursor (inotify / journal seek) is VM-wired.
            let content = match fs::read_to_string(&self.log) {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(io_err("read deny log", e)),
            };
            Ok(content
                .lines()
                .rev()
                .find(|l| l.contains("dec=deny"))
                .and_then(parse_denial_path))
        }
    }

    // ---- cgroup-v2 freezer (sysfs write) ----------------------------------

    /// cgroup-v2 app-slice freezer. Writes `1`/`0` to `<root>/<target>/
    /// cgroup.freeze` (production root `/sys/fs/cgroup`; `with_root` redirects
    /// for tests). The freeze target is always the managed app slice — the
    /// caller (`charterd`) guards that it is never charterd/lock.
    pub struct RealCgroupFreezer {
        root: PathBuf,
    }
    impl Default for RealCgroupFreezer {
        fn default() -> Self {
            Self {
                root: "/sys/fs/cgroup".into(),
            }
        }
    }
    impl RealCgroupFreezer {
        /// Test/override constructor: redirect the cgroup root.
        pub fn with_root(root: impl Into<PathBuf>) -> Self {
            Self { root: root.into() }
        }
        fn freeze_file(&self, target: &str) -> PathBuf {
            self.root.join(target).join("cgroup.freeze")
        }
        fn set(&self, target: &str, value: &str) -> SysResult<()> {
            // Direct write — cgroup.freeze is a kernel pseudo-file (no atomic
            // temp+rename; the cgroup dir is created by systemd, not us).
            fs::write(self.freeze_file(target), value).map_err(|e| io_err("cgroup.freeze", e))
        }
    }
    #[async_trait]
    impl CgroupFreezer for RealCgroupFreezer {
        async fn freeze(&self, target: &str) -> SysResult<()> {
            self.set(target, "1")
        }
        async fn thaw(&self, target: &str) -> SysResult<()> {
            self.set(target, "0")
        }
        async fn exists(&self, target: &str) -> bool {
            // systemd creates the slice + its `cgroup.freeze` when the user has a
            // live session; its presence is our "there is something to freeze".
            self.freeze_file(target).exists()
        }
    }
    /// Root-owned content-addressed exec store. A binary lives at
    /// `<base>/approved/<sha256>/run` and its launcher at
    /// `<base>/applications/<sha256>.desktop`. `admit` re-hashes the **copied**
    /// bytes (TOCTOU close) and stores under the sha256 filename — `meta.name`
    /// is validated and only ever reaches the desktop-entry `Name=`, never a
    /// path. The production root is `/var/lib/charter`, matching the fapolicyd
    /// allow-anchor and the `charter-setup` 0700 dir.
    ///
    /// `harden` (on for `Default`, off for `with_base` tests) drives the
    /// root-only belt-and-braces: store binaries read-only and `chattr +i`
    /// immutable so the managed user cannot tamper with or unlink them. These
    /// are best-effort (ignored when unprivileged) and VM-verified.
    pub struct RealApprovedExecStore {
        base: PathBuf,
        harden: bool,
    }
    impl Default for RealApprovedExecStore {
        fn default() -> Self {
            Self {
                base: CHARTER_BASE.into(),
                harden: true,
            }
        }
    }
    impl RealApprovedExecStore {
        /// Test/override constructor: redirect the store root and skip the
        /// root-only hardening (so headless tests run unprivileged + clean up).
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self {
                base: base.into(),
                harden: false,
            }
        }
        fn store_dir(&self) -> PathBuf {
            self.base.join("approved")
        }
        fn apps_dir(&self) -> PathBuf {
            self.base.join("applications")
        }
        fn bin_path(&self, sha256_hex: &str) -> PathBuf {
            self.store_dir().join(sha256_hex).join("run")
        }
        fn launcher_path(&self, sha256_hex: &str) -> PathBuf {
            self.apps_dir().join(format!("{sha256_hex}.desktop"))
        }
        /// Best-effort `chattr +i/-i` (root-only; ignored otherwise).
        fn set_immutable(&self, path: &Path, immutable: bool) {
            if !self.harden {
                return;
            }
            let flag = if immutable { "+i" } else { "-i" };
            let _ = Command::new("chattr").arg(flag).arg(path).status();
        }

        fn inspect_impl(&self, src_path: &str) -> SysResult<InspectResult> {
            let path = Path::new(src_path);
            // The ceiling is checked from the INODE, before the file is opened
            // — the old code derived `size` from a buffer it had already
            // allocated, which is exactly one allocation too late.
            candidate_size(path)?;
            let (sha256, size) = stream_file(path, None)?;
            Ok(InspectResult { sha256, size })
        }
        fn contains_impl(&self, sha256_hex: &str) -> SysResult<bool> {
            Ok(self.bin_path(sha256_hex).is_file())
        }
        fn admit_impl(
            &self,
            src_path: &str,
            expected_sha256: &str,
            meta: &AdmitMeta,
        ) -> SysResult<()> {
            // Validate the display name BEFORE anything reaches the filesystem.
            validate_display_name(&meta.name)
                .map_err(|_| SysError::Conflict("invalid display name".into()))?;
            // Copy the bytes NOW and re-hash exactly what we will store (TOCTOU
            // close: a later mutation of the source cannot affect stored bytes).
            // Streamed, not buffered — and, as in `inspect`, refused on size
            // before the file is opened. The old code held the whole candidate
            // TWICE over (the read buffer plus `atomic_write`'s copy).
            let src = Path::new(src_path);
            candidate_size(src)?;
            let store_dir = self.store_dir();
            fs::create_dir_all(&store_dir).map_err(|e| io_err("create_dir_all approved", e))?;
            // Stage the copy under the store dir (same filesystem as the final
            // path, so the rename is atomic) and hash it as it lands.
            let staging = staging_path(&store_dir);
            let actual = {
                use std::os::unix::fs::OpenOptionsExt as _;
                // 0600 from the moment it exists — the staged copy is a
                // not-yet-verified binary and must never be reachable, however
                // briefly, by anyone but us.
                let mut f = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&staging)
                    .map_err(|e| io_err("create admit staging", e))?;
                match stream_file(src, Some(&mut f)).and_then(|(sha, _)| {
                    f.sync_all().map_err(|e| io_err("fsync admit staging", e))?;
                    Ok(sha)
                }) {
                    Ok(sha) => sha,
                    Err(e) => {
                        drop(f);
                        let _ = fs::remove_file(&staging);
                        return Err(e);
                    }
                }
            };
            if actual != expected_sha256 {
                let _ = fs::remove_file(&staging); // nothing stored
                return Err(SysError::Conflict("hash mismatch".into()));
            }
            let dst = self.bin_path(&actual);
            if let Some(parent) = dst.parent() {
                if let Err(e) = fs::create_dir_all(parent) {
                    let _ = fs::remove_file(&staging);
                    return Err(io_err("create_dir_all approved", e));
                }
            }
            if let Err(e) = fs::rename(&staging, &dst) {
                let _ = fs::remove_file(&staging);
                return Err(io_err("rename admitted binary", e));
            }
            // Root-only hardening: owner read+exec only, then immutable.
            if self.harden {
                use std::os::unix::fs::PermissionsExt;
                let _ = fs::set_permissions(&dst, fs::Permissions::from_mode(0o500));
            }
            self.set_immutable(&dst, true);
            Ok(())
        }
        fn launcher_impl(&self, sha256_hex: &str, name: &str) -> SysResult<()> {
            let escaped = desktop_escape(name);
            let exec = self.bin_path(sha256_hex);
            let desktop = format!(
                "[Desktop Entry]\nType=Application\nName={escaped}\nExec={}\n",
                exec.display()
            );
            atomic_write(&self.launcher_path(sha256_hex), desktop.as_bytes())
        }
        fn list_impl(&self) -> SysResult<Vec<String>> {
            let rd = match fs::read_dir(self.store_dir()) {
                Ok(rd) => rd,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
                Err(e) => return Err(io_err("read_dir approved", e)),
            };
            let mut out = vec![];
            for entry in rd {
                let entry = entry.map_err(|e| io_err("dir entry", e))?;
                if entry.path().join("run").is_file() {
                    if let Some(name) = entry.file_name().to_str() {
                        out.push(name.to_string());
                    }
                }
            }
            out.sort();
            Ok(out)
        }
        fn remove_impl(&self, sha256_hex: &str) -> SysResult<()> {
            self.set_immutable(&self.bin_path(sha256_hex), false);
            match fs::remove_dir_all(self.store_dir().join(sha256_hex)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err("remove approved", e)),
            }
            match fs::remove_file(self.launcher_path(sha256_hex)) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err("remove launcher", e)),
            }
        }
    }
    #[async_trait]
    impl ApprovedExecStore for RealApprovedExecStore {
        async fn inspect(&self, src_path: &str) -> SysResult<InspectResult> {
            self.inspect_impl(src_path)
        }
        async fn contains(&self, sha256_hex: &str) -> SysResult<bool> {
            self.contains_impl(sha256_hex)
        }
        async fn admit(
            &self,
            src_path: &str,
            expected_sha256: &str,
            meta: &AdmitMeta,
        ) -> SysResult<()> {
            self.admit_impl(src_path, expected_sha256, meta)
        }
        async fn launcher(&self, sha256_hex: &str, name: &str) -> SysResult<()> {
            self.launcher_impl(sha256_hex, name)
        }
        async fn list(&self) -> SysResult<Vec<String>> {
            self.list_impl()
        }
        async fn remove(&self, sha256_hex: &str) -> SysResult<()> {
            self.remove_impl(sha256_hex)
        }
    }
    // ---- the on-display lock-down (VT lock + lock screen) -----------------

    /// `VT_UNLOCKSWITCH` when switching is (re-)enabled, `VT_LOCKSWITCH` when it
    /// is disabled — the request the [`RealVtControl`] ioctl issues.
    /// Typed `libc::Ioctl` (c_ulong on glibc, c_int on bionic) so the crate
    /// cross-compiles for Android, where this VT code is never called.
    fn vt_request(enabled: bool) -> libc::Ioctl {
        const VT_LOCKSWITCH: libc::Ioctl = 0x560B;
        const VT_UNLOCKSWITCH: libc::Ioctl = 0x560C;
        if enabled {
            VT_UNLOCKSWITCH
        } else {
            VT_LOCKSWITCH
        }
    }

    /// Virtual-terminal switch lock. Disabling switching (`enabled=false`) issues
    /// `VT_LOCKSWITCH` so the child cannot `Ctrl+Alt+Fn` out from under the lock;
    /// re-enabling unlocks it. The mapping is unit-tested; the ioctl is VM-run.
    #[derive(Default)]
    pub struct RealVtControl;
    impl RealVtControl {
        fn console() -> SysResult<fs::File> {
            fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("/dev/console")
                .or_else(|_| {
                    fs::OpenOptions::new()
                        .read(true)
                        .write(true)
                        .open("/dev/tty0")
                })
                .map_err(|e| io_err("open console", e))
        }
        fn ioctl(&self, enabled: bool) -> SysResult<()> {
            use std::os::unix::io::AsRawFd as _;
            let f = Self::console()?;
            // SAFETY: a parameterless VT ioctl on a console fd we own.
            let rc = unsafe { libc::ioctl(f.as_raw_fd(), vt_request(enabled)) };
            if rc == 0 {
                Ok(())
            } else {
                Err(SysError::Io(format!(
                    "VT ioctl failed: {}",
                    std::io::Error::last_os_error()
                )))
            }
        }
        fn ioctl_activate(&self, vt: u32) -> SysResult<()> {
            use std::os::unix::io::AsRawFd as _;
            const VT_ACTIVATE: libc::Ioctl = 0x5606;
            let f = Self::console()?;
            // SAFETY: VT_ACTIVATE with the target VT number on a console fd we
            // own. Deliberately not VT_WAITACTIVE — never block the tick loop.
            let rc =
                unsafe { libc::ioctl(f.as_raw_fd(), VT_ACTIVATE, vt as std::os::raw::c_ulong) };
            if rc == 0 {
                Ok(())
            } else {
                Err(SysError::Io(format!(
                    "VT_ACTIVATE {vt} failed: {}",
                    std::io::Error::last_os_error()
                )))
            }
        }
    }
    #[async_trait]
    impl VtControl for RealVtControl {
        async fn set_vt_switching(&self, enabled: bool) -> SysResult<()> {
            self.ioctl(enabled)
        }
        async fn activate_vt(&self, vt: u32) -> SysResult<()> {
            self.ioctl_activate(vt)
        }
    }

    /// Shows/hides the undismissable lock by spawning + reaping the
    /// `charter-lock` X11 binary (the daemon runs as root; the lock targets the
    /// managed session's display). `show_lock` is idempotent — a still-running
    /// lock is left in place; `hide_lock` kills it (dropping the X connection
    /// releases the input grab). The binary path + display are env-overridable
    /// for the host. Compile-verified; the live spawn is VM-run.
    pub struct RealSessionControl {
        bin: String,
        display: String,
        child: Mutex<Option<std::process::Child>>,
    }
    impl Default for RealSessionControl {
        fn default() -> Self {
            Self {
                bin: std::env::var("CHARTER_LOCK_BIN")
                    .unwrap_or_else(|_| "/usr/bin/charter-lock".into()),
                display: std::env::var("CHARTER_DISPLAY").unwrap_or_else(|_| ":0".into()),
                child: Mutex::new(None),
            }
        }
    }
    impl RealSessionControl {
        /// True if a previously-spawned lock is still running.
        fn lock_alive(child: &mut Option<std::process::Child>) -> bool {
            match child {
                Some(c) => match c.try_wait() {
                    Ok(Some(_)) => false, // exited
                    Ok(None) => true,     // still up
                    Err(_) => false,
                },
                None => false,
            }
        }
    }
    #[async_trait]
    impl SessionControl for RealSessionControl {
        async fn show_lock(&self, title: &str, detail: &str) -> SysResult<()> {
            let mut guard = self.child.lock().expect("lock");
            if Self::lock_alive(&mut guard) {
                return Ok(()); // already locked
            }
            let mut cmd = Command::new(&self.bin);
            cmd.env("DISPLAY", &self.display)
                .env("CHARTER_LOCK_TITLE", title)
                .env("CHARTER_LOCK_DETAIL", detail);
            if let Ok(xauth) = std::env::var("CHARTER_XAUTHORITY") {
                cmd.env("XAUTHORITY", xauth);
            }
            let child = cmd.spawn().map_err(|e| io_err("spawn charter-lock", e))?;
            *guard = Some(child);
            Ok(())
        }
        async fn hide_lock(&self) -> SysResult<()> {
            let mut guard = self.child.lock().expect("lock");
            if let Some(mut c) = guard.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
            Ok(())
        }
    }

    /// Firefox machine-policy writer. Writes the rendered `policies.json` to
    /// `<base>/policies.json` (production `/etc/firefox/policies/policies.json`)
    /// atomically, then (production only) re-asserts root-owned immutability so a
    /// child cannot delete or edit it. Machine policy is profile-independent and
    /// highest-precedence, so this cannot be escaped by resetting the profile.
    pub struct RealWebPolicyOps {
        base: PathBuf,
        harden: bool,
    }
    impl Default for RealWebPolicyOps {
        fn default() -> Self {
            Self {
                base: FIREFOX_POLICY_DIR.into(),
                harden: true,
            }
        }
    }
    impl RealWebPolicyOps {
        /// Test/override constructor: redirect the policy dir, skip immutability.
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self {
                base: base.into(),
                harden: false,
            }
        }
        fn path(&self) -> PathBuf {
            self.base.join("policies.json")
        }
        fn set_immutable(&self, immutable: bool) {
            if !self.harden {
                return;
            }
            let flag = if immutable { "+i" } else { "-i" };
            let _ = Command::new("chattr").arg(flag).arg(self.path()).status();
        }
        fn write_impl(&self, policies_json: &str) -> SysResult<()> {
            self.set_immutable(false); // unlock before overwrite (re-assert on drift)
            atomic_write(&self.path(), policies_json.as_bytes())?;
            self.set_immutable(true);
            Ok(())
        }
        fn clear_impl(&self) -> SysResult<()> {
            self.set_immutable(false);
            match fs::remove_file(self.path()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err("remove policies", e)),
            }
        }
    }
    #[async_trait]
    impl WebPolicyOps for RealWebPolicyOps {
        async fn write_policies(&self, policies_json: &str) -> SysResult<()> {
            self.write_impl(policies_json)
        }
        async fn clear_policies(&self) -> SysResult<()> {
            self.clear_impl()
        }
    }
    /// DNS-layer plan writer. Durably persists the rendered plan to
    /// `<base>/plan.json` (production `/var/lib/charter/dns/plan.json`) so the
    /// privileged resolver applier (AdGuard Home reconfigure, `resolv.conf` pin,
    /// DNS/QUIC firewall) consumes a crash-consistent, idempotent state file.
    /// The live resolver/firewall application is the root-only, VM-verified half;
    /// this file-backed half is what runs and is verified headlessly.
    pub struct RealDnsFilterOps {
        base: PathBuf,
    }
    impl Default for RealDnsFilterOps {
        fn default() -> Self {
            Self {
                base: Path::new(CHARTER_BASE).join("dns"),
            }
        }
    }
    impl RealDnsFilterOps {
        /// Test/override constructor: redirect the DNS state dir.
        pub fn with_base(base: impl Into<PathBuf>) -> Self {
            Self { base: base.into() }
        }
        fn plan_path(&self) -> PathBuf {
            self.base.join("plan.json")
        }
        fn apply_impl(&self, plan_json: &str) -> SysResult<()> {
            atomic_write(&self.plan_path(), plan_json.as_bytes())
        }
        fn clear_impl(&self) -> SysResult<()> {
            match fs::remove_file(self.plan_path()) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(io_err("remove dns plan", e)),
            }
        }
    }
    #[async_trait]
    impl DnsFilterOps for RealDnsFilterOps {
        async fn apply_plan(&self, plan_json: &str) -> SysResult<()> {
            self.apply_impl(plan_json)
        }
        async fn clear(&self) -> SysResult<()> {
            self.clear_impl()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::fs;
        use std::future::Future;
        use std::path::PathBuf;

        /// Drive a pure-sync `async_trait` future to completion with no runtime.
        /// The file-backed ports do only synchronous `std::fs` work, so the
        /// future is `Ready` on the first poll — a no-op waker suffices.
        fn block_on<F: Future>(fut: F) -> F::Output {
            use std::pin::pin;
            use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};
            fn raw() -> RawWaker {
                fn nop(_: *const ()) {}
                fn clone(_: *const ()) -> RawWaker {
                    raw()
                }
                RawWaker::new(std::ptr::null(), &RawWakerVTable::new(clone, nop, nop, nop))
            }
            let waker = unsafe { Waker::from_raw(raw()) };
            let mut cx = Context::from_waker(&waker);
            let mut fut = pin!(fut);
            loop {
                if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                    return v;
                }
            }
        }

        fn tmp(name: &str) -> PathBuf {
            let p =
                std::env::temp_dir().join(format!("charter-effects-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&p);
            p
        }

        fn sha_hex(bytes: &[u8]) -> String {
            hex_bytes(&charter_crypto::sha256(bytes))
        }

        #[test]
        fn web_policy_writes_then_clears() {
            let base = tmp("web");
            let ops = RealWebPolicyOps::with_base(&base);
            let doc = r#"{"policies":{"DisableTelemetry":true}}"#;
            block_on(ops.write_policies(doc)).unwrap();
            let path = base.join("policies.json");
            assert_eq!(fs::read_to_string(&path).unwrap(), doc);
            // Idempotent re-write.
            block_on(ops.write_policies(doc)).unwrap();
            assert_eq!(fs::read_to_string(&path).unwrap(), doc);
            block_on(ops.clear_policies()).unwrap();
            assert!(!path.exists());
            // Clearing an absent file is a no-op.
            block_on(ops.clear_policies()).unwrap();
        }

        #[test]
        fn dns_filter_persists_plan_then_clears() {
            let base = tmp("dns");
            let ops = RealDnsFilterOps::with_base(&base);
            let plan = r#"{"mode":"allowlist","allowDomains":["kids.example"]}"#;
            block_on(ops.apply_plan(plan)).unwrap();
            let path = base.join("plan.json");
            assert_eq!(fs::read_to_string(&path).unwrap(), plan);
            block_on(ops.clear()).unwrap();
            assert!(!path.exists());
            block_on(ops.clear()).unwrap(); // no-op
        }

        #[test]
        fn approved_admit_relocates_and_rehashes() {
            let base = tmp("exec-admit");
            let ops = RealApprovedExecStore::with_base(&base);
            let src = base.join("src.bin");
            let bytes = b"#!/bin/sh\necho hi\n";
            fs::create_dir_all(&base).unwrap();
            fs::write(&src, bytes).unwrap();

            let inspect = block_on(ops.inspect(src.to_str().unwrap())).unwrap();
            assert_eq!(inspect.size, bytes.len() as u64);
            assert_eq!(inspect.sha256, sha_hex(bytes));

            assert!(!block_on(ops.contains(&inspect.sha256)).unwrap());
            let meta = AdmitMeta {
                name: "Hi".into(),
                size: inspect.size,
                origin: None,
            };
            block_on(ops.admit(src.to_str().unwrap(), &inspect.sha256, &meta)).unwrap();
            assert!(block_on(ops.contains(&inspect.sha256)).unwrap());
            // The stored bytes are the copied bytes, under the sha256 filename.
            let stored = base.join("approved").join(&inspect.sha256).join("run");
            assert_eq!(fs::read(&stored).unwrap(), bytes);
            assert_eq!(block_on(ops.list()).unwrap(), vec![inspect.sha256.clone()]);
        }

        // ---- B6: the candidate is streamed, and bounded ------------------

        #[test]
        fn a_candidate_over_the_ceiling_is_refused_without_being_read() {
            // The attack: `fallocate -l 64G ~/big.bin && charter run ~/big.bin`.
            // Every confused-deputy guard passes (regular file, own home,
            // readable) and root charterd used to allocate the lot on the
            // REQUEST path, before any guardian approval. A sparse `set_len`
            // file is the same inode to `stat` and costs no disk here.
            let base = tmp("exec-huge");
            let ops = RealApprovedExecStore::with_base(&base);
            fs::create_dir_all(&base).unwrap();
            let src = base.join("huge.bin");
            let f = fs::File::create(&src).unwrap();
            f.set_len(MAX_CANDIDATE_BYTES + 1).unwrap();
            drop(f);
            assert_eq!(
                fs::metadata(&src).unwrap().len(),
                MAX_CANDIDATE_BYTES + 1,
                "the sparse file really is over the ceiling"
            );

            let err = block_on(ops.inspect(src.to_str().unwrap())).unwrap_err();
            assert!(
                matches!(&err, SysError::Unsupported(m) if m.contains("over the")),
                "inspect: {err:?}"
            );
            let meta = AdmitMeta {
                name: "Huge".into(),
                size: MAX_CANDIDATE_BYTES + 1,
                origin: None,
            };
            let err =
                block_on(ops.admit(src.to_str().unwrap(), &"aa".repeat(32), &meta)).unwrap_err();
            assert!(
                matches!(&err, SysError::Unsupported(m) if m.contains("over the")),
                "admit: {err:?}"
            );
            // And nothing was staged or stored on the way out.
            assert!(block_on(ops.list()).unwrap().is_empty());
            let staged: Vec<_> = fs::read_dir(base.join("approved"))
                .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
                .unwrap_or_default();
            assert!(staged.is_empty(), "staging left behind: {staged:?}");

            // Exactly at the ceiling is allowed through the `stat` guard (the
            // boundary, checked directly — actually streaming 512 MiB of holes
            // would only be measuring the page cache).
            let ok = base.join("ok.bin");
            let f = fs::File::create(&ok).unwrap();
            f.set_len(MAX_CANDIDATE_BYTES).unwrap();
            drop(f);
            assert_eq!(candidate_size(&ok).unwrap(), MAX_CANDIDATE_BYTES);
            assert!(candidate_size(&src).is_err(), "one byte over is refused");
        }

        #[test]
        fn the_streamed_hash_equals_the_old_whole_read_hash() {
            // The fix must not change the identity of anything already stored:
            // a chunked sha256 is byte-for-byte the single-shot one, including
            // across a chunk boundary and for an empty file.
            let base = tmp("exec-stream-hash");
            let ops = RealApprovedExecStore::with_base(&base);
            fs::create_dir_all(&base).unwrap();
            for (name, bytes) in [
                ("empty", Vec::new()),
                ("tiny", b"#!/bin/sh\necho hi\n".to_vec()),
                // Straddles the 64 KiB buffer: more than one `update` call.
                (
                    "multi",
                    (0..(CANDIDATE_CHUNK_BYTES * 2 + 7))
                        .map(|i| i as u8)
                        .collect(),
                ),
            ] {
                let src = base.join(name);
                fs::write(&src, &bytes).unwrap();
                let got = block_on(ops.inspect(src.to_str().unwrap())).unwrap();
                assert_eq!(got.sha256, sha_hex(&bytes), "{name}: hash");
                assert_eq!(got.size, bytes.len() as u64, "{name}: size");
            }
        }

        #[test]
        fn a_streamed_admit_stores_the_exact_bytes_across_chunks() {
            let base = tmp("exec-stream-admit");
            let ops = RealApprovedExecStore::with_base(&base);
            fs::create_dir_all(&base).unwrap();
            let bytes: Vec<u8> = (0..(CANDIDATE_CHUNK_BYTES * 3 + 11))
                .map(|i| (i % 251) as u8)
                .collect();
            let src = base.join("big.bin");
            fs::write(&src, &bytes).unwrap();
            let inspect = block_on(ops.inspect(src.to_str().unwrap())).unwrap();
            let meta = AdmitMeta {
                name: "Big".into(),
                size: inspect.size,
                origin: None,
            };
            block_on(ops.admit(src.to_str().unwrap(), &inspect.sha256, &meta)).unwrap();
            let stored = base.join("approved").join(&inspect.sha256).join("run");
            assert_eq!(fs::read(&stored).unwrap(), bytes);
            // No staging file survives a success either.
            let leftovers: Vec<_> = fs::read_dir(base.join("approved"))
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with(".admit-"))
                .collect();
            assert!(leftovers.is_empty(), "staging left behind: {leftovers:?}");
        }

        #[test]
        fn approved_admit_rejects_hash_mismatch_and_stores_nothing() {
            let base = tmp("exec-toctou");
            let ops = RealApprovedExecStore::with_base(&base);
            let src = base.join("src.bin");
            fs::create_dir_all(&base).unwrap();
            fs::write(&src, b"original").unwrap();
            let expected = sha_hex(b"original");
            // Source is swapped after the hash was taken (TOCTOU attempt).
            fs::write(&src, b"swapped-payload").unwrap();
            let meta = AdmitMeta {
                name: "X".into(),
                size: 8,
                origin: None,
            };
            let err = block_on(ops.admit(src.to_str().unwrap(), &expected, &meta)).unwrap_err();
            assert!(matches!(err, SysError::Conflict(_)));
            // Nothing was stored on mismatch.
            assert!(!block_on(ops.contains(&expected)).unwrap());
            assert!(block_on(ops.list()).unwrap().is_empty());
        }

        #[test]
        fn approved_admit_rejects_bad_display_name() {
            let base = tmp("exec-name");
            let ops = RealApprovedExecStore::with_base(&base);
            let src = base.join("src.bin");
            fs::create_dir_all(&base).unwrap();
            fs::write(&src, b"payload").unwrap();
            let sha = sha_hex(b"payload");
            let meta = AdmitMeta {
                name: "../escape".into(), // path-traversal name
                size: 7,
                origin: None,
            };
            let err = block_on(ops.admit(src.to_str().unwrap(), &sha, &meta)).unwrap_err();
            assert!(matches!(err, SysError::Conflict(_)));
            assert!(!block_on(ops.contains(&sha)).unwrap());
        }

        #[test]
        fn approved_admit_missing_source_is_not_found() {
            let base = tmp("exec-missing");
            let ops = RealApprovedExecStore::with_base(&base);
            let meta = AdmitMeta {
                name: "X".into(),
                size: 0,
                origin: None,
            };
            let err = block_on(ops.admit(base.join("nope").to_str().unwrap(), "deadbeef", &meta))
                .unwrap_err();
            assert!(matches!(err, SysError::NotFound));
        }

        #[test]
        fn approved_launcher_then_remove() {
            let base = tmp("exec-launch");
            let ops = RealApprovedExecStore::with_base(&base);
            let src = base.join("src.bin");
            fs::create_dir_all(&base).unwrap();
            fs::write(&src, b"game").unwrap();
            let sha = sha_hex(b"game");
            let meta = AdmitMeta {
                name: "SuperTuxKart".into(),
                size: 4,
                origin: None,
            };
            block_on(ops.admit(src.to_str().unwrap(), &sha, &meta)).unwrap();
            block_on(ops.launcher(&sha, "SuperTuxKart")).unwrap();

            let desktop = base.join("applications").join(format!("{sha}.desktop"));
            let body = fs::read_to_string(&desktop).unwrap();
            assert!(body.contains("Name=SuperTuxKart"));
            // Exec= points at the stored sha256 path, never the display name.
            let stored = base.join("approved").join(&sha).join("run");
            assert!(body.contains(&format!("Exec={}", stored.display())));

            block_on(ops.remove(&sha)).unwrap();
            assert!(!block_on(ops.contains(&sha)).unwrap());
            assert!(!desktop.exists());
            block_on(ops.remove(&sha)).unwrap(); // no-op
        }

        // ---- OS-query ports: pure cores ----------------------------------

        const PROC_MOUNTS: &str = "\
tmpfs /tmp tmpfs rw,nosuid,nodev,noexec,relatime 0 0
/dev/sda1 /home ext4 rw,relatime 0 0
tmpfs /dev/shm tmpfs rw,nosuid,nodev 0 0
overlay /home ext4 rw,nosuid,nodev,noexec 0 0";

        #[test]
        fn mount_options_parses_and_takes_last_for_overmount() {
            // /tmp is hardened.
            let opts = parse_mount_options(PROC_MOUNTS, "/tmp").unwrap();
            assert!(opts.contains(&"noexec".to_string()));
            assert!(opts.contains(&"nosuid".to_string()));
            assert!(opts.contains(&"nodev".to_string()));
            // /home appears twice; the LAST (effective) mount wins.
            let home = parse_mount_options(PROC_MOUNTS, "/home").unwrap();
            assert!(home.contains(&"noexec".to_string()));
            // An unmounted target is absent.
            assert!(parse_mount_options(PROC_MOUNTS, "/nope").is_none());
        }

        const ETC_GROUP: &str = "\
root:x:0:
sudo:x:27:alice
adm:x:4:syslog,alice
lpadmin:x:120:
charter-managed:x:990:kid";

        #[test]
        fn admin_group_membership_from_group_file() {
            assert!(user_in_admin_group(ETC_GROUP, "alice", ADMIN_GROUPS));
            // The managed kid is in no admin group (charter-setup removed them).
            assert!(!user_in_admin_group(ETC_GROUP, "kid", ADMIN_GROUPS));
            assert!(!user_in_admin_group(ETC_GROUP, "nobody", ADMIN_GROUPS));
        }

        #[test]
        fn systemctl_is_active_interpretation() {
            assert!(interpret_is_active("active\n"));
            assert!(!interpret_is_active("inactive\n"));
            assert!(!interpret_is_active("failed"));
            assert!(!interpret_is_active("activating"));
        }

        #[test]
        fn polkit_rules_present_checks_the_file() {
            let dir = tmp("polkit");
            fs::create_dir_all(&dir).unwrap();
            let rules = dir.join("49-charter.rules");
            let ops = RealPolkitOps::with_path(&rules);
            assert!(!block_on(ops.rules_present()).unwrap());
            fs::write(&rules, "// charter polkit rules").unwrap();
            assert!(block_on(ops.rules_present()).unwrap());
        }

        // ---- shell-out / sysfs enactor ports -----------------------------

        fn argv(v: &[&str]) -> Vec<String> {
            v.iter().map(|s| s.to_string()).collect()
        }

        #[test]
        fn cgroup_freeze_writes_one_then_zero() {
            let root = tmp("cgroup");
            let slice = root.join("user.slice/app.slice");
            fs::create_dir_all(&slice).unwrap();
            fs::write(slice.join("cgroup.freeze"), "0").unwrap();
            let fz = RealCgroupFreezer::with_root(&root);
            block_on(fz.freeze("user.slice/app.slice")).unwrap();
            assert_eq!(
                fs::read_to_string(slice.join("cgroup.freeze")).unwrap(),
                "1"
            );
            block_on(fz.thaw("user.slice/app.slice")).unwrap();
            assert_eq!(
                fs::read_to_string(slice.join("cgroup.freeze")).unwrap(),
                "0"
            );
        }

        #[test]
        fn cgroup_exists_tracks_the_slice_appearing() {
            // Models a managed child who is not logged in yet: the slice (hence
            // cgroup.freeze) does not exist, then appears when they log in. The
            // enforcement loop reconciles off this so a freeze is never lost.
            let root = tmp("cgroup-exists");
            let fz = RealCgroupFreezer::with_root(&root);
            let target = "user.slice/user-1002.slice";
            assert!(
                !block_on(fz.exists(target)),
                "absent slice must not report as existing"
            );
            let slice = root.join(target);
            fs::create_dir_all(&slice).unwrap();
            fs::write(slice.join("cgroup.freeze"), "0").unwrap();
            assert!(
                block_on(fz.exists(target)),
                "slice present once the session exists"
            );
        }

        #[test]
        fn fapolicyd_trust_argv_and_path_are_args_array() {
            assert_eq!(
                approved_bin_path("abcd"),
                "/var/lib/charter/approved/abcd/run"
            );
            assert_eq!(
                trust_add_argv("/var/lib/charter/approved/abcd/run"),
                argv(&[
                    "--file",
                    "add",
                    "/var/lib/charter/approved/abcd/run",
                    "--trust-file",
                    "charter"
                ])
            );
        }

        #[test]
        fn fapolicyd_trust_db_membership() {
            let dump =
                "1 /usr/bin/bash 1234 abcd...\n1 /var/lib/charter/approved/abcd/run 10 ef..\n";
            assert!(trust_db_contains(
                dump,
                "/var/lib/charter/approved/abcd/run"
            ));
            assert!(!trust_db_contains(dump, "/var/lib/charter/approved/zz/run"));
        }

        #[test]
        fn fapolicyd_denial_path_extraction() {
            let line = "rule=9 dec=deny_audit perm=execute path=/home/kid/evil.AppImage ftype=x";
            assert_eq!(
                parse_denial_path(line).as_deref(),
                Some("/home/kid/evil.AppImage")
            );
            assert_eq!(parse_denial_path("nothing here"), None);
        }

        #[test]
        fn flatpak_install_argv_is_noninteractive_system_scope() {
            assert_eq!(
                flatpak_install_argv("org.kde.kdenlive"),
                argv(&[
                    "install",
                    "--system",
                    "--noninteractive",
                    "--assumeyes",
                    "flathub",
                    "org.kde.kdenlive"
                ])
            );
        }

        #[test]
        fn flatpak_installed_list_parse() {
            let out = "org.kde.kdenlive\norg.mozilla.firefox\n\n";
            let refs = parse_installed_refs(out);
            assert!(refs.contains(&"org.kde.kdenlive".to_string()));
            assert!(!refs.contains(&"com.evil.app".to_string()));
            assert_eq!(refs.len(), 2); // blank line skipped
        }

        #[test]
        fn vt_request_locks_when_switching_disabled() {
            // disabling switching -> VT_LOCKSWITCH; enabling -> VT_UNLOCKSWITCH.
            assert_eq!(vt_request(false), 0x560B);
            assert_eq!(vt_request(true), 0x560C);
        }
    }
}

#[cfg(feature = "real-os")]
pub use real::{
    RealAccountOps, RealApprovedExecStore, RealCgroupFreezer, RealDenialWatch, RealDnsFilterOps,
    RealExecGate, RealFlatpakOps, RealMountOps, RealPolkitOps, RealSessionControl,
    RealSystemctlOps, RealTrustDb, RealVtControl, RealWebPolicyOps,
};
