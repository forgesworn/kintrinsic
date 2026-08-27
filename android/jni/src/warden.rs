//! The Android warden core: the shared decide/verify brain wired to file-backed
//! stores, ready to be driven over JNI. Enforcement (suspend/lock/usage) is
//! Kotlin's job (port-spec §3); this side owns verify + schedule/budget +
//! the fail-closed decision, single-sourced with the Linux warden.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use charter_primitives::kinds::CHARTER_DEVICE_CLAUSE;
use charter_primitives::{NostrEvent, PubKey};
use charter_proto::ClausePayload;
use charter_proto::StatusPayload;
use charter_schedule::clause::WeekStart;
use charter_schedule::enforcer::{
    enforcement_tz_of, AuditKind, EnforcerCore, EnforcerEffect, EnforcerInputs, LockReason,
    WarnLevel,
};
use charter_schedule::extension::ExtensionLedger;
use charter_schedule::usage::{Activity, UsageLedger};
use charter_schedule::{GrantBudget, GrantSchedule};
use charter_spine::child_policy::PolicySource;
use charter_spine::enactors::{AppOpenEnactor, TimeExtendEnactor};
use charter_spine::enforcer_runtime::EnforcerRuntime as SpineEnforcerRuntime;
use charter_spine::lifecycle::RequestRecord;
use charter_spine::multi_child::ExtensionInbox;
use charter_spine::ports::NullEventSink;
use charter_spine::{Broker, EnactorRegistry};
use charter_sys::persistence::{
    ChildClauseStore, ClauseStore, CuratorListStore, ExtensionStore, PairingStore,
    RealChildClauseStore, RealClauseStore, RealConsumedIdStore, RealCuratorListStore,
    RealExtensionStore, RealPairingStore, RealPendingStore, RealUsageStore, UsageStore,
};
use charter_sys::signer::{MachineSigner, RealMachineSigner};
use charter_transport::pairing::{pin_from_connect, Pairing, PairingError};

use crate::dto;
use crate::relay::{AndroidTransportFacade, OsEntropy, WardenRelay};
use crate::system::AndroidSystem;

/// The Android broker: the SAME spine as charterd, over Android's system
/// composition and the shared relay transport.
pub type AndroidBroker = Broker<AndroidSystem, AndroidTransportFacade, OsEntropy>;

/// Max seconds a single tick may credit — a clock step, doze gap, or a
/// suspend/resume must never credit as a burst (I14; runtime.rs:810-829).
const MAX_TICK_ELAPSED_SECS: i64 = 300;

/// Poll cursor lookback — matches the broker's 2-day rule (broker.rs:28-39,
/// == the NIP-59 wrap-jitter bound): relay `since` filters must never
/// permanently exclude a backdated wrap; reprocessing is idempotent (the
/// per-(subject,kind) issuedAt floor).
const POLL_LOOKBACK_SECS: u64 = 2 * 24 * 60 * 60;

/// STATUS heartbeat: emit on displayable state change OR every 60s
/// (status_emit.rs:60-76). Its absence is the guardian's wipe-detection tell.
const STATUS_HEARTBEAT_SECS: u64 = 60;

/// Our own applicationId — the only package a self-update clause may target.
const OWN_PACKAGE: &str = "org.forgesworn.charter";

/// Staged bring-up (port-spec §3.8; enforce_mode.rs). Unrecognized → Enforce.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum EnforceMode {
    /// Compute + audit; apply nothing (no suspend, no lock).
    Observe,
    /// Suspend/thaw only; no lock surface.
    FreezeOnly,
    /// Full enforcement. Production default.
    Enforce,
}

impl EnforceMode {
    pub fn parse(s: &str) -> Self {
        match s {
            "observe" | "Observe" => EnforceMode::Observe,
            "freeze-only" | "FreezeOnly" | "freezeOnly" => EnforceMode::FreezeOnly,
            _ => EnforceMode::Enforce, // fail-closed default
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            EnforceMode::Observe => "observe",
            EnforceMode::FreezeOnly => "freezeOnly",
            EnforceMode::Enforce => "enforce",
        }
    }
}

/// The whole warden. One per process; behind a global Mutex — the single-writer
/// discipline the file stores assume (persistence.rs:408-414).
pub struct Warden {
    // `base` + `consumed` + `pending` back the request/grant broker (time-extend
    // asks, install.apk) landing in the next increment; kept now so the store
    // wiring and single-writer discipline are established once.
    #[allow(dead_code)]
    base: PathBuf,
    signer: RealMachineSigner,
    #[allow(dead_code)]
    consumed: RealConsumedIdStore,
    #[allow(dead_code)]
    pending: RealPendingStore,
    child_clauses: RealChildClauseStore,
    /// Cached, signature-verified curator web-content lists (the broker
    /// refreshes these); [`Warden::web_dns_plan`]'s evaluator input.
    curator_lists: RealCuratorListStore,
    usage_store: RealUsageStore,
    extension_store: RealExtensionStore,
    /// The exact snapshot bytes last written to each store, so a tick that
    /// changed nothing writes nothing — see [`Warden::persist_ledgers`].
    /// `None` means "unknown, write on the next tick".
    usage_written: Option<String>,
    extension_written: Option<String>,
    pairing: RealPairingStore,

    guardian: Option<PubKey>,
    subject: Option<PubKey>,
    relays: Vec<String>,
    /// When the CURRENT pairing was established (unix secs; 0 = unknown/legacy).
    /// A RELEASE dated before this predates the current pairing epoch — even
    /// if signed by the same guardian, it is a stale replay of an EARLIER
    /// decision the guardian has since superseded by re-pairing, not a fresh
    /// instruction (issue #49 sibling — found live 2026-07-22: a re-paired
    /// device kept getting un-paired by its own prior release, redelivered
    /// within the 48h lookback/freshness window).
    paired_at: u64,
    /// The raw machine scalar — the NIP-44 ECDH + BIP-340 key the transport
    /// seals/signs with (signer.rs:133-137; same file as the signer's key).
    ///
    /// `Zeroizing` so the bytes are wiped when the warden drops, rather than
    /// left in a freed heap page for the process lifetime (S12, review
    /// 2026-08-07). Exploiting a leftover needs a memory-read primitive — root,
    /// or a second bug — so this is depth, not a fix for anything reachable;
    /// it costs one wrapper type and removes a copy that had no reason to
    /// outlive its owner. Derefs to `[u8; 32]`, so every caller is unchanged.
    machine_sk: zeroize::Zeroizing<[u8; 32]>,
    /// The live relay facade; `None` until paired with ≥1 relay (or when the
    /// build carries no relay feature — host mock tests inject one). Behind an
    /// `Arc` so the poll orchestrator clones it out and runs the network half
    /// WITHOUT holding the global warden lock.
    relay: Option<std::sync::Arc<dyn WardenRelay>>,
    /// Last emitted STATUS, for change-or-heartbeat throttling (I19).
    last_status: Option<StatusPayload>,
    /// The device's installed launchable apps (pkg + label), set by Kotlin each
    /// slow tick; ridden on STATUS so the guardian can pick apps to control by
    /// name (D3). Empty until Kotlin reports them.
    installed_apps: Vec<charter_proto::AppRef>,
    /// The account of the last guardian-opened install window, computed by
    /// Kotlin (only the platform knows package install times) from the span
    /// this side tracks. Ridden on STATUS. `None` until a window has happened.
    install_window: Option<charter_proto::InstallWindowReport>,
    /// This build's own versionCode (from Kotlin BuildConfig) — reported in
    /// STATUS and compared against the `update` clause (#44). 0 = unknown.
    app_version_code: u64,
    /// QR-onboarding echo: `(token, expires_unix)` from the scanned pairing
    /// URI, stamped on STATUS until it expires so the guardian app can match
    /// the device that scanned its QR (then never again).
    pair_token: Option<(String, u64)>,
    /// The boot-gap watch (S1): the platform boot counter this warden last
    /// ran under, and the tally of boots it did not. Written on each start,
    /// read onto STATUS as [`charter_proto::EnforcementGap`].
    boot_watch: BootWatch,
    /// The request/grant broker (None until paired with relays). Arc so the
    /// poll orchestrator drives it with the warden lock RELEASED.
    broker: Option<std::sync::Arc<AndroidBroker>>,
    /// Verified time-extensions the enactor deposits; the tick drains them
    /// into the ONE enforcing ledger (I13).
    inbox: ExtensionInbox,
    /// Durable queue of guardian-approved installs the install.apk enactor
    /// parks; Kotlin drains it each slow tick and reports outcomes (§2.4).
    installs: std::sync::Arc<crate::install::InstallQueue>,
    /// The break-glass override store (durable window + offline audit spool).
    break_glass: crate::breakglass::BreakGlass,
    /// When the CURRENT lock spell began, for the `listening` grace.
    ///
    /// In memory on purpose, unlike the stand-down grace which is persisted so a
    /// reboot cannot restart it. Here a reboot needs no defending against: it
    /// stops the audio, and the exemption requires audio to be actually playing,
    /// so there is nothing for a restarted clock to hand back.
    listening_lock_started: Option<u64>,
    /// Blocks on the broker's async surface (submit / poll).
    rt: std::sync::Arc<tokio::runtime::Runtime>,
    /// Machine-wide clause mirror of the sole ward's clauses — what
    /// `time_extend_eod` reads for the today-only grant cap (the Linux
    /// single-ward layout).
    machine_clauses: RealClauseStore,
    mode: EnforceMode,

    enforcer: EnforcerCore,
    usage: Option<UsageLedger>,
    extension: Option<ExtensionLedger>,
    ledger_tz: String,
    ledger_week_start: WeekStart,
    last_tick_unix: Option<i64>,
    /// Latches true once a charter has been resolved for the ward this session,
    /// so a later unreadable tick still reports `configured=true` and the
    /// install-lockdown is preserved rather than torn down (I22).
    ever_configured: bool,
    /// Denied asks the ward has already been told about, so the answer is
    /// announced ONCE however many ticks read the same terminal record.
    /// Seeded from the store on first use: a request denied before this
    /// process started has already had its moment (and still shows on the
    /// shade) — re-announcing it on every reboot would be nagging.
    announced_denials: Option<std::collections::HashSet<String>>,
}

impl Warden {
    pub fn init(
        base_dir: &str,
        enforce_mode: &str,
        app_version_code: u64,
    ) -> Result<Warden, String> {
        let base = PathBuf::from(base_dir);
        std::fs::create_dir_all(&base).map_err(|e| format!("create base dir: {e}"))?;

        let signer = RealMachineSigner::load_or_create(base.join("machine.key"))
            .map_err(|e| format!("machine identity: {e:?}"))?;
        let machine_sk = zeroize::Zeroizing::new(
            RealMachineSigner::load_or_create_secret(base.join("machine.key"))
                .map_err(|e| format!("machine secret: {e:?}"))?,
        );

        let pairing = RealPairingStore::with_base(&base);
        let (guardian, subject, relays, paired_at) = load_pairing(&pairing);
        let pair_token = load_pair_token(&base);
        let boot_watch = load_boot_watch(&base).unwrap_or_default();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("tokio runtime: {e}"))?;

        let mut w = Warden {
            consumed: RealConsumedIdStore::with_base(&base),
            pending: RealPendingStore::with_base(&base),
            child_clauses: RealChildClauseStore::with_base(&base),
            curator_lists: RealCuratorListStore::with_base(&base),
            usage_store: RealUsageStore::with_base(&base),
            extension_store: RealExtensionStore::with_base(&base),
            usage_written: None,
            extension_written: None,
            pairing,
            signer,
            machine_clauses: RealClauseStore::with_base(&base),
            base,
            guardian,
            subject,
            relays,
            paired_at,
            machine_sk,
            relay: None,
            last_status: None,
            pair_token,
            boot_watch,
            broker: None,
            inbox: std::sync::Arc::new(Mutex::new(Vec::new())),
            installs: std::sync::Arc::new(crate::install::InstallQueue::new(Path::new(base_dir))),
            break_glass: crate::breakglass::BreakGlass::new(Path::new(base_dir)),
            listening_lock_started: None,
            installed_apps: Vec::new(),
            install_window: None,
            app_version_code,
            rt: std::sync::Arc::new(rt),
            mode: EnforceMode::parse(enforce_mode),
            enforcer: EnforcerCore::new(),
            usage: None,
            extension: None,
            ledger_tz: "UTC".into(),
            ledger_week_start: WeekStart::Mon,
            last_tick_unix: None,
            ever_configured: false,
            announced_denials: None,
        };
        w.rebuild_relay();
        Ok(w)
    }

    /// (Re)build the relay facade from the current pairing. Production builds
    /// (feature `real-relay`) get the real wss client; mock/test builds keep
    /// whatever the test injected via [`Warden::inject_relay`] — so this only
    /// replaces the facade when a real one can be built.
    fn rebuild_relay(&mut self) {
        #[cfg(feature = "real-relay")]
        {
            if let (Some(g), false) = (self.guardian, self.relays.is_empty()) {
                self.relay =
                    crate::relay::real_relay(*self.machine_sk, g, self.relays.clone(), &self.base)
                        .ok();
                self.broker = self
                    .build_broker(Box::new(charter_sys::relay::RealRelayTransport::default()))
                    .ok();
            } else {
                self.relay = None;
                self.broker = None;
            }
        }
    }

    /// Stand up the spine broker over `relay` (M9 crash-reconcile runs inside
    /// `Broker::new`). The time.extend enactor validates against the signed
    /// grant (u16/1440, today-only EOD in clause tz) and deposits into the
    /// inbox the tick drains — exactly the Linux seam (I13).
    fn build_broker(
        &self,
        relay: Box<dyn charter_sys::relay::RelayTransport + Send + Sync>,
    ) -> Result<std::sync::Arc<AndroidBroker>, String> {
        let guardian = self.guardian.ok_or("not paired")?;
        let subject = self.subject.ok_or("no sole ward")?;
        let sys = AndroidSystem::with_base(&self.base).map_err(|e| format!("system: {e:?}"))?;
        let facade =
            AndroidTransportFacade::new(relay, *self.machine_sk, guardian, self.relays.clone())?
                .with_outbox(&self.base);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let mut registry = EnactorRegistry::new();
        registry.register(Box::new(
            TimeExtendEnactor::new(std::sync::Arc::new(Mutex::new(
                SpineEnforcerRuntime::fresh(&self.ledger_tz, now),
            )))
            .with_inbox(self.inbox.clone()),
        ));
        registry.register(Box::new(crate::install::InstallApkEnactor::new(
            self.installs.clone(),
        )));
        // app.open enacts nothing (the apps clause carries the hold) —
        // registered so an allowed ask reaches `Enacted` instead of stalling
        // as `Failed` with no enactor at all (see `AppOpenEnactor`'s doc).
        registry.register(Box::new(AppOpenEnactor));
        Ok(std::sync::Arc::new(Broker::new(
            sys,
            facade,
            OsEntropy,
            registry,
            Box::new(NullEventSink),
            subject,
        )))
    }

    /// Verify each polled RELEASE against the pinned guardian + this machine;
    /// apply the first that authenticates AND postdates the CURRENT pairing
    /// epoch. Returns true iff released.
    ///
    /// `verify_release`'s own freshness bound (48h) alone is not enough: it
    /// stops a WEEK-old replay, but a release from an EARLIER pairing to the
    /// SAME guardian — e.g. "disconnect" followed minutes later by "set up
    /// again" — re-authenticates cleanly (same signer, same machine, still
    /// fresh) and would otherwise re-unpair a device the guardian just
    /// re-paired. `self.paired_at` (from the pinned `Pairing.paired_at`,
    /// stamped at `pair()` time) is the monotonic floor: a release dated
    /// before it predates the guardian's later decision to re-pair, so it's
    /// stale relative to THIS pairing even though it's a genuine, fresh-
    /// enough, correctly-signed event. Caught live 2026-07-22 — Robin's Pixel
    /// re-paired repeatedly and was immediately un-paired again by its own
    /// prior release, redelivered every poll for the rest of the 48h window.
    /// Returns `(applied, diagnostic)` — `diagnostic` is surfaced in the
    /// poll's `reason` (Kotlin logs every `PollResult`) so a live device's
    /// release handling is observable without a debugger: how many
    /// candidates arrived and why each one was or wasn't honored (#49
    /// sibling investigation, 2026-07-22).
    fn try_apply_releases(
        &mut self,
        releases: &[(charter_primitives::NostrEvent, PubKey)],
        now: u64,
    ) -> (bool, String) {
        let Some(guardian) = self.guardian else {
            return (false, "release(s) seen but already unpaired".into());
        };
        let machine = self.signer.pubkey();
        let mut detail = format!(
            "release check: {} candidate(s), paired_at={}",
            releases.len(),
            self.paired_at
        );
        for (event, _seal_author) in releases {
            if let Err(e) = charter_verify::verify_release(event, &guardian, &machine, now) {
                detail.push_str(&format!(" | verify_release failed: {e:?}"));
                continue;
            }
            let payload = match charter_proto::ReleasePayload::from_json(&event.content) {
                Ok(p) => p,
                Err(e) => {
                    detail.push_str(&format!(" | payload parse failed: {e:?}"));
                    continue;
                }
            };
            if payload.issued_at < self.paired_at {
                detail.push_str(&format!(
                    " | REJECTED stale: issued_at={} < paired_at={}",
                    payload.issued_at, self.paired_at
                ));
                continue;
            }
            self.apply_release();
            return (
                true,
                format!("{detail} | APPLIED: issued_at={}", payload.issued_at),
            );
        }
        (false, detail)
    }

    /// Apply a guardian-signed RELEASE: the parent-gated unpair. Drops the
    /// pairing, forgets every clause, resets enforcement, and clears the
    /// pin-once — the device returns to "waiting to pair" and can be re-paired,
    /// including to a DIFFERENT guardian. The next tick emits the inert +
    /// unlocked decision, so Kotlin lifts all restrictions. Caller has already
    /// authenticated the event via `verify_release`.
    fn apply_release(&mut self) {
        if let Some(subject) = self.subject {
            let _ = self.child_clauses.clear_for(&subject.to_hex());
        }
        let _ = self.pairing.clear();
        self.guardian = None;
        self.subject = None;
        self.relays.clear();
        self.paired_at = 0;
        self.pair_token = None;
        self.relay = None;
        self.broker = None;
        self.last_status = None;
        self.enforcer = EnforcerCore::new();
        self.usage = None;
        self.extension = None;
        self.last_tick_unix = None;
        self.ever_configured = false;
        if let Ok(mut v) = self.inbox.lock() {
            v.clear();
        }
    }

    /// Every guardian-approved install currently parked, oldest-first — Kotlin
    /// drains this each slow tick, performs each install (verifying signing
    /// continuity against `signerCertSha256` BEFORE committing), and reports
    /// each outcome back via [`Warden::install_result`].
    pub fn drain_installs(&self, now: u64) -> Vec<crate::install::PendingInstall> {
        self.installs.list(now)
    }

    /// Record the outcome of an install Kotlin attempted. `ok`/`terminal` clear
    /// the directive (success, or a permanent failure not worth retrying — a
    /// signing-continuity mismatch, a corrupt archive); anything else leaves it
    /// queued for the next tick (a transient failure — the enactors are
    /// idempotent, so a retry of an already-present version is a no-op success).
    pub fn install_result(&self, req_id: &str, status: &str, reason: &str) {
        if matches!(status, "ok" | "terminal") {
            self.installs.remove(req_id);
            return;
        }
        // Transient: it stays queued and retries — but RECORD that it failed,
        // and why. A directive retrying forever in silence is what let a phone
        // sit two days behind while the guardian saw nothing but an Update
        // button that kept coming back (2026-07-24..26).
        if let Some(mut item) = self.installs.get(req_id) {
            item.attempts = item.attempts.saturating_add(1);
            item.last_error = (!reason.is_empty()).then(|| {
                // Bounded: this goes out on STATUS.
                reason.chars().take(120).collect::<String>()
            });
            self.installs.update(&item);
        }
    }

    /// Has a guardian said no since we last looked? True at most once per
    /// denied request.
    ///
    /// The ward asked and then went about their day; the answer has to find
    /// them, not wait on the lock screen for them to come back — an answer
    /// nobody sees is indistinguishable from being ignored, which is the one
    /// thing a companion tool must never do.
    ///
    /// First call SEEDS instead of announcing: everything already terminal when
    /// this process started has had its moment (and still reads on the shade),
    /// so a reboot never replays old refusals.
    fn take_fresh_denial(&mut self) -> bool {
        let denied: Vec<String> = self
            .list_requests(32)
            .into_iter()
            .filter(|r| r.state == charter_spine::lifecycle::RequestState::Denied)
            .map(|r| r.req_id.to_hex())
            .collect();
        match self.announced_denials.as_mut() {
            None => {
                self.announced_denials = Some(denied.into_iter().collect());
                false
            }
            Some(seen) => {
                let mut fresh = false;
                for id in denied {
                    if seen.insert(id) {
                        fresh = true;
                    }
                }
                fresh
            }
        }
    }

    /// A guardian's unasked-for gift of time, if one is live right now:
    /// `(ledger id, minutes, named-times group id)`. Fail-CLOSED on anything
    /// malformed or expired, like every clause that loosens enforcement. The
    /// third field is the gift's `groupId` verbatim — routing it against the
    /// ward's CURRENT `buckets` clause (existence can change between the
    /// gift's authoring and this tick) is the caller's job, exactly as
    /// charterd's `gift_route` does.
    ///
    /// The id is namespaced so it can never collide with a reqId hex in the
    /// ledger's applied-list — the two share one pool by design (a gift and a
    /// granted ask are the same thing to the enforcer), but they must never be
    /// mistaken for each other.
    pub fn gift_now(&self, now: u64) -> Option<(String, u16, Option<String>)> {
        let subject = self.subject?;
        let stored = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::Gift.store_key(),
            )
            .ok()??;
        let body = serde_json::from_str::<charter_proto::GiftBody>(&stored).ok()?;
        let minutes = body.minutes_now(now)?;
        Some((format!("gift:{}", body.id), minutes, body.group_id))
    }

    /// The guardian's standing stand-down as enforcement needs it, or `None`.
    ///
    /// Fails OPEN, unlike [`Warden::gift_now`]: a stand-down TIGHTENS
    /// enforcement, so any doubt must resolve to "no lock". A malformed clause
    /// that locks a ward out of her own phone while her guardian believes she is
    /// fine is the worse failure, and he can see it did not apply and try again.
    pub fn stand_down_now(&self, now: u64) -> Option<charter_schedule::enforcer::StandDown> {
        let subject = self.subject?;
        let hex = subject.to_hex();
        // The grace rule (first-sight pinning, the monotonic-floor nudge) is
        // SHARED — spec/contract.md promises it lives in one place, because a
        // second copy is exactly where a lock-that-never-lands drifts in.
        charter_spine::standdown::stand_down_now(
            &charter_spine::standdown::ChildSlot {
                store: &self.child_clauses,
                subject_hex: &hex,
            },
            now,
        )
    }

    /// Is a guardian-signed maintenance window open right now? Fail-CLOSED:
    /// no clause, malformed, expired or absurdly long ⇒ shut, and the install
    /// lock stays on. This is the one clause that loosens enforcement, so
    /// every uncertainty resolves to "locked".
    pub fn maintenance_open(&self, now: u64) -> bool {
        self.maintenance_body()
            .map(|b| b.is_open(now))
            .unwrap_or(false)
    }

    /// The stored maintenance clause body, if any. Split out because the ward's
    /// own notice needs the window's REAL expiry: it was showing the core's
    /// one-hour ceiling for every window, so a 30-minute one told the child they
    /// had 60 (caught on Robin's phone, 2026-07-30). A ward being told they
    /// have longer than they do is the one direction this must never round.
    fn maintenance_body(&self) -> Option<charter_proto::MaintenanceBody> {
        let subject = self.subject?;
        let stored = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::Maintenance.store_key(),
            )
            .ok()
            .flatten()?;
        serde_json::from_str::<charter_proto::MaintenanceBody>(&stored).ok()
    }

    /// What the guardian needs to see about a stuck update: the oldest install
    /// directive that has failed at least once, as JSON, or "" when everything
    /// is healthy. Deliberately reports the DIRECTIVE, not the app inventory —
    /// per-app detail is not the guardian's business (see the per-device memo).
    pub fn install_health(&self, now: u64) -> String {
        let worst = self
            .installs
            .list(now)
            .into_iter()
            .filter(|i| i.attempts > 0)
            .min_by_key(|i| i.at);
        match worst {
            None => String::new(),
            Some(i) => serde_json::json!({
                "packageName": i.package_name,
                "versionCode": i.version_code,
                "attempts": i.attempts,
                "sinceUnix": i.at,
                "lastError": i.last_error,
            })
            .to_string(),
        }
    }

    /// The sole ward's stored `buckets` clause, if there is a parseable,
    /// structurally valid one — mirrors charterd's `parse_buckets`. Fail-safe
    /// by design: `None` on absent/unparseable JSON or a body that fails its
    /// own invariants (`is_valid`, e.g. a duplicate bucket id) — a cap that
    /// cannot be trusted must never confiscate an app the ward is entitled to.
    ///
    /// Pause is deliberately NOT filtered here: STATUS still reports a paused
    /// set's raw meters (so the family sees what was spent even while the
    /// cap is lifted). [`charter_schedule::bucket_for_app`] and
    /// [`charter_schedule::spent_bucket_apps`] already fail-safe on pause
    /// internally (credit/suspend nothing), so every caller shares one rule.
    fn buckets_clause(&self) -> Option<charter_schedule::GrantBuckets> {
        let subject = self.subject?;
        let stored = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::Buckets.store_key(),
            )
            .ok()??;
        let body: charter_schedule::GrantBuckets = serde_json::from_str(&stored).ok()?;
        body.is_valid().then_some(body)
    }

    /// The sole ward's per-bucket ("named time") suspensions at `now_unix`:
    /// the packages whose bucket has spent EITHER axis (day or week) right
    /// now — extra-adjusted by any granted `time.extend`/gift to that
    /// bucket's own pool, mirroring the Linux ward's spent-bucket sweep
    /// (charterd `runtime.rs`, the `blocked.extend(spent)` block) and this
    /// module's own [`Warden::app_rule_suspensions`] shape. Spending a bucket
    /// closes THAT bucket's apps only — never the whole device. Fail-safe: no
    /// ward / no clause / malformed / paused / no ledgers yet ⇒ empty (suspend
    /// nothing — an unreadable cap must never confiscate an app the ward is
    /// entitled to).
    ///
    /// Same shape as charterd's twin, so the same dependency holds here: this
    /// subtracts `extra` from spent before comparing to the RAW cap, while
    /// the ward's own view adds `extra` onto the DISPLAYED cap instead —
    /// algebraically the same inequality, agreeing only because the
    /// `saturating_sub` floor at 0 never bites, which itself only holds
    /// because `GrantBuckets::is_valid` rejects a 0-minute cap. See
    /// `runtime.rs`'s identical sweep for the worked-through reasoning.
    pub fn bucket_suspensions(&self, now_unix: i64) -> Vec<String> {
        let Some(body) = self.buckets_clause() else {
            return Vec::new();
        };
        let Some(usage) = self.usage.as_ref() else {
            return Vec::new();
        };
        let extension = self.extension.as_ref();
        charter_schedule::spent_bucket_apps(&body, |id| {
            let extra = extension
                .map(|e| e.bucket_extra_secs(now_unix, id))
                .unwrap_or(0);
            charter_schedule::BucketSpent {
                day_secs: usage
                    .app_bucket_today_secs(now_unix, id)
                    .saturating_sub(extra),
                week_secs: usage
                    .app_bucket_week_secs(now_unix, id)
                    .saturating_sub(extra),
            }
        })
    }

    /// The `askFirst` apps still gated right now, labelled from the device's
    /// own inventory where possible — the ward's "Ask to open" list. Mirrors
    /// charterd's `ask_first_apps_from_apps` pkg-for-pkg (same clause, same
    /// fail-safe rules — [`Warden::apps_clause`] has already dropped an
    /// absent/malformed/paused clause).
    ///
    /// "Gated" is POSTURE-aware, same as charterd's twin: under `blocklist` a
    /// pkg is offered when it is BOTH named in `askFirst` AND still actually
    /// in `blocked` right now (after holds resolve). Under `allowlist`,
    /// `blocked` is never populated by the clause itself (it is DERIVED —
    /// everything not in `allowed`), so checking it here would always read
    /// empty and the ask affordance would be permanently dead (found in
    /// review) — a pkg is offered instead whenever it is NOT in `allowed`.
    fn ask_first_now(&self, now_unix: i64) -> Vec<(String, String)> {
        let Some(g) = self.apps_clause() else {
            return Vec::new();
        };
        if g.ask_first.is_empty() {
            return Vec::new();
        }
        let effective = g.effective_at(now_unix.max(0) as u64);
        let gated = |pkg: &str| -> bool {
            match effective.posture {
                charter_proto::AppPosture::Blocklist => effective.blocked.iter().any(|b| b == pkg),
                charter_proto::AppPosture::Allowlist => !effective.allowed.iter().any(|a| a == pkg),
            }
        };
        g.ask_first
            .iter()
            .filter(|pkg| gated(pkg))
            .map(|pkg| {
                let label = self
                    .installed_apps
                    .iter()
                    .find(|a| &a.pkg == pkg)
                    .map(|a| a.label.clone())
                    .unwrap_or_else(|| pkg.clone());
                (pkg.clone(), label)
            })
            .collect()
    }

    /// The named-times mirror for the ward's own surface (spec D-Bus parity,
    /// Task 9 UI): every bucket's day/week picture — same field shape as
    /// Linux's `BucketView` (`charter-ipc::dto`) — plus the `askFirst` list
    /// with labels, so Task 9's Kotlin UI needs no policy read of its own.
    /// `""` bucket meters when the ledgers aren't loaded yet (before the
    /// first tick): `used_seconds: 0`, walls reported as configured.
    pub fn bucket_views_json(&self, now_unix: i64) -> String {
        let buckets = self.buckets_clause();
        let groups: Vec<dto::BucketView> = match &buckets {
            Some(body) => {
                let usage = self.usage.as_ref();
                let extension = self.extension.as_ref();
                body.buckets
                    .iter()
                    .map(|b| {
                        let day_used = usage
                            .map(|u| u.app_bucket_today_secs(now_unix, &b.id))
                            .unwrap_or(0);
                        let week_used = usage
                            .map(|u| u.app_bucket_week_secs(now_unix, &b.id))
                            .unwrap_or(0);
                        // M-4: `spent` stays RAW here — `bucket_view` adds
                        // `extra` onto the CAP instead of subtracting it from
                        // spent, so the ward's remaining counts down honestly
                        // through a granted surplus instead of freezing at
                        // the base cap (see `bucket_view`'s doc comment).
                        let extra = extension
                            .map(|e| e.bucket_extra_secs(now_unix, &b.id))
                            .unwrap_or(0);
                        let spent = charter_schedule::BucketSpent {
                            day_secs: day_used,
                            week_secs: week_used,
                        };
                        bucket_view(b, body.is_paused(), day_used, spent, extra)
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        let ask_first: Vec<dto::AskFirstAppView> = self
            .ask_first_now(now_unix)
            .into_iter()
            .map(|(pkg, label)| dto::AskFirstAppView { pkg, label })
            .collect();
        serde_json::to_string(&dto::BucketViewsPayload {
            buckets: groups,
            ask_first,
        })
        .unwrap_or_default()
    }

    /// The sole ward's stored `apps` clause at this build's version, paused or
    /// not. Only [`Warden::app_policy`] wants the paused one (for `hidden`,
    /// which `paused` does not lift); everything else reads [`Warden::apps_clause`].
    fn stored_apps_clause(&self) -> Option<charter_proto::GrantApps> {
        let subject = self.subject?;
        let stored = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::Apps.store_key(),
            )
            .ok()??;
        match serde_json::from_str::<charter_proto::GrantApps>(&stored) {
            // A future clause version this build does not understand is
            // treated exactly like a malformed body — matches Linux's
            // `blocked_pkgs_from_apps`/`ask_first_apps_from_apps`
            // (`apps_policy.rs`), so a version bump behaves identically on
            // both platforms: the loosening direction (a blocked-list simply
            // stops being enforced) rather than confiscating on a guess.
            Ok(g) if g.v == charter_proto::APPS_VERSION => Some(g),
            _ => None,
        }
    }

    /// The sole ward's stored `apps` clause, if there is a usable one.
    fn apps_clause(&self) -> Option<charter_proto::GrantApps> {
        // Fail-closed: a malformed, future-versioned, or paused clause
        // suspends nothing.
        self.stored_apps_clause().filter(|g| !g.is_paused())
    }

    /// The sole ward's standing per-app policy AS ENFORCED AT `now_unix`
    /// (`GrantApps` JSON), or "" when there is none / it's paused / not paired /
    /// malformed (fail-closed: no per-app suspension). Kotlin parses it and
    /// suspends the resulting package set level-triggered, INDEPENDENT of the
    /// time lock — a blocked app stays blocked even during allowed screen time
    /// (the D3 per-app dimension).
    ///
    /// Any live time-boxed HOLD is folded into the lists here, so Kotlin never
    /// has to know holds exist: it reads posture + lists exactly as it always
    /// did. Recomputed per call for the same reason [`Warden::tethering_mode`]
    /// is — a hold ends at an absolute instant, and a stored clause re-read
    /// after a reboot must not extend it by a second.
    ///
    /// `hidden` ("remove from device") is the one field `paused` does NOT
    /// lift: pausing app blocks for an hour must not make a tablet's OEM
    /// bloatware reappear. A paused clause that names hidden apps is still
    /// surfaced — with its posture lists EMPTIED, so Kotlin's suspend set
    /// (which reads posture + lists and knows nothing of `paused`) blocks
    /// nothing, while its hide reconcile still sees the list. A paused
    /// clause hiding nothing stays "" exactly as before (2026-08-27).
    pub fn app_policy(&self, now_unix: i64) -> String {
        let Some(g) = self.stored_apps_clause() else {
            return String::new();
        };
        let mut eff = g.effective_at(now_unix.max(0) as u64);
        if eff.is_paused() {
            if eff.hidden.is_empty() {
                return String::new();
            }
            eff.blocked.clear();
            eff.allowed.clear();
            eff.holds.clear();
            eff.ask_first.clear();
        }
        serde_json::to_string(&eff).unwrap_or_default()
    }

    /// The sole ward's LIVE app holds at `now_unix`, as a JSON array of
    /// `{pkg, state, untilUnix}` — "" when there are none.
    ///
    /// Kotlin diffs this against what it last saw so the ward can be told an app
    /// opened and told again when it closed. Deliberately separate from
    /// [`Warden::app_policy`], which has already dissolved holds into lists:
    /// enforcement wants the resolved answer, a notice wants to know a hold is
    /// what caused it.
    pub fn app_holds(&self, now_unix: i64) -> String {
        let Some(g) = self.apps_clause() else {
            return String::new();
        };
        let live = g.live_holds(now_unix.max(0) as u64);
        if live.is_empty() {
            return String::new();
        }
        serde_json::to_string(&live).unwrap_or_default()
    }

    /// The sole ward's per-app RULE suspensions at `now_unix`: the packages whose
    /// `appRules` clause (store_key 6) access evaluates to `Blocked` RIGHT NOW —
    /// blocked outright, OR outside their per-app allowed hours. Schedule-
    /// dependent, so it is recomputed each call (unlike the standing
    /// [`Warden::app_policy`]). Fail-safe: no ward / no clause / a malformed
    /// clause ⇒ an empty set (suspend nothing). Kotlin unions this with the
    /// standing per-app policy and suspends the union level-triggered — a package
    /// suspended by EITHER source stays suspended (the D3 per-app dimension).
    pub fn app_rule_suspensions(&self, now_unix: i64) -> Vec<String> {
        let Some(subject) = self.subject else {
            return Vec::new();
        };
        let stored = self.child_clauses.get_child_clause(
            &subject.to_hex(),
            charter_proto::ClauseKind::AppRules.store_key(),
        );
        let Ok(Some(body)) = stored else {
            return Vec::new();
        };
        // Fail-safe: an absent or unparseable rule set suspends nothing.
        let rules = match serde_json::from_str::<charter_schedule::GrantAppRules>(&body) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        charter_schedule::evaluate_app_rules(&rules, now_unix)
            .into_iter()
            .filter_map(|(pkg, access)| match access {
                charter_schedule::AppAccess::Blocked => Some(pkg),
                charter_schedule::AppAccess::Allowed => None,
            })
            .collect()
    }

    /// The sole ward's tethering posture at `now_unix`: `"blocked"`, `"raw"`,
    /// or `"filtered"` (the `tethering` clause, store_key 7). Time-boxed grants
    /// end AT their `until`, so this is recomputed per tick like the app rules.
    /// Fail-safe: no ward / no clause / a malformed clause ⇒ `"blocked"`.
    pub fn tethering_mode(&self, now_unix: i64) -> &'static str {
        let Some(subject) = self.subject else {
            return "blocked";
        };
        let stored = self.child_clauses.get_child_clause(
            &subject.to_hex(),
            charter_proto::ClauseKind::Tethering.store_key(),
        );
        let Ok(Some(body)) = stored else {
            return "blocked";
        };
        let grant = match serde_json::from_str::<charter_schedule::GrantTethering>(&body) {
            Ok(g) => g,
            Err(_) => return "blocked",
        };
        match charter_schedule::evaluate_tethering(Some(&grant), now_unix.max(0) as u64) {
            charter_schedule::TetherMode::Blocked => "blocked",
            charter_schedule::TetherMode::Raw => "raw",
            charter_schedule::TetherMode::Filtered => "filtered",
        }
    }

    /// The ward-facing "your charter" mirror (spec D8): live lock state,
    /// minutes left, and the weekly schedule lines — rendered by the SHARED
    /// spine composer (`charter_spine::lock_info`), so the ward's phone and
    /// the Linux lock panel phrase the week identically. `""` when no charter
    /// is configured yet (the ward sees "no charter yet", never a guess).
    pub fn schedule_view(&self, now: i64) -> String {
        let Some(subject) = self.subject else {
            return String::new();
        };
        let Ok(clauses) = self.child_clauses.clauses_for(&subject.to_hex()) else {
            return String::new();
        };
        let (schedule, budget) = resolve_effective(&clauses);
        // A valid `buckets` clause is its own liveness signal here too — the
        // same fourth condition `tick()`/`time_left()` already carry (Task 9
        // review round): this is the D8 "Your charter" mirror BOTH the phone
        // screen and the home-screen widget render from, so a buckets-only
        // ward ("Play is an hour a day", nothing else) headlined "No charter
        // set yet" directly above its own correctly-metered named-times rows
        // — an active, actively-contradicted lie sitting right next to the
        // truth.
        if schedule.is_none() && budget.is_none() && self.buckets_clause().is_none() {
            return String::new();
        }
        let tl = self.time_left(now);
        let used_today = self.usage.as_ref().map(|u| u.used_today(now)).unwrap_or(0);
        let info = charter_spine::lock_info::lock_info(
            schedule.as_ref(),
            budget.as_ref(),
            tl.next_open_secs,
            used_today,
            now,
        );
        // Out-of-hours (spec 2026-08-03): the WEEK counters, straight off
        // this device's own ledger — raw numbers, not a pre-formatted
        // sentence, so Kotlin's `GroupMirror.outOfHoursLine` can render the
        // ward's line with the exact same wording as the guardian's
        // (`outOfHoursLine` in `insights/usageHistory.ts`). Zero before the
        // first tick / with no ledger loaded yet, same as every other
        // pre-tick default on this surface.
        let (oh_nights, oh_week_secs) = self
            .usage
            .as_ref()
            .map(|u| (u.out_of_hours_nights_week(), u.out_of_hours_week_secs()))
            .unwrap_or((0, 0));
        serde_json::to_string(&serde_json::json!({
            "locked": tl.locked,
            // -1 (never a clamped 0) when the WHOLE DEVICE carries no time
            // wall at all — a buckets-only ward, or (pre-existing, same fix)
            // a schedule/budget combo that is itself unbounded. `0` would
            // read as "about to lock any second", which is the opposite of
            // what an unbounded charter means; -1 is the same "unset, never
            // 0" sentinel every other view on this surface already uses
            // (e.g. `BucketView.weekLimitSeconds`).
            "minutesLeft": if tl.effective_secs < 0 { -1 } else { tl.effective_secs / 60 },
            "secondsLeft": tl.effective_secs,
            "detail": info.detail,
            "lines": info.lines,
            "outOfHoursNightsWeek": oh_nights,
            "outOfHoursWeekSecs": oh_week_secs,
        }))
        .unwrap_or_default()
    }

    /// The ward's lifeline numbers (`lifeline` clause, store_key 9) as JSON
    /// `[{"label":…,"number":…}]`, or `""` when unpaired / no clause /
    /// malformed. The lock screen renders one call button per entry, so this
    /// is fail-closed: a body that doesn't validate renders NO buttons rather
    /// than a garbage dial string (spec D9).
    pub fn lifeline(&self) -> String {
        let Some(subject) = self.subject else {
            return String::new();
        };
        let stored = self.child_clauses.get_child_clause(
            &subject.to_hex(),
            charter_proto::ClauseKind::Lifeline.store_key(),
        );
        // NUMBERS stay fail-closed — a body that doesn't validate must never
        // render a garbage dial string. BREAK-GLASS does not: a device with no
        // lifeline clause (or a broken one) still gets the escape hatch, or the
        // ward has no way out when the device can't reach the relay. So an
        // absent/unusable clause yields a view with the safety net and nothing
        // else, rather than no view at all.
        let parsed = match &stored {
            Ok(Some(body)) => serde_json::from_str::<charter_proto::LifelineBody>(body)
                .ok()
                .filter(|p: &charter_proto::LifelineBody| p.validate().is_ok()),
            _ => None,
        };
        let Some(parsed) = parsed else {
            return serde_json::json!({
                "numbers": [],
                "emergencyServices": false,
                "torch": false,
                "breakGlass": charter_proto::BreakGlassCfg::safety_net(),
            })
            .to_string();
        };
        // v2 (2026-07-24): the shade needs the emergency-services flag and the
        // break-glass config alongside the numbers. Emitted as an OBJECT;
        // Kotlin accepts both this and the legacy bare array so an in-flight
        // .so/APK mismatch can never blank the lifeline (fail-closed would
        // mean no way to call a guardian — the one thing that must not break).
        serde_json::json!({
            "numbers": parsed.numbers,
            "emergencyServices": parsed.emergency_services.unwrap_or(false),
            "torch": parsed.torch.unwrap_or(false),
            // Absent means ON (see `BreakGlassCfg::safety_net`), so a guardian
            // who configured numbers but never thought about the escape hatch
            // still leaves one.
            "breakGlass": parsed
                .break_glass
                .clone()
                .unwrap_or_else(charter_proto::BreakGlassCfg::safety_net),
        })
        .to_string()
    }

    /// The ward breaks the glass. Returns the JSON the shade renders:
    /// `{allowed, scope, secsLeft, reason}`. Never blocked by the network —
    /// the unlock is immediate and the guardian's audit is spooled durably
    /// for the next poll. Refused only when the guardian hasn't enabled it
    /// (fail-closed on config, never on connectivity).
    pub fn break_glass(&mut self, now: u64) -> String {
        let cfg = self.break_glass_cfg();
        let Some((scope, duration_secs)) = cfg else {
            return serde_json::json!({
                "allowed": false,
                "reason": "not-enabled",
            })
            .to_string();
        };
        let w = crate::breakglass::Window {
            at: now,
            scope: scope.clone(),
            duration_secs,
        };
        self.break_glass.open(&w);
        // Loud, but never blocking: spooled now, emitted on the next poll
        // (or immediately after, if the phone is online).
        self.break_glass
            .queue_audit(crate::breakglass::audit_tags(&scope, duration_secs));
        serde_json::json!({
            "allowed": true,
            "scope": scope,
            "secsLeft": duration_secs,
        })
        .to_string()
    }

    /// The guardian-configured break-glass, if enabled: `(scope, secs)`.
    fn break_glass_cfg(&self) -> Option<(String, u64)> {
        let subject = self.subject?;
        // Absent, unparseable or invalid lifeline still leaves the safety net
        // up: the escape hatch must not depend on a guardian having configured
        // anything, because the device that most needs it is the one nobody
        // finished setting up. Only an explicit `enabled: false` removes it.
        let bg = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::Lifeline.store_key(),
            )
            .ok()
            .flatten()
            .and_then(|body| serde_json::from_str::<charter_proto::LifelineBody>(&body).ok())
            .filter(|p| p.validate().is_ok())
            .and_then(|p| p.break_glass)
            .unwrap_or_else(charter_proto::BreakGlassCfg::safety_net);
        if !bg.enabled || bg.duration_minutes == 0 {
            return None;
        }
        let scope = match bg.scope {
            charter_proto::BreakGlassScope::Full => crate::breakglass::SCOPE_FULL,
            charter_proto::BreakGlassScope::Calls => crate::breakglass::SCOPE_CALLS,
        };
        Some((scope.to_string(), u64::from(bg.duration_minutes) * 60))
    }

    /// An active FULL-scope override suspends the shade for its window.
    /// `calls` scope changes no enforcement — calls already pass — it only
    /// changes what the shade says.
    fn break_glass_open(&self, now: u64) -> bool {
        self.break_glass
            .window()
            .filter(|w| w.active(now) && w.scope == crate::breakglass::SCOPE_FULL)
            .is_some()
    }

    /// Level-triggered self-update check (#44): if the stored `update` clause
    /// targets our own package at a versionCode ahead of this build, park a
    /// url-sourced install directive. Idempotent — the queue is keyed by
    /// req_id (= the archive's sha256), the Kotlin installer no-ops on
    /// an already-present version, and a device at-or-past the clause does
    /// nothing. Invalid bodies are ignored fail-closed.
    pub fn check_self_update(&mut self, now: u64) {
        let Some(subject) = self.subject else {
            return;
        };
        let stored = self.child_clauses.get_child_clause(
            &subject.to_hex(),
            charter_proto::ClauseKind::Update.store_key(),
        );
        let Ok(Some(body)) = stored else {
            return;
        };
        let body = match serde_json::from_str::<charter_proto::UpdateAppBody>(&body) {
            Ok(b) => b,
            Err(_) => return,
        };
        if body.validate().is_err() || body.package_name != OWN_PACKAGE {
            return;
        }
        if body.version_code <= self.app_version_code {
            return;
        }
        // The queue is filename-keyed and hex-guarded; the archive digest is
        // the natural key — same artifact, same directive, idempotent.
        self.installs.put(&crate::install::PendingInstall {
            req_id: body.apk_sha256.to_hex(),
            package_name: body.package_name.clone(),
            version_code: Some(body.version_code),
            signer_cert_sha256: body.signer_cert_sha256.to_hex(),
            source: "url".into(),
            url: Some(body.url.clone()),
            apk_sha256: Some(body.apk_sha256.to_hex()),
            at: now,
            attempts: 0,
            last_error: None,
        });
    }

    /// The ward's effective web-content policy rendered as a `DnsFilterPlan`
    /// (the SAME renderer the Linux warden uses — charter-webpolicy). Returns
    /// `{"revision":<hex8>,"plan":{…}}` JSON, or "" when not paired. Kotlin
    /// applies the plan in a DO-pinned VpnService. Fail-closed: an undecodable
    /// or paused clause evaluates to `DnsMode::Locked` (block-all + exceptions).
    pub fn web_dns_plan(&self) -> String {
        let Some(subject) = self.subject else {
            return String::new();
        };
        // Cached, signature-verified curator lists (ingested by the broker).
        let lists: Vec<charter_content::CuratorList> = self
            .curator_lists
            .all_lists()
            .unwrap_or_default()
            .iter()
            .filter_map(|j| serde_json::from_str::<charter_content::CuratorList>(j).ok())
            .collect();
        // The stored content clause; absent ⇒ unrestricted (no web constraint yet).
        let clause_json = match self.child_clauses.get_child_clause(
            &subject.to_hex(),
            charter_proto::ClauseKind::Content.store_key(),
        ) {
            Ok(Some(body)) => body,
            // No content clause for this ward → no web constraint.
            Ok(None) => {
                let plan = charter_webpolicy::render_dns_filter(
                    &charter_content::EffectiveWebPolicy::unrestricted(),
                );
                return wrap_plan(&plan);
            }
            // A store/envelope read error is fail-CLOSED: block all (minus
            // exceptions) rather than silently drop web filtering while the
            // device still appears enforced.
            Err(_) => {
                let plan = charter_webpolicy::render_dns_filter(
                    &charter_content::EffectiveWebPolicy::locked(),
                );
                return wrap_plan(&plan);
            }
        };
        // evaluate_content_json fails CLOSED (locked) on any decode error.
        let eff = charter_content::evaluate_content_json(&clause_json, &lists);
        let plan = charter_webpolicy::render_dns_filter(&eff);
        wrap_plan(&plan)
    }

    /// Kotlin reports the device's installed launchable apps (JSON
    /// `[{"pkg":…,"label":…}]`) each slow tick; they ride the next STATUS so the
    /// guardian can pick apps by name (D3). Malformed input keeps the last list.
    pub fn set_installed_apps(&mut self, apps_json: &str) {
        if let Ok(apps) = serde_json::from_str::<Vec<charter_proto::AppRef>>(apps_json) {
            self.installed_apps = apps;
        }
    }

    /// Kotlin reports what changed during the install window this side is
    /// tracking (it alone can read package install times). Malformed input
    /// keeps the last account rather than blanking it — a guardian who has been
    /// shown "2 apps installed" must never see that silently become "nothing".
    /// Record that the warden is running under platform boot number
    /// `boot_count`, and count any boots it missed (S1).
    ///
    /// The warden cannot witness its own absence. In Android safe mode every
    /// third-party package is disabled — a Device Owner included — so there
    /// is no tick to notice, nothing suspended, no lock, and no report. What
    /// the platform keeps regardless is `Settings.Global.BOOT_COUNT`, which
    /// increments on every boot whether or not we ran. So: remember the count
    /// we last ran under, and treat any daylight beyond the expected +1 as
    /// boots that happened without us.
    ///
    /// The three cases that are NOT a gap, all of them ordinary:
    /// - the same count again — the service restarted inside one boot
    ///   (crash-restart, `START_STICKY`, an update), which is not a boot;
    /// - exactly one more — the normal reboot, which we are running through
    ///   right now;
    /// - a count that went BACKWARDS — the counter itself was reset (a wipe
    ///   or a restore), so there is no baseline to compare against and we
    ///   take the new one rather than reporting a spurious gap.
    ///
    /// First run has no baseline and can only start one; a phone that was
    /// used before Charter arrived is not owed an accusation about it.
    pub fn note_boot(&mut self, boot_count: u64, now: u64) {
        let prev = self.boot_watch;
        // No baseline yet (fresh install, or a counter the platform doesn't
        // give us): start one, report nothing.
        if prev.last_boot_count == 0 || boot_count == 0 {
            self.boot_watch = BootWatch {
                last_boot_count: boot_count,
                ..prev
            };
            save_boot_watch(&self.base, &self.boot_watch);
            return;
        }
        let missed = boot_count.saturating_sub(prev.last_boot_count + 1);
        self.boot_watch = BootWatch {
            last_boot_count: boot_count,
            unexplained_boots: prev
                .unexplained_boots
                .saturating_add(missed.min(u32::MAX as u64) as u32),
            last_noticed_at: if missed > 0 {
                now
            } else {
                prev.last_noticed_at
            },
        };
        if self.boot_watch != prev {
            save_boot_watch(&self.base, &self.boot_watch);
        }
    }

    /// The boot watch as STATUS reports it — `None` while the device has
    /// never booted unwarded, which is the ordinary state.
    fn enforcement_gap(&self) -> Option<charter_proto::EnforcementGap> {
        (self.boot_watch.unexplained_boots > 0).then_some(charter_proto::EnforcementGap {
            unexplained_boots: self.boot_watch.unexplained_boots,
            last_noticed_at: self.boot_watch.last_noticed_at,
        })
    }

    pub fn set_install_window(&mut self, report_json: &str) {
        if let Ok(r) = serde_json::from_str::<charter_proto::InstallWindowReport>(report_json) {
            self.install_window = Some(r);
        }
    }

    /// Track the install window as this device SEES it, and hand Kotlin the
    /// span to account for: `{"startedAt":N,"endedAt":N|null,"open":bool}`, or
    /// `{}` when no window has ever been opened here.
    ///
    /// Level-triggered off `maintenance_open`, and it writes the local slot on
    /// each edge. Recording locally is what makes the account survive the
    /// guardian pressing Close: that publishes a NEWER maintenance clause whose
    /// `issuedAt` is the close time, so the open clause — and with it the real
    /// start — is gone from the store the moment it lands.
    pub fn maintenance_span(&mut self, now: u64) -> String {
        let Some(subject) = self.subject else {
            return "{}".into();
        };
        let subject_hex = subject.to_hex();
        let key = charter_proto::MAINTENANCE_SPAN_STORE_KEY;
        let open = self.maintenance_open(now);
        let stored = self
            .child_clauses
            .get_child_clause(&subject_hex, key)
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_str::<charter_proto::MaintenanceSpan>(&b).ok());

        let span = match (open, stored) {
            // Still open and already recorded — leave the start where it is.
            (true, Some(span)) if span.ended_at.is_none() => span,
            // Open, but the last one we recorded is closed (or there is none):
            // this is a NEW window, starting the moment we saw it.
            (true, _) => {
                let span = charter_proto::MaintenanceSpan {
                    started_at: now,
                    ended_at: None,
                };
                self.put_maintenance_span(&subject_hex, now, &span);
                span
            }
            // Shut, and the one we recorded was still open: close it HERE, so
            // the account is bounded by when the lock actually came back.
            (false, Some(span)) if span.ended_at.is_none() => {
                let closed = charter_proto::MaintenanceSpan {
                    started_at: span.started_at,
                    ended_at: Some(now),
                };
                self.put_maintenance_span(&subject_hex, now, &closed);
                closed
            }
            // Shut, already closed — the last window's account, still reportable.
            (false, Some(span)) => span,
            // Shut and nothing ever recorded.
            (false, None) => return "{}".into(),
        };

        // The window's real end, straight from the clause — NOT the span's
        // start plus the cap. Only meaningful while open; once shut, the ward
        // has no notice to put a number on.
        let until = if span.ended_at.is_none() {
            self.maintenance_body().map(|b| b.until_unix)
        } else {
            None
        };
        serde_json::json!({
            "startedAt": span.started_at,
            "endedAt": span.ended_at,
            "untilUnix": until,
            "open": span.ended_at.is_none(),
        })
        .to_string()
    }

    /// The slot's `issued_at` is the write time, so the store's monotonic floor
    /// accepts the close after the open. A failed write is not fatal: the next
    /// tick retries, and until then the account simply reports the older span.
    fn put_maintenance_span(
        &mut self,
        subject_hex: &str,
        now: u64,
        span: &charter_proto::MaintenanceSpan,
    ) {
        let body = serde_json::to_string(span).unwrap_or_default();
        let _ = self.child_clauses.put_child_clause(
            subject_hex,
            charter_proto::MAINTENANCE_SPAN_STORE_KEY,
            now,
            &body,
        );
    }

    /// Submit a brokered request ("ask for more time"). Persists Pending
    /// BEFORE publishing (M16); returns the reqId hex.
    pub fn submit_request(&self, op: &str, params_json: &str) -> Result<String, String> {
        let broker = self.broker.as_ref().ok_or("not paired (no relay)")?;
        let op = match op {
            "time.extend" => charter_proto::OpType::TimeExtend,
            "install.apk" => charter_proto::OpType::InstallApk,
            // Found while verifying C1 (review round 1, 2026-08-03): this arm
            // was simply missing, so MainActivity's `submitAppOpenAsk` (via
            // `CharterCore.submitRequest("app.open", …)`) always failed with
            // "unsupported op" — the REQUEST never left the phone at all, let
            // alone hit the grant-verification gap the review found. Both
            // halves of the loop had to be broken for the button to look like
            // it did nothing.
            "app.open" => charter_proto::OpType::AppOpen,
            other => return Err(format!("unsupported op: {other}")),
        };
        let params: serde_json::Value =
            serde_json::from_str(params_json).map_err(|e| format!("bad params: {e}"))?;
        // Fail-closed on malformed request params (the broker only shape-checks
        // that params is an object; validate the typed contract here).
        if op == charter_proto::OpType::InstallApk {
            let p: charter_proto::params::InstallApkRequestParams =
                serde_json::from_value(params.clone())
                    .map_err(|e| format!("bad install.apk params: {e}"))?;
            p.validate().map_err(|e| format!("{e:?}"))?;
        }
        if op == charter_proto::OpType::AppOpen {
            let p: charter_proto::AppOpenRequestParams = serde_json::from_value(params.clone())
                .map_err(|e| format!("bad app.open params: {e}"))?;
            p.validate().map_err(|e| format!("{e:?}"))?;
        }
        self.rt
            .block_on(broker.submit(op, params, None))
            .map(|id| id.to_hex())
            .map_err(|e| format!("{e:?}"))
    }

    /// Newest-first request records (the child/UI surface).
    pub fn list_requests(&self, limit: usize) -> Vec<RequestRecord> {
        match &self.broker {
            Some(b) => b.list(limit),
            None => Vec::new(),
        }
    }

    /// Records for one reqId ("" = all).
    pub fn query_status(&self, req_id: &str) -> Vec<RequestRecord> {
        match &self.broker {
            Some(b) => b.status(req_id),
            None => Vec::new(),
        }
    }

    /// Cancel a Pending ask; true iff it was Pending.
    pub fn cancel_request(&self, req_id: &str) -> bool {
        match &self.broker {
            Some(b) => b.cancel(req_id),
            None => false,
        }
    }

    /// Test seam: wire BOTH relay halves (STATUS facade + broker) over an
    /// in-memory relay whose clones share one store.
    #[cfg(any(test, feature = "mock"))]
    #[allow(dead_code)] // test seam: callers live under cfg(test)
    pub fn wire_test_relay(
        &mut self,
        mock: charter_sys::relay::MockRelayTransport,
    ) -> Result<(), String> {
        let guardian = self.guardian.ok_or("pair first")?;
        self.relay = Some(std::sync::Arc::new(
            crate::relay::BlockingRelay::new(
                mock.clone(),
                *self.machine_sk,
                guardian,
                self.relays.clone(),
            )?
            .with_outbox(&self.base),
        ));
        self.broker = Some(self.build_broker(Box::new(mock))?);
        Ok(())
    }

    /// Test seam: inject a relay facade and select the LEGACY direct-ingest
    /// path (no broker) — tests of the broker path use [`Warden::wire_test_relay`].
    #[cfg(any(test, feature = "mock"))]
    #[allow(dead_code)] // a test seam: callers live under cfg(test)
    pub fn inject_relay(&mut self, relay: std::sync::Arc<dyn WardenRelay>) {
        self.relay = Some(relay);
        self.broker = None;
    }

    pub fn machine_pubkey_hex(&self) -> String {
        self.signer.pubkey().to_hex()
    }

    /// Machine pubkey grouped 8×8 for the pairing display (device_code.rs:47-53).
    pub fn device_code(&self) -> String {
        let hex = self.machine_pubkey_hex();
        hex.as_bytes()
            .chunks(8)
            .map(|c| std::str::from_utf8(c).unwrap_or(""))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn init_result(&self) -> dto::InitResult {
        dto::InitResult {
            machine_pubkey: self.machine_pubkey_hex(),
            paired: self.guardian.is_some(),
            guardian: self.guardian.map(|g| g.to_hex()),
            subject: self.subject.map(|s| s.to_hex()),
            enforce_mode: self.mode.as_str().into(),
        }
    }

    pub fn pairing_state(&self) -> dto::PairingState {
        dto::PairingState {
            paired: self.guardian.is_some(),
            guardian_short: self.guardian.map(|g| g.to_hex()[..8].to_string()),
            guardian: self.guardian.map(|g| g.to_hex()),
            subject: self.subject.map(|s| s.to_hex()),
            relays: self.relays.clone(),
        }
    }

    /// The REAL pairing: pin the guardian from the `bunker://` link the parent
    /// pastes from MyCharter (pin_from_connect — percent-decode fix included,
    /// §5.1). The sole ward's subject defaults to the machine key itself: the
    /// deployed PWA emits subject-absent clauses (Stage-0 children carry no
    /// pubkey) and the device routes them to its sole ward, so any stable key
    /// works — the machine key needs no extra setup step. Pin once: refuse to
    /// silently re-point an existing pairing to a different guardian.
    pub fn pair(&mut self, bunker_uri: &str, now: u64) -> Result<dto::PairingState, String> {
        let machine = self.signer.pubkey();
        let subject = self.subject.unwrap_or(machine);
        let pairing = pin_from_connect(bunker_uri.trim(), machine, subject, now)
            .map_err(|e| pairing_error_text(&e).to_string())?;
        if let Some(existing) = self.guardian {
            if existing != pairing.guardian_pubkey {
                return Err("already paired to a different guardian".into());
            }
        }
        let json = serde_json::to_string_pretty(&pairing)
            .map_err(|e| format!("serialize pairing: {e}"))?;
        self.pairing
            .save(&json)
            .map_err(|e| format!("persist pairing: {e:?}"))?;
        self.guardian = Some(pairing.guardian_pubkey);
        self.subject = Some(pairing.subject_pubkey);
        self.relays = pairing.relays;
        self.paired_at = pairing.paired_at;
        // QR onboarding: a `token` param in the scanned URI is echoed on
        // STATUS for a short window so the guardian app can bind this machine
        // without anyone typing the device code. Survives a restart (file).
        self.pair_token = extract_token(bunker_uri).map(|t| {
            let expires = now + PAIR_TOKEN_WINDOW_SECS;
            save_pair_token(&self.base, &t, expires);
            (t, expires)
        });
        self.rebuild_relay();
        Ok(self.pairing_state())
    }

    /// Establish the guardian + sole ward directly (onboarding/test path; no
    /// relays, so no polling — [`Warden::pair`] is the production route). Pin
    /// once: refuse to silently re-point an existing pairing to a new guardian.
    pub fn set_pairing(&mut self, guardian_hex: &str, subject_hex: &str) -> Result<(), String> {
        let guardian =
            PubKey::from_hex(guardian_hex).map_err(|_| "bad guardian hex".to_string())?;
        let subject = PubKey::from_hex(subject_hex).map_err(|_| "bad subject hex".to_string())?;
        if let Some(existing) = self.guardian {
            if existing != guardian {
                return Err("already paired to a different guardian".into());
            }
        }
        let pairing = Pairing {
            guardian_pubkey: guardian,
            relays: Vec::new(),
            machine: self.signer.pubkey(),
            subject_pubkey: subject,
            audit_transparency: false,
            paired_at: 0,
        };
        let json = serde_json::to_string_pretty(&pairing).map_err(|e| format!("serialize: {e}"))?;
        self.pairing
            .save(&json)
            .map_err(|e| format!("persist pairing: {e:?}"))?;
        self.guardian = Some(guardian);
        self.subject = Some(subject);
        self.relays = Vec::new();
        // Test/onboarding-only path (no `now` param) — matches the paired_at:
        // 0 already persisted above; 0 disables the release-staleness guard
        // for this pairing, which is fine off the production `pair()` route.
        self.paired_at = 0;
        Ok(())
    }

    /// One slow-tick network round, split into lock-scoped phases so the
    /// network IO never runs under the global warden lock (a wedged relay
    /// costs a poll, never an enforcement tick): [`Warden::poll_prepare`] →
    /// (network) → [`Warden::poll_absorb`] → (network) →
    /// [`Warden::mark_status_emitted`]. [`poll_once_locked`] orchestrates.
    ///
    /// Grants/requests join when the broker lands (next increment).
    #[allow(clippy::type_complexity)]
    fn poll_prepare(
        &self,
        now: u64,
    ) -> Result<
        (
            std::sync::Arc<dyn WardenRelay>,
            u64,
            Option<std::sync::Arc<AndroidBroker>>,
            std::sync::Arc<tokio::runtime::Runtime>,
        ),
        String,
    > {
        match &self.relay {
            Some(r) => Ok((
                r.clone(),
                now.saturating_sub(POLL_LOOKBACK_SECS),
                self.broker.clone(),
                self.rt.clone(),
            )),
            None => Err(if self.guardian.is_none() {
                "not paired".into()
            } else {
                "no relays pinned".into()
            }),
        }
    }

    /// Mirror the sole ward's clauses into the machine-wide store the broker's
    /// `time_extend_eod` reads (the Linux single-ward layout) — rollback-safe
    /// (`put_clause` keeps the monotonic issuedAt floor).
    fn mirror_ward_clauses(&self) {
        let Some(subject) = self.subject else { return };
        let Ok(clauses) = self.child_clauses.clauses_for(&subject.to_hex()) else {
            return;
        };
        for (kind, json) in clauses {
            let issued_at = serde_json::from_str::<serde_json::Value>(&json)
                .ok()
                .and_then(|v| v.get("issuedAt").and_then(|x| x.as_u64()))
                .unwrap_or(0);
            let _ = self.machine_clauses.put_clause(kind, issued_at, &json);
        }
    }

    /// Ingest polled clauses (cursor = `now − 2d`, self-healing + idempotent —
    /// the per-(subject,kind) issuedAt floor makes re-delivery a no-op), then
    /// hand back the STATUS payload to publish, if one is due (change-or-60s
    /// throttle, I19).
    fn poll_absorb(
        &mut self,
        received: Vec<charter_transport::transport::ReceivedClause>,
        now: u64,
    ) -> (u32, u32, Option<StatusPayload>) {
        let mut seen = 0;
        let mut accepted = 0;
        for rc in received {
            seen += 1;
            if self.ingest_clause_event(&rc.clause, now).accepted {
                accepted += 1;
            }
        }
        let due = self.build_status(now).filter(|status| {
            charter_spine::status_emit::should_emit_status(
                self.last_status.as_ref(),
                status,
                STATUS_HEARTBEAT_SECS,
            )
        });
        (seen, accepted, due)
    }

    fn mark_status_emitted(&mut self, status: StatusPayload) {
        self.last_status = Some(status);
    }

    /// The ward's live STATUS payload, when enough state exists to report one.
    fn build_status(&self, now: u64) -> Option<StatusPayload> {
        let subject = self.subject?;
        let clauses = self.child_clauses.clauses_for(&subject.to_hex()).ok()?;
        let (schedule, budget) = resolve_effective(&clauses);
        let source = if schedule.is_some() || budget.is_some() {
            PolicySource::Guardian
        } else {
            PolicySource::Unconstrained
        };
        let (usage, extension) = (self.usage.as_ref(), self.extension.as_ref());
        // Before the first tick the ledgers are unloaded; report zeros rather
        // than skip — the heartbeat must beat even on an idle, fresh boot.
        let (used_today, remaining) = match (usage, extension) {
            (Some(u), Some(e)) => {
                let pooled = charter_spine::usage_pool::load_consolidated_from(
                    &self.child_clauses,
                    &subject.to_hex(),
                );
                let rem = charter_schedule::enforcer::compute_remaining(&EnforcerInputs {
                    stand_down: self.stand_down_now(now),
                    now_unix: now as i64,
                    schedule: schedule.as_ref(),
                    budget: budget.as_ref(),
                    usage: u,
                    extension: e,
                    consolidated: pooled.as_ref(),
                });
                (u.used_today(now as i64), rem)
            }
            _ => (
                0,
                charter_schedule::enforcer::compute_remaining(&EnforcerInputs {
                    stand_down: self.stand_down_now(now),
                    now_unix: now as i64,
                    schedule: schedule.as_ref(),
                    budget: budget.as_ref(),
                    usage: &UsageLedger::new(&self.ledger_tz, self.ledger_week_start, now as i64),
                    extension: &ExtensionLedger::new(&self.ledger_tz, now as i64),
                    consolidated: None,
                }),
            ),
        };
        let tz: chrono_tz::Tz = self.ledger_tz.parse().unwrap_or(chrono_tz::UTC);
        let mut status = charter_spine::status_emit::build_status(
            subject,
            self.signer.pubkey(),
            now,
            &remaining,
            used_today,
            None,
            charter_schedule::day_key(tz, now as i64),
            None,
            source,
        );
        // The union-rule input (B3): today's active-minutes journal, omitted
        // while empty rather than sent as an all-zero bitmap.
        if let Some(u) = self.usage.as_ref() {
            let m = u.minutes_today(now as i64);
            if !m.is_empty() {
                status.active_minutes_today = Some(m.to_b64url());
            }
        }
        // The onboarding echo — only within the post-pairing window.
        if let Some((token, expires)) = &self.pair_token {
            if now < *expires {
                status.pair_token = Some(token.clone());
            }
        }
        // Ride the device's app inventory so the guardian can pick apps by name.
        if !self.installed_apps.is_empty() {
            status.apps = Some(self.installed_apps.clone());
        }
        // Report our own version so the guardian can see update state (#44).
        if self.app_version_code > 0 {
            status.app_version_code = Some(self.app_version_code);
        }
        // …and, if our own update is stuck, WHY. Without this the phone retries
        // in silence and the guardian sees only an Update button that keeps
        // reappearing (two days lost, 2026-07-24..26).
        let health = self.install_health(now);
        if !health.is_empty() {
            status.update_health = serde_json::from_str(&health).ok();
        }
        // The account of the last window the guardian opened: they took the
        // lock off, so they get to see what came through it.
        if let Some(w) = &self.install_window {
            status.install_window = Some(w.clone());
        }
        // Per-bucket ("named time") progress, so the guardian's app can show
        // "Play: 22 of 60 used" instead of a blank. RAW meters — not
        // extra-adjusted — because this is the guardian's own account of what
        // was actually spent, unlike the ward's own view (which shows what
        // still binds after any grant she approved). Capped defensively at
        // MAX_STATUS_GROUPS, though a valid `buckets` clause can never
        // exceed it (mirrors charterd's STATUS group progress).
        if let (Some(body), Some(u)) = (self.buckets_clause(), self.usage.as_ref()) {
            status.groups = Some(
                body.buckets
                    .iter()
                    .take(charter_proto::MAX_STATUS_GROUPS)
                    .map(|b| charter_proto::GroupSpent {
                        id: b.id.clone(),
                        day_secs: u.app_bucket_today_secs(now as i64, &b.id),
                        week_secs: u.app_bucket_week_secs(now as i64, &b.id),
                    })
                    .collect(),
            );
        }
        // Out-of-hours use (spec 2026-08-03): the `alwaysavailable` clause
        // that opens a locked device only exists on Android, so this is the
        // ONE platform that ever stamps this — the mirror image of
        // `unrecognised_today_secs`, which is Linux-only (see that field's
        // own doc comment and `warden.rs`'s
        // `unrecognised_secs_and_user_installed_are_absent_from_android_status`
        // test). Absent while zero, same "absent, not a zero" posture as
        // every other STATUS meter on this surface.
        if let Some(u) = self.usage.as_ref() {
            let today = u.out_of_hours_today_secs();
            if today > 0 {
                status.out_of_hours_today_secs = Some(today);
            }
            let week = u.out_of_hours_week_secs();
            if week > 0 {
                status.out_of_hours_week_secs = Some(week);
            }
            let nights = u.out_of_hours_nights_week();
            if nights > 0 {
                status.out_of_hours_nights_week = Some(nights);
            }
        }
        // Boots this phone went through with no warden running (S1). Safe
        // mode leaves no other trace: nothing ran, so nothing was recorded,
        // and the phone comes back enforcing as though the holiday never
        // happened. This is the one line that says otherwise.
        status.enforcement_gap = self.enforcement_gap();
        Some(status)
    }

    pub fn set_enforce_mode(&mut self, mode: &str) {
        self.mode = EnforceMode::parse(mode);
    }

    /// The last emitted STATUS (test observability only).
    #[cfg(test)]
    pub fn last_status_for_tests(&self) -> Option<&charter_proto::StatusPayload> {
        self.last_status.as_ref()
    }

    /// Authenticate + store one CLAUSE event (the schedule/budget the guardian
    /// signed). Rollback-protected per (subject, kind) by the store (I7).
    pub fn ingest_clause(&mut self, event_json: &str, now: u64) -> dto::ClauseResult {
        let event: NostrEvent = match serde_json::from_str(event_json) {
            Ok(e) => e,
            Err(_) => {
                return dto::ClauseResult {
                    accepted: false,
                    kind: None,
                    subject: None,
                    reason: "malformed event json".into(),
                }
            }
        };
        self.ingest_clause_event(&event, now)
    }

    /// The shared verify+store path — fed by both the JNI test/onboarding entry
    /// (`ingest_clause`) and the relay poll (`poll_once`).
    fn ingest_clause_event(&mut self, event: &NostrEvent, now: u64) -> dto::ClauseResult {
        let reject = |reason: &str| dto::ClauseResult {
            accepted: false,
            kind: None,
            subject: None,
            reason: reason.into(),
        };

        let guardian = match self.guardian {
            Some(g) => g,
            None => return reject("not paired"),
        };
        if event.kind != CHARTER_DEVICE_CLAUSE {
            return reject("wrong event kind");
        }
        let payload = match ClausePayload::from_json(&event.content) {
            Ok(p) => p,
            Err(_) => return reject("malformed clause payload"),
        };
        // Absent subject routes to the sole ward (contract single-child rule).
        let subject = match payload.subject.or(self.subject) {
            Some(s) => s,
            None => return reject("no subject and no sole ward"),
        };
        let kind = payload.kind;
        let store_key = kind.store_key();
        let subject_hex = subject.to_hex();

        let prev = self
            .child_clauses
            .highest_issued_at(&subject_hex, store_key)
            .ok()
            .flatten();

        match charter_verify::verify_clause(event, &guardian, kind, prev, now) {
            Ok(vc) => {
                let body_json = serde_json::to_string(vc.body()).unwrap_or_default();
                match self.child_clauses.put_child_clause(
                    &subject_hex,
                    store_key,
                    vc.issued_at(),
                    &body_json,
                ) {
                    Ok(true) => dto::ClauseResult {
                        accepted: true,
                        kind: Some(format!("{kind:?}")),
                        subject: Some(subject_hex),
                        reason: "stored".into(),
                    },
                    Ok(false) => reject("rollback (stale issuedAt)"),
                    Err(e) => reject(&format!("store error: {e:?}")),
                }
            }
            Err(e) => reject(&format!("verify failed: {e:?}")),
        }
    }

    /// One enforcement tick for the sole ward. Returns the decision the Kotlin
    /// service applies. Inert until a charter exists for the ward (I17).
    pub fn tick(
        &mut self,
        active_subject_hex: Option<&str>,
        screen_interactive: bool,
        now: i64,
        foreground_pkg: Option<&str>,
    ) -> Vec<dto::ChildDecision> {
        let subject = match self.subject {
            // No ward (never paired, or released): emit an explicit inert +
            // UNLOCKED decision so the Kotlin enforcer actively lifts every
            // restriction, unsuspends, and hides the lock. Returning empty here
            // would leave a just-released device stuck under its old DPM
            // restrictions forever (nothing would clear them).
            None => {
                return vec![dto::ChildDecision {
                    subject: None,
                    locked: false,
                    reason: "none".into(),
                    effects: Vec::new(),
                    remaining_secs: -1,
                    configured: false,
                    enforce_mode: self.mode.as_str().into(),
                }]
            }
            Some(s) => s,
        };
        let subject_hex = subject.to_hex();

        let clauses = match self.child_clauses.clauses_for(&subject_hex) {
            Ok(c) => c,
            // I22: an unreadable store preserves prior state, skips accrual.
            Err(_) => return self.hold_decision(&subject_hex),
        };
        let (schedule, budget) = resolve_effective(&clauses);
        // The stand-down is consulted BEFORE the inert early-return: a paired
        // ward with no time rules yet is still stoppable ("an unbounded
        // charter is capped too"). Going inert first while STATUS reported
        // "standdown" made the guardian's app claim a lock the phone never
        // enforced.
        let stand_down = self.stand_down_now(now as u64);
        // A valid `buckets` clause is its OWN liveness signal, exactly like
        // schedule/budget/stand-down: MyCharter signs only the dimensions that
        // actually changed, so "Play is an hour a day" with no schedule/budget
        // at all is a perfectly ordinary buckets-only charter — not a rare
        // edge case. Read ONCE here and reused below (the tz/week-start
        // precedence and the foreground-credit block) so one tick's decision
        // is internally consistent even if the clause store were to change
        // mid-tick.
        let buckets = self.buckets_clause();
        if schedule.is_none() && budget.is_none() && stand_down.is_none() && buckets.is_none() {
            // Inert: no charter for this ward — configured=false, so Kotlin
            // applies no restrictions (I17). Does NOT latch ever_configured.
            return vec![dto::ChildDecision {
                subject: Some(subject_hex),
                locked: false,
                reason: "none".into(),
                effects: Vec::new(),
                remaining_secs: -1,
                configured: false,
                enforce_mode: self.mode.as_str().into(),
            }];
        }
        // A charter exists for this ward — latch it so a later unreadable tick
        // keeps the install-lockdown up (I22). A stand-down alone does not
        // latch: it lapses by tonight, and the latch never lets go. A
        // buckets-only charter is a genuine, signed charter too (D3-equivalent:
        // "Play is an hour a day" and nothing else) — it latches exactly like
        // schedule/budget.
        if schedule.is_some() || budget.is_some() || buckets.is_some() {
            self.ever_configured = true;
        }

        // Key the usage ledger's day/week keys — screen AND bucket meters
        // share the one ledger — on: the BUDGET clause's tz + weekStart (the
        // cap authority) when a budget clause is in force; else the BUCKETS
        // clause's own tz + weekStart, so a buckets-only family's chosen
        // weekStart is actually honoured instead of silently defaulting to
        // Monday; else the ordinary enforcement tz + Monday (fully
        // unconstrained, or schedule/budget with no buckets). Mirrors
        // charterd's `MultiChildEnforcer::tick_attributed` `usage_tz`
        // precedence exactly — see its comment for why budget wins on the
        // rare family that sets both differently. The SCHEDULE clause's own
        // window evaluation is untouched by this: `evaluate_grant_schedule`
        // reads the schedule's own embedded `tz` field directly, never this
        // ledger tz.
        let (tz, week_start) = match (budget.as_ref(), buckets.as_ref()) {
            (Some(b), _) => (b.tz.clone(), b.week_start.unwrap_or(WeekStart::Mon)),
            (None, Some(bk)) => (bk.tz.clone(), bk.week_start.unwrap_or(WeekStart::Mon)),
            (None, None) => (
                enforcement_tz_of(schedule.as_ref(), budget.as_ref())
                    .name()
                    .to_string(),
                WeekStart::Mon,
            ),
        };
        self.ensure_ledgers(&tz, week_start, now);

        // Credit the interval on the PRIOR lock state, then decide.
        let elapsed = self.elapsed_since(now);
        let activity = if self.enforcer.is_locked() {
            Activity::FrozenByCharter
        } else if active_subject_hex == Some(subject_hex.as_str()) && screen_interactive {
            Activity::Active
        } else {
            Activity::Idle
        };
        if let Some(u) = self.usage.as_mut() {
            u.credit(now, activity, elapsed);
        }

        // The named-times bucket the foreground app belongs to, if any, is
        // credited ALONGSIDE the ordinary screen credit above — an hour of
        // Minecraft is an hour of screen time AND an hour of Play, so a
        // capped bucket never makes time free the way the learning bucket
        // does (mirrors `MultiChildEnforcer::tick_attributed`, Linux's twin).
        // Only while genuinely ACTIVE (screen interactive, this ward is the
        // foreground session, not frozen by a live lock) — the SAME single
        // foreground-package probe Kotlin already made for the screen credit;
        // no second probe. Fail-safe: no bucket clause, no match, or a
        // paused/malformed clause credits nothing (`bucket_for_app` already
        // fails safe on that internally).
        if activity == Activity::Active {
            if let Some(fg) = foreground_pkg {
                if let Some(bk) = buckets.as_ref() {
                    if let Some(b) = charter_schedule::bucket_for_app(bk, fg) {
                        let id = b.id.clone();
                        if let Some(u) = self.usage.as_mut() {
                            u.credit_app_bucket(now, &id, elapsed);
                        }
                    }
                }
            }
        }

        // Drain verified time-extensions (broker enactor deposits) into the
        // ONE enforcing ledger — idempotent by reqId (I13). Newly-applied
        // minutes ride the decision as a Granted effect so the child hears
        // the yes even away from the lock screen. A per-group grant
        // (`bucket_id` set) credits that bucket's own pool; the ordinary
        // whole-device routing credits the schedule/budget pool the enactor
        // decided on (mirrors charterd's drain, runtime.rs ~1467).
        let mut granted_minutes: u32 = 0;
        {
            let pending: Vec<_> = self
                .inbox
                .lock()
                .map(|mut v| v.drain(..).collect())
                .unwrap_or_default();
            if let Some(e) = self.extension.as_mut() {
                for p in pending {
                    let applied = match (&p.bucket_id, p.dim) {
                        (Some(bucket_id), _) => {
                            e.apply_bucket(p.at, &p.req_id, p.minutes, bucket_id)
                        }
                        (None, Some(dim)) => e.apply(p.at, &p.req_id, p.minutes, dim),
                        // Malformed deposit (neither set) — nothing to apply.
                        (None, None) => false,
                    };
                    if applied {
                        granted_minutes += p.minutes as u32;
                    }
                }
            }
        }

        // A gift the guardian gave without being asked rides the SAME pool as a
        // granted ask — to the enforcer they are the same thing, so the lock
        // math, the time-left readout and STATUS all need no new cases. The
        // clause is re-read every tick; the ledger's idempotency (by the gift's
        // own id) is what makes that apply exactly once.
        if let Some((gift_id, minutes, group_id)) = self.gift_now(now as u64) {
            // A named-times gift (`groupId` set, and the bucket still exists in
            // the ward's CURRENT `buckets` clause) tops up that bucket's own
            // pool. An unknown/deleted group — or no group at all — falls back
            // to the ordinary whole-device routing, exactly like a groupId-less
            // gift always has: "Mum gave me time and nothing happened" is the
            // one failure this must never produce (mirrors charterd's
            // `gift_route`).
            let live_bucket = group_id.as_deref().filter(|gid| {
                buckets
                    .as_ref()
                    .is_some_and(|b| b.buckets.iter().any(|bk| bk.id == *gid))
            });
            if let Some(bucket_id) = live_bucket {
                if let Some(e) = self.extension.as_mut() {
                    if e.apply_bucket(now, &gift_id, minutes, bucket_id) {
                        granted_minutes += minutes as u32;
                    }
                }
            } else {
                // Route like the shade's ask does: minutes charged to the
                // budget pool while a CLOSED window is what's holding them out
                // would never unlock anything (M7).
                let dim = match schedule.as_ref() {
                    Some(s)
                        if matches!(
                            charter_schedule::evaluate_grant_schedule(s, now),
                            charter_schedule::ScheduleStatus::Locked { .. }
                        ) =>
                    {
                        charter_schedule::Dimension::Schedule
                    }
                    _ => charter_schedule::Dimension::Budget,
                };
                if let Some(e) = self.extension.as_mut() {
                    if e.apply(now, &gift_id, minutes, dim) {
                        granted_minutes += minutes as u32;
                    }
                }
            }
        }

        // Out-of-window, a granted schedule extension burns wall-clock (the
        // fail-open fix: "+30m past curfew" must count down and re-lock, not
        // freeze at 30:00 until midnight).
        if let Some(e) = self.extension.as_mut() {
            charter_schedule::burn_schedule_extension(e, schedule.as_ref(), now, elapsed);
        }

        // Ledgers were just ensured; if somehow absent, hold rather than panic.
        let (usage, extension) = match (self.usage.as_ref(), self.extension.as_ref()) {
            (Some(u), Some(e)) => (u, e),
            _ => return self.hold_decision(&subject_hex),
        };
        // The broker-verified cross-device usage view (USAGE_SYNC, B3): the
        // budget draws down the POOLED spend; absent/stale → local-only.
        let pooled =
            charter_spine::usage_pool::load_consolidated_from(&self.child_clauses, &subject_hex);
        let effs = self.enforcer.tick(&EnforcerInputs {
            stand_down,
            now_unix: now,
            schedule: schedule.as_ref(),
            budget: budget.as_ref(),
            usage,
            extension,
            consolidated: pooled.as_ref(),
        });

        self.persist_ledgers();
        self.last_tick_unix = Some(now);

        let mut decision = self.map_effects(subject_hex, effs);
        if granted_minutes > 0 {
            decision.effects.push(dto::Effect::Granted {
                minutes: granted_minutes,
            });
        }
        if self.take_fresh_denial() {
            decision.effects.push(dto::Effect::Denied);
        }
        // An open FULL-scope break-glass window lifts the shade for its
        // duration — the ward's emergency valve outranks the schedule, and
        // the guardian already knows (audit spooled at activation). When it
        // expires the very next tick re-locks: nothing is forgiven, only
        // deferred.
        if self.break_glass_open(now as u64) {
            decision.locked = false;
            decision.reason = "breakglass".into();
        }

        // Out-of-hours (spec 2026-08-03): the device is locked, the
        // foreground app is one the family agreed is open at any hour, and
        // she is actually looking at it.
        //
        // Requires the elapsed interval to have been locked at BOTH ends —
        // `activity == Activity::FrozenByCharter` (the PRIOR state, checked
        // near the top of this function, ~1815, the same basis `used_today_secs`
        // was just credited on) AND `decision.locked` (the state after THIS
        // tick's `self.enforcer.tick()` decided). On an ordinary tick where
        // the device was already locked, the two agree and this is a
        // no-op distinction. On the unlocked→locked TRANSITION tick (a
        // schedule window closing, a budget exhausting, an extension
        // expiring), they disagree: `activity` was Active, so `elapsed`
        // already went to the budget above, and crediting it here too would
        // double-count the same seconds — worse, it could flip
        // `out_of_hours_nights_week` from 0 to 1 for a ward who stopped the
        // instant the lock landed, fabricating a night that never happened.
        // Requiring both ends makes a transition tick belong to the budget
        // and never to this counter, matching the attribution basis the
        // budget already uses.
        //
        // Accepted consequence, not fixed: the REVERSE transition
        // (locked→unlocked, e.g. break-glass opening) credits that one
        // interval to NEITHER counter — a one-tick under-report, consistent
        // with the "a floor, not a total" precedent `unrecognised_today_secs`
        // already sets.
        //
        // `screen_interactive` is load-bearing, not a nicety. A sleep timer
        // stops the audio but leaves the app in the foreground, so counting
        // regardless of screen state would report eight hours for a
        // thirty-minute session and make the guardian's line useless in
        // exactly the case it exists for.
        //
        // `decision.locked` / `decision.reason` are used verbatim — the SAME
        // values this tick's decision reports, read AFTER the break-glass
        // override just above so an open break-glass window (which lifts the
        // shade) credits nothing here either. Deliberately not re-derived
        // from `self.enforcer` independently: `always_available_view` itself
        // gates on "schedule"/"budget" only, so a standdown or malformed lock
        // credits nothing regardless.
        if activity == Activity::FrozenByCharter && decision.locked && screen_interactive {
            if let Some(fg) = foreground_pkg {
                // Borrow order: resolve the open set into an OWNED
                // `Vec<String>` (`&self`) before touching `self.usage`
                // (`&mut self`) below.
                let open: Vec<String> = serde_json::from_str::<serde_json::Value>(
                    &self.always_available_view(now as u64, decision.locked, &decision.reason),
                )
                .ok()
                .and_then(|v| serde_json::from_value(v["open"].clone()).ok())
                .unwrap_or_default();
                if open.iter().any(|p| p == fg) {
                    if let Some(u) = self.usage.as_mut() {
                        u.mark_out_of_hours(elapsed);
                    }
                    // Persist again (deduped on content-equality by
                    // `persist_ledgers`) so this tick's credit reaches disk
                    // the same tick as every other ledger mutation above,
                    // rather than waiting for the next tick.
                    self.persist_ledgers();
                }
            }
        }

        vec![decision]
    }

    /// The `charterTimeLeft` view (I10: unknown → locked).
    pub fn time_left(&self, now: i64) -> dto::TimeLeftView {
        let subject = match self.subject {
            Some(s) => s,
            None => return dto::TimeLeftView::unknown(),
        };
        let clauses = match self.child_clauses.clauses_for(&subject.to_hex()) {
            Ok(c) => c,
            Err(_) => return dto::TimeLeftView::unknown(),
        };
        let (schedule, budget) = resolve_effective(&clauses);
        // Same order as the tick: a standing stand-down means there IS a view
        // to report, even for a ward with no time rules. A valid `buckets`
        // clause is its own liveness signal too — tick()'s fourth condition,
        // carried over here after the round-1 review found this early-return
        // predated it: a buckets-only ward ("Play is an hour a day", nothing
        // else) rendered as `known: false` ("unmanaged/unknown") on the
        // mirror/widget even though it is a perfectly ordinary, signed
        // charter. `compute_remaining` below already tolerates
        // schedule/budget both being `None` (reports unbounded/unlocked at
        // the whole-device level, which is correct — spending a bucket never
        // locks the device), so admitting buckets-only wards past this gate
        // needs no other change.
        let stand_down = self.stand_down_now(now as u64);
        let buckets = self.buckets_clause();
        if schedule.is_none() && budget.is_none() && stand_down.is_none() && buckets.is_none() {
            return dto::TimeLeftView::unknown();
        }
        let (usage, extension) = match (self.usage.as_ref(), self.extension.as_ref()) {
            (Some(u), Some(e)) => (u, e),
            _ => return dto::TimeLeftView::unknown(),
        };
        let pooled = charter_spine::usage_pool::load_consolidated_from(
            &self.child_clauses,
            &subject.to_hex(),
        );
        let rem = charter_schedule::enforcer::compute_remaining(&EnforcerInputs {
            stand_down,
            now_unix: now,
            schedule: schedule.as_ref(),
            budget: budget.as_ref(),
            usage,
            extension,
            consolidated: pooled.as_ref(),
        });
        dto::TimeLeftView {
            known: true,
            locked: rem.locked,
            reason: reason_str(rem.reason),
            effective_secs: rem.effective_secs,
            schedule_secs: rem.schedule_secs,
            budget_secs: rem.budget_secs,
            extension_secs: rem.extension_secs,
            next_open_secs: rem.next_open_secs,
        }
    }

    /// The child-facing lock copy. tz-safe relative phrasing for v1 (the full
    /// clause-tz "back at 07:00 tomorrow" formatting lands with lock_info.rs).
    pub fn lock_info(&self, now: i64) -> dto::LockInfo {
        let tl = self.time_left(now);
        let dormant =
            charter_spine::enforcer_runtime::schedule_is_dormant(self.schedule_now().as_ref());
        let (title, come_back) = match tl.reason.as_str() {
            // A dormant device — off until a guardian opens it — locks as
            // "schedule" like any closed window, because the gift and ask
            // routing key off that reason and a fourth reason would send the
            // grant to the budget pool, which cannot open a schedule lock. The
            // WORDS differ: it has no allowed hours to be outside of, and no
            // "come back in …" to offer, so promising one would be a lie the
            // device can never make good on.
            "schedule" if dormant => {
                let (t, d) =
                    charter_spine::enforcer_runtime::lock_message_for(LockReason::Schedule, true);
                (t.to_string(), d.to_string())
            }
            "schedule" => (
                "Outside allowed hours".to_string(),
                tl.next_open_secs
                    .map(|s| format!("You can come back in {}.", humanize(s)))
                    .unwrap_or_default(),
            ),
            "budget" => ("Time's up for today".to_string(), String::new()),
            // The shared child-facing copy, verbatim (enforcer_runtime doc:
            // "never re-derived platform-side"). A stood-down ward once saw a
            // locked shade headlined "Unlocked" — this arm was missing.
            "standdown" => {
                let (t, d) = charter_spine::enforcer_runtime::lock_message(LockReason::StandDown);
                (t.to_string(), d.to_string())
            }
            "malformed" => (
                "Locked — setup needs attention".to_string(),
                "Ask your guardian.".to_string(),
            ),
            "unknown" => (
                "Locked".to_string(),
                "Waiting to hear from your guardian.".to_string(),
            ),
            _ => ("Unlocked".to_string(), String::new()),
        };
        let used_line = self.used_line(now);
        dto::LockInfo {
            locked: tl.locked,
            reason: tl.reason,
            title,
            come_back,
            used_line,
            dormant,
        }
    }

    // ---- internals --------------------------------------------------------

    fn used_line(&self, now: i64) -> String {
        match (self.usage.as_ref(), self.budget_now()) {
            (Some(u), Some(b)) => {
                let used = u.used_today(now);
                let cap = b.daily_minutes.map(|m| m as u64 * 60);
                // Sub-minute usage would render as raw seconds ("0s used
                // today") on the ward-facing lock screen — omit it below one
                // minute, matching the spine's lock_info wording (issue #41).
                match (cap, used >= 60) {
                    (Some(c), true) => format!(
                        "up to {} a day — {} used today",
                        humanize(c as i64),
                        humanize(used as i64)
                    ),
                    (Some(c), false) => format!("up to {} a day", humanize(c as i64)),
                    (None, true) => format!("{} used today", humanize(used as i64)),
                    (None, false) => String::new(),
                }
            }
            _ => String::new(),
        }
    }

    fn budget_now(&self) -> Option<GrantBudget> {
        let subject = self.subject?;
        let clauses = self.child_clauses.clauses_for(&subject.to_hex()).ok()?;
        resolve_effective(&clauses).1
    }

    /// Which packages may keep playing through the lock, and how long is left.
    ///
    /// Returns `{"exempt":[…],"secsLeft":n|null}`. `exempt` is what the app gate
    /// subtracts from the suspend set; `secsLeft` is what the shade counts down
    /// under `grace`, because cutting out silently at minute 30 is the same
    /// surprise in a smaller box.
    ///
    /// Fails CLOSED at every turn (no clause, unparseable, unlocked, no audio):
    /// an empty exempt set is exactly the behaviour before this clause existed.
    pub fn listening_view(&mut self, now: u64, locked: bool, audio_playing: bool) -> String {
        const EMPTY: &str = r#"{"exempt":[],"secsLeft":null}"#;
        if !locked {
            // The spell is over; the next one starts its own clock.
            self.listening_lock_started = None;
            return EMPTY.to_string();
        }
        let started = *self.listening_lock_started.get_or_insert(now);
        let Some(subject) = self.subject else {
            return EMPTY.to_string();
        };
        let body = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::Listening.store_key(),
            )
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_str::<charter_proto::ListeningBody>(&b).ok());
        let Some(body) = body else {
            return EMPTY.to_string();
        };

        let mut exempt: Vec<String> = Vec::new();
        let mut secs_left: Option<u64> = None;
        for pkg in &body.apps {
            match body.verdict(pkg, audio_playing, started, now) {
                charter_proto::ListeningVerdict::Stop => {}
                charter_proto::ListeningVerdict::Continue => exempt.push(pkg.clone()),
                charter_proto::ListeningVerdict::Grace { secs_left: s } => {
                    exempt.push(pkg.clone());
                    // The shortest remaining wins: they all began at the same
                    // lock, so this is simply "how long the listening has left".
                    secs_left = Some(secs_left.map_or(s, |cur: u64| cur.min(s)));
                }
            }
        }
        serde_json::json!({ "exempt": exempt, "secsLeft": secs_left }).to_string()
    }

    /// Which packages may be OPENED right now, though the device is locked.
    ///
    /// Returns `{"open":[…]}` — what the app gate subtracts from the lock
    /// posture, and what the shade offers as its Open row.
    ///
    /// Deliberately unlike [`Warden::listening_view`] in two ways. It takes
    /// `&self`: there is no first-sight instant to pin, because there is no
    /// grace running down — an entry's `untilUnix` is absolute and answers for
    /// itself. And it asks no question about audio: this clause is about
    /// STARTING something, not about letting one finish.
    ///
    /// Fails CLOSED at every turn (no clause, unparseable, unlocked, a lock
    /// reason that does not exempt): an empty set is exactly the behaviour
    /// before this clause existed.
    pub fn always_available_view(&self, now: u64, locked: bool, lock_reason: &str) -> String {
        const EMPTY: &str = r#"{"open":[]}"#;
        if !locked {
            return EMPTY.to_string();
        }
        let Some(subject) = self.subject else {
            return EMPTY.to_string();
        };
        let body = self
            .child_clauses
            .get_child_clause(
                &subject.to_hex(),
                charter_proto::ClauseKind::AlwaysAvailable.store_key(),
            )
            .ok()
            .flatten()
            .and_then(|b| serde_json::from_str::<charter_proto::AlwaysAvailableBody>(&b).ok());
        let Some(body) = body else {
            return EMPTY.to_string();
        };
        serde_json::json!({ "open": body.exempt_packages(lock_reason, now) }).to_string()
    }

    fn schedule_now(&self) -> Option<GrantSchedule> {
        let subject = self.subject?;
        let clauses = self.child_clauses.clauses_for(&subject.to_hex()).ok()?;
        resolve_effective(&clauses).0
    }

    fn ensure_ledgers(&mut self, tz: &str, week_start: WeekStart, now: i64) {
        if self.usage.is_none() {
            let restored = self
                .usage_store
                .load_snapshot()
                .ok()
                .flatten()
                .and_then(|s| UsageLedger::from_snapshot(&s));
            let mut u = restored.unwrap_or_else(|| UsageLedger::new(tz, week_start, now));
            u.reconcile(tz, week_start, now);
            self.usage = Some(u);
        } else if self.ledger_tz != tz || self.ledger_week_start != week_start {
            if let Some(u) = self.usage.as_mut() {
                u.reconcile(tz, week_start, now);
            }
        }
        if self.extension.is_none() {
            let restored = self
                .extension_store
                .load_snapshot()
                .ok()
                .flatten()
                .and_then(|s| ExtensionLedger::from_snapshot(&s));
            self.extension = Some(restored.unwrap_or_else(|| ExtensionLedger::new(tz, now)));
        }
        self.ledger_tz = tz.to_string();
        self.ledger_week_start = week_start;
    }

    fn elapsed_since(&self, now: i64) -> u64 {
        match self.last_tick_unix {
            Some(prev) => (now - prev).clamp(0, MAX_TICK_ELAPSED_SECS) as u64,
            None => 0,
        }
    }

    /// Write the ledgers, but only where the bytes actually moved.
    ///
    /// The loop ticks a few times a second while the ward is on their phone and
    /// kept re-writing both snapshots every single time — including through the
    /// long stretches where there is provably nothing to record: a dark screen,
    /// a locked ward and an idle one all credit zero (`Activity::counts()` is
    /// false, so `credit_bucket` returns before touching a field), which means
    /// the snapshot it just serialised is byte-identical to the one already on
    /// disk. That was tens of thousands of pointless flash writes a day on a
    /// phone doing nothing, and flash writes cost both battery and wear.
    ///
    /// Comparing against what we last wrote is deliberately the *whole* rule —
    /// no timer, no coalescing window. A change is still written on the very
    /// tick it happens, so a kill -9 can never lose accrued time, and the ward
    /// can never gain minutes from a crash. The saving comes entirely from the
    /// writes that were never worth making.
    fn persist_ledgers(&mut self) {
        if let Some(u) = self.usage.as_ref() {
            let snap = u.snapshot();
            if self.usage_written.as_deref() != Some(snap.as_str()) {
                if self.usage_store.save_snapshot(&snap).is_ok() {
                    self.usage_written = Some(snap);
                } else {
                    // A failed write must not be remembered as written, or the
                    // comparison would suppress every retry and the ledger
                    // would never reach the disk again.
                    self.usage_written = None;
                }
            }
        }
        if let Some(e) = self.extension.as_ref() {
            let snap = e.snapshot();
            if self.extension_written.as_deref() != Some(snap.as_str()) {
                if self.extension_store.save_snapshot(&snap).is_ok() {
                    self.extension_written = Some(snap);
                } else {
                    self.extension_written = None;
                }
            }
        }
    }

    /// A tick that must not accrue or transition (store read / ledger failure):
    /// hold the current lock state, no edge effects, but PRESERVE the charter
    /// signal so the install-lockdown is not torn down (I22).
    fn hold_decision(&self, subject_hex: &str) -> Vec<dto::ChildDecision> {
        vec![dto::ChildDecision {
            subject: Some(subject_hex.to_string()),
            locked: self.enforcer.is_locked(),
            reason: if self.enforcer.is_locked() {
                "hold".into()
            } else {
                "none".into()
            },
            effects: Vec::new(),
            remaining_secs: 0,
            configured: self.ever_configured,
            enforce_mode: self.mode.as_str().into(),
        }]
    }

    /// Suspend and lock are driven LEVEL-triggered by the Kotlin enforcer from
    /// `locked` + `enforce_mode` (not from edges), so this only surfaces the
    /// notification/audit effects. Audit always fires (the parent-side signal,
    /// I18/I19); warnings fire only in Enforce mode.
    fn map_effects(&self, subject_hex: String, effs: Vec<EnforcerEffect>) -> dto::ChildDecision {
        let mut out = Vec::new();
        let mut locked = self.enforcer.is_locked();
        let mut reason = if locked {
            "hold".to_string()
        } else {
            "none".to_string()
        };
        let mut remaining_secs = -1i64;

        for e in effs {
            match e {
                EnforcerEffect::ShowLock(r) => reason = reason_str(Some(r)),
                EnforcerEffect::Warn(w) => {
                    if self.mode == EnforceMode::Enforce {
                        out.push(dto::Effect::Warn {
                            level: match w {
                                WarnLevel::Ten => "ten".into(),
                                WarnLevel::One => "one".into(),
                            },
                        });
                    }
                }
                EnforcerEffect::Audit(k) => out.push(dto::Effect::Audit {
                    outcome: audit_str(k),
                }),
                EnforcerEffect::TimeLeft(rem) => {
                    locked = rem.locked;
                    remaining_secs = rem.effective_secs;
                    reason = if rem.locked {
                        reason_str(rem.reason)
                    } else {
                        "none".into()
                    };
                }
                EnforcerEffect::Freeze(_) | EnforcerEffect::Thaw(_) | EnforcerEffect::HideLock => {}
            }
        }

        dto::ChildDecision {
            subject: Some(subject_hex),
            locked,
            reason,
            effects: out,
            remaining_secs,
            configured: true,
            enforce_mode: self.mode.as_str().into(),
        }
    }
}

/// The effective schedule + budget from the cached clauses (child_policy.rs
/// resolve_effective, replicated): a present-but-unparseable **schedule**
/// fail-SAFES to paused (locks); an unparseable **budget** fail-OPENS to no cap.
fn resolve_effective(clauses: &[(u16, String)]) -> (Option<GrantSchedule>, Option<GrantBudget>) {
    use charter_proto::ClauseKind;
    let mut schedule = None;
    let mut budget = None;
    for (kind, json) in clauses {
        if *kind == ClauseKind::Schedule.store_key() {
            schedule = Some(
                serde_json::from_str::<GrantSchedule>(json)
                    .unwrap_or_else(|_| fail_safe_schedule()),
            );
        } else if *kind == ClauseKind::Budget.store_key() {
            budget = serde_json::from_str::<GrantBudget>(json).ok();
        }
    }
    (schedule, budget)
}

/// `paused` blocks all time regardless of tz, so the ward locks.
fn fail_safe_schedule() -> GrantSchedule {
    GrantSchedule {
        v: 1,
        tz: "UTC".into(),
        paused: Some(true),
        weekly: Default::default(),
        overrides: None,
        issued_at: 0,
    }
}

/// Build one named bucket's ward-facing view from its cap and this tick's
/// usage — mirrors charterd's `bucket_view` (`runtime.rs`) field-for-field so
/// the two platforms' mirror surfaces read identically. `day_used`/`week_used`
/// implicitly ride inside `spent` for the wall math; `day_used` alone is also
/// reported RAW (never extra-adjusted) as `used_seconds`, so "Play: 22 of 60
/// used" always shows what was actually spent. `spent` (the caller-supplied
/// [`charter_schedule::BucketSpent`]) is ALSO raw — `extra` is added onto the
/// CAP instead, so the remainders count down honestly through a granted
/// surplus rather than freezing.
///
/// M-4 (2026-08-03, hardware-proven): the previous shape subtracted `extra`
/// from `spent` before judging the caps, which saturates at zero for as long
/// as `spent < extra` — reading "still exactly at the base cap" for the
/// entire time the ward is spending the granted surplus ("15m of 15m left"
/// frozen through 18m24s of continuous play after a 30-minute gift on a spent
/// 15-minute bucket, even though enforcement correctly allowed the extra
/// time). Adding `extra` to the cap instead means `remaining` moves on every
/// tick from the moment the grant lands, reaching 0 only once the FULL
/// cap-plus-extra has actually been spent.
///
/// A paused set caps nothing: day fields read `0` (paired with
/// `capped: false`) and week fields read `-1`, the same "not set" sentinel
/// used everywhere else, so a week field is never misread as "the week is
/// used up" when nothing is actually capped. `used_seconds`/`day_used` still
/// shows whatever the meter last read — the LAST-KNOWN picture, not a live
/// one: `bucket_for_app` never attributes a tick to a paused bucket, so the
/// meter itself freezes rather than keeps counting while paused.
fn bucket_view(
    b: &charter_schedule::AppBucket,
    paused: bool,
    day_used: u64,
    spent: charter_schedule::BucketSpent,
    extra: u64,
) -> dto::BucketView {
    // Extras lift BOTH walls today by the SAME shared pool (one grant per
    // bucket, not per-axis — `ExtensionLedger::apply_bucket`), so both caps
    // grow by `extra` before the remainder is judged.
    let day_limit = b.daily_minutes.map(|m| u64::from(m) * 60 + extra);
    let week_limit = b.weekly_minutes.map(|m| u64::from(m) * 60 + extra);
    let week_remaining = week_limit.map(|lim| lim.saturating_sub(spent.week_secs));
    let binding_remaining = day_limit
        .map(|lim| lim.saturating_sub(spent.day_secs))
        .into_iter()
        .chain(week_remaining)
        .min()
        .unwrap_or(0);
    dto::BucketView {
        id: b.id.clone(),
        label: b.label.clone(),
        used_seconds: day_used,
        // The BINDING wall (whichever axis is tightest), not just the day
        // axis: a weekly-only bucket has no day limit at all, and showing the
        // day figure here would read "0m left / capped" while real
        // week-minutes remain (colliding with `limit_seconds == 0` meaning
        // "paused"). `day_limit.or(week_limit)` so a day-only bucket keeps
        // showing its day wall unchanged, and a weekly-only bucket shows its
        // real (week) wall instead of a phantom zero.
        limit_seconds: if paused {
            0
        } else {
            day_limit.or(week_limit).unwrap_or(0)
        },
        remaining_seconds: if paused { 0 } else { binding_remaining },
        week_limit_seconds: if paused {
            -1
        } else {
            week_limit.map(|s| s as i64).unwrap_or(-1)
        },
        week_remaining_seconds: if paused {
            -1
        } else {
            week_remaining.map(|s| s as i64).unwrap_or(-1)
        },
        spent: !paused && binding_remaining == 0,
        capped: !paused,
    }
}

fn reason_str(r: Option<LockReason>) -> String {
    match r {
        Some(LockReason::Schedule) => "schedule",
        Some(LockReason::Budget) => "budget",
        Some(LockReason::Malformed) => "malformed",
        Some(LockReason::StandDown) => "standdown",
        None => "none",
    }
    .into()
}

fn audit_str(k: AuditKind) -> String {
    match k {
        AuditKind::Locked => "locked",
        AuditKind::Thawed => "thawed",
    }
    .into()
}

/// Wrap a rendered plan with a short content-hash revision so Kotlin can apply
/// it idempotently (only re-program the VpnService when the plan changes).
fn wrap_plan(plan: &charter_webpolicy::DnsFilterPlan) -> String {
    let plan_json = serde_json::to_string(plan).unwrap_or_default();
    let digest = charter_crypto::sha256(plan_json.as_bytes());
    let revision = digest[..4]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    format!("{{\"revision\":\"{revision}\",\"plan\":{plan_json}}}")
}

/// "1h 05m" / "45m" / "30s" — relative, tz-free.
fn humanize(secs: i64) -> String {
    let s = secs.max(0);
    let h = s / 3600;
    let m = (s % 3600) / 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

/// Normie-facing text per rejection reason (in lockstep with the Linux
/// `charterd::pairing_setup::map_pairing_error` wording).
fn pairing_error_text(e: &PairingError) -> &'static str {
    match e {
        PairingError::NotBunkerUri => {
            "That's not a pairing link. In MyCharter, copy the link that starts with \"bunker://\"."
        }
        PairingError::BadGuardianPubkey => {
            "That pairing link looks corrupted. Copy it again from MyCharter."
        }
        PairingError::MissingRelays => {
            "That pairing link is missing a secure relay (wss://). Copy the whole link from MyCharter."
        }
        PairingError::NonCharterKind => {
            "That isn't a Charter pairing link. In MyCharter choose \"Pair Charter\", not a regular app."
        }
    }
}

/// How long a scanned pairing token keeps riding STATUS (QR onboarding).
///
/// Widened from 15 minutes to an hour (S2, 2026-08-07). The echo is no longer
/// only a convenience that saves someone typing a device code — it is now the
/// guardian app's PROOF that this machine was handed a pairing link by that
/// guardian, and the sole gate on the split-brain recovery. The recovery is
/// for the case where the guardian's app lost the pairing, so the guardian may
/// not look again for a while; a window that shuts in fifteen minutes closes
/// the recovery long before they notice they need it.
///
/// An hour is not a wider exposure: the token rides INSIDE the gift-wrap,
/// readable only by the guardian it was minted by, and it authorises nothing
/// on its own. Its worst case is that a guardian can adopt their own phone for
/// an hour instead of a quarter of one.
const PAIR_TOKEN_WINDOW_SECS: u64 = 60 * 60;

/// Pull the `token` query param out of a pairing URI (percent-decoded).
fn extract_token(bunker_uri: &str) -> Option<String> {
    let (_, query) = bunker_uri.split_once('?')?;
    for pair in query.split('&') {
        if let Some(("token", v)) = pair.split_once('=') {
            let v = charter_primitives::uri::percent_decode(v);
            if !v.is_empty() && v.len() <= 128 {
                return Some(v);
            }
        }
    }
    None
}

fn pair_token_path(base: &std::path::Path) -> PathBuf {
    base.join("pair_token.json")
}

fn save_pair_token(base: &std::path::Path, token: &str, expires: u64) {
    let json = serde_json::json!({ "token": token, "expires": expires });
    let _ = std::fs::write(pair_token_path(base), json.to_string());
}

fn load_pair_token(base: &std::path::Path) -> Option<(String, u64)> {
    let text = std::fs::read_to_string(pair_token_path(base)).ok()?;
    let v: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some((
        v.get("token")?.as_str()?.to_string(),
        v.get("expires")?.as_u64()?,
    ))
}

fn boot_watch_path(base: &std::path::Path) -> PathBuf {
    base.join("boot_watch.json")
}

/// What the warden remembers about boots — see [`Warden::note_boot`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BootWatch {
    /// The platform boot counter the last time the warden ran.
    pub last_boot_count: u64,
    /// How many boots it has missed since the counter was first seen.
    pub unexplained_boots: u32,
    /// When it last noticed it had missed one (unix secs).
    pub last_noticed_at: u64,
}

fn load_boot_watch(base: &std::path::Path) -> Option<BootWatch> {
    let text = std::fs::read_to_string(boot_watch_path(base)).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_boot_watch(base: &std::path::Path, w: &BootWatch) {
    if let Ok(json) = serde_json::to_string(w) {
        let _ = std::fs::write(boot_watch_path(base), json);
    }
}

fn load_pairing(store: &RealPairingStore) -> (Option<PubKey>, Option<PubKey>, Vec<String>, u64) {
    let json = match store.load() {
        Ok(Some(j)) => j,
        _ => return (None, None, Vec::new(), 0),
    };
    // The full pinned `Pairing` (what `pair`/`set_pairing` write today).
    if let Ok(p) = serde_json::from_str::<Pairing>(&json) {
        return (
            Some(p.guardian_pubkey),
            Some(p.subject_pubkey),
            p.relays,
            p.paired_at,
        );
    }
    // Legacy `{guardian, subject}` shape from the pre-relay onboarding builds:
    // still paired for enforcement, no relays until re-paired. No paired_at
    // on this shape (0 = "unknown/always stale-checkable" — see try_apply_releases).
    let v: serde_json::Value = match serde_json::from_str(&json) {
        Ok(v) => v,
        Err(_) => return (None, None, Vec::new(), 0),
    };
    let g = v
        .get("guardian")
        .and_then(|x| x.as_str())
        .and_then(|s| PubKey::from_hex(s).ok());
    let s = v
        .get("subject")
        .and_then(|x| x.as_str())
        .and_then(|s| PubKey::from_hex(s).ok());
    (g, s, Vec::new(), 0)
}

/// The process-global warden.
pub static WARDEN: Mutex<Option<Warden>> = Mutex::new(None);

/// One full slow-tick poll against `lock`, holding it only around state
/// transitions — both network phases (clause poll, STATUS publish) run with
/// the lock RELEASED, so the 2s enforcement tick is never stalled by a slow
/// relay (hardening #14). A poisoned lock recovers (same policy as lib.rs).
pub fn poll_once_locked(lock: &Mutex<Option<Warden>>, now: u64) -> dto::PollResult {
    fn guard(lock: &Mutex<Option<Warden>>) -> std::sync::MutexGuard<'_, Option<Warden>> {
        lock.lock().unwrap_or_else(|p| p.into_inner())
    }
    let mut out = dto::PollResult {
        polled: false,
        clauses_seen: 0,
        clauses_accepted: 0,
        status_emitted: false,
        publish_failures: 0,
        released: false,
        reason: String::new(),
    };

    // Phase 1 (locked): snapshot the relay + broker handles + cursor.
    let (relay, since, broker, rt) = {
        match guard(lock).as_ref() {
            None => {
                out.reason = "warden not initialized".into();
                return out;
            }
            Some(w) => match w.poll_prepare(now) {
                Ok(x) => x,
                Err(reason) => {
                    out.reason = reason;
                    return out;
                }
            },
        }
    };
    out.polled = true;

    // Phase 2 (unlocked): network. First retry anything parked in the offline
    // spool (§2.2 — REQUESTs queued while offline, stale-but-unsent STATUS).
    let (spool_sent, _spool_left) = relay.drain_outbox(now);
    if spool_sent > 0 {
        out.reason = format!("spool: {spool_sent} retried");
    }

    // A guardian RELEASE wins over anything else this round — if the parent
    // unpaired the device, we must stop enforcing, not process one more clause.
    // Pull (unlocked), verify + apply (locked); on release, return immediately.
    if let Ok(releases) = relay.poll_releases(since, now) {
        if !releases.is_empty() {
            if let Some(w) = guard(lock).as_mut() {
                let (applied, detail) = w.try_apply_releases(&releases, now);
                if applied {
                    out.released = true;
                    out.reason = format!("released by guardian | {detail}");
                    return out;
                }
                out.reason = detail;
            }
        }
    }

    // With a broker, ONE authoritative round pulls grants + clauses + curator
    // lists, verifies, stores, and enacts (the spine path — same as
    // charterd). Without one (legacy/test), pull clause wraps directly.
    // The broker path does the pulling, verifying and storing itself, so there
    // is nothing left for `poll_absorb` to absorb — but its counts are the ONLY
    // honest answer to "did anything arrive?". Reporting the empty vec's zeros
    // instead made the diagnostic read "nothing has ever been delivered" on a
    // device that was applying clauses perfectly well (Blue Tablet, 2026-07-29).
    let mut broker_counts = charter_spine::broker::PollCounts::default();
    let received = if let Some(b) = &broker {
        broker_counts = rt.block_on(b.poll_once());
        Vec::new()
    } else {
        match relay.poll_clauses(since, now) {
            Ok(r) => r,
            Err(e) => {
                // Offline is routine on a phone: keep enforcing from the
                // cached clauses; report and retry next slow tick.
                out.reason = e;
                return out;
            }
        }
    };
    // Bring-up diagnostic: nothing unwrapped — distinguish "nothing arrives"
    // from "wraps arrive but fail to unwrap" (key/jitter mismatch).
    if broker.is_none() && received.is_empty() {
        if let Ok(raw) = relay.probe_raw_wraps(since) {
            out.reason = format!("0 clauses unwrapped of {raw} raw wraps");
        }
    }

    // Phase 3 (locked): authenticate + store (legacy path); mirror the ward's
    // clauses for the broker's EOD cap; decide whether STATUS is due.
    let status_due = {
        match guard(lock).as_mut() {
            None => {
                out.reason = "warden shut down mid-poll".into();
                return out;
            }
            Some(w) => {
                let (seen, accepted, due) = w.poll_absorb(received, now);
                // Exactly one of these is ever non-zero: the broker path leaves
                // `received` empty, the legacy path leaves the broker counts at
                // default. Summing keeps both honest without a second branch.
                out.clauses_seen = seen + broker_counts.clauses_seen;
                out.clauses_accepted = accepted + broker_counts.clauses_accepted;
                if broker.is_some() {
                    w.mirror_ward_clauses();
                }
                // Level-triggered: a freshly absorbed (or long-stored) update
                // clause parks its install directive within this same poll.
                w.check_self_update(now);
                due
            }
        }
    };

    // Phase 3b (unlocked): flush any spooled break-glass audits — the ward's
    // emergency unlock happened immediately; this is the LOUD half catching
    // up (offline it waited here, durably).
    {
        let pending = guard(lock)
            .as_ref()
            .map(|w| w.break_glass.pending_audits())
            .unwrap_or_default();
        if !pending.is_empty() {
            let mut all_sent = true;
            for tags in pending {
                if relay.emit_audit(tags, now).is_err() {
                    all_sent = false;
                    break;
                }
            }
            if all_sent {
                if let Some(w) = guard(lock).as_ref() {
                    w.break_glass.clear_audits();
                }
            }
        }
    }

    // Phase 4 (unlocked): publish the STATUS heartbeat.
    if let Some(status) = status_due {
        match serde_json::to_string(&status) {
            Ok(json) => match relay.emit_status(&json, now) {
                Ok(accepted) if accepted > 0 => {
                    out.status_emitted = true;
                    // Phase 5 (locked): record it for the throttle.
                    if let Some(w) = guard(lock).as_mut() {
                        w.mark_status_emitted(status);
                    }
                }
                Ok(_) => out.publish_failures += 1,
                Err(e) => {
                    out.publish_failures += 1;
                    out.reason = if out.reason.is_empty() {
                        e
                    } else {
                        format!("{}; {e}", out.reason)
                    };
                }
            },
            Err(_) => out.publish_failures += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use charter_verify::test_support::{ClauseBuilder, TestGuardian};
    use serde_json::json;

    // Mon 2026-06-29, Europe/London. Inside a 16:00-20:00 window vs after it.
    const IN_WINDOW: i64 = 1_782_752_400; // 18:00 BST
    const AFTER_WINDOW: i64 = 1_782_766_800; // 22:00 BST

    /// The pinned test guardian's raw secret (SeedSigner::from_seed sets the
    /// low byte) — needed to author NIP-59 seals guardian-side in tests.
    fn guardian_sk() -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = 0x11;
        s
    }

    fn temp_base(label: &str) -> PathBuf {
        // Unique per test so parallel tests don't share file-backed stores.
        let mut p = std::env::temp_dir();
        p.push(format!("charter-jni-test-{}-{}", std::process::id(), label));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn after_school_schedule(issued_at: u64) -> serde_json::Value {
        json!({
            "v": 1,
            "tz": "Europe/London",
            "weekly": { "mon": [{ "start": "16:00", "end": "20:00" }] },
            "issuedAt": issued_at
        })
    }

    fn locked(decisions: &[dto::ChildDecision]) -> bool {
        decisions.first().map(|d| d.locked).unwrap_or(false)
    }

    fn configured(decisions: &[dto::ChildDecision]) -> bool {
        decisions.first().map(|d| d.configured).unwrap_or(false)
    }

    /// Putting an ALREADY-RULED device to sleep. The fresh-warden case below
    /// passes happily while this one is what a guardian actually does: the
    /// tablet has a schedule, they turn on "Off unless I open it", and the
    /// dormant clause must REPLACE the standing week rather than lose to it.
    ///
    /// Written after the Blue Tablet sat on "Outside allowed hours — back in
    /// 4h 12m" while a signed dormant clause was published and acknowledged
    /// (2026-07-29 — which turned out to be poll latency, not a defect, but
    /// the replacement path had no test either way).
    #[test]
    fn a_dormant_clause_replaces_a_standing_schedule() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x52);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let subj = ward.pubkey().to_hex();

        // 1) The guardian first gives it an after-school window.
        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(after_school_schedule(1))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        assert!(res.accepted, "schedule rejected: {}", res.reason);
        let d = w.tick(Some(&subj), true, AFTER_WINDOW, None);
        assert!(locked(&d), "locked outside the window");
        assert!(
            w.lock_info(AFTER_WINDOW).come_back.contains("come back"),
            "a windowed schedule names an hour to return"
        );

        // 2) Then puts the device to sleep.
        let dormant = json!({
            "v": 1, "tz": "Europe/London", "weekly": {}, "paused": true, "issuedAt": 2
        });
        let ev = ClauseBuilder::schedule(2)
            .subject(ward.pubkey())
            .body(dormant)
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), AFTER_WINDOW as u64);
        assert!(res.accepted, "dormant clause rejected: {}", res.reason);

        let d = w.tick(Some(&subj), true, AFTER_WINDOW + 1, None);
        assert!(locked(&d), "still locked");
        let info = w.lock_info(AFTER_WINDOW + 1);
        assert_eq!(
            info.title, "This device is off",
            "the dormant clause must supersede the standing week"
        );
        assert!(
            !info.come_back.contains("come back"),
            "a dormant device has no hour to name, got {:?}",
            info.come_back
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A device that is off until a guardian opens it (spec 2026-07-28): the
    /// spare family tablet. Two things must hold together, and the second is
    /// the one a refactor breaks silently.
    ///
    /// 1. It must not tell the child to wait for a window. There isn't one.
    /// 2. It must STILL lock with reason "schedule", so a gift routes to the
    ///    schedule pool. Give it a fourth reason and the grant goes to the
    ///    budget pool, which cannot open a schedule lock — the guardian taps
    ///    "Give time", is told it was sent, and the tablet stays shut.
    #[test]
    fn a_dormant_device_says_so_and_a_gift_still_opens_it() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x51);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // What MyCharter writes for "Off unless I open it": an empty week,
        // encoded explicitly as wire-paused so the device cannot read it as
        // "no schedule, always allowed".
        let dormant =
            json!({"v": 1, "tz": "Europe/London", "weekly": {}, "paused": true, "issuedAt": 1});
        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(dormant)
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        assert!(res.accepted, "dormant clause rejected: {}", res.reason);

        let subj = ward.pubkey().to_hex();
        let d = w.tick(Some(&subj), true, IN_WINDOW, None);
        assert!(locked(&d), "a dormant device is locked even mid-afternoon");
        assert_eq!(
            d[0].reason, "schedule",
            "the routing reason must not change"
        );

        // (1) The copy is honest.
        let info = w.lock_info(IN_WINDOW);
        assert_eq!(info.title, "This device is off");
        assert!(
            !info.title.contains("Outside allowed hours"),
            "there are no allowed hours to be outside of"
        );
        assert!(
            !info.come_back.contains("come back"),
            "no hour can be named, so none may be promised: {:?}",
            info.come_back
        );

        // (2) The guardian opens it for ten minutes, and it actually opens.
        let expires = IN_WINDOW as u64 + 6 * 3600;
        let gift = ClauseBuilder::gift(2, "open-it", 10, expires)
            .subject(ward.pubkey())
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&gift).unwrap(), IN_WINDOW as u64);
        assert!(res.accepted, "gift rejected: {}", res.reason);

        let d = w.tick(Some(&subj), true, IN_WINDOW + 1, None);
        assert!(!locked(&d), "a gift must open a dormant device");
        let left = w.time_left(IN_WINDOW + 2).schedule_secs;
        assert!(
            left > 0 && left <= 10 * 60,
            "the ten minutes must land on the SCHEDULE pool, got {left}s"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn schedule_locks_outside_the_window_and_unlocks_inside() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // Guardian signs an after-school schedule targeting the ward.
        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(after_school_schedule(1))
            .build(&guardian);
        let ev_json = serde_json::to_string(&ev).unwrap();
        let res = w.ingest_clause(&ev_json, IN_WINDOW as u64);
        assert!(res.accepted, "clause rejected: {}", res.reason);

        // Inside the window → not locked.
        let subj = ward.pubkey().to_hex();
        let d = w.tick(Some(&subj), true, IN_WINDOW, None);
        assert!(!locked(&d), "should be unlocked inside the window");

        // Inside the window a charter exists → configured=true (so the Kotlin
        // side keeps the install-lockdown up even while unlocked — the fix for
        // the review's critical fail-open).
        assert!(
            configured(&d),
            "a charted ward must report configured even when unlocked"
        );

        // After the window → locked (Kotlin drives the lock level-triggered).
        let d = w.tick(Some(&subj), true, AFTER_WINDOW, None);
        assert!(locked(&d), "should be locked after the window");
        assert!(configured(&d), "still configured when locked");
        assert_eq!(d[0].reason, "schedule");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A paired ward with NO time rules yet is still stoppable — the contract's
    /// "an unbounded charter (no clauses at all) is capped too". The tick used
    /// to go inert before consulting the stand-down, while STATUS reported
    /// "standdown": the guardian's app claiming a lock the phone never
    /// enforced.
    #[test]
    fn a_stand_down_stops_a_ward_with_no_rules() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let now = IN_WINDOW as u64;
        let ev = ClauseBuilder::stand_down(now, "sd-norules", now + 4 * 3600)
            .subject(ward.pubkey())
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), now);
        assert!(res.accepted, "clause rejected: {}", res.reason);

        let subj = ward.pubkey().to_hex();
        // First sight: she is owed her minute, not the lock.
        let d = w.tick(Some(&subj), true, IN_WINDOW, None);
        assert!(!locked(&d), "the grace comes before the lock");
        assert!(
            configured(&d),
            "a standing stand-down must reach Kotlin as enforceable"
        );
        // Past the minute: the lock lands, named for the person who called it.
        let d = w.tick(Some(&subj), true, IN_WINDOW + 61, None);
        assert!(locked(&d), "a ward with no rules must still be stoppable");
        assert_eq!(d[0].reason, "standdown");
        assert!(configured(&d));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The shade must name the person. A stood-down ward once saw a locked
    /// screen headlined "Unlocked": lock_info had no "standdown" arm, and the
    /// shared child-facing copy in `enforcer_runtime::lock_message` was being
    /// re-derived platform-side, against its own doc.
    #[test]
    fn the_stand_down_shade_names_the_person() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let now = IN_WINDOW as u64;
        // An ordinary charted ward: a generous budget, nowhere near spent.
        let budget = ClauseBuilder::budget(now)
            .subject(ward.pubkey())
            .build(&guardian);
        w.ingest_clause(&serde_json::to_string(&budget).unwrap(), now);
        let sd = ClauseBuilder::stand_down(now, "sd-shade", now + 4 * 3600)
            .subject(ward.pubkey())
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&sd).unwrap(), now);
        assert!(res.accepted, "clause rejected: {}", res.reason);

        let subj = ward.pubkey().to_hex();
        w.tick(Some(&subj), true, IN_WINDOW, None);
        let d = w.tick(Some(&subj), true, IN_WINDOW + 61, None);
        assert!(locked(&d), "the stand-down lock must land");
        assert_eq!(d[0].reason, "standdown");

        let info = w.lock_info(IN_WINDOW + 61);
        assert!(info.locked);
        assert_eq!(
            info.title, "That's it for today",
            "the shade must render the shared stand-down copy, not fall through"
        );
        assert!(
            info.come_back.contains("guardian"),
            "the way back is a person: {}",
            info.come_back
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A gift needs no ask: a guardian-signed `gift` clause unlocks a ward the
    /// schedule has shut out, applies EXACTLY once however many ticks read it,
    /// and a second gift genuinely adds again.
    #[test]
    fn a_gift_adds_time_without_any_request() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(after_school_schedule(1))
            .build(&guardian);
        w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        let subj = ward.pubkey().to_hex();

        // Shut out by the schedule, with no ask outstanding — the case that had
        // no answer before: a guardian could only rewrite the schedule.
        let d = w.tick(Some(&subj), true, AFTER_WINDOW, None);
        assert!(locked(&d), "locked after the window");

        // The guardian gives 30 minutes, unprompted.
        let expires = AFTER_WINDOW as u64 + 6 * 3600;
        let gift = ClauseBuilder::gift(2, "gift-1", 30, expires)
            .subject(ward.pubkey())
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&gift).unwrap(), AFTER_WINDOW as u64);
        assert!(res.accepted, "gift clause rejected: {}", res.reason);

        let d = w.tick(Some(&subj), true, AFTER_WINDOW + 1, None);
        assert!(!locked(&d), "a gift must open a schedule-locked ward");

        // The clause is re-read every tick; the ledger's id keeps it to 30.
        let left_after_first = w.time_left(AFTER_WINDOW + 2).schedule_secs;
        for i in 2..8 {
            w.tick(Some(&subj), true, AFTER_WINDOW + i, None);
        }
        let left_later = w.time_left(AFTER_WINDOW + 8).schedule_secs;
        assert!(
            left_later <= left_after_first,
            "a re-read gift must never top itself back up ({left_after_first} → {left_later})",
        );
        assert!(
            left_later > 0 && left_later <= 30 * 60,
            "one gift is 30 minutes, not more: {left_later}s",
        );

        // A SECOND gift is a new id, so it genuinely adds again.
        let gift2 = ClauseBuilder::gift(3, "gift-2", 15, expires)
            .subject(ward.pubkey())
            .build(&guardian);
        let res = w.ingest_clause(
            &serde_json::to_string(&gift2).unwrap(),
            AFTER_WINDOW as u64 + 9,
        );
        assert!(res.accepted, "second gift rejected: {}", res.reason);
        w.tick(Some(&subj), true, AFTER_WINDOW + 10, None);
        let after_second = w.time_left(AFTER_WINDOW + 10).schedule_secs;
        assert!(
            after_second > left_later,
            "a second gift must add ({left_later} → {after_second})",
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// An expired gift is dead: the phone that spent the evening offline must
    /// not wake up tomorrow and apply yesterday's half hour.
    #[test]
    fn an_expired_gift_gives_nothing() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(after_school_schedule(1))
            .build(&guardian);
        w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        let subj = ward.pubkey().to_hex();

        // Expires BEFORE the tick that would apply it.
        let gift = ClauseBuilder::gift(2, "stale", 30, AFTER_WINDOW as u64)
            .subject(ward.pubkey())
            .build(&guardian);
        w.ingest_clause(&serde_json::to_string(&gift).unwrap(), IN_WINDOW as u64);

        let d = w.tick(Some(&subj), true, AFTER_WINDOW + 60, None);
        assert!(locked(&d), "an expired gift must not unlock anything");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The standing per-app (D3) clause: a guardian-signed apps clause is stored
    /// and surfaced via `app_policy()`; a later paused one lifts it (fail-closed).
    #[test]
    fn apps_clause_drives_the_per_app_policy() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No apps clause yet → no per-app policy.
        assert!(
            w.app_policy(100).is_empty(),
            "no policy before any apps clause"
        );

        // Guardian blocks a package for the ward.
        let ev = ClauseBuilder::apps(1)
            .subject(ward.pubkey())
            .body(json!({"v":1,"posture":"blocklist","blocked":["app.example.block"],"issuedAt":1}))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 100);
        assert!(res.accepted, "apps clause rejected: {}", res.reason);

        let policy = w.app_policy(100);
        assert!(policy.contains("blocklist"), "policy: {policy}");
        assert!(policy.contains("app.example.block"), "policy: {policy}");

        // A later PAUSED clause (higher issuedAt) lifts it — fail-closed to empty.
        let ev2 = ClauseBuilder::apps(2)
            .subject(ward.pubkey())
            .body(json!({"v":1,"posture":"blocklist","blocked":["app.example.block"],"paused":true,"issuedAt":2}))
            .build(&guardian);
        let res2 = w.ingest_clause(&serde_json::to_string(&ev2).unwrap(), 200);
        assert!(res2.accepted, "paused clause rejected: {}", res2.reason);
        assert!(
            w.app_policy(200).is_empty(),
            "paused apps clause suspends nothing"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// `hidden` ("remove from device") outlives `paused`: a paused clause that
    /// names hidden apps still reaches Kotlin, with the blocking lists emptied
    /// so nothing is suspended while the tidy stands (2026-08-27).
    #[test]
    fn paused_apps_clause_still_carries_hidden_with_lists_emptied() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ev = ClauseBuilder::apps(1)
            .subject(ward.pubkey())
            .body(json!({
                "v":1,"posture":"blocklist","blocked":["app.example.block"],
                "askFirst":["app.example.block"],
                "holds":[{"pkg":"app.example.block","state":"allowed","untilUnix":500}],
                "hidden":["com.samsung.android.bixby.agent"],
                "paused":true,"issuedAt":1
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 100);
        assert!(res.accepted, "clause rejected: {}", res.reason);

        let policy: serde_json::Value =
            serde_json::from_str(&w.app_policy(100)).expect("paused-with-hidden is surfaced");
        assert_eq!(policy["hidden"], json!(["com.samsung.android.bixby.agent"]));
        assert_eq!(policy["paused"], json!(true));
        // Nothing to suspend: the lists are emptied, not merely flagged.
        assert!(policy.get("blocked").map_or(true, |b| b.as_array().unwrap().is_empty()));
        assert!(policy.get("allowed").map_or(true, |a| a.as_array().unwrap().is_empty()));
        assert!(policy.get("holds").is_none(), "holds dissolved: {policy}");
        assert!(policy.get("askFirst").is_none(), "askFirst dropped: {policy}");

        // A live one keeps the whole policy AND the hidden list.
        let ev2 = ClauseBuilder::apps(2)
            .subject(ward.pubkey())
            .body(json!({"v":1,"posture":"blocklist","blocked":["app.example.block"],
                "hidden":["com.samsung.android.bixby.agent"],"issuedAt":2}))
            .build(&guardian);
        assert!(w.ingest_clause(&serde_json::to_string(&ev2).unwrap(), 200).accepted);
        let live = w.app_policy(200);
        assert!(live.contains("app.example.block") && live.contains("bixby"), "live: {live}");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// A time-boxed HOLD inside the apps clause: the guardian lets one blocked
    /// app through for an hour, and the phone puts it back on its own. This is
    /// the case that made the feature exist — a standing toggle flipped "just
    /// for now" is a rule change nobody remembers to undo.
    #[test]
    fn an_app_hold_opens_one_app_and_closes_it_again() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // Vanadium STAYS on the blocklist; it is merely held open until t+3600.
        let issued: i64 = 1000;
        let ev = ClauseBuilder::apps(1)
            .subject(ward.pubkey())
            .body(json!({
                "v": 1,
                "posture": "blocklist",
                "blocked": ["org.chromium.vanadium", "app.example.other"],
                "holds": [{
                    "pkg": "org.chromium.vanadium",
                    "state": "allowed",
                    "untilUnix": issued + 3600,
                }],
                "issuedAt": issued,
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), issued as u64);
        assert!(
            res.accepted,
            "apps clause with a hold rejected: {}",
            res.reason
        );

        // During the hold: Vanadium is not in the enforced set, the other app is.
        let during = w.app_policy(issued + 60);
        assert!(
            !during.contains("org.chromium.vanadium"),
            "held app still enforced as blocked: {during}"
        );
        assert!(during.contains("app.example.other"), "policy: {during}");
        assert!(
            !during.contains("holds"),
            "the enforced policy must be lists, not holds: {during}"
        );

        // …and the ward can be told about it.
        let holds = w.app_holds(issued + 60);
        assert!(holds.contains("org.chromium.vanadium"), "holds: {holds}");
        assert!(holds.contains("allowed"), "holds: {holds}");

        // AT the instant it ends — no new clause, no guardian action.
        let after = w.app_policy(issued + 3600);
        assert!(
            after.contains("org.chromium.vanadium"),
            "hold did not end on its own: {after}"
        );
        assert!(
            w.app_holds(issued + 3600).is_empty(),
            "a dead hold must not be announced"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// F1: under an ALLOWLIST posture, `blocked` is never populated by the
    /// clause itself (it is DERIVED — everything not in `allowed`), so the
    /// "Ask to open" list has to gate on `allowed` directly. Before this fix
    /// `ask_first_now` always checked `effective.blocked`, which is
    /// permanently empty under allowlist, so an allowlist family's on-request
    /// app never rendered an ask row at all — the affordance was dead.
    #[test]
    fn allowlist_ask_first_pkg_not_in_allowed_is_offered() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x46);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        w.set_installed_apps(
            &json!([{"pkg": "com.mojang.minecraftpe", "label": "Minecraft"}]).to_string(),
        );

        let ev = ClauseBuilder::apps(1)
            .subject(ward.pubkey())
            .body(json!({
                "v": 1,
                "posture": "allowlist",
                "allowed": ["org.mozilla.fenix"],
                "askFirst": ["com.mojang.minecraftpe"],
                "issuedAt": 1,
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 100);
        assert!(
            res.accepted,
            "allowlist apps clause rejected: {}",
            res.reason
        );

        let payload: serde_json::Value = serde_json::from_str(&w.bucket_views_json(100)).unwrap();
        let ask_first = payload["askFirst"].as_array().expect("askFirst array");
        assert_eq!(ask_first.len(), 1, "payload: {payload}");
        assert_eq!(ask_first[0]["pkg"], "com.mojang.minecraftpe");
        assert_eq!(ask_first[0]["label"], "Minecraft");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// If an askFirst pkg is (incorrectly, upstream) ALSO in `allowed`, it is
    /// already running unconditionally — there is nothing to ask for, so it
    /// must not be offered (the mirror of the blocklist invariant).
    #[test]
    fn allowlist_ask_first_pkg_already_in_allowed_is_not_offered() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x47);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ev = ClauseBuilder::apps(1)
            .subject(ward.pubkey())
            .body(json!({
                "v": 1,
                "posture": "allowlist",
                "allowed": ["com.mojang.minecraftpe"],
                "askFirst": ["com.mojang.minecraftpe"],
                "issuedAt": 1,
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 100);
        assert!(
            res.accepted,
            "allowlist apps clause rejected: {}",
            res.reason
        );

        let payload: serde_json::Value = serde_json::from_str(&w.bucket_views_json(100)).unwrap();
        let ask_first = payload["askFirst"].as_array().expect("askFirst array");
        assert!(ask_first.is_empty(), "payload: {payload}");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The tethering clause (store_key 7): default-blocked before any clause;
    /// a guardian grant opens raw/filtered for a window that ends AT `until`;
    /// a superseding clause replaces the posture; `allow:none` revokes. Every
    /// failure direction lands on Blocked (fail-safe).
    #[test]
    fn update_clause_queues_url_install_when_behind() {
        let sha_a = "a".repeat(64);
        let sha_b = "b".repeat(64);
        let body = json!({
            "v": 1,
            "packageName": "org.forgesworn.charter",
            "versionCode": 21,
            "versionName": "0.21.0",
            "url": "https://charter.mysignet.app/charter-latest.apk",
            "apkSha256": sha_a,
            "signerCertSha256": sha_b,
        });

        // Behind (build 20 < clause 21): exactly one url directive, once.
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x46);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::Update,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(body.clone())
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1_000);
        assert!(res.accepted, "update clause rejected: {}", res.reason);

        w.check_self_update(1_000);
        let q = w.drain_installs(1_000);
        assert_eq!(q.len(), 1, "one directive queued");
        let p = &q[0];
        assert_eq!(p.req_id, sha_a, "keyed by the archive digest");
        assert_eq!(p.package_name, "org.forgesworn.charter");
        assert_eq!(p.version_code, Some(21));
        assert_eq!(p.source, "url");
        assert_eq!(
            p.url.as_deref(),
            Some("https://charter.mysignet.app/charter-latest.apk")
        );
        assert_eq!(p.apk_sha256.as_deref(), Some(sha_a.as_str()));
        assert_eq!(p.signer_cert_sha256, sha_b);

        // Idempotent: a second check never duplicates (req_id-keyed queue).
        w.check_self_update(1_001);
        assert_eq!(w.drain_installs(1_001).len(), 1, "still exactly one");
        let _ = std::fs::remove_dir_all(&base);

        // At the version (build 21 == clause 21): nothing queued.
        let base2 = temp_base(concat!(module_path!(), line!()));
        let mut w2 = Warden::init(base2.to_str().unwrap(), "enforce", 21).unwrap();
        w2.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let ev2 = ClauseBuilder {
            kind: charter_proto::ClauseKind::Update,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(body)
        .build(&guardian);
        assert!(
            w2.ingest_clause(&serde_json::to_string(&ev2).unwrap(), 1_000)
                .accepted
        );
        w2.check_self_update(1_000);
        assert!(
            w2.drain_installs(1_000).is_empty(),
            "at-or-past the version queues nothing"
        );
        let _ = std::fs::remove_dir_all(&base2);
    }

    /// The D8 mirror: "" before any charter; once a schedule lands, the view
    /// carries the week lines from the SHARED spine composer.
    #[test]
    fn schedule_view_mirrors_the_week() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No charter yet ⇒ "" (the ward sees "no charter yet", never a guess).
        assert_eq!(w.schedule_view(1_000), "");

        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(json!({
                "v": 1, "tz": "Europe/London",
                "weekly": {
                    "mon": [{"start": "07:00", "end": "20:00"}],
                    "tue": [{"start": "07:00", "end": "20:00"}]
                },
                "issuedAt": 1
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1_000);
        assert!(res.accepted, "schedule clause rejected: {}", res.reason);
        let view: serde_json::Value = serde_json::from_str(&w.schedule_view(1_000)).unwrap();
        let lines = view["lines"].as_array().unwrap();
        assert!(!lines.is_empty(), "week lines must render");
        let joined = lines
            .iter()
            .filter_map(|l| l.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            joined.contains("07:00"),
            "window times must appear: {joined}"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The lifeline clause (store_key 9). NUMBERS are fail-closed — no clause
    /// or a malformed body renders none, because the lock screen must never
    /// show a garbage dial string. BREAK-GLASS is deliberately the opposite
    /// (2026-07-29): it is present unless a guardian explicitly turns it off,
    /// so a device that cannot reach the relay is never a device with no way
    /// out. The two live in the same clause and pull in opposite directions on
    /// purpose.
    #[test]
    fn lifeline_clause_exposes_numbers_fail_closed() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No clause ⇒ no CALL buttons, but the escape hatch is still there.
        let none_yet: serde_json::Value = serde_json::from_str(&w.lifeline()).unwrap();
        assert_eq!(none_yet["numbers"], serde_json::json!([]));
        assert_eq!(none_yet["breakGlass"]["enabled"], serde_json::json!(true));
        assert_eq!(none_yet["breakGlass"]["scope"], serde_json::json!("full"));

        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::Lifeline,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v": 1, "issuedAt": 1, "numbers": [
            {"label": "Mum", "number": "+44 7700 900123"},
            {"label": "Dad", "number": "07700 900456"}
        ]}))
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1_000);
        assert!(res.accepted, "lifeline clause rejected: {}", res.reason);
        let view: serde_json::Value = serde_json::from_str(&w.lifeline()).unwrap();
        assert_eq!(view["numbers"][0]["label"], "Mum");
        assert_eq!(view["numbers"][1]["number"], "07700 900456");
        // v1 clause ⇒ v2 features OFF (fail-closed defaults).
        assert_eq!(view["emergencyServices"], serde_json::json!(false));
        // Absent in the clause = off, so a pre-torch family sees no button.
        assert_eq!(view["torch"], serde_json::json!(false));
        // …but break-glass absent = ON. A guardian who set numbers and never
        // considered the escape hatch still leaves their ward one.
        assert_eq!(view["breakGlass"]["enabled"], serde_json::json!(true));

        // A superseding malformed body (USSD smuggle) ⇒ back to NO buttons.
        let ev2 = ClauseBuilder {
            kind: charter_proto::ClauseKind::Lifeline,
            issued_at: 2,
            body: json!({}),
            created_at: 2,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v": 1, "issuedAt": 2, "numbers": [
            {"label": "Mum", "number": "*#06#"}
        ]}))
        .build(&guardian);
        let res2 = w.ingest_clause(&serde_json::to_string(&ev2).unwrap(), 1_000);
        assert!(res2.accepted);
        let broken: serde_json::Value = serde_json::from_str(&w.lifeline()).unwrap();
        assert_eq!(
            broken["numbers"],
            serde_json::json!([]),
            "invalid numbers must render no call buttons"
        );
        // A clause we refused to trust must not also cost the ward their way
        // out — that would make a malformed lifeline strictly worse than none.
        assert_eq!(broken["breakGlass"]["enabled"], serde_json::json!(true));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The safety net is ON by default, but it is a DEFAULT and not a law: a
    /// guardian who explicitly switches it off gets it switched off. Without
    /// this the MyCharter toggle would be decorative, which is worse than not
    /// offering it.
    #[test]
    fn an_explicit_off_still_removes_the_safety_net() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x53);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ll = ClauseBuilder {
            kind: charter_proto::ClauseKind::Lifeline,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v":1,"issuedAt":1,
                     "numbers":[{"label":"Mum","number":"07700900123"}],
                     "breakGlass":{"enabled":false,"scope":"full","durationMinutes":10}}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&ll).unwrap(), 1_000)
                .accepted
        );

        let res: serde_json::Value =
            serde_json::from_str(&w.break_glass(AFTER_WINDOW as u64)).unwrap();
        assert_eq!(res["allowed"], serde_json::json!(false));
        assert_eq!(res["reason"], serde_json::json!("not-enabled"));
        assert!(w.break_glass.pending_audits().is_empty());

        // …and the shade draws no button for it either.
        let view: serde_json::Value = serde_json::from_str(&w.lifeline()).unwrap();
        assert_eq!(view["breakGlass"]["enabled"], serde_json::json!(false));

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Break-glass: refused when the guardian hasn't enabled it; when they
    /// have, it unlocks IMMEDIATELY (no wire round-trip), spools the loud
    /// audit, survives a restart, and re-locks the moment it expires.
    #[test]
    fn break_glass_unlocks_then_relocks_and_spools_the_audit() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No lifeline clause at all ⇒ the safety net is up anyway (2026-07-29).
        // It used to refuse here, which meant every device nobody had finished
        // configuring had no way out of a lock it could not reach the relay to
        // lift.
        // Taken at IN_WINDOW, four hours before the rest of this test runs, so
        // the ten-minute window it opens is long expired and cannot unlock the
        // schedule-locked device the later assertions depend on.
        let by_default: serde_json::Value =
            serde_json::from_str(&w.break_glass(IN_WINDOW as u64)).unwrap();
        assert_eq!(by_default["allowed"], serde_json::json!(true));
        assert_eq!(by_default["scope"], serde_json::json!("full"));
        // Still loud: an unconfigured escape is audited exactly like a
        // configured one, or it becomes a quiet back door.
        assert!(!w.break_glass.pending_audits().is_empty());
        w.break_glass.clear_audits();

        // A schedule that is CLOSED at AFTER_WINDOW, so the ward is locked.
        let sched = ClauseBuilder {
            kind: charter_proto::ClauseKind::Schedule,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v":1,"tz":"Europe/London","issuedAt":1,
                     "weekly":{"mon":[{"start":"16:00","end":"20:00"}]}}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&sched).unwrap(), 1_000)
                .accepted
        );

        // The guardian enables break-glass: full scope, 10 minutes.
        let ll = ClauseBuilder {
            kind: charter_proto::ClauseKind::Lifeline,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v":1,"issuedAt":1,
                     "numbers":[{"label":"Mum","number":"07700900123"}],
                     "breakGlass":{"enabled":true,"scope":"full","durationMinutes":10}}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&ll).unwrap(), 1_000)
                .accepted
        );

        // Locked outside the window, before breaking the glass.
        let before = w.tick(None, true, AFTER_WINDOW, None);
        assert!(before[0].locked, "must be schedule-locked to begin with");

        let now = AFTER_WINDOW as u64;
        let res: serde_json::Value = serde_json::from_str(&w.break_glass(now)).unwrap();
        assert_eq!(res["allowed"], serde_json::json!(true));
        assert_eq!(res["scope"], serde_json::json!("full"));
        assert_eq!(res["secsLeft"], serde_json::json!(600));

        // The unlock is immediate and did NOT wait on any network.
        let during = w.tick(None, true, AFTER_WINDOW + 60, None);
        assert!(!during[0].locked, "break-glass must unlock immediately");
        assert_eq!(during[0].reason, "breakglass");

        // Loud: the audit is spooled with the contract's exact tags.
        let pending = w.break_glass.pending_audits();
        assert_eq!(pending.len(), 1, "exactly one audit queued");
        assert!(pending[0].contains(&vec!["outcome".into(), "override".into()]));
        assert!(pending[0].contains(&vec!["op".into(), "unlock.breakglass".into()]));
        assert!(pending[0].contains(&vec!["durationSecs".into(), "600".into()]));

        // Durable: a restart mid-emergency must not slam the door.
        drop(w);
        let mut w2 = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w2.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        assert!(
            !w2.tick(None, true, AFTER_WINDOW + 120, None)[0].locked,
            "the window must survive a restart"
        );

        // Nothing is forgiven, only deferred: expiry re-locks.
        let after = w2.tick(None, true, AFTER_WINDOW + 601, None);
        assert!(after[0].locked, "must re-lock the moment the window ends");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A v2 lifeline clause (5 numbers + emergency entry + break-glass)
    /// surfaces every field to the shade.
    #[test]
    fn lifeline_v2_exposes_emergency_and_break_glass() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::Lifeline,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v": 1, "issuedAt": 1, "numbers": [
            {"label": "Mum", "number": "+44 7700 900123"},
            {"label": "Dad", "number": "07700 900456"},
            {"label": "Nan", "number": "07700 900457"},
            {"label": "Auntie", "number": "07700 900458"},
            {"label": "Neighbour", "number": "07700 900459"}
        ], "emergencyServices": true,
           "torch": true,
           "breakGlass": {"enabled": true, "scope": "full", "durationMinutes": 10}}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1_000)
                .accepted,
            "v2 lifeline clause must be accepted"
        );

        let view: serde_json::Value = serde_json::from_str(&w.lifeline()).unwrap();
        assert_eq!(view["numbers"].as_array().unwrap().len(), 5);
        assert_eq!(view["emergencyServices"], serde_json::json!(true));
        // The torch reaches the shade's view (Kotlin reads this exact key).
        assert_eq!(view["torch"], serde_json::json!(true));
        assert_eq!(view["breakGlass"]["enabled"], serde_json::json!(true));
        assert_eq!(view["breakGlass"]["scope"], serde_json::json!("full"));
        assert_eq!(view["breakGlass"]["durationMinutes"], serde_json::json!(10));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The tethering clause (store_key 7): default-blocked before any clause;
    /// (docs continue on the original test below)
    #[test]
    fn tethering_clause_drives_mode_and_expires() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x45);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // Default-closed: no clause ⇒ blocked.
        assert_eq!(w.tethering_mode(1_000), "blocked");

        // Raw grant until t=2000 — live inside, dead AT the boundary.
        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::Tethering,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v": 1, "issuedAt": 1, "allow": "raw", "until": 2_000}))
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1_000);
        assert!(res.accepted, "tethering clause rejected: {}", res.reason);
        assert_eq!(w.tethering_mode(1_000), "raw");
        assert_eq!(w.tethering_mode(2_000), "blocked", "grant ends AT until");

        // Superseding filtered grant with no `until` — until revoked.
        let ev2 = ClauseBuilder {
            kind: charter_proto::ClauseKind::Tethering,
            issued_at: 2,
            body: json!({}),
            created_at: 2,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v": 1, "issuedAt": 2, "allow": "filtered"}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&ev2).unwrap(), 3_000)
                .accepted
        );
        assert_eq!(w.tethering_mode(1_000_000), "filtered");

        // Revocation: allow none ⇒ blocked again.
        let ev3 = ClauseBuilder {
            kind: charter_proto::ClauseKind::Tethering,
            issued_at: 3,
            body: json!({}),
            created_at: 3,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v": 1, "issuedAt": 3, "allow": "none"}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&ev3).unwrap(), 4_000)
                .accepted
        );
        assert_eq!(w.tethering_mode(5_000), "blocked");

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The schedule-dependent per-app RULE clause (appRules / store_key 6): a
    /// guardian-signed rule set with a blocked-outright app, an always-allowed
    /// app, and one gated to a Mon 16:00-20:00 window is stored and evaluated
    /// per-tick. The suspension set is EXACTLY the packages Blocked right now —
    /// so the scheduled app joins the set only OUTSIDE its window; an absent
    /// clause suspends nothing (fail-safe).
    #[test]
    fn app_rule_suspensions_are_exactly_the_blocked_now_packages() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No appRules clause yet → nothing to suspend (fail-safe).
        assert!(
            w.app_rule_suspensions(IN_WINDOW).is_empty(),
            "no suspensions before any appRules clause"
        );

        // Guardian signs the full rule set for the ward (replace-the-set).
        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::AppRules,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({
            "v": 1,
            "issuedAt": 1,
            "rules": [
                { "pkg": "app.example.block", "blocked": true },
                { "pkg": "app.example.allow", "blocked": false },
                { "pkg": "app.example.sched", "blocked": false,
                  "schedule": after_school_schedule(1) }
            ]
        }))
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        assert!(res.accepted, "appRules clause rejected: {}", res.reason);

        // INSIDE the 16:00-20:00 window (18:00 BST): only the blocked-outright
        // app is suspended — the scheduled app is inside its window (allowed),
        // and the always-allowed app is never touched.
        assert_eq!(
            w.app_rule_suspensions(IN_WINDOW),
            vec!["app.example.block".to_string()],
            "inside the window only the blocked app suspends"
        );

        // AFTER the window (22:00 BST): the scheduled app is now outside its
        // allowed hours, so it joins the blocked app; the allowed app never does.
        assert_eq!(
            w.app_rule_suspensions(AFTER_WINDOW),
            vec![
                "app.example.block".to_string(),
                "app.example.sched".to_string(),
            ],
            "after the window the scheduled app joins the suspend set"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// She must be able to START an audiobook at 2am — the whole distinction
    /// from `listening`, which needs audio already sounding.
    #[test]
    fn always_available_opens_named_apps_but_only_for_the_right_lock() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // No clause yet → nothing opens (fail-closed).
        assert_eq!(
            w.always_available_view(5_000, true, "schedule"),
            r#"{"open":[]}"#,
            "fail closed before any clause"
        );

        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::AlwaysAvailable,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({
            "v": 1,
            "issuedAt": 1,
            "apps": [
                { "pkg": "app.example.book" },
                { "pkg": "app.example.chat", "untilUnix": 5000 }
            ]
        }))
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 1);
        assert!(
            res.accepted,
            "alwaysavailable clause rejected: {}",
            res.reason
        );

        // A schedule lock: both are open while the temporary one is live.
        let v = w.always_available_view(4_999, true, "schedule");
        assert!(v.contains("app.example.book"), "got {v}");
        assert!(v.contains("app.example.chat"), "got {v}");

        // A budget lock exempts exactly the same way.
        assert!(w
            .always_available_view(4_999, true, "budget")
            .contains("app.example.book"));

        // The expiry ends the temporary grant at the instant, with no restart.
        let v = w.always_available_view(5_000, true, "schedule");
        assert!(
            v.contains("app.example.book"),
            "the standing one stands: {v}"
        );
        assert!(
            !v.contains("app.example.chat"),
            "the lapsed one is gone: {v}"
        );

        // Unlocked, and the two locks this clause may never outlive.
        assert_eq!(
            w.always_available_view(4_999, false, "schedule"),
            r#"{"open":[]}"#
        );
        assert_eq!(
            w.always_available_view(4_999, true, "standdown"),
            r#"{"open":[]}"#
        );
        assert_eq!(
            w.always_available_view(4_999, true, "malformed"),
            r#"{"open":[]}"#
        );
        assert_eq!(
            w.always_available_view(4_999, true, "unknown"),
            r#"{"open":[]}"#
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The metering half of always-available (spec 2026-08-03): a tick with
    /// the device schedule-locked, the screen interactive, and the
    /// always-available app in the foreground must credit
    /// `out_of_hours_today_secs` while leaving `used_today` — the budget —
    /// completely untouched. This is the counting/enforcing agreement the
    /// clause exists to keep: a bad night's sleep must never cost her
    /// tomorrow's screen time.
    #[test]
    fn a_tick_with_the_screen_on_and_an_always_available_foreground_app_credits_out_of_hours() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let hex = ward.pubkey().to_hex();

        // A schedule that is CLOSED at AFTER_WINDOW (22:00), so the ward is
        // schedule-locked.
        let sched = ClauseBuilder {
            kind: charter_proto::ClauseKind::Schedule,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v":1,"tz":"Europe/London","issuedAt":1,
                     "weekly":{"mon":[{"start":"16:00","end":"20:00"}]}}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&sched).unwrap(), 1)
                .accepted
        );

        let always = ClauseBuilder {
            kind: charter_proto::ClauseKind::AlwaysAvailable,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({
            "v": 1,
            "issuedAt": 1,
            "apps": [{ "pkg": "app.example.book" }]
        }))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&always).unwrap(), 1)
                .accepted
        );

        // Establish the ledger with an inert tick first, exactly like the
        // nearest existing accrual tests (`ruled_warden` / `bucket_warden`
        // idiom): the interval a tick credits is since the PRIOR tick, so
        // the very first tick after pairing never accrues anything.
        w.tick(Some(&hex), true, AFTER_WINDOW, Some("app.example.book"));

        let d = w.tick(
            Some(&hex),
            true,
            AFTER_WINDOW + 300,
            Some("app.example.book"),
        );
        assert!(locked(&d), "schedule-closed at 22:00 must stay locked");
        assert_eq!(d[0].reason, "schedule");

        let usage = w.usage.as_ref().expect("ledger established");
        assert_eq!(
            usage.out_of_hours_today_secs(),
            300,
            "the 2am (well, 10pm) audiobook must be metered"
        );
        assert_eq!(
            usage.used_today(AFTER_WINDOW + 300),
            0,
            "the budget must not move for out-of-hours use"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Fix round 1 regression: the unlocked→locked TRANSITION tick (a
    /// schedule window closing mid-tick) must credit its whole elapsed
    /// interval to the BUDGET, never to out-of-hours — even though the
    /// device ends that tick locked with the always-available app in the
    /// foreground. `activity` is classified on the PRIOR lock state (the
    /// same basis `used_today_secs` is credited on) while `decision.locked`
    /// reflects the state AFTER this tick decided; crediting out-of-hours on
    /// `decision.locked` alone double-counted the transition tick's seconds
    /// and could fabricate a whole "night" for a ward who stopped the
    /// instant the lock landed. Pins BOTH sides of the boundary: the
    /// transition tick (must not count) and the very next tick, now locked
    /// at both ends (must count, and must be the first night).
    #[test]
    fn a_transition_tick_credits_the_budget_not_out_of_hours() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();
        let hex = ward.pubkey().to_hex();

        // Schedule: Monday 16:00-20:00 London — open at 19:59, closed at 20:04.
        let sched = ClauseBuilder {
            kind: charter_proto::ClauseKind::Schedule,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({"v":1,"tz":"Europe/London","issuedAt":1,
                     "weekly":{"mon":[{"start":"16:00","end":"20:00"}]}}))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&sched).unwrap(), 1)
                .accepted
        );

        let always = ClauseBuilder {
            kind: charter_proto::ClauseKind::AlwaysAvailable,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({
            "v": 1,
            "issuedAt": 1,
            "apps": [{ "pkg": "app.example.book" }]
        }))
        .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&always).unwrap(), 1)
                .accepted
        );

        // 20:00 BST Monday — the window's close, on the same day as IN_WINDOW
        // (18:00 BST) / AFTER_WINDOW (22:00 BST) above.
        const WINDOW_CLOSE: i64 = IN_WINDOW + 2 * 3600;
        // T0: 19:59, inside the window — establishes the ledger and the
        // PRIOR lock state (unlocked). First tick ever, so elapsed is 0
        // regardless.
        const T0: i64 = WINDOW_CLOSE - 60;
        // T1: 20:04, outside the window — the TRANSITION tick. The interval
        // [T0, T1) was spent ENTIRELY inside the window (unlocked), so
        // `activity` for this tick is Active even though the device is
        // locked by the time the tick decides.
        const T1: i64 = T0 + 300;
        // T2: 20:09, still outside the window — both endpoints of [T1, T2)
        // are locked.
        const T2: i64 = T1 + 300;

        w.tick(Some(&hex), true, T0, Some("app.example.book"));

        let d1 = w.tick(Some(&hex), true, T1, Some("app.example.book"));
        assert!(locked(&d1), "20:04 is outside the 16:00-20:00 window");
        assert_eq!(d1[0].reason, "schedule");
        {
            let usage = w.usage.as_ref().expect("ledger established");
            assert_eq!(
                usage.out_of_hours_today_secs(),
                0,
                "the transition tick's interval was spent unlocked — it must not count"
            );
            assert_eq!(
                usage.out_of_hours_nights_week(),
                0,
                "no night may be fabricated from a tick that ended the moment it locked"
            );
            assert_eq!(
                usage.used_today(T1),
                300,
                "the whole transition interval belongs to the budget"
            );
        }

        let d2 = w.tick(Some(&hex), true, T2, Some("app.example.book"));
        assert!(locked(&d2), "still outside the window");
        {
            let usage = w.usage.as_ref().expect("ledger established");
            assert_eq!(
                usage.out_of_hours_today_secs(),
                300,
                "an interval locked at BOTH ends must be metered"
            );
            assert_eq!(
                usage.out_of_hours_nights_week(),
                1,
                "this genuinely is the first night"
            );
            assert_eq!(
                usage.used_today(T2),
                300,
                "the budget must not move for the out-of-hours interval"
            );
        }

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Task 4: `appRules` carrying a `cmdline:` identity (the Linux
    /// JVM-vs-launcher form — meaningless on Android, where an identity IS
    /// the package name and there is no launcher-vs-process ambiguity to
    /// solve) must still VALIDATE, not crash, and not stop the rest of the
    /// rule set from working. `evaluate_app_rules` (charter-schedule, shared
    /// by both platforms) treats `pkg` as opaque data — no shape-branching
    /// keyed on its prefix — so it comes back exactly as authored; it is up
    /// to the CALLER never to hand a `cmdline:` string to a real Android
    /// suspend-by-package-name call, which Kotlin cannot do anyway since no
    /// installed Android package is ever literally named `cmdline:…`.
    #[test]
    fn app_rules_tolerate_a_cmdline_identity_alongside_a_real_package() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ev = ClauseBuilder {
            kind: charter_proto::ClauseKind::AppRules,
            issued_at: 1,
            body: json!({}),
            created_at: 1,
            subject: None,
        }
        .subject(ward.pubkey())
        .body(json!({
            "v": 1,
            "issuedAt": 1,
            "rules": [
                { "pkg": "cmdline:net.minecraft.client.main.Main", "blocked": true },
                { "pkg": "app.example.block", "blocked": true },
                { "pkg": "app.example.allow", "blocked": false },
            ]
        }))
        .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        assert!(
            res.accepted,
            "a cmdline: identity must not make the clause malformed: {}",
            res.reason
        );

        // The real package's own rule keeps working exactly as it would
        // alone; the cmdline: entry rides along inertly (Kotlin has no
        // Android package literally named `cmdline:…` to ever suspend).
        let suspensions = w.app_rule_suspensions(IN_WINDOW);
        assert!(
            suspensions.contains(&"app.example.block".to_string()),
            "the real package must still be blocked: {suspensions:?}"
        );
        assert!(
            !suspensions.contains(&"app.example.allow".to_string()),
            "the always-allowed package must stay untouched: {suspensions:?}"
        );

        let _ = std::fs::remove_dir_all(&base);
    }

    /// The web-content clause (D-web): a guardian-signed blocklist clause
    /// renders to a `DnsFilterPlan` via the SAME evaluator + renderer the
    /// Linux warden uses — forced SafeSearch, the YouTube-moderate rewrite,
    /// and the parent-denied domain all land in the plan.
    #[test]
    fn web_dns_plan_blocklist_forces_safesearch() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ev = ClauseBuilder::content(10)
            .subject(ward.pubkey())
            .body(json!({
                "v": 1, "posture": "blocklist", "ageTier": "older",
                "parentDeny": ["bad.example"], "safeSearch": true,
                "youtubeRestrict": "moderate", "issuedAt": 10
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 100);
        assert!(res.accepted, "content clause rejected: {}", res.reason);

        let out = w.web_dns_plan();
        assert!(!out.is_empty(), "paired ward yields a plan");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["plan"]["mode"], "blocklist");
        assert!(v["plan"]["blockDomains"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d == "bad.example"));
        assert_eq!(v["plan"]["safeSearch"], true);
        assert!(v["plan"]["rewrites"].as_array().unwrap().iter().any(|r| {
            r["host"] == "www.youtube.com" && r["answer"] == "restrictmoderate.youtube.com"
        }));
        assert!(v["revision"].as_str().unwrap().len() >= 8);

        let _ = std::fs::remove_dir_all(&base);
    }

    /// Not paired ⇒ no ward to evaluate a plan for ⇒ "" (fail-closed: Kotlin
    /// applies nothing rather than guessing).
    #[test]
    fn web_dns_plan_unpaired_is_empty() {
        let base = temp_base(concat!(module_path!(), line!()));
        let w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        assert_eq!(w.web_dns_plan(), "");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A paused content clause fails CLOSED — the plan locks (block-all)
    /// rather than falling open.
    #[test]
    fn web_dns_plan_paused_locks() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let ev = ClauseBuilder::content(10)
            .subject(ward.pubkey())
            .body(json!({
                "v": 1, "posture": "blocklist", "ageTier": "older",
                "parentDeny": [], "paused": true, "issuedAt": 10
            }))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), 100);
        assert!(res.accepted, "content clause rejected: {}", res.reason);

        let v: serde_json::Value = serde_json::from_str(&w.web_dns_plan()).unwrap();
        assert_eq!(v["plan"]["mode"], "locked");

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn forged_clause_from_wrong_signer_is_rejected() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let impostor = TestGuardian::from_seed(0x99);
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        // Signed by the impostor, not the pinned guardian.
        let ev = ClauseBuilder::schedule(1)
            .subject(ward.pubkey())
            .body(after_school_schedule(1))
            .build(&impostor);
        let res = w.ingest_clause(&serde_json::to_string(&ev).unwrap(), IN_WINDOW as u64);
        assert!(!res.accepted, "forged clause must be rejected");

        // With no accepted charter, the ward is inert (not locked, not enforced).
        let subj = ward.pubkey().to_hex();
        let d = w.tick(Some(&subj), true, AFTER_WINDOW, None);
        assert!(!locked(&d));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn stale_issued_at_clause_is_rejected_as_rollback() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let ward = TestGuardian::from_seed(0x44);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        w.set_pairing(&guardian.pubkey().to_hex(), &ward.pubkey().to_hex())
            .unwrap();

        let newer = ClauseBuilder::schedule(5)
            .subject(ward.pubkey())
            .body(after_school_schedule(5))
            .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&newer).unwrap(), IN_WINDOW as u64)
                .accepted
        );

        // A clause with a lower issuedAt is a rollback attempt.
        let older = ClauseBuilder::schedule(3)
            .subject(ward.pubkey())
            .body(after_school_schedule(3))
            .build(&guardian);
        let res = w.ingest_clause(&serde_json::to_string(&older).unwrap(), IN_WINDOW as u64);
        assert!(
            !res.accepted,
            "stale issuedAt must be refused: {}",
            res.reason
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unknown_ward_time_left_is_locked_not_unlimited() {
        let base = temp_base(concat!(module_path!(), line!()));
        let w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        // No pairing, no clause → unknown → LOCKED (I10).
        let tl = w.time_left(IN_WINDOW);
        assert!(!tl.known);
        assert!(tl.locked);
        let _ = std::fs::remove_dir_all(&base);
    }

    // ---- bunker pairing + relay poll (the §3b increment) -------------------

    #[test]
    fn pair_pins_guardian_from_percent_encoded_bunker_uri_and_survives_restart() {
        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();

        // The EXACT shape older deployed MyCharter builds emit (§5.1): the
        // percent-encoded relay must pin end-to-end on the phone too.
        let uri = format!(
            "bunker://{}?relay=wss%3A%2F%2Frelay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let state = w.pair(&uri, 1_000).expect("pairing pins");
        assert!(state.paired);
        assert_eq!(state.relays, vec!["wss://relay.example".to_string()]);
        // Sole ward defaults to the machine identity (the deployed PWA emits
        // subject-absent clauses; any stable key works, this needs no setup).
        assert_eq!(
            state.subject.as_deref(),
            Some(w.machine_pubkey_hex().as_str())
        );

        // Pin-once: a different guardian is refused.
        let other = TestGuardian::from_seed(0x22);
        let uri2 = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            other.pubkey().to_hex()
        );
        assert!(w.pair(&uri2, 2_000).is_err());

        // A normie-facing error for a non-bunker paste.
        let err = w.pair("https://nope", 2_000).unwrap_err();
        assert!(err.contains("bunker://"), "friendly text, got: {err}");

        // The full pairing (incl. relays) survives a warden restart.
        drop(w);
        let w2 = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let s2 = w2.pairing_state();
        assert!(s2.paired);
        assert_eq!(
            s2.guardian.as_deref(),
            Some(guardian.pubkey().to_hex().as_str())
        );
        assert_eq!(s2.relays, vec!["wss://relay.example".to_string()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn poll_once_unpaired_reports_not_paired() {
        let base = temp_base(concat!(module_path!(), line!()));
        let w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let lock = Mutex::new(Some(w));
        let r = poll_once_locked(&lock, IN_WINDOW as u64);
        assert!(!r.polled);
        assert_eq!(r.reason, "not paired");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The whole §3b loop, headless: the guardian gift-wraps a signed clause to
    /// the machine over an in-memory relay; `poll_once` pulls, authenticates,
    /// and stores it; the next tick enforces it; and a STATUS wrap comes back
    /// that only the guardian can open.
    #[test]
    fn poll_once_ingests_wrapped_clause_enforces_and_emits_status() {
        use charter_primitives::kinds::{CHARTER_DEVICE_STATUS, GIFT_WRAP};
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        w.pair(&uri, IN_WINDOW as u64 - 100).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();

        // Guardian-side: sign a PAUSED schedule (locks immediately), subject
        // ABSENT — exactly what the deployed PWA emits for a Stage-0 child —
        // and gift-wrap it to the machine on the shared in-memory relay.
        let ev = ClauseBuilder::schedule(1)
            .body(json!({
                "v": 1, "tz": "Europe/London", "paused": true,
                "weekly": {}, "issuedAt": 1
            }))
            .build(&guardian);
        let rumor = Rumor::from_signed_event(&ev);
        let wrap = nip59::wrap(
            &rumor,
            &guardian_sk(),
            machine.as_bytes(),
            &WrapRandomness {
                ephemeral_secret: [7u8; 32],
                seal_nonce: [8u8; 32],
                wrap_nonce: [9u8; 32],
                seal_created_at: IN_WINDOW as u64,
                wrap_created_at: IN_WINDOW as u64,
            },
        )
        .expect("wrap builds");
        let mock = MockRelayTransport::new();
        mock.inject(wrap);

        // Device-side: drive the poll over the injected mock relay.
        let relay = crate::relay::BlockingRelay::new(
            mock.clone(),
            *w.machine_sk,
            guardian.pubkey(),
            vec!["wss://relay.example".into()],
        )
        .unwrap();
        w.inject_relay(std::sync::Arc::new(relay));
        let lock = Mutex::new(Some(w));

        let r = poll_once_locked(&lock, IN_WINDOW as u64);
        assert!(r.polled);
        assert_eq!(r.clauses_seen, 1, "reason: {}", r.reason);
        assert_eq!(r.clauses_accepted, 1);
        assert!(r.status_emitted, "the STATUS heartbeat must beat");
        // The emitted STATUS reports this build's versionCode (#44) — 20 is
        // what every test Warden::init passes.
        {
            let g = lock.lock().unwrap();
            let last = g.as_ref().unwrap().last_status_for_tests();
            assert_eq!(last.and_then(|s| s.app_version_code), Some(20));
        }

        // The stored clause now enforces: paused ⇒ locked, charter configured.
        let (subj, d) = {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            let subj = w.pairing_state().subject.unwrap();
            let d = w.tick(Some(&subj), true, IN_WINDOW, None);
            (subj, d)
        };
        let _ = subj;
        assert!(locked(&d), "paused schedule must lock");
        assert!(configured(&d));

        // Idempotent re-poll: the same wrap re-delivered is a rollback no-op,
        // and the unchanged state within the heartbeat window emits nothing.
        let r2 = poll_once_locked(&lock, IN_WINDOW as u64 + 5);
        assert_eq!(r2.clauses_seen, 1);
        assert_eq!(r2.clauses_accepted, 0, "re-delivery must not re-store");
        assert!(!r2.status_emitted, "no state change, within heartbeat");

        // The STATUS wrap on the relay opens ONLY for the guardian and carries
        // the machine + locked state (numbers/enums only — no PII fields).
        let status_wrap = {
            let all = futures_events(&mock);
            all.into_iter()
                .filter(|e| e.kind == GIFT_WRAP)
                .rfind(|e| {
                    e.tags.iter().any(|t| {
                        t.first().map(|k| k == "p").unwrap_or(false)
                            && t.get(1) == Some(&guardian.pubkey().to_hex())
                    })
                })
                .expect("a STATUS wrap addressed to the guardian")
        };
        let (status_rumor, seal_author) = nip59::unwrap_with_author(
            &status_wrap,
            &guardian_sk(),
            IN_WINDOW as u64,
            nip59::MAX_JITTER_SECS,
        )
        .expect("guardian can open it");
        assert_eq!(status_rumor.kind, CHARTER_DEVICE_STATUS);
        assert_eq!(seal_author, machine);
        let payload: serde_json::Value = serde_json::from_str(&status_rumor.content).unwrap();
        assert_eq!(payload["machine"], machine.to_hex());
        assert_eq!(payload["locked"], true);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// QR onboarding: a `token` in the scanned URI rides STATUS only within
    /// the post-pairing window, so the guardian app can match its own QR.
    #[test]
    fn pair_token_rides_status_within_window_then_expires() {
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59;

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter&token=t0k3n42",
            guardian.pubkey().to_hex()
        );
        let t0 = IN_WINDOW as u64;
        w.pair(&uri, t0).unwrap();

        let mock = MockRelayTransport::new();
        let relay = crate::relay::BlockingRelay::new(
            mock.clone(),
            *w.machine_sk,
            guardian.pubkey(),
            vec!["wss://relay.example".into()],
        )
        .unwrap();
        w.inject_relay(std::sync::Arc::new(relay));
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();
        let lock = Mutex::new(Some(w));

        // Within the window: STATUS carries the echo.
        let r = poll_once_locked(&lock, t0 + 5);
        assert!(r.status_emitted, "reason: {}", r.reason);
        let open = |ev: &NostrEvent| {
            nip59::unwrap_with_author(ev, &guardian_sk(), t0 + 5, nip59::MAX_JITTER_SECS).ok()
        };
        let statuses: Vec<String> = futures_events(&mock)
            .iter()
            .filter_map(open)
            .filter(|(r, a)| {
                r.kind == charter_primitives::kinds::CHARTER_DEVICE_STATUS && *a == machine
            })
            .map(|(r, _)| r.content)
            .collect();
        assert!(statuses
            .last()
            .unwrap()
            .contains("\"pairToken\":\"t0k3n42\""));

        // Past the window (heartbeat refresh forces a re-emit): echo is gone.
        let later = t0 + PAIR_TOKEN_WINDOW_SECS + 61;
        let r2 = poll_once_locked(&lock, later);
        assert!(r2.status_emitted);
        let open2 = |ev: &NostrEvent| {
            nip59::unwrap_with_author(ev, &guardian_sk(), later, nip59::MAX_JITTER_SECS).ok()
        };
        let statuses2: Vec<String> = futures_events(&mock)
            .iter()
            .filter_map(open2)
            .filter(|(r, a)| {
                r.kind == charter_primitives::kinds::CHARTER_DEVICE_STATUS && *a == machine
            })
            .map(|(r, _)| r.content)
            .collect();
        assert!(
            !statuses2.last().unwrap().contains("pairToken"),
            "expired token must not ride: {}",
            statuses2.last().unwrap()
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The whole ask-for-more-time loop, headless over one in-memory relay:
    /// guardian signs clauses (open schedule + 1-min budget) → phone burns the
    /// budget and LOCKS → phone submits a time.extend ask through the spine
    /// broker (M16 pending-before-publish) → guardian opens the REQUEST wrap,
    /// signs a 30-min allow GRANT echoing reqId/nonce → phone polls, verifies
    /// (consume-before-enact), enacts into the inbox → the next tick drains it
    /// into the enforcing ledger and UNLOCKS. Re-delivered grant is a no-op.
    #[test]
    fn ask_for_more_time_full_loop_unlocks() {
        use charter_primitives::kinds::{CHARTER_DEVICE_REQUEST, GIFT_WRAP};
        use charter_proto::OpType;
        use charter_spine::lifecycle::RequestState;
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};
        use charter_verify::test_support::GrantBuilder;

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        // The broker reads the REAL clock (AndroidClock), so the whole test
        // runs on wall time — the all-day-open schedule keeps it deterministic
        // at any hour, and the wrap-jitter/freshness windows stay satisfied.
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 600).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();
        let mock = MockRelayTransport::new();
        w.wire_test_relay(mock.clone()).unwrap();

        let wrap_to_machine = |ev: &NostrEvent, seed: u8, at: u64| {
            nip59::wrap(
                &Rumor::from_signed_event(ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed.wrapping_add(1); 32],
                    wrap_nonce: [seed.wrapping_add(2); 32],
                    seal_created_at: at,
                    wrap_created_at: at,
                },
            )
            .expect("wrap")
        };

        // Guardian signs an all-day-open schedule + a 1-minute budget.
        let sched = ClauseBuilder::schedule(t0 - 500)
            .body(json!({
                "v": 1, "tz": "Europe/London",
                "weekly": {
                    "mon": [{"start": "00:00", "end": "23:59"}],
                    "tue": [{"start": "00:00", "end": "23:59"}],
                    "wed": [{"start": "00:00", "end": "23:59"}],
                    "thu": [{"start": "00:00", "end": "23:59"}],
                    "fri": [{"start": "00:00", "end": "23:59"}],
                    "sat": [{"start": "00:00", "end": "23:59"}],
                    "sun": [{"start": "00:00", "end": "23:59"}]
                },
                "issuedAt": t0 - 500
            }))
            .build(&guardian);
        let budget = ClauseBuilder::budget(t0 - 499)
            .body(json!({
                "v": 1, "tz": "Europe/London", "dailyMinutes": 1, "issuedAt": t0 - 499
            }))
            .build(&guardian);
        mock.inject(wrap_to_machine(&sched, 20, t0));
        mock.inject(wrap_to_machine(&budget, 30, t0));

        let lock = Mutex::new(Some(w));
        let r = poll_once_locked(&lock, t0);
        assert!(r.polled, "reason: {}", r.reason);

        // Burn the 1-minute budget: init tick, then a 300s active interval.
        let subj = machine.to_hex();
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            w.tick(Some(&subj), true, t0 as i64, None);
            let d = w.tick(Some(&subj), true, t0 as i64 + 300, None);
            assert!(locked(&d), "budget must be exhausted");

            // The child asks for more time (the lock-screen one-shot).
            let req_id = w
                .submit_request(
                    "time.extend",
                    "{\"minutesRequested\":30,\"reason\":\"\",\"limitHit\":\"budget\"}",
                )
                .expect("submit");
            let recs = w.list_requests(10);
            assert_eq!(recs.len(), 1);
            assert_eq!(recs[0].req_id.to_hex(), req_id);
            assert_eq!(recs[0].state, RequestState::Pending);
        }

        // Guardian side: open the REQUEST wrap, echo reqId/nonce in an allow.
        // Grant freshness is checked against the broker's REAL clock (±300s),
        // so the grant is stamped with fresh wall time.
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req_wraps: Vec<NostrEvent> = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            use charter_sys::relay::{Filter, RelayTransport};
            rt.block_on(mock.query(
                &["wss://relay.example".to_string()],
                Filter {
                    kinds: vec![GIFT_WRAP],
                    p_tags: vec![guardian.pubkey()],
                    ..Default::default()
                },
            ))
            .unwrap()
        };
        let request = req_wraps
            .iter()
            .filter_map(|wrp| {
                nip59::unwrap_with_author(wrp, &guardian_sk(), now, nip59::MAX_JITTER_SECS).ok()
            })
            .find(|(r, _)| r.kind == CHARTER_DEVICE_REQUEST)
            .map(|(r, _)| charter_proto::RequestPayload::from_json(&r.content).unwrap())
            .expect("guardian sees the ask");
        assert_eq!(request.op, OpType::TimeExtend);

        let mut gb = GrantBuilder::install_allow(request.req_id, request.nonce)
            .op(OpType::TimeExtend)
            .params(json!({"minutesGranted": 30, "limitHit": "budget"}))
            .ts(now)
            .exp(now + 240);
        gb.created_at = now;
        let grant = gb.build(&guardian);
        mock.inject(wrap_to_machine(&grant, 40, now));

        // Phone polls: verify → enact → inbox; the next tick unlocks.
        let r2 = poll_once_locked(&lock, now);
        assert!(r2.polled);
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            let recs = w.list_requests(10);
            let d = w.tick(Some(&subj), true, t0 as i64 + 310, None);
            assert!(
                !locked(&d),
                "the granted extension must unlock; record={:?} decision={:?}",
                recs.first(),
                d.first().map(|x| (&x.reason, x.remaining_secs))
            );
            assert_eq!(recs[0].state, RequestState::Enacted, "{:?}", recs[0]);
        }

        // A re-delivered grant must not double-extend or flip state (M8/I3).
        let r3 = poll_once_locked(&lock, now + 10);
        assert!(r3.polled);
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            assert_eq!(w.list_requests(10)[0].state, RequestState::Enacted);
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The DENY loop, headless — the case decented hit on 2026-07-26. The child
    /// asks; the guardian says no; the phone must (a) reach RequestState::Denied
    /// and (b) raise the Denied effect EXACTLY ONCE, so the ward is told the
    /// answer wherever they are without being nagged every tick.
    #[test]
    fn a_denied_ask_announces_itself_once() {
        use charter_primitives::kinds::{CHARTER_DEVICE_REQUEST, GIFT_WRAP};
        use charter_proto::OpType;
        use charter_spine::lifecycle::RequestState;
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};
        use charter_verify::test_support::GrantBuilder;

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 600).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();
        let mock = MockRelayTransport::new();
        w.wire_test_relay(mock.clone()).unwrap();

        let wrap_to_machine = |ev: &NostrEvent, seed: u8, at: u64| {
            nip59::wrap(
                &Rumor::from_signed_event(ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed.wrapping_add(1); 32],
                    wrap_nonce: [seed.wrapping_add(2); 32],
                    seal_created_at: at,
                    wrap_created_at: at,
                },
            )
            .expect("wrap")
        };

        let sched = ClauseBuilder::schedule(t0 - 500)
            .body(json!({
                "v": 1, "tz": "Europe/London",
                "weekly": {
                    "mon": [{"start": "00:00", "end": "23:59"}],
                    "tue": [{"start": "00:00", "end": "23:59"}],
                    "wed": [{"start": "00:00", "end": "23:59"}],
                    "thu": [{"start": "00:00", "end": "23:59"}],
                    "fri": [{"start": "00:00", "end": "23:59"}],
                    "sat": [{"start": "00:00", "end": "23:59"}],
                    "sun": [{"start": "00:00", "end": "23:59"}]
                },
                "issuedAt": t0 - 500
            }))
            .build(&guardian);
        let budget = ClauseBuilder::budget(t0 - 499)
            .body(json!({
                "v": 1, "tz": "Europe/London", "dailyMinutes": 1, "issuedAt": t0 - 499
            }))
            .build(&guardian);
        mock.inject(wrap_to_machine(&sched, 20, t0));
        mock.inject(wrap_to_machine(&budget, 30, t0));

        let lock = Mutex::new(Some(w));
        assert!(poll_once_locked(&lock, t0).polled);

        let subj = machine.to_hex();
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            w.tick(Some(&subj), true, t0 as i64, None);
            let d = w.tick(Some(&subj), true, t0 as i64 + 300, None);
            assert!(locked(&d), "budget must be exhausted");
            // This tick also SEEDS the announced set (nothing denied yet), so
            // the assertion below is about a genuinely fresh refusal.
            w.submit_request(
                "time.extend",
                "{\"minutesRequested\":30,\"reason\":\"\",\"limitHit\":\"budget\"}",
            )
            .expect("submit");
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req_wraps: Vec<NostrEvent> = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            use charter_sys::relay::{Filter, RelayTransport};
            rt.block_on(mock.query(
                &["wss://relay.example".to_string()],
                Filter {
                    kinds: vec![GIFT_WRAP],
                    p_tags: vec![guardian.pubkey()],
                    ..Default::default()
                },
            ))
            .unwrap()
        };
        let request = req_wraps
            .iter()
            .filter_map(|wrp| {
                nip59::unwrap_with_author(wrp, &guardian_sk(), now, nip59::MAX_JITTER_SECS).ok()
            })
            .find(|(r, _)| r.kind == CHARTER_DEVICE_REQUEST)
            .map(|(r, _)| charter_proto::RequestPayload::from_json(&r.content).unwrap())
            .expect("guardian sees the ask");

        // The guardian says no: a signed GRANT carrying decision=deny, 0 minutes.
        let mut gb = GrantBuilder::install_allow(request.req_id, request.nonce)
            .op(OpType::TimeExtend)
            .deny()
            .params(json!({"minutesGranted": 0, "limitHit": "budget"}))
            .ts(now)
            .exp(now + 240);
        gb.created_at = now;
        mock.inject(wrap_to_machine(&gb.build(&guardian), 40, now));

        assert!(poll_once_locked(&lock, now).polled);
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            assert_eq!(
                w.list_requests(10)[0].state,
                RequestState::Denied,
                "a signed deny must reach the record"
            );

            let d = w.tick(Some(&subj), true, t0 as i64 + 310, None);
            assert!(
                d.iter()
                    .any(|c| c.effects.iter().any(|e| matches!(e, dto::Effect::Denied))),
                "the ward must be told the answer"
            );
            assert!(locked(&d), "a deny never adds time");

            // ...and never again: a refusal announced every 2 seconds is nagging.
            for i in 311..316 {
                let again = w.tick(Some(&subj), true, t0 as i64 + i, None);
                assert!(
                    !again
                        .iter()
                        .any(|c| c.effects.iter().any(|e| matches!(e, dto::Effect::Denied))),
                    "the denial must be announced exactly once"
                );
            }
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The whole install.apk loop, headless: the child asks to install a
    /// package; the guardian opens the ask and signs an allow that pins the
    /// exact signing-cert; the phone verifies → enacts → PARKS a durable
    /// install directive that Kotlin will drain; reporting `ok` clears it; and
    /// a re-delivered grant never double-queues (I1/M8 through to §2.4).
    #[test]
    fn install_apk_full_loop_queues_then_clears() {
        use charter_primitives::kinds::{CHARTER_DEVICE_REQUEST, GIFT_WRAP};
        use charter_proto::OpType;
        use charter_spine::lifecycle::RequestState;
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};
        use charter_verify::test_support::GrantBuilder;

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 600).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();
        let mock = MockRelayTransport::new();
        w.wire_test_relay(mock.clone()).unwrap();

        let wrap_to_machine = |ev: &NostrEvent, seed: u8, at: u64| {
            nip59::wrap(
                &Rumor::from_signed_event(ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed.wrapping_add(1); 32],
                    wrap_nonce: [seed.wrapping_add(2); 32],
                    seal_created_at: at,
                    wrap_created_at: at,
                },
            )
            .expect("wrap")
        };

        // Child side: ask to install Fenix (parent-staged). The install queue
        // is empty until a guardian-signed allow lands.
        let pkg = "org.mozilla.fenix";
        let cert = "ab".repeat(32); // the SHA-256 the parent pins.
        let lock = Mutex::new(Some(w));
        let req_id = {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            let id = w
                .submit_request(
                    "install.apk",
                    &format!("{{\"packageName\":\"{pkg}\",\"source\":\"staged\"}}"),
                )
                .expect("submit install ask");
            assert!(w.drain_installs(t0).is_empty(), "nothing queued pre-grant");
            id
        };
        let r = poll_once_locked(&lock, t0);
        assert!(r.polled, "reason: {}", r.reason);

        // Guardian side: open the REQUEST wrap, echo reqId/nonce in an allow
        // that pins the package + cert (fresh wall-time for the ±300s window).
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req_wraps: Vec<NostrEvent> = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            use charter_sys::relay::{Filter, RelayTransport};
            rt.block_on(mock.query(
                &["wss://relay.example".to_string()],
                Filter {
                    kinds: vec![GIFT_WRAP],
                    p_tags: vec![guardian.pubkey()],
                    ..Default::default()
                },
            ))
            .unwrap()
        };
        let request = req_wraps
            .iter()
            .filter_map(|wrp| {
                nip59::unwrap_with_author(wrp, &guardian_sk(), now, nip59::MAX_JITTER_SECS).ok()
            })
            .find(|(r, _)| r.kind == CHARTER_DEVICE_REQUEST)
            .map(|(r, _)| charter_proto::RequestPayload::from_json(&r.content).unwrap())
            .expect("guardian sees the ask");
        assert_eq!(request.op, OpType::InstallApk);
        assert_eq!(request.req_id.to_hex(), req_id);

        let mut gb = GrantBuilder::install_allow(request.req_id, request.nonce)
            .op(OpType::InstallApk)
            .params(json!({
                "packageName": pkg,
                "versionCode": 424242,
                "signerCertSha256": cert,
                "source": "staged"
            }))
            .ts(now)
            .exp(now + 240);
        gb.created_at = now;
        let grant = gb.build(&guardian);
        mock.inject(wrap_to_machine(&grant, 40, now));

        // Phone polls: verify → enact → the directive is PARKED for Kotlin.
        let r2 = poll_once_locked(&lock, now);
        assert!(r2.polled);
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            let queued = w.drain_installs(now);
            assert_eq!(queued.len(), 1, "the approved install must be parked");
            assert_eq!(queued[0].req_id, req_id);
            assert_eq!(queued[0].package_name, pkg);
            assert_eq!(queued[0].version_code, Some(424242));
            assert_eq!(queued[0].signer_cert_sha256, cert);
            assert_eq!(queued[0].source, "staged");
            assert_eq!(
                w.list_requests(10)[0].state,
                RequestState::Enacted,
                "the ask is Enacted once queued"
            );
        }

        // A re-delivered grant must NOT double-queue (M8/I3 idempotence).
        let r3 = poll_once_locked(&lock, now + 10);
        assert!(r3.polled);
        {
            let g = lock.lock().unwrap();
            let w = g.as_ref().unwrap();
            assert_eq!(w.drain_installs(now).len(), 1, "still exactly one");
        }

        // Kotlin reports success → the directive clears; a transient failure
        // would have left it for the next tick.
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            w.install_result(&req_id, "transient", "test");
            assert_eq!(w.drain_installs(now).len(), 1, "transient keeps it queued");
            w.install_result(&req_id, "ok", "");
            assert!(w.drain_installs(now).is_empty(), "ok clears the directive");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// C1 (review round 1, 2026-08-03): proves the ANDROID broker wiring
    /// itself — `Warden::build_broker`'s `EnactorRegistry`, not just the
    /// shared `charter-spine` machinery `linux/crates/charterd`'s
    /// `app_open_loop.rs` already proves — actually registers
    /// `AppOpenEnactor`. Before the fix, `app.open` had no grant params at
    /// all, so an allow could never verify; a registry that simply forgot to
    /// register the (now-required) enactor would strand every allow at
    /// `Enacting` -> `Failed` instead, which is exactly the config-only
    /// mistake this test — as opposed to the shared-crate one — would catch.
    #[test]
    fn app_open_full_loop_reaches_enacted() {
        use charter_primitives::kinds::{CHARTER_DEVICE_REQUEST, GIFT_WRAP};
        use charter_proto::OpType;
        use charter_spine::lifecycle::RequestState;
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};
        use charter_verify::test_support::GrantBuilder;

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 600).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();
        let mock = MockRelayTransport::new();
        w.wire_test_relay(mock.clone()).unwrap();

        let wrap_to_machine = |ev: &NostrEvent, seed: u8, at: u64| {
            nip59::wrap(
                &Rumor::from_signed_event(ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed.wrapping_add(1); 32],
                    wrap_nonce: [seed.wrapping_add(2); 32],
                    seal_created_at: at,
                    wrap_created_at: at,
                },
            )
            .expect("wrap")
        };

        let pkg = "com.mojang.minecraftpe";
        let lock = Mutex::new(Some(w));
        let req_id = {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            w.submit_request("app.open", &format!("{{\"pkg\":\"{pkg}\"}}"))
                .expect("submit app.open ask")
        };
        let r = poll_once_locked(&lock, t0);
        assert!(r.polled, "reason: {}", r.reason);

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let req_wraps: Vec<NostrEvent> = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            use charter_sys::relay::{Filter, RelayTransport};
            rt.block_on(mock.query(
                &["wss://relay.example".to_string()],
                Filter {
                    kinds: vec![GIFT_WRAP],
                    p_tags: vec![guardian.pubkey()],
                    ..Default::default()
                },
            ))
            .unwrap()
        };
        let request = req_wraps
            .iter()
            .filter_map(|wrp| {
                nip59::unwrap_with_author(wrp, &guardian_sk(), now, nip59::MAX_JITTER_SECS).ok()
            })
            .find(|(r, _)| r.kind == CHARTER_DEVICE_REQUEST)
            .map(|(r, _)| charter_proto::RequestPayload::from_json(&r.content).unwrap())
            .expect("guardian sees the ask");
        assert_eq!(request.op, OpType::AppOpen);
        assert_eq!(request.req_id.to_hex(), req_id);

        // The answer-signal grant — {pkg, minutesGranted} — echoes pkg
        // verbatim; the AppHold clause that actually opens the app is a
        // separate, guardian-side apps clause, out of scope for this loop.
        let mut gb = GrantBuilder::install_allow(request.req_id, request.nonce)
            .op(OpType::AppOpen)
            .params(json!({"pkg": pkg, "minutesGranted": 30}))
            .ts(now)
            .exp(now + 240);
        gb.created_at = now;
        let grant = gb.build(&guardian);
        mock.inject(wrap_to_machine(&grant, 40, now));

        let r2 = poll_once_locked(&lock, now);
        assert!(r2.polled);
        {
            let g = lock.lock().unwrap();
            let w = g.as_ref().unwrap();
            // Before the fix this stayed Pending forever (an allow could
            // never verify) — the whole point of this test.
            assert_eq!(
                w.list_requests(10)[0].state,
                RequestState::Enacted,
                "{:?}",
                w.list_requests(10)[0]
            );
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Offline resilience (§2.2): with EVERY relay refusing, the STATUS wrap
    /// and a child's ask both PARK in the outbox instead of vanishing; when
    /// the network returns (fresh relay, same disk — a restart, even), the
    /// drain delivers both and the guardian can open the parked ask.
    #[test]
    fn offline_spool_parks_and_drains_after_network_returns() {
        use charter_primitives::kinds::{CHARTER_DEVICE_REQUEST, GIFT_WRAP};
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59;

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 600).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();

        // The network is down: every publish to the pinned relay fails.
        let dead = MockRelayTransport::new().with_failing_relay("wss://relay.example");
        w.wire_test_relay(dead).unwrap();
        let lock = Mutex::new(Some(w));

        let r = poll_once_locked(&lock, t0);
        assert!(r.polled);
        assert!(!r.status_emitted, "nothing can be emitted offline");
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            // The child asks while offline — M16 keeps it Pending, the spool
            // keeps the wrap.
            w.submit_request(
                "time.extend",
                "{\"minutesRequested\":30,\"reason\":\"\",\"limitHit\":\"budget\"}",
            )
            .expect("submit persists + spools even offline");
            let spooled = crate::outbox::Outbox::new(&w.base).len();
            assert_eq!(spooled, 2, "one STATUS (latest-only) + one REQUEST parked");
        }

        // The network returns (fresh relay, same disk) — the drain delivers.
        let alive = MockRelayTransport::new();
        {
            let mut g = lock.lock().unwrap();
            g.as_mut().unwrap().wire_test_relay(alive.clone()).unwrap();
        }
        let r2 = poll_once_locked(&lock, t0 + 30);
        assert!(r2.polled);
        assert!(
            r2.reason.contains("spool: 2 retried"),
            "reason: {}",
            r2.reason
        );
        {
            let g = lock.lock().unwrap();
            assert_eq!(
                crate::outbox::Outbox::new(&g.as_ref().unwrap().base).len(),
                0,
                "outbox drained"
            );
        }

        // The guardian can open the parked ask off the relay.
        let wraps = {
            let rt = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            use charter_sys::relay::{Filter, RelayTransport};
            rt.block_on(alive.query(
                &["wss://relay.example".to_string()],
                Filter {
                    kinds: vec![GIFT_WRAP],
                    p_tags: vec![guardian.pubkey()],
                    ..Default::default()
                },
            ))
            .unwrap()
        };
        let ask = wraps
            .iter()
            .filter_map(|wr| {
                nip59::unwrap_with_author(wr, &guardian_sk(), t0 + 30, nip59::MAX_JITTER_SECS).ok()
            })
            .find(|(r, a)| r.kind == CHARTER_DEVICE_REQUEST && *a == machine);
        assert!(ask.is_some(), "the offline ask reached the guardian");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The parent-gated unpair, headless: a paired+enforcing device honors a
    /// guardian-signed RELEASE — goes inert (unlocked, not-configured) — but
    /// IGNORES a release from anyone else (the child can't forge it), and after
    /// release can re-pair to a DIFFERENT guardian.
    #[test]
    fn guardian_release_unpairs_but_a_forged_one_does_not() {
        use charter_primitives::kinds::CHARTER_DEVICE_RELEASE;
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 100).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();

        // Enforce a paused schedule → locked + configured.
        let sched = ClauseBuilder::schedule(t0 - 50)
            .body(json!({"v":1,"tz":"Europe/London","paused":true,"weekly":{},"issuedAt": t0 - 50}))
            .build(&guardian);
        assert!(
            w.ingest_clause(&serde_json::to_string(&sched).unwrap(), t0)
                .accepted
        );
        let subj = w.pairing_state().subject.unwrap();
        assert!(locked(&w.tick(Some(&subj), true, t0 as i64, None)));

        // A signed RELEASE — but from an ATTACKER, not the guardian.
        let release_wrap = |signer: &charter_sys::signer::SeedSigner, seed: u8| {
            let payload = charter_proto::ReleasePayload {
                v: 1,
                machine,
                issued_at: t0,
            };
            let ev = charter_verify::test_support::sign_event(
                signer,
                CHARTER_DEVICE_RELEASE,
                t0,
                vec![charter_primitives::kinds::marker_tag()],
                payload.to_json(),
            );
            nip59::wrap(
                &Rumor::from_signed_event(&ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed ^ 0x11; 32],
                    wrap_nonce: [seed ^ 0x22; 32],
                    seal_created_at: t0,
                    wrap_created_at: t0,
                },
            )
            .expect("wrap")
        };

        // The attacker's release is on the relay — it must be IGNORED.
        let attacker = charter_sys::signer::SeedSigner::from_seed(0x99);
        let mock = MockRelayTransport::new();
        mock.inject(release_wrap(&attacker, 40));
        w.wire_test_relay(mock.clone()).unwrap();
        let lock = Mutex::new(Some(w));
        let r = poll_once_locked(&lock, t0 + 1);
        assert!(
            !r.released,
            "a forged release must NOT unpair (parent-gated)"
        );
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            assert!(
                w.pairing_state().paired,
                "still paired after a forged release"
            );
            assert!(locked(&w.tick(Some(&subj), true, t0 as i64 + 1, None)));
        }

        // Now the REAL guardian releases it.
        mock.inject(release_wrap(&guardian.signer, 60));
        let r2 = poll_once_locked(&lock, t0 + 2);
        assert!(
            r2.released,
            "the guardian's release must unpair; {}",
            r2.reason
        );
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            let st = w.pairing_state();
            assert!(!st.paired, "unpaired after the guardian release");
            assert!(st.relays.is_empty());
            // The tick now emits an inert + UNLOCKED decision so Kotlin lifts
            // every restriction.
            let d = w.tick(Some(&subj), true, t0 as i64 + 2, None);
            assert!(!locked(&d));
            assert!(!configured(&d), "no charter after release");
        }

        // Survives a restart AND can re-pair to a DIFFERENT guardian (pin-once
        // cleared): the old clauses are gone, so the new pairing starts clean.
        drop(lock);
        let mut w2 = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        assert!(
            !w2.pairing_state().paired,
            "release persisted across restart"
        );
        let other = TestGuardian::from_seed(0x33);
        let uri2 = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            other.pubkey().to_hex()
        );
        assert!(
            w2.pair(&uri2, t0 + 3).is_ok(),
            "re-pair to a new guardian works"
        );
        assert_eq!(
            w2.pairing_state().guardian.as_deref(),
            Some(other.pubkey().to_hex().as_str())
        );
        // No resurrected clauses from the old guardian.
        let subj2 = w2.pairing_state().subject.unwrap();
        assert!(!configured(&w2.tick(
            Some(&subj2),
            true,
            t0 as i64 + 4,
            None
        )));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A release, once applied, keeps living on the relay — the 48h poll
    /// lookback (`POLL_LOOKBACK_SECS`) and the release's own 48h freshness
    /// window mean it authenticates cleanly for up to two days. If the
    /// guardian re-pairs the SAME device to the SAME guardian within that
    /// window (exactly "disconnect, then set up again" — caught live
    /// 2026-07-22), the stale release must NOT re-unpair it: it predates the
    /// new pairing's `paired_at`, so it's superseded, not a fresh instruction.
    /// A genuinely NEW release (issued after the re-pairing) must still work.
    #[test]
    fn redelivered_release_from_before_a_repair_does_not_reunpair() {
        use charter_primitives::kinds::CHARTER_DEVICE_RELEASE;
        use charter_sys::relay::MockRelayTransport;
        use charter_transport::nip59::{self, Rumor, WrapRandomness};

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 100).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();

        let release_wrap = |issued_at: u64, seed: u8| {
            let payload = charter_proto::ReleasePayload {
                v: 1,
                machine,
                issued_at,
            };
            let ev = charter_verify::test_support::sign_event(
                &guardian.signer,
                CHARTER_DEVICE_RELEASE,
                issued_at,
                vec![charter_primitives::kinds::marker_tag()],
                payload.to_json(),
            );
            nip59::wrap(
                &Rumor::from_signed_event(&ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed ^ 0x11; 32],
                    wrap_nonce: [seed ^ 0x22; 32],
                    seal_created_at: issued_at,
                    wrap_created_at: issued_at,
                },
            )
            .expect("wrap")
        };

        // The guardian releases at t0 — applies, as proven by the sibling test.
        let mock = MockRelayTransport::new();
        mock.inject(release_wrap(t0, 10));
        w.wire_test_relay(mock.clone()).unwrap();
        let lock = Mutex::new(Some(w));
        let r = poll_once_locked(&lock, t0 + 1);
        assert!(r.released, "the release must apply; {}", r.reason);
        assert!(
            !lock
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .pairing_state()
                .paired
        );

        // Re-pair to the SAME guardian shortly after, at t0+10.
        {
            let mut g = lock.lock().unwrap();
            let w = g.as_mut().unwrap();
            let uri2 = format!(
                "bunker://{}?relay=wss://relay.example&kind=charter",
                guardian.pubkey().to_hex()
            );
            w.pair(&uri2, t0 + 10).unwrap();
            assert!(w.pairing_state().paired, "re-paired to the same guardian");
            // pair()'s rebuild_relay() clobbers the injected mock — rewire it.
            w.wire_test_relay(mock.clone()).unwrap();
        }

        // The relay still has the OLD (t0) release — a durable relay would
        // return it again inside the 48h lookback. Redeliver it.
        mock.inject(release_wrap(t0, 20));
        let r2 = poll_once_locked(&lock, t0 + 11);
        assert!(
            !r2.released,
            "a release predating the current pairing must be ignored"
        );
        assert!(
            lock.lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .pairing_state()
                .paired,
            "still paired — the stale release did not re-unpair the device"
        );

        // A GENUINELY NEW release, issued AFTER the re-pairing, still works.
        mock.inject(release_wrap(t0 + 20, 30));
        let r3 = poll_once_locked(&lock, t0 + 21);
        assert!(
            r3.released,
            "a release issued after the re-pairing must still apply; {}",
            r3.reason
        );
        assert!(
            !lock
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .pairing_state()
                .paired
        );

        drop(lock);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Sub-minute usage must not render on the ward-facing lock screen
    /// ("0s used today" — issue #41); at a minute and beyond it appears in
    /// minutes, matching the spine's lock_info wording.
    #[test]
    fn used_line_omits_sub_minute_usage() {
        use charter_transport::nip59::{self, Rumor, WrapRandomness};

        let base = temp_base(concat!(module_path!(), line!()));
        let guardian = TestGuardian::new();
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 20).unwrap();
        let uri = format!(
            "bunker://{}?relay=wss://relay.example&kind=charter",
            guardian.pubkey().to_hex()
        );
        let t0 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        w.pair(&uri, t0 - 600).unwrap();
        let machine = PubKey::from_hex(&w.machine_pubkey_hex()).unwrap();
        let mock = charter_sys::relay::MockRelayTransport::new();
        w.wire_test_relay(mock.clone()).unwrap();

        let wrap_to_machine = |ev: &NostrEvent, seed: u8, at: u64| {
            nip59::wrap(
                &Rumor::from_signed_event(ev),
                &guardian_sk(),
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: [seed; 32],
                    seal_nonce: [seed.wrapping_add(1); 32],
                    wrap_nonce: [seed.wrapping_add(2); 32],
                    seal_created_at: at,
                    wrap_created_at: at,
                },
            )
            .expect("wrap")
        };

        // A 90-minute daily budget (no schedule — the budget alone drives
        // used_line; nothing locks inside this test's two minutes).
        let budget = ClauseBuilder::budget(t0 - 499)
            .body(json!({
                "v": 1, "tz": "Europe/London", "dailyMinutes": 90, "issuedAt": t0 - 499
            }))
            .build(&guardian);
        mock.inject(wrap_to_machine(&budget, 40, t0));

        let lock = Mutex::new(Some(w));
        let r = poll_once_locked(&lock, t0);
        assert!(r.polled, "reason: {}", r.reason);

        let mut g = lock.lock().unwrap();
        let w = g.as_mut().unwrap();
        let subj = machine.to_hex();

        // 30 seconds of active use: cap renders, raw seconds do not.
        w.tick(Some(&subj), true, t0 as i64, None);
        w.tick(Some(&subj), true, t0 as i64 + 30, None);
        let li = w.lock_info(t0 as i64 + 30);
        assert_eq!(li.used_line, "up to 1h 30m a day");

        // Past one minute: the suffix appears, in minutes.
        w.tick(Some(&subj), true, t0 as i64 + 90, None);
        let li = w.lock_info(t0 as i64 + 90);
        assert_eq!(li.used_line, "up to 1h 30m a day — 1m used today");

        drop(g);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Drain the mock relay's stored events (query with a match-all filter).
    fn futures_events(mock: &charter_sys::relay::MockRelayTransport) -> Vec<NostrEvent> {
        use charter_sys::relay::{Filter, RelayTransport};
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        rt.block_on(mock.query(
            &["wss://relay.example".to_string()],
            Filter {
                kinds: vec![charter_primitives::kinds::GIFT_WRAP],
                ..Default::default()
            },
        ))
        .unwrap()
    }
}

#[cfg(test)]
mod update_health_tests {

    /// A directive that keeps failing must become VISIBLE. Robin's phone retried
    /// a broken staging path every 17 seconds for two days while MyCharter
    /// showed nothing but an Update button that kept reappearing — identical,
    /// from the guardian's chair, to "still downloading".
    #[test]
    fn repeated_failures_accumulate_and_surface() {
        let base = std::env::temp_dir().join(format!("charter-health-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let q = crate::install::InstallQueue::new(&base);
        let item = crate::install::PendingInstall {
            req_id: "aa".into(),
            package_name: "org.forgesworn.charter".into(),
            version_code: Some(16),
            signer_cert_sha256: "ab".repeat(32),
            source: "url".into(),
            url: Some("https://example/x.apk".into()),
            apk_sha256: Some("cd".repeat(32)),
            at: 1_000,
            attempts: 0,
            last_error: None,
        };
        q.put(&item);

        // Three failed rounds, as the device would report them.
        for _ in 0..3 {
            let mut cur = q.get("aa").expect("still queued");
            cur.attempts = cur.attempts.saturating_add(1);
            cur.last_error = Some("couldn't download the update".into());
            // `put` would be a NO-OP here (it dedupes on reqId and refuses to
            // touch an existing entry) — which silently threw the retry count
            // away until this test caught it.
            q.update(&cur);
        }

        // And `put` must still refuse to resurrect/alter an existing entry.
        let mut sneaky = q.get("aa").expect("queued");
        sneaky.attempts = 999;
        q.put(&sneaky);
        assert_eq!(q.get("aa").unwrap().attempts, 3, "put must not overwrite");

        let got = q.get("aa").expect("a failing directive stays queued");
        assert_eq!(got.attempts, 3, "each failure must be counted");
        assert_eq!(
            got.last_error.as_deref(),
            Some("couldn't download the update")
        );

        // Reading one item must never sweep the queue (the bug that ate it).
        assert!(q.get("aa").is_some());
        assert_eq!(q.list(1_000).len(), 1);

        let _ = std::fs::remove_dir_all(&base);
    }
}

#[cfg(test)]
mod maintenance_span_tests {
    use super::*;

    /// A real warden on a temp base dir, with a ward but no relay.
    fn warden(tag: &str) -> (Warden, String) {
        let base =
            std::env::temp_dir().join(format!("charter-span-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&base);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 1).expect("warden");
        let subject = charter_primitives::PubKey::from_bytes([0x11; 32]);
        w.subject = Some(subject);
        let hex = subject.to_hex();
        (w, hex)
    }

    /// Write the maintenance clause directly — this exercises the SPAN, not
    /// clause verification, which has its own tests.
    fn open_window(w: &Warden, subject_hex: &str, issued_at: u64, until: u64) {
        let body = serde_json::json!({ "v": 1, "untilUnix": until, "issuedAt": issued_at });
        w.child_clauses
            .put_child_clause(
                subject_hex,
                charter_proto::ClauseKind::Maintenance.store_key(),
                issued_at,
                &body.to_string(),
            )
            .expect("stored");
    }

    fn span_of(json: &str) -> (u64, Option<u64>, bool) {
        let v: serde_json::Value = serde_json::from_str(json).expect("json");
        (
            v["startedAt"].as_u64().unwrap_or(0),
            v["endedAt"].as_u64(),
            v["open"].as_bool().unwrap_or(false),
        )
    }

    #[test]
    fn nothing_is_reported_before_any_window_has_opened() {
        let (mut w, _hex) = warden("none");
        assert_eq!(w.maintenance_span(1_000), "{}");
    }

    #[test]
    fn the_span_starts_when_the_device_sees_the_window_and_ends_when_it_shuts() {
        let (mut w, hex) = warden("basic");
        open_window(&w, &hex, 1_000, 2_800);

        // First sight at 1_000 — not the clause's issuedAt, but when we looked.
        let (started, ended, open) = span_of(&w.maintenance_span(1_000));
        assert_eq!(started, 1_000);
        assert_eq!(ended, None);
        assert!(open);

        // Still open later: the start must NOT drift forward, or the account
        // would silently stop covering the beginning of the window.
        let (started, _, open) = span_of(&w.maintenance_span(2_000));
        assert_eq!(started, 1_000, "the start is first sight, once");
        assert!(open);

        // Past the expiry: shut, and bounded by when we noticed.
        let (started, ended, open) = span_of(&w.maintenance_span(3_000));
        assert_eq!(started, 1_000);
        assert_eq!(ended, Some(3_000));
        assert!(!open);
    }

    /**
     * THE case this slot exists for. Closing early publishes a NEWER
     * maintenance clause whose `issuedAt` is the moment of closing — so the
     * open clause, and with it the real start, is gone from the store exactly
     * when the guardian most wants the account. The local record must survive
     * that and still cover the whole window.
     */
    #[test]
    fn an_early_close_cannot_erase_when_the_window_started() {
        let (mut w, hex) = warden("early-close");
        open_window(&w, &hex, 1_000, 2_800);
        let (started, _, open) = span_of(&w.maintenance_span(1_000));
        assert_eq!(started, 1_000);
        assert!(open);

        // The guardian presses Close at 1_500: a clause issued now, already shut.
        open_window(&w, &hex, 1_500, 1_500);

        let (started, ended, open) = span_of(&w.maintenance_span(1_500));
        assert!(!open, "closing early must shut the window");
        assert_eq!(started, 1_000, "the original start must survive the close");
        assert_eq!(
            ended,
            Some(1_500),
            "and the account ends where it was closed"
        );
    }

    #[test]
    fn a_closed_span_stays_reportable_until_the_next_window() {
        let (mut w, hex) = warden("reportable");
        open_window(&w, &hex, 1_000, 1_100);
        w.maintenance_span(1_000);
        let (started, ended, _) = span_of(&w.maintenance_span(2_000));
        assert_eq!((started, ended), (1_000, Some(2_000)));

        // Re-reading it much later must not move or reopen anything: the
        // guardian's account of the last window is a fixed fact.
        let (started, ended, open) = span_of(&w.maintenance_span(9_999));
        assert_eq!((started, ended), (1_000, Some(2_000)));
        assert!(!open);
    }

    /// The ward's notice takes its minutes from THIS number. It was taking them
    /// from the core's one-hour cap instead, so a 30-minute window told the
    /// child they had 60 (Robin's phone, 2026-07-30). A ward must never be
    /// told they have longer than they really do.
    #[test]
    fn an_open_span_reports_the_windows_real_expiry_not_the_cap() {
        let (mut w, hex) = warden("until");
        open_window(&w, &hex, 1_000, 1_000 + 30 * 60);

        let v: serde_json::Value = serde_json::from_str(&w.maintenance_span(1_000)).expect("json");
        assert_eq!(
            v["untilUnix"].as_u64(),
            Some(1_000 + 30 * 60),
            "a 30-minute window must report 30 minutes, not the 1-hour ceiling"
        );

        // Shut: there is no notice to number, so no expiry is claimed.
        let v: serde_json::Value = serde_json::from_str(&w.maintenance_span(9_999)).expect("json");
        assert!(
            v["untilUnix"].is_null(),
            "a shut window claims no time left"
        );
    }

    #[test]
    fn a_second_window_starts_a_fresh_span_rather_than_extending_the_old_one() {
        let (mut w, hex) = warden("second");
        open_window(&w, &hex, 1_000, 1_100);
        w.maintenance_span(1_000);
        w.maintenance_span(2_000); // shut

        open_window(&w, &hex, 5_000, 6_800);
        let (started, ended, open) = span_of(&w.maintenance_span(5_000));
        assert_eq!(
            started, 5_000,
            "a new window must not inherit the old start"
        );
        assert_eq!(ended, None);
        assert!(open);
    }
}

#[cfg(test)]
mod persist_tests {
    use super::*;
    use charter_proto::ClauseKind;
    use serde_json::json;

    // Mon 2026-06-29 18:00 BST — inside the window the schedule below allows.
    const IN_WINDOW: i64 = 1_782_752_400;

    /// A real warden on a temp base dir, with a ward under a week schedule so
    /// the tick is not inert. Returns the ward's hex and the usage file.
    fn ruled_warden(tag: &str) -> (Warden, String, PathBuf) {
        let base =
            std::env::temp_dir().join(format!("charter-persist-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&base);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 1).expect("warden");
        let subject = charter_primitives::PubKey::from_bytes([0x11; 32]);
        w.subject = Some(subject);
        let hex = subject.to_hex();
        let body = json!({
            "v": 1,
            "tz": "Europe/London",
            "weekly": { "mon": [{ "start": "16:00", "end": "20:00" }] },
            "issuedAt": 1
        });
        w.child_clauses
            .put_child_clause(&hex, ClauseKind::Schedule.store_key(), 1, &body.to_string())
            .expect("stored");
        let usage = base.join("usage.json");
        (w, hex, usage)
    }

    /// The battery fix, stated as a property: the loop runs a few times a
    /// second, and a phone that is asleep, locked or simply not being used
    /// credits nothing — so those ticks must not touch the disk at all.
    ///
    /// Asserted by planting a sentinel where the snapshot lives: if the tick
    /// rewrote the file the sentinel is gone. Deliberately not an mtime check,
    /// which two writes in the same millisecond would pass.
    #[test]
    fn an_idle_tick_does_not_rewrite_the_ledger() {
        let (mut w, _hex, usage) = ruled_warden("idle");

        // First tick establishes the file.
        w.tick(None, false, IN_WINDOW, None);
        assert!(usage.exists(), "the first tick should write the ledger");

        std::fs::write(&usage, b"SENTINEL").expect("plant");
        // Dark screen, nothing in the foreground: no accrual, nothing to save.
        w.tick(None, false, IN_WINDOW + 2, None);
        w.tick(None, false, IN_WINDOW + 4, None);
        assert_eq!(
            std::fs::read_to_string(&usage).expect("read"),
            "SENTINEL",
            "an idle tick rewrote a ledger it had not changed",
        );
    }

    /// The other half, and the one that matters more: skipping writes must
    /// never lose time. The moment the ward is actually using the phone the
    /// snapshot changes, and it goes to disk on that very tick — never
    /// deferred, so a kill can't cost the ward's day or hand it back to them.
    #[test]
    fn a_tick_that_accrues_writes_immediately() {
        let (mut w, hex, usage) = ruled_warden("active");

        w.tick(None, false, IN_WINDOW, None);
        std::fs::write(&usage, b"SENTINEL").expect("plant");

        // Screen lit, the ward's own session in the foreground: time accrues.
        w.tick(Some(&hex), true, IN_WINDOW + 60, None);
        assert_ne!(
            std::fs::read_to_string(&usage).expect("read"),
            "SENTINEL",
            "accrued time was left in memory instead of being saved",
        );
    }
}

/// Named-times ("buckets") enforcement on Android — Task 8, the parity
/// headline of the named-times branch: buckets go from stored-but-never-read
/// to actually metered and enforced. Clause bodies are written directly to
/// the child-clause store (bypassing the signature-verification machinery
/// already covered by `mod tests`'s ingest-clause tests) — this module tests
/// the pure meter/suspend/gift-routing logic, mirroring charterd's own
/// `runtime.rs` bucket test suite field-for-field.
#[cfg(test)]
mod bucket_tests {
    use super::*;
    use charter_proto::ClauseKind;
    use serde_json::json;

    // Mon 2026-06-29 17:00 UTC — an ordinary weekday afternoon, far from any
    // day/week boundary so short test spans never straddle a rollover.
    const T0: i64 = 1_782_752_400;

    fn locked(decisions: &[dto::ChildDecision]) -> bool {
        decisions.first().map(|d| d.locked).unwrap_or(false)
    }

    fn play_bucket(daily: Option<u16>, weekly: Option<u16>) -> serde_json::Value {
        json!({
            "id": "play",
            "label": "Play",
            "apps": ["com.mojang.play"],
            "dailyMinutes": daily,
            "weeklyMinutes": weekly,
        })
    }

    fn buckets_body(buckets: Vec<serde_json::Value>, paused: bool) -> serde_json::Value {
        json!({
            "v": 1,
            "buckets": buckets,
            "paused": paused,
            "tz": "UTC",
            "issuedAt": 1,
        })
    }

    /// A real warden, paired to nobody, with a ward under a generous whole-
    /// device daily budget (600 minutes) so ordinary schedule/budget
    /// enforcement never interferes with the bucket assertions — the tick is
    /// never inert and the whole device never locks on its own. The buckets
    /// clause (if any) is planted directly; callers add/replace clauses via
    /// the returned handle + `hex`.
    fn bucket_warden(tag: &str, buckets: Option<serde_json::Value>) -> (Warden, String) {
        let base =
            std::env::temp_dir().join(format!("charter-buckets-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&base);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 1).expect("warden");
        let subject = charter_primitives::PubKey::from_bytes([0x33; 32]);
        w.subject = Some(subject);
        let hex = subject.to_hex();
        let budget = json!({"v": 1, "tz": "UTC", "dailyMinutes": 600, "issuedAt": 1});
        w.child_clauses
            .put_child_clause(&hex, ClauseKind::Budget.store_key(), 1, &budget.to_string())
            .expect("stored budget");
        if let Some(b) = buckets {
            w.child_clauses
                .put_child_clause(&hex, ClauseKind::Buckets.store_key(), 1, &b.to_string())
                .expect("stored buckets");
        }
        (w, hex)
    }

    fn put_buckets(w: &Warden, hex: &str, body: &serde_json::Value) {
        w.child_clauses
            .put_child_clause(hex, ClauseKind::Buckets.store_key(), 2, &body.to_string())
            .expect("stored buckets");
    }

    fn put_gift(w: &Warden, hex: &str, id: &str, minutes: u16, group_id: Option<&str>) {
        let mut body = json!({
            "v": 1,
            "issuedAt": 1,
            "id": id,
            "minutes": minutes,
            "expiresAt": T0 as u64 + 6 * 3600,
        });
        if let Some(gid) = group_id {
            body["groupId"] = json!(gid);
        }
        w.child_clauses
            .put_child_clause(hex, ClauseKind::Gift.store_key(), 1, &body.to_string())
            .expect("stored gift");
    }

    fn view(w: &Warden, now: i64, id: &str) -> serde_json::Value {
        let payload: serde_json::Value = serde_json::from_str(&w.bucket_views_json(now)).unwrap();
        payload["buckets"]
            .as_array()
            .unwrap()
            .iter()
            .find(|b| b["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no bucket {id} in {payload}"))
    }

    /// A real warden with a ward under NO clause at all except `buckets` — no
    /// schedule, no budget. This is the ordinary shape of a family whose ONLY
    /// rule is a named-times bucket ("Play is an hour a day"): MyCharter signs
    /// only the dimensions that changed, so a buckets-only charter is routine,
    /// not an edge case.
    fn bucket_only_warden(tag: &str, buckets: serde_json::Value) -> (Warden, String) {
        let base = std::env::temp_dir().join(format!(
            "charter-buckets-only-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&base);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 1).expect("warden");
        let subject = charter_primitives::PubKey::from_bytes([0x44; 32]);
        w.subject = Some(subject);
        let hex = subject.to_hex();
        w.child_clauses
            .put_child_clause(
                &hex,
                ClauseKind::Buckets.store_key(),
                1,
                &buckets.to_string(),
            )
            .expect("stored buckets");
        (w, hex)
    }

    /// The headline behaviour: a foreground app inside a named bucket credits
    /// BOTH axes of that bucket's meter — alongside, never instead of, the
    /// ordinary screen credit — while the whole device stays unlocked.
    #[test]
    fn a_bucketed_foreground_pkg_credits_the_meter_day_and_week() {
        let (mut w, hex) = bucket_warden(
            "credit",
            Some(buckets_body(vec![play_bucket(Some(60), Some(300))], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 300, Some("com.mojang.play"));
        assert!(!locked(&d), "spending a bucket must never lock the device");

        let play = view(&w, T0 + 300, "play");
        assert_eq!(play["usedSeconds"], 300, "day meter: {play}");
        assert_eq!(play["limitSeconds"], 3600);
        assert_eq!(play["remainingSeconds"], 3300, "day binds: {play}");
        assert_eq!(play["weekLimitSeconds"], 18000);
        assert_eq!(
            play["weekRemainingSeconds"], 17700,
            "week meter must credit alongside the day meter: {play}"
        );
        assert_eq!(play["spent"], false);
        assert_eq!(play["capped"], true);

        // "Bucket time also charges the day": an hour of Minecraft is an hour
        // of screen time AND an hour of Play, not one instead of the other —
        // the whole-device budget pool (600 minutes = 36000s here) must show
        // the SAME 300s drawn down too.
        assert_eq!(
            w.time_left(T0 + 300).budget_secs,
            36_000 - 300,
            "the ordinary screen/budget meter must also move"
        );
    }

    /// Spending the DAY axis closes the bucket's own apps — and ONLY those
    /// apps, never the device.
    #[test]
    fn the_daily_cap_suspends_only_the_bucket_apps() {
        let (mut w, hex) = bucket_warden(
            "daily-cap",
            Some(buckets_body(vec![play_bucket(Some(1), None)], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 61, Some("com.mojang.play"));
        assert!(!locked(&d), "the DEVICE must stay unlocked");

        let suspended = w.bucket_suspensions(T0 + 61);
        assert_eq!(suspended, vec!["com.mojang.play".to_string()]);

        let play = view(&w, T0 + 61, "play");
        assert_eq!(play["remainingSeconds"], 0);
        assert_eq!(play["spent"], true);
    }

    /// The weekly wall binds even with daily headroom left — the two axes
    /// are each independently binding, never summed.
    #[test]
    fn the_weekly_wall_closes_the_bucket_while_daily_headroom_remains() {
        let (mut w, hex) = bucket_warden(
            "weekly-cap",
            Some(buckets_body(vec![play_bucket(Some(60), Some(1))], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        w.tick(Some(&hex), true, T0 + 61, Some("com.mojang.play"));

        let suspended = w.bucket_suspensions(T0 + 61);
        assert_eq!(
            suspended,
            vec!["com.mojang.play".to_string()],
            "the weekly wall alone must close the bucket"
        );
        let play = view(&w, T0 + 61, "play");
        assert_eq!(play["usedSeconds"], 61, "the day axis has plenty left");
        assert_eq!(play["weekRemainingSeconds"], 0);
        assert_eq!(play["remainingSeconds"], 0, "the tighter axis binds");
    }

    /// A named-times gift re-opens BOTH walls today — the extension pool is
    /// one shared pot per bucket, not per-axis. M-4 (2026-08-03): the
    /// ward-facing `remainingSeconds` must count down HONESTLY through the
    /// granted surplus — cap PLUS extra, minus what's actually been spent —
    /// not clamp/freeze at the base cap the moment the gift lands (the old
    /// shape read a flat, unmoving 60s here forever, hardware-proven as
    /// "15m of 15m left" frozen through 18m24s of continued play).
    #[test]
    fn a_group_gift_reopens_both_walls_today() {
        let (mut w, hex) = bucket_warden(
            "group-gift",
            Some(buckets_body(vec![play_bucket(Some(1), None)], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        w.tick(Some(&hex), true, T0 + 60, Some("com.mojang.play"));
        assert_eq!(
            w.bucket_suspensions(T0 + 60),
            vec!["com.mojang.play".to_string()],
            "spent before the gift"
        );

        put_gift(&w, &hex, "extra-play", 10, Some("play"));
        let d = w.tick(Some(&hex), true, T0 + 60, None);
        assert!(
            d[0].effects
                .iter()
                .any(|e| matches!(e, dto::Effect::Granted { minutes: 10 })),
            "the gift must ride as a Granted effect: {:?}",
            d[0].effects
        );
        assert!(
            w.bucket_suspensions(T0 + 60).is_empty(),
            "a group gift must reopen the bucket the same tick"
        );
        let v1 = view(&w, T0 + 60, "play");
        assert_eq!(
            v1["limitSeconds"], 660,
            "the shown cap grows by the 600s gift: 60s base + 600s extra: {v1}"
        );
        assert_eq!(
            v1["remainingSeconds"], 600,
            "660s total - 60s already spent, honestly: {v1}"
        );
        assert_eq!(v1["spent"], false);

        // The ward keeps playing INTO the surplus: the clock must MOVE
        // between ticks, never sit pinned at v1's figure.
        let d2 = w.tick(Some(&hex), true, T0 + 120, Some("com.mojang.play"));
        assert!(
            !d2[0].locked,
            "the device must stay unlocked in the surplus"
        );
        assert!(
            w.bucket_suspensions(T0 + 120).is_empty(),
            "the surplus must still be open"
        );
        let v2 = view(&w, T0 + 120, "play");
        assert_eq!(v2["usedSeconds"], 120, "raw spend keeps rising: {v2}");
        assert_eq!(v2["remainingSeconds"], 540, "660s - 120s spent: {v2}");
        assert!(
            // Unwrapped, not compared as `Option<i64>`: `None < Some(_)` is
            // `true` in `Option`'s own `Ord` impl, so if `remainingSeconds`
            // were ever renamed or dropped on one side this would keep
            // passing vacuously instead of failing loudly (found in review).
            v2["remainingSeconds"]
                .as_i64()
                .expect("remainingSeconds present")
                < v1["remainingSeconds"]
                    .as_i64()
                    .expect("remainingSeconds present"),
            "remaining must move between two ticks inside the surplus: {v1} -> {v2}"
        );

        // Only once the FULL 660s (cap+extra) is spent does the bucket close
        // again — not the moment the base 60s cap alone was reached. Ticked
        // in <=100s steps (a real device ticks every few seconds; a single
        // multi-hundred-second jump would hit the background power-pass
        // accrual clamp and under-credit, see `background-power-shipped`).
        let mut t = T0 + 120;
        while t < T0 + 660 {
            t = (t + 100).min(T0 + 660);
            w.tick(Some(&hex), true, t, Some("com.mojang.play"));
        }
        assert_eq!(
            w.bucket_suspensions(T0 + 660),
            vec!["com.mojang.play".to_string()],
            "spent only after the FULL cap+extra is used: {}",
            view(&w, T0 + 660, "play")
        );
        let v3 = view(&w, T0 + 660, "play");
        assert_eq!(v3["remainingSeconds"], 0);
        assert_eq!(v3["spent"], true);
    }

    /// A gift naming a group that does not exist in the ward's CURRENT
    /// `buckets` clause must not vanish — it falls back to the whole-device
    /// pool, exactly like a groupId-less gift, and must NOT reopen the
    /// (unrelated) spent bucket it happened to almost-name.
    #[test]
    fn an_unknown_group_gift_falls_back_to_the_device_and_never_touches_the_bucket() {
        let (mut w, hex) = bucket_warden(
            "unknown-group",
            Some(buckets_body(vec![play_bucket(Some(1), None)], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        w.tick(Some(&hex), true, T0 + 60, Some("com.mojang.play"));
        assert_eq!(
            w.bucket_suspensions(T0 + 60),
            vec!["com.mojang.play".to_string()]
        );

        put_gift(&w, &hex, "misdirected", 10, Some("does-not-exist"));
        let before = w.time_left(T0 + 60).budget_secs;
        let d = w.tick(Some(&hex), true, T0 + 60, None);
        assert!(
            d[0].effects
                .iter()
                .any(|e| matches!(e, dto::Effect::Granted { minutes: 10 })),
            "the gift must still land somewhere: {:?}",
            d[0].effects
        );
        let after = w.time_left(T0 + 60).budget_secs;
        assert!(
            after > before,
            "an unknown group must fall back to the whole-device budget pool \
             (before={before}, after={after})"
        );
        assert_eq!(
            w.bucket_suspensions(T0 + 60),
            vec!["com.mojang.play".to_string()],
            "the unrelated bucket must stay exactly as spent as it was"
        );
    }

    /// An app outside every bucket is ordinary screen time: it credits no
    /// bucket meter and can never be bucket-suspended.
    #[test]
    fn a_non_bucketed_pkg_credits_nothing_and_never_suspends() {
        let (mut w, hex) = bucket_warden(
            "non-bucketed",
            Some(buckets_body(vec![play_bucket(Some(1), None)], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.other.app"));
        let d = w.tick(Some(&hex), true, T0 + 300, Some("com.other.app"));
        assert!(!locked(&d));

        let play = view(&w, T0 + 300, "play");
        assert_eq!(
            play["usedSeconds"], 0,
            "an unrelated app must credit nothing"
        );
        assert!(w.bucket_suspensions(T0 + 300).is_empty());
    }

    /// A malformed `buckets` clause must suspend nothing — a cap that cannot
    /// be trusted must never confiscate an app the ward is entitled to.
    #[test]
    fn a_malformed_buckets_clause_suspends_nothing() {
        let (mut w, hex) = bucket_warden("malformed", None);
        w.child_clauses
            .put_child_clause(&hex, ClauseKind::Buckets.store_key(), 1, "not json at all")
            .expect("stored garbage");
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 3700, Some("com.mojang.play"));
        assert!(!locked(&d));
        assert!(w.bucket_suspensions(T0 + 3700).is_empty());
        assert!(
            w.bucket_views_json(T0 + 3700).contains("\"buckets\":[]"),
            "an unreadable clause must show no buckets, not guess: {}",
            w.bucket_views_json(T0 + 3700)
        );
    }

    /// A paused set FAILS OPEN: the family still sees what was spent (the
    /// meter kept running while it was live), but nothing is confiscated once
    /// the guardian lifts the cap — even though the stored meter is already
    /// over it.
    #[test]
    fn a_paused_set_suspends_nothing_even_though_the_meter_is_already_spent() {
        let (mut w, hex) = bucket_warden(
            "paused",
            Some(buckets_body(vec![play_bucket(Some(1), None)], false)),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        w.tick(Some(&hex), true, T0 + 61, Some("com.mojang.play"));
        assert_eq!(
            w.bucket_suspensions(T0 + 61),
            vec!["com.mojang.play".to_string()],
            "spent while live"
        );

        put_buckets(
            &w,
            &hex,
            &buckets_body(vec![play_bucket(Some(1), None)], true),
        );
        assert!(
            w.bucket_suspensions(T0 + 61).is_empty(),
            "pausing must lift the confiscation immediately"
        );
        let play = view(&w, T0 + 61, "play");
        assert_eq!(
            play["limitSeconds"], 0,
            "paused reads as no day limit: {play}"
        );
        assert_eq!(play["remainingSeconds"], 0);
        assert_eq!(play["weekLimitSeconds"], -1, "paused: {play}");
        assert_eq!(play["weekRemainingSeconds"], -1);
        assert_eq!(play["capped"], false);
    }

    /// The `bucketViewsJson` shape's `-1` sentinels: a daily-only bucket must
    /// leave its week fields unset (never `0`, which would collide with
    /// "the week is used up"), and a weekly-only bucket must show ITS wall as
    /// the reported `limitSeconds`/`remainingSeconds` rather than a phantom
    /// zero from the missing day axis.
    #[test]
    fn bucket_views_json_reports_unset_sentinels_correctly() {
        let (mut w, hex) = bucket_warden(
            "sentinels",
            Some(buckets_body(
                vec![
                    json!({
                        "id": "daily-only", "label": "Daily", "apps": ["a.app"],
                        "dailyMinutes": 30,
                    }),
                    json!({
                        "id": "weekly-only", "label": "Weekly", "apps": ["b.app"],
                        "weeklyMinutes": 120,
                    }),
                ],
                false,
            )),
        );
        w.tick(Some(&hex), true, T0, None);

        let daily_only = view(&w, T0, "daily-only");
        assert_eq!(daily_only["limitSeconds"], 1800);
        assert_eq!(daily_only["weekLimitSeconds"], -1, "{daily_only}");
        assert_eq!(daily_only["weekRemainingSeconds"], -1, "{daily_only}");

        let weekly_only = view(&w, T0, "weekly-only");
        assert_eq!(
            weekly_only["limitSeconds"], 7200,
            "a weekly-only bucket shows its OWN wall, not a phantom zero: {weekly_only}"
        );
        assert_eq!(weekly_only["remainingSeconds"], 7200);
        assert_eq!(weekly_only["weekLimitSeconds"], 7200);
    }

    // --- Task 4: Android tolerance for the `cmdline:` identity form ---------
    //
    // `cmdline:` (charter-schedule's `is_cmdline_id`/`cmdline_needle`) names a
    // RUNNING PROCESS's command line, built for Linux's JVM-vs-launcher
    // problem (a renamed/swapped Minecraft launcher). On Android an identity
    // IS a package name and there is no launcher-vs-process ambiguity at
    // all — `cmdline:` is simply meaningless here. A charter that names one
    // anyway (authored on the Linux side of a cross-platform family, copied
    // by mistake, or just stale) must still validate, must never match a
    // real Android package (`bucket_for_app`/foreground attribution is exact
    // string equality — never a substring/heuristic search the way Linux's
    // flatpak-id recognition is), must never panic, and must never stop the
    // REST of the bucket from working.

    /// A bucket carrying both a `cmdline:` identity and a real Android
    /// package still validates, still credits/suspends the real package
    /// exactly as if the `cmdline:` entry were not there, and the `cmdline:`
    /// entry itself never matches any real foreground package — including
    /// one that happens to equal the bare NEEDLE `cmdline_needle` would
    /// extract, proving this is exact-string matching, never a substring
    /// search of the kind that bit Linux's flatpak-id recognition.
    #[test]
    fn a_cmdline_identity_in_a_bucket_never_matches_an_android_pkg_but_the_bucket_still_works() {
        let (mut w, hex) = bucket_warden(
            "cmdline-tolerant",
            Some(buckets_body(
                vec![json!({
                    "id": "play",
                    "label": "Play",
                    "apps": [
                        "com.mojang.play",
                        "cmdline:net.minecraft.client.main.Main",
                    ],
                    "dailyMinutes": 1,
                })],
                false,
            )),
        );

        // The clause VALIDATES (buckets_clause()/is_valid() accept it): the
        // real package still credits and, once its own cap is spent, still
        // suspends — exactly as if the cmdline: entry were never listed.
        // `bucket_suspensions` echoes back the WHOLE spent bucket's app list
        // (Kotlin then suspends each by name), so the inert `cmdline:` string
        // rides along in the list too — same as it would for a second real
        // package sharing the group. That is harmless: no installed Android
        // package is ever literally named `cmdline:…`, so suspending "it" is
        // a no-op, never a real app.
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 61, Some("com.mojang.play"));
        assert!(!locked(&d), "the DEVICE must stay unlocked");
        let suspended = w.bucket_suspensions(T0 + 61);
        assert!(
            suspended.contains(&"com.mojang.play".to_string()),
            "the real package still spends and suspends normally: {suspended:?}"
        );
        assert_eq!(
            suspended.len(),
            2,
            "just the bucket's own two listed identities, nothing else swept in: {suspended:?}"
        );
        let play = view(&w, T0 + 61, "play");
        assert_eq!(
            play["usedSeconds"], 61,
            "only the real package's play time was credited, no crash reading the view: {play}"
        );

        // A foreground pkg equal to the bare NEEDLE (what `cmdline_needle`
        // would extract from the identity, minus the `cmdline:` prefix) must
        // NOT be treated as a match either — Android attribution never does
        // prefix/substring reasoning on identities, only exact equality.
        let before = w.bucket_suspensions(T0 + 61);
        w.tick(
            Some(&hex),
            true,
            T0 + 61,
            Some("net.minecraft.client.main.Main"),
        );
        assert_eq!(
            w.bucket_suspensions(T0 + 61),
            before,
            "a foreground pkg equal to the cmdline needle must not match the cmdline: identity"
        );
        // And it must not have been silently credited into the bucket either.
        let play_after = view(&w, T0 + 61, "play");
        assert_eq!(
            play_after["usedSeconds"], 61,
            "the bare needle must credit nothing: {play_after}"
        );
    }

    /// A bucket made up ENTIRELY of a `cmdline:` identity (no real Android
    /// package in it at all — the degenerate case of a Linux-authored
    /// bucket landing on an Android ward) still validates and metering
    /// simply never fires for it: no crash, no suspension, nothing credited.
    #[test]
    fn a_bucket_that_is_only_a_cmdline_identity_validates_and_meters_nothing() {
        let (mut w, hex) = bucket_warden(
            "cmdline-only",
            Some(buckets_body(
                vec![json!({
                    "id": "play",
                    "label": "Play",
                    "apps": ["cmdline:net.minecraft.client.main.Main"],
                    "dailyMinutes": 1,
                })],
                false,
            )),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 3700, Some("com.mojang.play"));
        assert!(!locked(&d), "no crash, no lock");
        assert!(
            w.bucket_suspensions(T0 + 3700).is_empty(),
            "a cmdline-only bucket has no real Android package to suspend"
        );
        let play = view(&w, T0 + 3700, "play");
        assert_eq!(
            play["usedSeconds"], 0,
            "an unmatchable identity credits nothing: {play}"
        );
        assert_eq!(play["spent"], false);
    }

    /// `unrecognisedTodaySecs` (STATUS) and `AppRef.userInstalled` are
    /// Linux-only concepts (the JVM-vs-launcher unrecognised-process counter,
    /// and the ward-writable `.desktop`-scan inventory flag respectively) —
    /// Android has no equivalent scan and never stamps either field. STATUS
    /// must omit them (absent, not a zero/false value pretending to mean
    /// something), and reading an inventory payload that happens to carry a
    /// `userInstalled` key (e.g. copied from a cross-platform export) must
    /// not crash `set_installed_apps`.
    #[test]
    fn unrecognised_secs_and_user_installed_are_absent_from_android_status() {
        let (mut w, hex) = bucket_warden("android-omits", None);
        w.set_installed_apps(r#"[{"pkg":"com.mojang.play","label":"Minecraft"}]"#);
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        w.tick(Some(&hex), true, T0 + 60, Some("com.mojang.play"));

        let status = w.build_status(T0 as u64 + 60).expect("STATUS must build");
        assert!(
            status.unrecognised_today_secs.is_none(),
            "Android never stamps unrecognisedTodaySecs: {status:?}"
        );
        let json = status.to_json();
        assert!(
            !json.contains("unrecognisedTodaySecs"),
            "must be absent from the wire, not a zero: {json}"
        );
        assert!(!json.contains("userInstalled"), "wire: {json}");

        let apps = status.apps.expect("the inventory rides STATUS");
        assert_eq!(apps[0].user_installed, None, "Android never sets it");

        // A cross-platform-looking payload that DOES carry `userInstalled`
        // must still parse without crashing `set_installed_apps` — it is
        // simply an additive, optional field on the shared `AppRef` shape.
        w.set_installed_apps(
            r#"[{"pkg":"com.mojang.play","label":"Minecraft","userInstalled":true}]"#,
        );
        let status2 = w
            .build_status(T0 as u64 + 60)
            .expect("STATUS must still build");
        let apps2 = status2.apps.expect("the inventory still rides STATUS");
        assert_eq!(
            apps2[0].user_installed,
            Some(true),
            "a supplied userInstalled value parses fine, it's just never Android's OWN doing"
        );
    }

    /// The mirror image of the test just above: `unrecognisedTodaySecs` is
    /// Linux-only and Android never stamps it, but out-of-hours use (spec
    /// 2026-08-03) is the REVERSE — the `alwaysavailable` clause that
    /// produces it exists only on Android, so this is the ONE platform that
    /// ever stamps `outOfHoursTodaySecs`/`outOfHoursWeekSecs`/
    /// `outOfHoursNightsWeek`. Pins that once the ledger's out-of-hours
    /// meter is genuinely non-zero, STATUS carries all three, camelCase, on
    /// the wire.
    #[test]
    fn android_stamps_out_of_hours_once_the_ledger_is_non_zero() {
        let (mut w, hex) = bucket_warden("android-out-of-hours", None);
        // A schedule closed by T0 (17:00 UTC / 18:00 BST Monday) every week —
        // the ward is schedule-locked for the whole test.
        w.child_clauses
            .put_child_clause(
                &hex,
                ClauseKind::Schedule.store_key(),
                1,
                &json!({"v":1,"tz":"Europe/London","issuedAt":1,
                        "weekly":{"mon":[{"start":"07:00","end":"16:00"}]}})
                .to_string(),
            )
            .expect("stored schedule");
        w.child_clauses
            .put_child_clause(
                &hex,
                ClauseKind::AlwaysAvailable.store_key(),
                1,
                &json!({"v":1,"issuedAt":1,"apps":[{"pkg":"com.mojang.play"}]}).to_string(),
            )
            .expect("stored alwaysavailable");

        // First tick establishes the ledger (the interval a tick credits is
        // since the PRIOR tick, same idiom as every other accrual test here).
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 300, Some("com.mojang.play"));
        assert!(locked(&d), "the window closed at 16:00; T0 is 18:00 BST");

        let status = w.build_status(T0 as u64 + 300).expect("STATUS must build");
        assert_eq!(
            status.out_of_hours_today_secs,
            Some(300),
            "Android DOES stamp outOfHoursTodaySecs: {status:?}"
        );
        assert_eq!(status.out_of_hours_week_secs, Some(300));
        assert_eq!(status.out_of_hours_nights_week, Some(1));
        let json = status.to_json();
        assert!(json.contains("\"outOfHoursTodaySecs\":300"), "{json}");
        assert!(json.contains("\"outOfHoursWeekSecs\":300"), "{json}");
        assert!(json.contains("\"outOfHoursNightsWeek\":1"), "{json}");
    }

    // --- review round 1 fixes (2026-08-02) ------------------------------------

    /// CRITICAL: a buckets-only ward (no schedule clause, no budget clause —
    /// nothing but "Play is an hour a day") is a routine charter, not a
    /// degenerate one: MyCharter signs only the dimensions that changed.
    /// Metering, suspension, the ward's own view, STATUS groups, and
    /// `configured` must all work exactly as when a budget clause happens to
    /// also be present — before this fix `tick()` went inert before
    /// `ensure_ledgers` ever ran, so a buckets-only charter silently never
    /// metered at all.
    #[test]
    fn a_buckets_only_ward_is_metered_suspended_and_reported() {
        let (mut w, hex) = bucket_only_warden(
            "liveness",
            buckets_body(vec![play_bucket(Some(1), None)], false),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        let d = w.tick(Some(&hex), true, T0 + 61, Some("com.mojang.play"));
        assert!(
            !locked(&d),
            "no schedule/budget clause at all means the DEVICE stays unlocked"
        );
        assert!(
            d[0].configured,
            "a buckets-only charter is a real, signed charter — configured must be true"
        );

        assert_eq!(
            w.bucket_suspensions(T0 + 61),
            vec!["com.mojang.play".to_string()],
            "the daily cap must close the bucket even with no other clause in force"
        );
        let play = view(&w, T0 + 61, "play");
        assert_eq!(play["usedSeconds"], 61);
        assert_eq!(play["spent"], true);

        let status = w.build_status(T0 as u64 + 61).expect("STATUS must build");
        let groups = status
            .groups
            .expect("STATUS must report groups for a buckets-only ward");
        let play_group = groups
            .iter()
            .find(|g| g.id == "play")
            .expect("the play group must be in STATUS");
        assert_eq!(
            play_group.day_secs, 61,
            "STATUS groups must meter even with no budget clause"
        );
    }

    /// CRITICAL (Task 9 round): `time_left()` carried the SAME pre-fourth-
    /// condition inert check `tick()` had before the round-1 fix, but never
    /// got the matching correction — a buckets-only ward's mirror/widget
    /// rendered `known: false` ("unmanaged/unknown") even while the bucket
    /// itself was live and metering correctly. The whole-device view must
    /// come back known+unlocked+unbounded (spending a bucket never locks the
    /// device), so the mirror can show the group rows without a false
    /// "waiting to hear from your guardian" banner on top of them.
    #[test]
    fn time_left_is_known_for_a_buckets_only_ward() {
        let (mut w, hex) = bucket_only_warden(
            "time-left-liveness",
            buckets_body(vec![play_bucket(Some(60), None)], false),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));

        let tl = w.time_left(T0);
        assert!(
            tl.known,
            "a buckets-only ward is a real, signed charter — time_left must be known"
        );
        assert!(
            !tl.locked,
            "no schedule/budget clause means the whole device stays unlocked"
        );
        assert_eq!(tl.effective_secs, -1, "unbounded at the device level");
        assert_eq!(tl.reason, "none");
    }

    /// IMPORTANT: pausing the ONLY clause a buckets-only ward has must not
    /// make the ward look unpaired/unconfigured again — `configured` latches
    /// on ANY valid buckets clause ever seen (I22-style), and a paused set is
    /// still a stored clause the mirror should show (fields zeroed, nothing
    /// suspended), not silence.
    #[test]
    fn a_paused_buckets_only_clause_stays_configured_and_suspends_nothing() {
        let (mut w, hex) = bucket_only_warden(
            "paused-only-liveness",
            buckets_body(vec![play_bucket(Some(60), None)], true),
        );
        let d = w.tick(Some(&hex), true, T0, Some("com.mojang.play"));
        assert!(
            d[0].configured,
            "a paused-but-valid buckets clause is still a real charter"
        );
        assert!(!locked(&d), "pausing never locks the device");
        assert!(
            w.bucket_suspensions(T0).is_empty(),
            "a paused set confiscates nothing"
        );

        let tl = w.time_left(T0);
        assert!(
            tl.known,
            "a paused-but-present buckets clause is still a liveness signal"
        );
        assert!(!tl.locked);
    }

    /// CRITICAL (Task 9 round): `schedule_view()` — the D8 "Your charter"
    /// mirror BOTH the phone screen and the home-screen widget render from —
    /// carried its OWN, independent copy of the same pre-fourth-condition
    /// liveness gate `time_left()` had, still unfixed. A buckets-only ward
    /// saw "No charter set yet" headlined directly above its own correctly-
    /// metered named-times rows. `secondsLeft` must also come back as the
    /// `-1` "unbounded" sentinel here, never a clamped `0` — the whole
    /// device has no time wall to count down at all when only a named-times
    /// bucket is in force, and `0` reads as "about to lock any second".
    #[test]
    fn schedule_view_is_populated_and_unbounded_for_a_buckets_only_ward() {
        let (mut w, hex) = bucket_only_warden(
            "schedule-view-liveness",
            buckets_body(vec![play_bucket(Some(60), None)], false),
        );
        w.tick(Some(&hex), true, T0, Some("com.mojang.play"));

        let raw = w.schedule_view(T0);
        assert!(
            !raw.is_empty(),
            "a buckets-only ward is a real, signed charter — the D8 mirror must not read \"\""
        );
        let view: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            view["locked"], false,
            "spending a bucket never locks the device"
        );
        assert_eq!(
            view["secondsLeft"], -1,
            "no whole-device wall at all: unbounded, not a clamped 0: {view}"
        );
        assert_eq!(view["minutesLeft"], -1, "{view}");
    }

    /// IMPORTANT: the bucket meter's week key must roll on the BUCKETS
    /// clause's own tz + weekStart when no budget clause overrides it —
    /// mirrors charterd's `MultiChildEnforcer::tick_attributed` `usage_tz`
    /// precedence exactly (budget wins when in force; else the buckets
    /// clause's own tz/weekStart; else the ordinary fallback). Before this
    /// fix the ledger always used `enforcement_tz_of(schedule, budget)` +
    /// Monday, so a buckets-only family that chose "week starts Sunday" in a
    /// non-UTC tz had it silently ignored.
    #[test]
    fn the_buckets_clauses_own_tz_and_week_start_govern_the_bucket_week_roll() {
        // Auckland Sunday 2026-06-28 00:00 NZST = 2026-06-27 12:00 UTC — still
        // SATURDAY in UTC across this whole timeline, so a tz=UTC/weekStart=Mon
        // fallback would see NO rollover at all here; only reading the buckets
        // clause's OWN tz+weekStart rolls the week at the right instant. Each
        // gap stays under `MAX_TICK_ELAPSED_SECS` (300s, the background-power
        // accrual clamp) so the elapsed credited is exactly what it looks like.
        const BOUNDARY: i64 = 1_782_561_600; // Sun 2026-06-28 00:00 NZST
        const T1: i64 = BOUNDARY - 250; // Sat 2026-06-27 11:55:50 UTC
        const T2: i64 = T1 + 150; // BOUNDARY - 100, still Saturday everywhere
        const T3: i64 = T2 + 220; // BOUNDARY + 120, crosses into Sunday NZST

        let body = json!({
            "v": 1,
            "buckets": [{
                "id": "play",
                "label": "Play",
                "apps": ["com.mojang.play"],
                "weeklyMinutes": 200,
            }],
            "tz": "Pacific/Auckland",
            "weekStart": "sun",
            "issuedAt": 1,
        });
        let (mut w, hex) = bucket_only_warden("tz-precedence", body);

        w.tick(Some(&hex), true, T1, Some("com.mojang.play"));
        w.tick(Some(&hex), true, T2, Some("com.mojang.play"));
        let before = view(&w, T2, "play");
        assert_eq!(
            before["weekRemainingSeconds"],
            12_000 - 150,
            "150s spent before the Auckland-Sunday boundary: {before}"
        );

        w.tick(Some(&hex), true, T3, Some("com.mojang.play"));
        let after = view(&w, T3, "play");
        assert_eq!(
            after["weekRemainingSeconds"],
            12_000 - 220,
            "the week must have ROLLED at Sunday-in-Auckland, so only THIS \
             tick's 220s counts — a Monday/UTC default would instead show \
             12000-(150+220)=11630 (the pre-boundary 150s still counted): {after}"
        );
    }
}

/// The boot-gap watch (S1): the warden counting the boots it did NOT run
/// through, which is the only trace an Android safe-mode session leaves.
#[cfg(test)]
mod boot_watch_tests {
    use super::*;

    /// A real warden on a temp base dir, with a ward but no relay.
    fn warden(tag: &str) -> Warden {
        let base =
            std::env::temp_dir().join(format!("charter-boot-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&base);
        let mut w = Warden::init(base.to_str().unwrap(), "enforce", 1).expect("warden");
        w.subject = Some(charter_primitives::PubKey::from_bytes([0x11; 32]));
        w
    }

    /// Reopen the SAME base dir, as a process restart would.
    fn restart(w: &Warden) -> Warden {
        let mut n = Warden::init(w.base.to_str().unwrap(), "enforce", 1).expect("warden");
        n.subject = w.subject;
        n
    }

    #[test]
    fn a_first_run_takes_a_baseline_and_accuses_nobody() {
        let mut w = warden("first");
        // A phone that was used for a year before Charter arrived shows up
        // with a high count; that history is not ours to report on.
        w.note_boot(412, 1_000);
        assert_eq!(w.enforcement_gap(), None, "a baseline is not a gap");
        assert_eq!(w.boot_watch.last_boot_count, 412);
    }

    #[test]
    fn an_ordinary_reboot_is_not_a_gap() {
        let mut w = warden("ordinary");
        w.note_boot(10, 1_000);
        let mut w = restart(&w);
        w.note_boot(11, 2_000);
        assert_eq!(w.enforcement_gap(), None);
    }

    #[test]
    fn a_service_restart_inside_one_boot_is_not_a_gap() {
        let mut w = warden("service");
        w.note_boot(10, 1_000);
        // START_STICKY, a crash-restart, an in-place update: same boot.
        w.note_boot(10, 1_100);
        w.note_boot(10, 1_200);
        assert_eq!(w.enforcement_gap(), None);
    }

    /*
     * THE case this exists for. A safe-mode session is two boots — one into
     * safe mode, one back out — and the warden runs through neither, so it
     * comes back to a counter two ahead of the one it left.
     */
    #[test]
    fn a_safe_mode_holiday_is_counted_and_stamped_on_status() {
        let mut w = warden("safemode");
        w.note_boot(10, 1_000);
        let mut w = restart(&w);
        w.note_boot(12, 5_000);
        let gap = w.enforcement_gap().expect("the holiday is reported");
        assert_eq!(gap.unexplained_boots, 1, "boot 11 happened without us");
        assert_eq!(
            gap.last_noticed_at, 5_000,
            "when we NOTICED — nothing was running when it began"
        );
        let status = w.build_status(5_000).expect("status");
        assert_eq!(status.enforcement_gap, Some(gap));
    }

    #[test]
    fn gaps_accumulate_and_survive_a_restart() {
        let mut w = warden("accumulate");
        w.note_boot(10, 1_000);
        let mut w = restart(&w);
        w.note_boot(12, 2_000);
        let mut w = restart(&w);
        w.note_boot(14, 3_000);
        assert_eq!(w.enforcement_gap().unwrap().unexplained_boots, 2);
        // Nothing clears it: a counter a ward can reset by waiting is not a
        // counter. Ordinary boots after the fact keep the tally.
        let mut w = restart(&w);
        w.note_boot(15, 4_000);
        let gap = w.enforcement_gap().expect("still reported");
        assert_eq!(gap.unexplained_boots, 2);
        assert_eq!(gap.last_noticed_at, 3_000, "the NOTICE time does not move");
    }

    /*
     * A counter that went backwards is the platform's, not the ward's: a wipe
     * or a restore reset it. Re-baseline rather than report the difference as
     * hundreds of unwarded boots.
     */
    #[test]
    fn a_reset_boot_counter_rebaselines_instead_of_crying_wolf() {
        let mut w = warden("rebaseline");
        w.note_boot(400, 1_000);
        let mut w = restart(&w);
        w.note_boot(2, 2_000);
        assert_eq!(w.enforcement_gap(), None);
        assert_eq!(w.boot_watch.last_boot_count, 2);
    }

    /*
     * A device whose platform keeps no BOOT_COUNT reads 0. An absent counter
     * must never manufacture an accusation — nor destroy a real tally already
     * earned on a device that used to report one.
     */
    #[test]
    fn an_absent_boot_counter_reports_nothing() {
        let mut w = warden("absent");
        w.note_boot(0, 1_000);
        w.note_boot(0, 2_000);
        assert_eq!(w.enforcement_gap(), None);
        let status = w.build_status(2_000).expect("status");
        assert_eq!(status.enforcement_gap, None, "absent, not a zero");
    }
}
