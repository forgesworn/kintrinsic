//! Real daemon runtime assembly (the `real` feature). Wires the broker spine to
//! the live OS via the `charter-sys`/`charter-transport` `real` impls: the OS
//! CSPRNG entropy, the gift-wrap transport over the rustls websocket relay, the
//! three enactors over their real ports, and the subscribe -> verify -> enact ->
//! enforce loop. Compiles on the headless gate (so the integration is proven to
//! type-check end to end) and runs on a provisioned, paired host.
//!
//! Pure helpers (entropy fill, pairing load) are unit-tested here; the live
//! loop — which needs a bus, a relay, a guardian, and privilege — is VM-verified.

#![cfg(feature = "real")]

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;

use charter_primitives::{NostrEvent, PubKey};
use charter_proto::ClauseKind;
use charter_schedule::{
    day_key, enforcement_tz_of, is_valid_freeze_target, reconcile_freeze, EnforcerEffect,
    FreezeAction, WarnLevel,
};
use charter_spine::PollHealth;
use charter_sys::effects::{
    CgroupFreezer, RealApprovedExecStore, RealFlatpakOps, RealTrustDb, VtControl,
};
use charter_sys::persistence::{ChildClauseStore, PairingStore};
use charter_sys::relay::{PublishOutcome, RealRelayTransport, RelayIoError, RelayUrl};
use charter_sys::signer::RealMachineSigner;
use charter_sys::{Clock, RealSystem, SystemLayer};
use charter_transport::pairing::Pairing;
use charter_transport::{
    CharterTransport, Entropy, FetchedCuratorList, ReceivedClause, ReceivedGrant, TransportError,
};

use crate::atomic_file::atomic_write;
use crate::broker::Broker;
use crate::child_policy::resolve_child_policies;
use crate::dbus_service::{
    remaining_to_view, serve, signal_pump, ChannelEventSink, TimeLeftSnapshots,
};
use crate::device_limits::{home_for_uid, load_child_configs, uid_for_user, user_for_uid};
use crate::enactor::EnactorRegistry;
use crate::enactors::{AppOpenEnactor, ExecAllowEnactor, InstallFlatpakEnactor, TimeExtendEnactor};
use crate::enforce_mode::{observe_lines, EnforceMode};
use crate::enforcer_runtime::{lock_message, managed_freeze_target, EnforcerRuntime};
use crate::managed_guard::ManagedRoster;
use crate::multi_child::{ChildDecision, ExtensionInbox, MultiChildEnforcer};
use crate::ports::EventSink;
use crate::transport_facade::TransportFacade;
use crate::web_content::WebContentEnforcer;

/// STATUS-feed heartbeat: re-publish an unchanged child's state at least this
/// often (seconds), so the PWA stays fresh even without a state change.
const STATUS_HEARTBEAT_SECS: u64 = 60;

/// How long the app-inventory fingerprint+scan gets before the enforcement
/// tick stops waiting for it.
///
/// C1/N6, the part `spawn_blocking` alone does NOT fix: moving the scan off
/// the async executor stops it occupying an executor worker, but the tick
/// still `.await`ed the handle, so a `stat`/`read_dir` that never returns —
/// a ward mounting a hung FUSE filesystem at `~/.local/share/applications`
/// takes no privilege at all — stalled the enforcement loop just the same. No
/// lock, no sweep, no STATUS, indefinitely, which is the very DoS this was
/// asked to close. A real scan of a real desktop is milliseconds; five
/// seconds is generous.
const INVENTORY_BUDGET: Duration = Duration::from_secs(5);

/// After a timed-out scan, how long before trying again. A timed-out
/// `spawn_blocking` task cannot be cancelled — dropping its handle detaches
/// it, and the thread stays parked on the hung path forever — so retrying
/// every 2s tick would burn a blocking-pool thread each time and exhaust the
/// pool within the hour. This also rate-limits the log line to one per
/// window.
const INVENTORY_RETRY_AFTER_STALL_SECS: i64 = 300;

/// Await an inventory task under a hard budget. `None` = "no answer this
/// tick" — a timeout, or a task that panicked — and the caller MUST carry on
/// with whatever inventory it already had. A stale app list is a cosmetic
/// loss the guardian may not even notice; a wedged enforcement loop is a
/// child with no limits.
async fn inventory_within<F, T>(budget: Duration, task: F) -> Option<T>
where
    F: std::future::Future<Output = Result<T, tokio::task::JoinError>>,
{
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(value)) => Some(value),
        // A panicked scan task is the same answer as a hung one: keep what we
        // had. It must never become an `.expect()` that takes the daemon down
        // over a ward-writable directory.
        Ok(Err(_)) | Err(_) => None,
    }
}

/// Did the inventory scan ANSWER? (D3 — named, not inlined, so the
/// unknown-vs-empty distinction below is a testable decision rather than a
/// `.is_some()` a refactor can quietly flip to `true`.)
///
/// `None` from [`inventory_within`] means the scan timed out or panicked — it
/// does NOT mean "this device has no apps". Everything downstream turns on
/// that difference; see [`user_installed_for_classify`].
fn inventory_answered<T>(seed: &Option<T>) -> bool {
    seed.is_some()
}

/// The `user_installed` argument [`crate::focus::classify`] gets this tick.
///
/// **`None` (inventory UNKNOWN) is not `Some(empty)` (inventory KNOWN to hold
/// nothing).** §2.4's gate refuses free learning time to a `flatpak install
/// --user` impostor of a governed app id, and it can only do that if it knows
/// what the ward installed. An empty set is a positive claim — *"nothing here
/// is user-installed"* — so handing one over while the scan has never
/// answered switches the gate off silently.
///
/// That is reachable with no privilege: a ward mounts a hung FUSE filesystem
/// at `~/.local/share/applications`, reboots so the SEED scan is the one that
/// wedges, then `flatpak install --user`s a governed learning app. Every
/// retry times out and backs off 300 s, so the free time persists. While the
/// inventory is unknown this returns `None` and `classify` fails that arm
/// CLOSED to `Bucket::Screen` — learning's documented fail direction.
fn user_installed_for_classify(
    known: bool,
    inventory: &[charter_proto::status::AppRef],
) -> Option<std::collections::BTreeSet<String>> {
    known.then(|| crate::focus::user_installed_ids(inventory))
}

/// The fully-concrete broker the real daemon runs.
pub type RealBroker = Broker<RealSystem, RealTransportFacade, RealEntropy>;

/// Default on-disk locations + managed identity. Overridable for the VM.
pub struct DaemonConfig {
    /// Machine identity key (hex, 0600). Created on first boot if absent.
    pub key_path: String,
    /// The managed child's uid — derives the freeze target (`app.slice`).
    pub managed_uid: u32,
    /// Seconds between poll + enforcement ticks.
    pub poll_interval_secs: u64,
    /// Per-child device-only limits dir (`<user>.json` each) — multi-child, no
    /// guardian app needed.
    pub child_limits_dir: String,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            key_path: "/var/lib/charter/machine.key".into(),
            managed_uid: 1000,
            poll_interval_secs: 10,
            child_limits_dir: "/etc/charter/limits.d".into(),
        }
    }
}

/// OS CSPRNG entropy for ephemeral keys + nonces (`/dev/urandom`). Opened per
/// fill — a few draws per publish, never on a hot path.
pub struct RealEntropy;

impl Entropy for RealEntropy {
    fn fill(&self, buf: &mut [u8]) {
        use std::io::Read as _;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(buf).is_ok() {
                return;
            }
        }
        // `/dev/urandom` is effectively never unavailable on Linux; if it is, the
        // daemon must not proceed with a predictable nonce.
        panic!("charterd: /dev/urandom unavailable — cannot generate entropy");
    }
}

/// Bridges the broker's [`TransportFacade`] onto the real gift-wrap transport
/// (`CharterTransport`) over the rustls websocket relay. Poll failures surface
/// as empty batches (offline is "nothing delivered", never a crash) — but the
/// underlying `RelayIoError::Unreachable` / per-relay `PublishOutcome::Failed`
/// are not thrown away: they accumulate in `relay_unreachable`/`publish_failed`
/// for `poll_health()` to read (and reset) once per broker `poll_once` round.
pub struct RealTransportFacade {
    inner: CharterTransport<RealRelayTransport, RealEntropy>,
    guardian: PubKey,
    machine: PubKey,
    relay_unreachable: std::sync::atomic::AtomicBool,
    publish_failed: std::sync::atomic::AtomicU32,
}

/// Scan-to-pair: while an unpaired ward has a live pairing token on screen,
/// watch the relay for a guardian's offer and pin the one that proves it.
///
/// This is the ONLY thing an unpaired ward listens for. It costs nothing when
/// no QR is showing — `pair_token::current` returns `None` and the tick is a
/// no-op — so a device-only ward that never pairs simply idles here forever.
///
/// On success the daemon restarts, coming back up through the paired arm with
/// a broker, enforcement, and the guardian's clauses.
fn spawn_pair_listener(machine_sk: [u8; 32], limits_dir: String) {
    tokio::spawn(async move {
        let paths = crate::pair_commit::PinPaths {
            limits_dir,
            ..crate::pair_commit::PinPaths::production()
        };
        let machine = PubKey::from_bytes(
            charter_crypto::xonly_pubkey(&machine_sk).expect("valid machine secret"),
        );
        let transport =
            RealTransportFacade::new(machine_sk, machine, crate::pair_listener::pair_relays());
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(
                crate::pair_listener::POLL_INTERVAL_SECS,
            ))
            .await;
            let now = charter_sys::clock::Clock::now_utc(&charter_sys::clock::RealClock::default());
            // Cheap guard first: no QR on screen means nothing to accept, so
            // we never even hit the relay.
            if crate::pair_token::current(&paths.token, now).is_none() {
                continue;
            }
            let since = now.saturating_sub(crate::pair_token::TOKEN_TTL_SECS);
            let offers = transport.poll_pair_offers(since, now).await;
            if offers.is_empty() {
                continue;
            }
            let Some(subject) = crate::pair_listener::draw_subject() else {
                continue;
            };
            if crate::pair_listener::try_pair_once(&paths, machine, &offers, subject, now).is_some()
            {
                let _ = std::process::Command::new("systemctl")
                    .args(["try-restart", "charterd.service"])
                    .status();
                return;
            }
        }
    });
}

impl RealTransportFacade {
    /// Build from the machine secret + pinned guardian + relay set.
    ///
    /// Panics on an unusable machine secret — kept for callers that have
    /// always treated that as unrecoverable (the pair listener, which has no
    /// pairing yet to fall back on). `run`'s own construction sites use
    /// [`Self::try_new`] instead, so a corrupted secret on an already-paired
    /// device degrades to cached-clause enforcement rather than aborting
    /// (B4).
    pub fn new(machine_sk: [u8; 32], guardian: PubKey, relays: Vec<RelayUrl>) -> Self {
        Self::try_new(machine_sk, guardian, relays).expect("valid machine secret")
    }

    /// Fallible counterpart to [`Self::new`] — `Err(TransportError)` instead
    /// of a panic when the machine secret does not derive a valid key (B4).
    pub fn try_new(
        machine_sk: [u8; 32],
        guardian: PubKey,
        relays: Vec<RelayUrl>,
    ) -> Result<Self, TransportError> {
        let inner = CharterTransport::try_new(
            RealRelayTransport::default(),
            RealEntropy,
            machine_sk,
            guardian,
            relays,
        )?;
        let machine = inner.machine_pubkey();
        Ok(Self {
            inner,
            guardian,
            machine,
            relay_unreachable: std::sync::atomic::AtomicBool::new(false),
            publish_failed: std::sync::atomic::AtomicU32::new(0),
        })
    }

    /// Record a poll's per-relay `Result`, folding `Unreachable` into the
    /// health accumulator instead of throwing it away — the shape every
    /// `poll_*` wrapper below shares.
    fn record_poll<T>(&self, r: Result<Vec<T>, RelayIoError>) -> Vec<T> {
        match r {
            Ok(v) => v,
            Err(RelayIoError::Unreachable(_)) => {
                self.relay_unreachable
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                Vec::new()
            }
        }
    }

    /// Record a publish's per-relay outcomes, folding any `Failed` entries
    /// into the health accumulator.
    fn record_publish(&self, outcomes: Vec<(RelayUrl, PublishOutcome)>) {
        let failed = outcomes
            .iter()
            .filter(|(_, o)| matches!(o, PublishOutcome::Failed(_)))
            .count() as u32;
        if failed > 0 {
            self.publish_failed
                .fetch_add(failed, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Publish a gift-wrapped STATUS (kind 31114) to the pinned guardian.
    /// Best-effort — a failed publish is a dropped update, never a crash.
    pub async fn emit_status(&self, status_json: &str, now: u64) {
        let outcomes = self.inner.emit_status(status_json, now).await;
        self.record_publish(outcomes);
    }

    /// Poll PAIR_OFFERs addressed to this machine (the unpaired scan-to-pair
    /// door). Best-effort: a relay outage is "no offers this tick", never a
    /// crash on a ward that is still enforcing limits offline.
    pub async fn poll_pair_offers(
        &self,
        since: u64,
        now: u64,
    ) -> Vec<charter_transport::transport::ReceivedPairOffer> {
        self.record_poll(self.inner.poll_pair_offers(since, now).await)
    }
}

#[async_trait]
impl TransportFacade for RealTransportFacade {
    async fn publish_request(&self, request_json: &str, now: u64) {
        let outcomes = self.inner.submit_request(request_json, now).await;
        self.record_publish(outcomes);
    }
    async fn poll_grants(&self, since: u64, now: u64) -> Vec<ReceivedGrant> {
        self.record_poll(self.inner.poll_grants(since, now).await)
    }
    async fn poll_clauses(&self, since: u64, now: u64) -> Vec<ReceivedClause> {
        self.record_poll(self.inner.poll_clauses(since, now).await)
    }
    async fn poll_usage_syncs(&self, since: u64, now: u64) -> Vec<(NostrEvent, PubKey)> {
        self.record_poll(self.inner.poll_usage_syncs(since, now).await)
    }
    /// The guardian's "Disconnect this device" (S7). Best-effort like every
    /// other poll: a relay outage is "no releases this tick", never a crash on
    /// a ward that is still enforcing limits offline.
    async fn poll_releases(&self, since: u64, now: u64) -> Vec<(NostrEvent, PubKey)> {
        self.record_poll(self.inner.poll_releases(since, now).await)
    }
    async fn poll_curator_lists(&self, curators: &[PubKey], since: u64) -> Vec<FetchedCuratorList> {
        self.record_poll(self.inner.poll_curator_lists(curators, since).await)
    }
    async fn emit_audit(&self, tags: Vec<Vec<String>>, now: u64) {
        let outcomes = self.inner.emit_audit(tags, now).await;
        self.record_publish(outcomes);
    }
    fn pinned_guardian(&self) -> PubKey {
        self.guardian
    }
    fn machine_pubkey(&self) -> PubKey {
        self.machine
    }
    fn poll_health(&self) -> PollHealth {
        PollHealth {
            relays_unreachable: self
                .relay_unreachable
                .swap(false, std::sync::atomic::Ordering::Relaxed),
            publish_failed: self
                .publish_failed
                .swap(0, std::sync::atomic::Ordering::Relaxed),
        }
    }
}

/// Load the pinned pairing (guardian pubkey + relays) from the persisted store.
fn load_pairing<S: SystemLayer>(sys: &S) -> Option<Pairing> {
    let json = sys.pairing().load().ok().flatten()?;
    serde_json::from_str::<Pairing>(&json).ok()
}

/// Build the broker over the real ports. The shared `enforcer` handle is also
/// used by the enforcement loop, so `time.extend` enact + the tick agree on one
/// ledger.
fn build_broker(
    sys: RealSystem,
    transport: RealTransportFacade,
    enforcer: Arc<Mutex<EnforcerRuntime>>,
    events: Box<dyn EventSink>,
    subject: PubKey,
    extension_inbox: ExtensionInbox,
) -> RealBroker {
    let mut registry = EnactorRegistry::new();
    registry.register(Box::new(InstallFlatpakEnactor::new(RealFlatpakOps)));
    registry.register(Box::new(ExecAllowEnactor::new(
        RealApprovedExecStore::default(),
        RealTrustDb,
    )));
    // The inbox routes each applied extension to the live MultiChildEnforcer the
    // enforce loop drives (the `enforcer` handle only feeds the D-Bus readout).
    registry.register(Box::new(
        TimeExtendEnactor::new(enforcer).with_inbox(extension_inbox),
    ));
    // app.open enacts nothing (the apps clause carries the hold) — registered
    // purely so an allowed ask reaches `Enacted` instead of stalling as
    // `Failed` with no enactor at all (see `AppOpenEnactor`'s module doc).
    registry.register(Box::new(AppOpenEnactor));
    Broker::new(sys, transport, RealEntropy, registry, events, subject)
}

/// The active (foreground) session on the seat (`CHARTER_SEAT` overrides
/// `seat0`), as `(uid, session id)`. Both [`active_session_uid`] and
/// [`session_activity`] need the session id — resolving it once here and
/// reusing it avoids a second `show-seat` round trip per tick. Degrades to
/// `None` if logind is unresponsive — see `loginctl_value` for why that must
/// never block.
fn active_session() -> Option<(u32, String)> {
    let seat = std::env::var("CHARTER_SEAT").unwrap_or_else(|_| "seat0".into());
    let sid = loginctl_value(&["show-seat", &seat, "--property=ActiveSession"])?;
    let uid = loginctl_value(&["show-session", &sid, "--property=User"])?
        .parse()
        .ok()?;
    Some((uid, sid))
}

/// The uid of the active (foreground) session on the seat. The only user
/// whose time is charged.
fn active_session_uid() -> Option<u32> {
    active_session().map(|(uid, _)| uid)
}

/// G1 (03b-linux-charterd-bins-matching): "screen time" means the screen is
/// ON and UNLOCKED, not merely that the ward's uid owns the active session —
/// a child who locks the screen (or whose screensaver blanks it) must stop
/// being charged. logind's own session hints carry exactly this: desktops
/// set `LockedHint` when the screensaver/lock engages, and `IdleHint` follows
/// input idleness but is INHIBITED by anything that counts as "using the
/// machine" (a fullscreen video player, a game) — so it does not charge a
/// child watching a film as idle the way raw input-idle would.
/// `LockedHint` wins over `IdleHint` (a locked-but-not-yet-idle screen is
/// still locked). Fails toward `Active` (today's behaviour, and the
/// direction every other probe in this file fails) when the probe itself
/// cannot answer — an idle/locked classification must be POSITIVELY
/// evidenced, never assumed from silence.
fn session_activity(sid: &str) -> charter_schedule::Activity {
    match loginctl_value(&[
        "show-session",
        sid,
        "--property=LockedHint",
        "--property=IdleHint",
    ]) {
        Some(out) => activity_from_hints(&out),
        None => charter_schedule::Activity::Active,
    }
}

/// Parse `loginctl show-session --property=LockedHint --property=IdleHint
/// --value`'s output: one "yes"/"no" line per property, in the order
/// requested (LockedHint first, IdleHint second) — confirmed on this machine
/// (`loginctl show-session <sid> -p LockedHint -p IdleHint --value` prints
/// two lines). Pure so it is unit-tested without a live logind; case and
/// stray whitespace come from loginctl, not from us (see
/// `display_protocol_note`'s test for the same discipline elsewhere in this
/// file). Anything short of two readable lines, or two "no"s, is Active —
/// the fail-toward-charging direction.
fn activity_from_hints(output: &str) -> charter_schedule::Activity {
    let mut lines = output.lines().map(|l| l.trim().to_ascii_lowercase());
    let locked = lines.next().is_some_and(|l| l == "yes");
    if locked {
        return charter_schedule::Activity::Locked;
    }
    let idle = lines.next().is_some_and(|l| l == "yes");
    if idle {
        return charter_schedule::Activity::Idle;
    }
    charter_schedule::Activity::Active
}

/// The active (foreground) session's `DISPLAY` + `XAUTHORITY`, read from its
/// leader process's environment. charterd runs as root, so it can read any
/// `/proc/<pid>/environ`. Display managers (LightDM/GDM) put the Xauthority in
/// varying runtime paths and each session gets its own `:N` display, so reading
/// the live session env is the only reliable, DM-agnostic source — hardcoding
/// `:0` + `~/.Xauthority` breaks multi-session (switch-user) and multi-child.
/// Returns `(display, xauthority?)`; `None` if the session/leader/env is
/// unavailable (the caller falls back).
fn active_session_x() -> Option<(String, Option<String>)> {
    // Authoritative path: the foreground VT names the display — the Xorg
    // process serving that VT names its own display + auth cookie in its argv
    // (e.g. `Xorg :1 -auth /var/run/lightdm/root/:1 vt8`). Under a user switch
    // there are MULTIPLE X servers (:0 on vt7, :1 on vt8, ...) — resolving by
    // VT targets the one the child is actually looking at; anything else
    // draws the lock into another session's void. The VT comes from (verified
    // on Mint/lightdm — switched sessions have EMPTY logind TTY and Display
    // properties, and the `lightdm --session-child` leader carries no DISPLAY
    // in its environ, so those are useless here):
    //   1. the kernel's own console state, /sys/class/tty/tty0/active — no
    //      logind involved, so it works even when logind is wedged (a frozen
    //      session's half-dead FIFOs can spin logind's event loop; observed
    //      live on Mint 22);
    //   2. the active session's VTNr via loginctl (populated even when TTY is
    //      empty) — the fallback for exotic seats where tty0 is absent.
    let kernel_vt = std::fs::read_to_string("/sys/class/tty/tty0/active")
        .ok()
        .map(|s| s.trim().to_string());
    let session_vt = || {
        let seat = std::env::var("CHARTER_SEAT").unwrap_or_else(|_| "seat0".into());
        loginctl_value(&["show-seat", &seat, "--property=ActiveSession"])
            .and_then(|sid| loginctl_value(&["show-session", &sid, "--property=VTNr"]))
            .map(|n| format!("tty{n}"))
    };
    let found = kernel_vt
        .and_then(|tty| xorg_for_vt(&tty))
        .or_else(|| session_vt().and_then(|tty| xorg_for_vt(&tty)));
    if found.is_none() {
        // Say WHICH kind of failure this is. A Wayland session has no Xorg to
        // find, so the search above can only ever fail — and the cgroup freeze
        // still lands, leaving the ward staring at a dead desktop with no panel,
        // no reason and no way to ask for more time. That is the one outcome
        // this daemon must never produce silently, and until a Wayland lock
        // exists the least we owe the family is a log line naming it rather
        // than a generic miss that reads like a transient hiccup.
        let session_type =
            loginctl_value(&["show-session", "--property=Type", "self"]).or_else(|| {
                let seat = std::env::var("CHARTER_SEAT").unwrap_or_else(|_| "seat0".into());
                loginctl_value(&["show-seat", &seat, "--property=ActiveSession"])
                    .and_then(|sid| loginctl_value(&["show-session", &sid, "--property=Type"]))
            });
        match display_protocol_note(session_type.as_deref()) {
            Some(note) => eprintln!("charterd: {note}"),
            None => eprintln!("charterd: active display resolution failed (no Xorg on the active VT) — falling back to CHARTER_DISPLAY"),
        }
    }
    found
}

/// The log line for a session whose display protocol the on-screen lock cannot
/// draw on. `None` means "nothing special about this session" — the caller then
/// reports the ordinary miss.
///
/// Pure so it is unit-tested; the loginctl call that feeds it is not.
fn display_protocol_note(session_type: Option<&str>) -> Option<&'static str> {
    match session_type
        .map(|s| s.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("wayland") => Some(
            "the active session is WAYLAND — charter-lock is an X11 client, so no lock \
             panel can be shown. Apps are still frozen when time is up, but the ward sees \
             no explanation and cannot ask for more time. Use an X11 (Xorg) session until \
             a Wayland lock ships.",
        ),
        _ => None,
    }
}

/// One `--value` property from loginctl, `None` if empty/unavailable.
///
/// Hard 2s timeout: loginctl talks to logind, and a wedged logind (see
/// `active_session_x`) would otherwise hang `.output()` forever — taking the
/// whole enforcement tick loop down with it. The enforcer must degrade
/// (skip a tick's accounting), never stall.
fn loginctl_value(args: &[&str]) -> Option<String> {
    let out = Command::new("timeout")
        .arg("2")
        .arg("loginctl")
        .args(args)
        .arg("--value")
        .output()
        .ok()?;
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// The display + auth cookie of the Xorg process serving `tty` (e.g. "tty8"),
/// read from its argv (`vt8` names the VT, `:1` the display, `-auth` the
/// cookie).
fn xorg_for_vt(tty: &str) -> Option<(String, Option<String>)> {
    let vt_arg = format!("vt{}", tty.strip_prefix("tty")?);
    for args in xorg_processes() {
        if args.contains(&vt_arg) {
            if let Some(display) = display_from_xorg_args(&args) {
                return Some((display, parse_xorg_auth_arg(&args)));
            }
        }
    }
    None
}

/// The bare `:N` display arg in an Xorg argv.
fn display_from_xorg_args(args: &[String]) -> Option<String> {
    args.iter()
        .find(|a| a.len() > 1 && a.starts_with(':') && a[1..].chars().all(|c| c.is_ascii_digit()))
        .cloned()
}

/// Argv of every running Xorg process.
fn xorg_processes() -> Vec<Vec<String>> {
    let mut found = Vec::new();
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return found,
    };
    for entry in entries.flatten() {
        let cmdline = match std::fs::read(entry.path().join("cmdline")) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let args: Vec<String> = cmdline
            .split(|&b| b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        let is_xorg = args
            .first()
            .map(|a| a.ends_with("Xorg") || a.ends_with("/X"))
            .unwrap_or(false);
        if is_xorg {
            found.push(args);
        }
    }
    found
}

/// The X authority file a display's X server was started with, read from the
/// Xorg process's own `-auth <path>` argument (e.g. the LightDM root cookie
/// `/var/run/lightdm/root/:1`). This is the authoritative, DM-agnostic cookie a
/// root process needs to draw on that display — the session leader's
/// `XAUTHORITY` env / `~/.Xauthority` are frequently absent or wrong on
/// LightDM/Mint, which is what made the lock fail with "Authorization required".
fn xorg_auth_for_display(display: &str) -> Option<String> {
    xorg_processes()
        .iter()
        // The Xorg process serving exactly this display (a bare `:N` arg).
        .find(|args| args.iter().any(|a| a == display))
        .and_then(|args| parse_xorg_auth_arg(args))
}

/// Extract the value following `-auth` in an Xorg argv.
fn parse_xorg_auth_arg(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if a == "-auth" {
            return it.next().cloned();
        }
    }
    None
}

/// Argv to pop a desktop notification in the CHILD's session (as the child, so
/// their session bus accepts it). charterd is root, so `runuser` drops to the
/// child and `env` supplies their display + session bus. `notify-send` ships
/// with libnotify (present on Cinnamon).
fn notify_argv(user: &str, uid: u32, display: &str, summary: &str, body: &str) -> Vec<String> {
    vec![
        "runuser".to_string(),
        "-u".to_string(),
        user.to_string(),
        "--".to_string(),
        "env".to_string(),
        format!("DISPLAY={display}"),
        format!("DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"),
        "notify-send".to_string(),
        "-u".to_string(),
        "critical".to_string(),
        "-a".to_string(),
        "Kintrinsic".to_string(),
        summary.to_string(),
        body.to_string(),
    ]
}

/// Fire-and-forget a warning notification to the child (best-effort; a missing
/// notify-send or session is never fatal to enforcement).
fn notify_child(passwd: &str, uid: u32, display: &str, summary: &str, body: &str) {
    if let Some(user) = user_for_uid(passwd, uid) {
        let argv = notify_argv(&user, uid, display, summary, body);
        if let Ok(mut child) = Command::new(&argv[0]).args(&argv[1..]).spawn() {
            // Reap in a detached thread so a fire-and-forget notification never
            // leaves a zombie in this long-running root daemon's process table.
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
}

fn child_usage_path(uid: u32) -> String {
    format!("/var/lib/charter/children/{uid}.usage.json")
}
/// The extension ledger sits beside the usage one, under the same mode, and
/// is written the same way. Two files rather than one so that neither can be
/// lost or torn by a write of the other, and so an operator reading
/// `/var/lib/charter/children/` can see at a glance what the ward has SPENT
/// and what they have been GIVEN.
fn child_extension_path(uid: u32) -> String {
    format!("/var/lib/charter/children/{uid}.extension.json")
}

/// Both of one child's persisted ledgers, in the order
/// `MultiChildEnforcer::sync` wants them: `(usage, extension)`.
///
/// The extension half used to be missing entirely — `snapshots()` produced
/// it, the persist loop named it `_ext` and dropped it, and nothing ever read
/// one back. See `ChildEnforcer::restore_extension` for what that cost.
fn load_child_ledgers(uid: u32) -> (Option<String>, Option<String>) {
    (
        std::fs::read_to_string(child_usage_path(uid)).ok(),
        std::fs::read_to_string(child_extension_path(uid)).ok(),
    )
}
/// Write one child's day-ledger, **0600** (review 2026-08-07), atomically.
///
/// These land under the default umask, i.e. world-readable, so on a shared
/// family box every local account could read every other child's ledger — how
/// long they had been on the computer, minute by minute. The world-readable
/// STATE files (`/run/charter/state/<user>.json`) are a deliberate decision,
/// documented, because the tray has to read the calling user's own; the usage
/// ledgers were never part of that decision and nothing outside this root
/// daemon reads them.
///
/// The mode is carried by the write rather than trusted to a umask: an
/// existing file keeps its old permissions through a plain write, so a ledger
/// created before that change would stay 0644 forever otherwise.
///
/// This was a bare `std::fs::write` — `O_TRUNC` then write, with no fsync and
/// no rename. It runs every slow tick (10 s by default), and a ward who holds
/// the power button through that window leaves a truncated file behind;
/// `restore_usage` swallows a snapshot that will not parse, so the child came
/// back with the whole day's accrued time gone. Repeatable at will, in the
/// fail-open direction. Now temp + fsync + rename, via [`atomic_write`].
fn save_child_usage(uid: u32, usage: &str) {
    if let Err(e) = atomic_write(&child_usage_path(uid), usage.as_bytes(), 0o600) {
        eprintln!(
            "charterd: could not persist the usage ledger for uid {uid} ({e}) — \
             the day's accrued time is still being enforced from memory"
        );
    }
}

/// Write one child's extension ledger — the guardian's given minutes and how
/// much of them is spent — with the same durability and mode as the usage
/// one. A failure here means a reboot loses today's grants, which is
/// fail-closed for the ward, so it is reported and never fatal.
fn save_child_extension(uid: u32, extension: &str) {
    if let Err(e) = atomic_write(&child_extension_path(uid), extension.as_bytes(), 0o600) {
        eprintln!(
            "charterd: could not persist the extension ledger for uid {uid} ({e}) — \
             a restart would lose any time given today"
        );
    }
}

/// The recovery pause flag. Lives under the unit's `RuntimeDirectory=charter`
/// (`/run/charter`, root-owned) so a managed child can't write it and a forgotten
/// pause **self-heals on reboot** — it can never durably disable a child's
/// limits. `CHARTER_PAUSE_FLAG` overrides for tests/VM.
fn pause_flag_path() -> String {
    std::env::var("CHARTER_PAUSE_FLAG").unwrap_or_else(|_| "/run/charter/paused".into())
}

/// How long a recovery pause may last before it lapses on its own (03-G5).
///
/// A pause is a recovery tool: "let me at this machine for a bit". It had no
/// expiry at all, so it self-healed only on reboot (`/run` is a
/// `RuntimeDirectory`) — and on a box nobody reboots, one click left a child
/// with no limits indefinitely, with the guardian's app still showing the last
/// pre-pause numbers as though they were live. Four hours is longer than any
/// genuine repair session and short enough that a forgotten pause is an
/// afternoon, not a term.
///
/// Re-issuing is one click in the Recovery tool, and re-issuing REFRESHES the
/// window (the flag file's mtime is what ages).
const PAUSE_MAX_SECS: i64 = 4 * 3600;

/// True while an admin has paused enforcement via the Recovery tool, with a
/// pause older than [`PAUSE_MAX_SECS`] lapsing here — once, loudly.
///
/// The age is the flag file's mtime, so "re-issue the pause" is simply
/// "rewrite the flag", which is what the Recovery tool already does. An
/// unreadable mtime is treated as a LIVE pause, not a lapsed one: the failure
/// direction that silently re-freezes a box somebody is in the middle of
/// repairing is the worse of the two, and the next reboot clears it regardless.
fn enforcement_paused() -> bool {
    let path = pause_flag_path();
    let Ok(meta) = std::fs::metadata(&path) else {
        return false;
    };
    let age = meta
        .modified()
        .ok()
        .and_then(|m| m.elapsed().ok())
        .map(|d| d.as_secs() as i64);
    if age.is_some_and(|a| a > PAUSE_MAX_SECS) {
        eprintln!(
            "charterd: the recovery pause has been in force for over {}h — it has \
             LAPSED and enforcement is resuming. Re-issue it from Recovery if the \
             machine still needs it.",
            PAUSE_MAX_SECS / 3600
        );
        let _ = std::fs::remove_file(&path);
        return false;
    }
    true
}

/// Make the box usable again: thaw every managed child's app.slice, drop the
/// running lock, and re-enable VT switching. The shared self-heal for startup
/// (crash recovery), a recovery pause, and the unit's ExecStopPost — a stuck
/// machine always becomes usable.
///
/// Known limitation (low): the sweep is scoped to the *current* roster, so a uid
/// that was frozen and then removed from `charter-managed` (or promoted to admin)
/// in the same session is not thawed here — that one account's apps stay frozen
/// until the user logs out (session teardown destroys `user@<uid>.service`) or
/// the box reboots. The parent/admin session and the rest of the machine remain
/// usable throughout.
async fn thaw_all(sys: &RealSystem, roster: &ManagedRoster, lock: &mut LockState) {
    for uid in roster.uids() {
        let slice = managed_freeze_target(uid);
        if is_valid_freeze_target(&slice) {
            let _ = sys.freezer().thaw(&slice).await;
        }
    }
    lock.kill();
    let _ = sys.vt().set_vt_switching(true).await;
}

/// The running on-display lock and the aftermath state that travels with it.
#[derive(Default)]
struct LockState {
    /// The spawned `charter-lock`, keyed by the locked child's uid.
    proc: Option<(u32, std::process::Child)>,
    /// Set when a running lock comes down (its session may have just died);
    /// cleared once the glass is confirmed on (or moved to) a live VT.
    vt_heal_pending: bool,
    /// Consecutive heal attempts that found NO live X anywhere. A greeter that
    /// has not reappeared after this many is not coming back by itself — see
    /// [`VT_HEAL_GIVE_UP_TICKS`].
    vt_heal_misses: u8,
    /// Display-manager restarts attempted for the CURRENT downed lock — see
    /// [`VT_HEAL_MAX_DM_RESTARTS`].
    vt_dm_restarts: u8,
}

impl LockState {
    /// Kill + reap the running lock, if any; reports whether one was up.
    fn kill(&mut self) -> bool {
        match self.proc.take() {
            Some((_, mut old)) => {
                let _ = old.kill();
                let _ = old.wait();
                true
            }
            None => false,
        }
    }
}

/// How many consecutive no-X-anywhere heals to tolerate before restarting the
/// display manager. At the 2s fast tick that is ~10 seconds.
///
/// Sized to be safely past a normal greeter respawn while still short enough
/// that nobody concludes the machine is dead. Robin's laptop sat black for over
/// three minutes on 2026-07-27 and the only way out anyone found was the power
/// button.
const VT_HEAL_GIVE_UP_TICKS: u8 = 5;

/// Whether a run of no-X-anywhere heals has gone on long enough that the
/// greeter is not coming back by itself.
fn should_restart_dm(misses: u8) -> bool {
    misses >= VT_HEAL_GIVE_UP_TICKS
}

/// How many display-manager restarts to attempt for one downed lock before
/// conceding. Each attempt only fires after another [`VT_HEAL_GIVE_UP_TICKS`]
/// of misses, so this bounds the storm on a box where the restart cannot work
/// (no `display-manager` alias, wedged unit) at a few tries — not one every
/// ~10s all night.
const VT_HEAL_MAX_DM_RESTARTS: u8 = 3;

/// Whether the heal stays armed after a display-manager restart attempt. A
/// FAILED restart must not clear it — that silently recreates the permanent
/// black screen the restart exists to fix — but at the cap we concede rather
/// than storm.
fn heal_stays_armed(restart_ok: bool, restarts: u8) -> bool {
    !restart_ok && restarts < VT_HEAL_MAX_DM_RESTARTS
}

/// Ask the display manager to bring a greeter back.
///
/// The last resort when a locked session has ended and no X server exists
/// ANYWHERE — not the "parked on a dead VT" case [`heal_stranded_vt`] handles,
/// but "there is nothing to switch TO". Losing the greeter leaves a black
/// screen with a cursor and no keyboard route out, so a DM restart (which costs
/// a greeter nobody is logged into) is unambiguously the lesser harm.
async fn restart_display_manager() -> bool {
    eprintln!("charterd: no X anywhere after the lock came down — restarting the display manager");
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        tokio::task::spawn_blocking(|| {
            Command::new("systemctl")
                .args(["restart", "display-manager"])
                .status()
        }),
    )
    .await;
    match out {
        Ok(Ok(Ok(st))) if st.success() => true,
        Ok(Ok(Ok(st))) => {
            eprintln!("charterd: display-manager restart exited {st}");
            false
        }
        Ok(Ok(Err(e))) => {
            eprintln!("charterd: display-manager restart failed: {e}");
            false
        }
        Ok(Err(e)) => {
            eprintln!("charterd: display-manager restart task failed: {e}");
            false
        }
        // A wedged unit can block `systemctl restart` for 90s+; the abandoned
        // thread lingers but the enforcement loop must not.
        Err(_) => {
            eprintln!("charterd: display-manager restart timed out");
            false
        }
    }
}

/// Outcome of a stranded-VT check (see [`heal_stranded_vt`]).
#[derive(Debug, PartialEq, Eq)]
enum VtHeal {
    /// The foreground VT has a live X server — nothing to do.
    Healthy,
    /// The display was parked on a dead VT; we switched it to a live X.
    Switched,
    /// No live X server exists yet to switch to — try again next tick.
    NoTarget,
}

/// The `vtN` argument of an Xorg argv, as a bare VT number.
fn vt_from_xorg_args(args: &[String]) -> Option<u32> {
    args.iter().find_map(|a| a.strip_prefix("vt")?.parse().ok())
}

/// Un-strand the console after the lock comes down.
///
/// When the locked child logs out via the panel, their X server dies and
/// LightDM starts a greeter on another VT — but its VT switch fails because we
/// still hold the VT lock at that instant (nothing retries it), leaving the
/// glass parked on the dead session's black VT. Once switching is re-enabled,
/// detect that state (foreground VT has no live Xorg) and switch to a VT that
/// does.
async fn heal_stranded_vt(sys: &RealSystem) -> VtHeal {
    // The kernel console read (/sys) and the live-Xorg VT scan (/proc) are both
    // synchronous — run them together on the blocking pool so this heal check
    // never occupies the async worker.
    let (active, vts) = tokio::task::spawn_blocking(|| {
        let active = std::fs::read_to_string("/sys/class/tty/tty0/active")
            .ok()
            .map(|s| s.trim().to_string());
        let vts: Vec<u32> = xorg_processes()
            .iter()
            .filter_map(|args| vt_from_xorg_args(args))
            .collect();
        (active, vts)
    })
    .await
    // B6: `None` = no answer this tick, never a panic out of the loop. A
    // probe that blew up is indistinguishable, here, from a greeter that is
    // not up yet — and the next tick is two seconds away.
    .unwrap_or((None, Vec::new()));
    let active = match active {
        Some(s) => s,
        None => return VtHeal::Healthy, // no VT console (headless) — nothing to heal
    };
    if vts.is_empty() {
        return VtHeal::NoTarget; // greeter X not up yet
    }
    if active
        .strip_prefix("tty")
        .and_then(|n| n.parse::<u32>().ok())
        .is_some_and(|n| vts.contains(&n))
    {
        return VtHeal::Healthy;
    }
    let target = vts[0];
    eprintln!("charterd: display stranded on dead {active} — switching to vt{target}");
    let _ = sys.vt().activate_vt(target).await;
    VtHeal::Switched
}

/// The child-facing title/detail for a locked child. The level-state
/// `decision.reason` is authoritative — the `ShowLock` effect fires only on
/// the lock EDGE, and the panel usually spawns ticks later (when the child
/// becomes foreground), so relying on the effect showed the generic fallback.
fn lock_text(d: &ChildDecision) -> (&'static str, &'static str) {
    if let Some(reason) = d.reason {
        return lock_message(reason);
    }
    for e in &d.effects {
        if let EnforcerEffect::ShowLock(reason) = e {
            return lock_message(*reason);
        }
    }
    ("Time's up", "Access is paused.")
}

/// Unambiguous 32-char challenge alphabet (no `I`/`O`, no `0`/`1`) so a shown
/// challenge is easy to read aloud. Exactly 32 chars means a random byte maps to
/// one character with a clean `% 32`.
const CHALLENGE_ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Map raw bytes to challenge characters via [`CHALLENGE_ALPHABET`]. Pure, so
/// the byte→char mapping is deterministically unit-tested.
fn challenge_from_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| CHALLENGE_ALPHABET[(*b as usize) % 32] as char)
        .collect()
}

/// A fresh 4-character offline-unlock challenge from 4 CSPRNG bytes
/// (`/dev/urandom`). `None` if the draw fails — the caller then omits the
/// challenge entirely rather than ship a predictable one (the lock keeps its
/// device-only escape).
fn random_challenge() -> Option<String> {
    use std::io::Read as _;
    let mut bytes = [0u8; 4];
    let mut f = std::fs::File::open("/dev/urandom").ok()?;
    f.read_exact(&mut bytes).ok()?;
    Some(challenge_from_bytes(&bytes))
}

/// Everything the lock/notify spawn paths read but never mutate: the identity
/// DB, the lock binary, each locked child's panel schedule/usage block, whether
/// a guardian is paired (offers the panel's "Ask for more time"), and — when
/// paired — the guardian↔machine conversation key the offline-unlock challenge
/// is answered against.
struct LockSpawnCtx<'a> {
    passwd: &'a str,
    lock_bin: &'a str,
    infos: &'a std::collections::BTreeMap<u32, crate::lock_info::LockInfo>,
    paired: bool,
    /// `Some` (paired) → derive a per-lock challenge + expected code and hand
    /// both to `charter-lock`; `None` (device-only) → no offline unlock.
    unlock_conv_key: Option<[u8; 32]>,
}

/// Apply each child's freeze/thaw to **their** app.slice, and show/kill the
/// fullscreen lock for the active (foreground) child when they're locked —
/// targeting that child's display so the lock draws on their session. `lock`
/// holds the running lock process (keyed by uid) across ticks.
async fn apply_child_decisions(
    sys: &RealSystem,
    decisions: &[ChildDecision],
    ctx: &LockSpawnCtx<'_>,
    roster: &ManagedRoster,
    lock: &mut LockState,
    show_lock: bool,
) {
    let passwd = ctx.passwd;
    let lock_bin = ctx.lock_bin;
    let lock_infos = ctx.infos;
    let mut active_locked: Option<&ChildDecision> = None;
    for d in decisions {
        // Defense-in-depth: only ever freeze/lock a charter-managed CHILD — never
        // the parent/admin/root/system (the policy list is already roster-filtered;
        // this guards the apply path too). Composes with is_valid_freeze_target.
        if !roster.is_lockable(d.uid) {
            continue;
        }
        let slice = managed_freeze_target(d.uid);
        // Reconcile the freeze slice to the child's CURRENT locked state every
        // tick (level-triggered), gated on the slice existing. The enforcer's
        // Freeze/Thaw *effects* are edge-triggered: a Freeze fires once when the
        // child crosses into "locked", and if that apply fails because the child
        // is not logged in yet (no cgroup slice) the edge is consumed and never
        // re-emitted — the child would then run unfrozen once they DO log in.
        // Reconciling on `d.locked` re-applies as soon as the slice appears, and
        // skipping an absent slice avoids erroring on a not-logged-in child.
        // Re-applying the same state is a kernel no-op, so this never thrashes.
        if is_valid_freeze_target(&slice) {
            match reconcile_freeze(d.locked, sys.freezer().exists(&slice).await) {
                Some(FreezeAction::Freeze) => {
                    if let Err(e) = sys.freezer().freeze(&slice).await {
                        eprintln!("charterd: freeze {slice} failed: {e}");
                    }
                }
                Some(FreezeAction::Thaw) => {
                    if let Err(e) = sys.freezer().thaw(&slice).await {
                        eprintln!("charterd: thaw {slice} failed: {e}");
                    }
                }
                None => {} // no live session -> nothing to freeze (not an error)
            }
        }
        for eff in &d.effects {
            if let EnforcerEffect::Warn(level) = eff {
                let (display, _) = tokio::task::spawn_blocking(active_session_x)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| {
                        (
                            std::env::var("CHARTER_DISPLAY").unwrap_or_else(|_| ":0".into()),
                            None,
                        )
                    });
                let body = match level {
                    WarnLevel::Ten => "10 minutes left",
                    WarnLevel::One => "1 minute left — save your game",
                };
                notify_child(
                    passwd,
                    d.uid,
                    &display,
                    "Kintrinsic — time's almost up",
                    body,
                );
            }
        }
        if d.active && d.locked {
            active_locked = Some(d);
        }
    }

    // FreezeOnly (show_lock=false): freeze/thaw is applied above, but the lock
    // screen + VT lock are never driven.
    if !show_lock {
        return;
    }

    match active_locked {
        Some(d) => {
            let already = match lock.proc.as_mut() {
                Some((u, c)) if *u == d.uid => c.try_wait().map(|w| w.is_none()).unwrap_or(false),
                _ => false,
            };
            if !already {
                lock.kill();
                let (title, detail) = lock_text(d);
                // The schedule/usage block: a computed "come back at …" detail
                // beats the static reason line, and the week's hours render
                // beneath it (the child's "when can I go on next?").
                let info = lock_infos.get(&d.uid);
                // A dormant device overrides the HEADLINE too, not just the
                // reason: it locks as `Schedule` (the grant routing needs that)
                // but has no allowed hours to be outside of.
                let title = info
                    .and_then(|i| i.title.as_deref())
                    .unwrap_or(title)
                    .to_string();
                let detail = info
                    .and_then(|i| i.detail.as_deref())
                    .unwrap_or(detail)
                    .to_string();
                let lines = info.map(|i| i.lines.join("\n")).unwrap_or_default();
                // Resolve the locked child's REAL display + X credentials from
                // their live session (DM-agnostic). Fall back to CHARTER_DISPLAY /
                // ~/.Xauthority only if the session env is unreadable.
                let (display, session_xauth) = match tokio::task::spawn_blocking(active_session_x)
                    .await
                    .ok()
                    .flatten()
                {
                    Some((disp, xa)) => (disp, xa),
                    None => (
                        std::env::var("CHARTER_DISPLAY").unwrap_or_else(|_| ":0".into()),
                        None,
                    ),
                };
                let mut cmd = Command::new(lock_bin);
                cmd.env("DISPLAY", &display)
                    .env("CHARTER_LOCK_UID", d.uid.to_string())
                    .env("CHARTER_LOCK_TITLE", title)
                    .env("CHARTER_LOCK_DETAIL", detail)
                    .env("CHARTER_LOCK_LINES", lines);
                // Paired only: the panel's "Ask for more time" (the request
                // publishes via `charter ask-for-more` run AS the child, so
                // the CLI's dimension routing reads the child's own time-left).
                if ctx.paired {
                    if let Some(user) = user_for_uid(passwd, d.uid) {
                        cmd.env("CHARTER_LOCK_CAN_ASK", "1")
                            .env("CHARTER_LOCK_USER", user);
                    }
                }
                // Offline guardian unlock (paired only): a FRESH per-lock
                // CHALLENGE the guardian answers in Kintrinsic. Derive the
                // EXPECTED 8-digit code from the guardian↔machine conversation
                // key and hand both to the lock; the lock compares the typed
                // code (constant-time) and pauses on a match — no network, no
                // clock, no memorized secret. A fresh challenge per lock means a
                // shoulder-surfed code never carries to the next lockout. If
                // entropy is momentarily unavailable we omit it, so the lock
                // falls back to its device-only escape (never a predictable
                // challenge).
                if let Some(conv_key) = ctx.unlock_conv_key {
                    if let Some(challenge) = random_challenge() {
                        let expected = charter_crypto::unlock::unlock_code(&conv_key, &challenge);
                        cmd.env("CHARTER_LOCK_CHALLENGE", &challenge)
                            .env("CHARTER_LOCK_UNLOCK_EXPECT", expected);
                    }
                }
                // Prefer the display's own Xorg `-auth` cookie (root-readable,
                // always valid); fall back to the session env, then ~/.Xauthority.
                // Resolving the Xorg `-auth` cookie scans /proc — off the
                // executor too (`display` is cloned into the closure).
                let display_for_auth = display.clone();
                let xorg_auth =
                    tokio::task::spawn_blocking(move || xorg_auth_for_display(&display_for_auth))
                        .await
                        .ok()
                        .flatten();
                let xauth = xorg_auth
                    .or(session_xauth)
                    .or_else(|| home_for_uid(passwd, d.uid).map(|h| format!("{h}/.Xauthority")));
                if let Some(xa) = xauth {
                    cmd.env("XAUTHORITY", xa);
                }
                if let Ok(child) = cmd.spawn() {
                    lock.proc = Some((d.uid, child));
                }
            }
            let _ = sys.vt().set_vt_switching(false).await;
        }
        None => {
            if lock.kill() {
                // A lock was up last tick: its session may have just ended,
                // stranding the glass on a dead VT (see heal_stranded_vt).
                lock.vt_heal_pending = true;
                lock.vt_heal_misses = 0;
                lock.vt_dm_restarts = 0;
            }
            // Always FIRST: while the VT lock is held the display manager cannot
            // claim a VT for a greeter, so a session that ends under it takes
            // the whole display with it.
            let _ = sys.vt().set_vt_switching(true).await;
            if lock.vt_heal_pending {
                match heal_stranded_vt(sys).await {
                    VtHeal::NoTarget => {
                        lock.vt_heal_misses = lock.vt_heal_misses.saturating_add(1);
                        // Say it ONCE. The silent retry is why a black screen
                        // left no trace in the journal at all.
                        if lock.vt_heal_misses == 1 {
                            eprintln!(
                                "charterd: lock came down but no live X yet — waiting for a greeter"
                            );
                        }
                        if should_restart_dm(lock.vt_heal_misses) {
                            lock.vt_dm_restarts = lock.vt_dm_restarts.saturating_add(1);
                            let ok = restart_display_manager().await;
                            if heal_stays_armed(ok, lock.vt_dm_restarts) {
                                // Failed with retries left: stay armed, wait
                                // out another round of misses. Clearing here
                                // is how the black screen went permanent.
                                lock.vt_heal_misses = 0;
                            } else {
                                if !ok {
                                    eprintln!(
                                        "charterd: display manager would not restart after {} tries — giving up on this heal",
                                        lock.vt_dm_restarts
                                    );
                                }
                                lock.vt_heal_pending = false;
                                lock.vt_heal_misses = 0;
                                lock.vt_dm_restarts = 0;
                            }
                        }
                    }
                    _ => {
                        lock.vt_heal_pending = false;
                        lock.vt_heal_misses = 0;
                    }
                }
            }
        }
    }
}

/// SIGTERM every process owned by a managed child whose exec / flatpak identity
/// matches one of that child's currently-blocked `pkg`s (the guardian's
/// `appRules` clause, evaluated for `now`). Linux has no clean per-app suspend
/// primitive, so terminating the process is the tractable mechanism: a blocked
/// or out-of-hours app cannot stay open, and a relaunch is terminated again on
/// the next tick.
///
/// A full `/proc` sweep, so it runs on the blocking pool (via `spawn_blocking`
/// in the caller). STRICTLY child-scoped, defense-in-depth like
/// `is_lockable`/`is_valid_freeze_target`: a process is a candidate ONLY when
/// `/proc/<pid>` is owned by a uid present in `blocked_by_uid` (the owner uid is
/// re-read here from the `/proc` entry, never trusted from elsewhere), so root
/// and every other user's processes are untouchable. The `blocked_by_uid` map
/// is already filtered to managed, lockable children by the caller. SIGTERM is
/// best-effort (a failed signal is never fatal). Returns the `(uid, pkg, pid)`
/// terminations for the caller to log.
/// NUL-split `/proc/<pid>/cmdline`. Empty when the process has gone or has no
/// cmdline at all (kernel threads) — both of which must read as "no identity"
/// rather than as an empty-string identity that matches every bare `pkg`.
fn read_cmdline(pid: u32) -> Vec<String> {
    std::fs::read(format!("/proc/{pid}/cmdline"))
        .map(|c| {
            c.split(|b| *b == 0)
                .filter(|s| !s.is_empty())
                .map(|s| String::from_utf8_lossy(s).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// Which of `pkgs` (a uid's blocked/scheduled-out identities), if any, a
/// process — by its already-read exe/argv0/cmdline/cgroup at `pid` — is an
/// instance of. Direct match OR a descendant of one: killing
/// `minecraft-launcher` while the Java game it started plays on would be a
/// limit that visibly does nothing. Ancestry is bounded and cycle-safe.
///
/// `lookup` is the ancestor-tree reader (production always
/// `crate::ancestry::read_proc`); injected here so this decision is
/// unit-tested without a live `/proc`, exactly like every other pure helper
/// in this module (`gift_route`, `bucket_view`, …).
#[allow(clippy::too_many_arguments)]
fn find_blocked_hit<'a, F>(
    pkgs: &'a [String],
    pid: u32,
    exe: Option<&str>,
    argv0: Option<&str>,
    cmdline: &[String],
    cmdline_joined: &str,
    cgroup: Option<&str>,
    lookup: F,
) -> Option<&'a str>
where
    F: Fn(u32) -> Option<crate::ancestry::ProcId> + Copy,
{
    pkgs.iter()
        .find(|pkg| {
            crate::app_rules::pkg_matches_process(pkg, exe, argv0, cmdline, cmdline_joined, cgroup)
                || crate::ancestry::matches_with_ancestors(pid, pkg, lookup)
        })
        .map(String::as_str)
}

///
/// # The site-app runtime lockdown
///
/// `site_apps_by_uid` carries each child's active SITE learning apps. For a
/// child that has any, a Chromium-family process is swept unless it is a
/// sanctioned launch of one of them ([`crate::site_app`]). That inverts the
/// usual shape — a default-deny on the runtime with a precisely-defined
/// carve-out, rather than a blocklist with a hole in it.
///
/// The same sanction also exempts a process from an EXPLICIT `appRules` block
/// of the runtime binary, not just from the implied lockdown. A family whose
/// runtime is Chrome and who also block Chrome must not thereby kill their
/// educational windows — that would be the original problem in a new hat.
#[allow(clippy::type_complexity)]
fn terminate_blocked_processes(
    blocked_by_uid: &std::collections::BTreeMap<u32, Vec<String>>,
    site_apps_by_uid: &std::collections::BTreeMap<u32, Vec<charter_proto::LearningApp>>,
) -> Vec<(u32, String, u32)> {
    use std::os::unix::fs::MetadataExt as _;
    let mut acted = Vec::new();
    if blocked_by_uid.is_empty() && site_apps_by_uid.is_empty() {
        return acted;
    }
    let entries = match std::fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return acted,
    };
    for entry in entries.flatten() {
        // Only numeric /proc entries are pids.
        let pid: u32 = match entry.file_name().to_str().and_then(|n| n.parse().ok()) {
            Some(p) => p,
            None => continue,
        };
        let path = entry.path();
        // The /proc/<pid> directory is owned by the process's REAL uid. Target
        // only a uid we were asked to enforce — never root/other users.
        let uid = match std::fs::metadata(&path) {
            Ok(m) => m.uid(),
            Err(_) => continue,
        };
        let blocked = blocked_by_uid.get(&uid);
        let sites = site_apps_by_uid.get(&uid);
        if blocked.is_none() && sites.is_none() {
            continue;
        }
        // Resolved executable path (root can read any /proc/<pid>/exe).
        // Shared with `focus.rs`/`ancestry::read_proc` via
        // `ancestry::read_exe_link` so all three readers of this link always
        // agree, including its "<path> (deleted)" stripping (see
        // `read_exe_link`'s doc comment for the divergence this closed).
        let exe = crate::ancestry::read_exe_link(pid);
        // The exe FILE's owner uid — root-owned (`0`) is enforcement-grade:
        // a ward cannot replace or rename their way into that identity. This
        // is what `site_app::is_sanctioned` now requires (C2) so a process
        // wearing a Chromium-recognised argv0 while actually running an
        // arbitrary ward-owned binary is never spared by the same predicate
        // that decides whether it counts as free time.
        // D2: from the magic symlink, NOT the rendered path — an
        // unprivileged user namespace can bind-mount a ward binary over a
        // distro path, making the rendered path claim root ownership and
        // buying a forged runtime the site-app sanction below. See
        // `ancestry::exe_owner_uid`.
        let exe_uid = crate::ancestry::exe_owner_uid(pid);
        // The cgroup line carries a flatpak app's systemd scope
        // (`app-flatpak-<id>-<instance>.scope`) — the reliable flatpak identity.
        let cgroup = std::fs::read_to_string(path.join("cgroup")).ok();
        // NUL-split argv. Needed twice over: argv0 is the only place a wrapper
        // that `exec -a`s its real binary (Google Chrome) still carries the
        // identity the guardian ticked, and the full list is what the site-app
        // sanction check reads.
        let cmdline = read_cmdline(pid);
        let argv0 = cmdline.first().map(String::as_str);

        // A sanctioned educational window is spared BOTH the implied lockdown
        // and any explicit rule naming the runtime binary — decided once, up
        // front, so the two paths can never disagree.
        let sanctioned = sites.is_some_and(|apps| {
            // The browser process itself…
            crate::site_app::is_sanctioned(&cmdline, exe_uid, apps)
                // …or a renderer/GPU child of one. Their argv is Chromium's own
                // business and bears no resemblance to a launch line, so the
                // only way to ask "is this an educational window?" is to ask
                // whether an ancestor is — using THAT ancestor's own exe_uid,
                // never this process's, so a renderer under a forged browser
                // does not inherit a sanction its own parent never earned.
                || crate::ancestry::has_ancestor_matching(
                    pid,
                    crate::ancestry::read_proc,
                    |id| crate::site_app::is_sanctioned(&id.cmdline, id.exe_uid, apps),
                )
        });

        // Which blocked identity, if any, this process is an instance of.
        let blocked_hit = blocked.and_then(|pkgs| {
            // Computed once per process (only when there is a blocked list to
            // test at all), not once per pkg in it.
            let cmdline_joined = cmdline.join(" ");
            find_blocked_hit(
                pkgs,
                pid,
                exe.as_deref(),
                argv0,
                &cmdline,
                &cmdline_joined,
                cgroup.as_deref(),
                crate::ancestry::read_proc,
            )
        });

        // One place decides, and it is pure + exhaustively unit-tested
        // (`site_app::sweep_decision`) — this is the call that decides whether
        // a child's homework window survives the tick.
        let verdict = crate::site_app::sweep_decision(
            sanctioned,
            sites.is_some(),
            crate::site_app::is_runtime_process(&cmdline, exe.as_deref()),
            blocked_hit,
        );
        if let Some(reason) = verdict {
            // `pid` was just confirmed owned by this managed child, so we never
            // signal another user's process. Best-effort SIGTERM.
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
            acted.push((uid, reason, pid));
        }
    }
    acted
}

/// The managed children's home directories — the per-uid half of the app
/// inventory scan (§2.4). Root-owned launcher dirs need no uid at all; a
/// ward's own `~/.local/share/applications` does, and this module has no
/// identity-DB access of its own. Scoped to roster-LOCKABLE uids only: the
/// parent/admin/root account's home is never treated as a ward's install
/// surface.
fn managed_home_dirs(passwd: &str, roster: &ManagedRoster) -> Vec<String> {
    roster
        .uids()
        .into_iter()
        .filter_map(|uid| home_for_uid(passwd, uid))
        .collect()
}

/// Provision identity, (optionally) stand up the guardian broker, and run the
/// per-child enforce loop forever. Returns only on a fatal setup error.
pub async fn run(config: DaemonConfig) -> Result<(), String> {
    let secret = RealMachineSigner::load_or_create_secret(&config.key_path)
        .map_err(|e| format!("machine key: {e}"))?;

    // Surface this device's pairing code (its pubkey) so the parent can enter it
    // in Kintrinsic to address guardian clauses to THIS device. The pubkey is
    // public — write it world-readable and log it for the setup flow to show.
    if let Some(code) = crate::device_code::device_pairing_code(&secret) {
        let _ = std::fs::write("/var/lib/charter/device.pub", format!("{code}\n"));
        eprintln!("charterd: device pairing code (enter in Kintrinsic): {code}");
    }

    let sys = RealSystem::default();

    // Staged bring-up: observe (log only) / freeze-only / enforce (default).
    let mode = EnforceMode::from_env();
    eprintln!("charterd: enforce mode = {mode:?}");

    // Fail-closed start: block the browser until the first good reconcile — but
    // ONLY when a web-content policy actually exists. On an unconfigured host
    // there is nothing to fail-closed for, and force-locking would block the
    // browser with no reconcile to clear it. Web filtering is also a full-enforce
    // concern; observe/freeze-only leave it off.
    let mut web = WebContentEnforcer::new();
    let mut learn_fs = crate::enactors::learning_apps::RealLearnFs;
    // Installed-app inventory for STATUS (guardian picks native learning apps
    // by name) AND for the `apps` clause's allowlist posture, which blocks
    // "every inventory app not ticked". Seeded BEFORE the loop: the periodic
    // refresh runs after the sweep, so starting empty would leave an allowlist
    // inert for the first tick — and an inventory that exists only from tick
    // two is a rule that briefly isn't. Rescanned when the dirs' mtimes move.
    // Also scans the managed users' own dirs (§2.4) — a one-off passwd/group
    // read here (mirrors the self-heal thaw's own read just below) since
    // `home_for_uid` needs the identity DB, and the loop itself hasn't read
    // it yet at this point in startup.
    let seed_passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
    let seed_group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    let seed_homes = managed_home_dirs(
        &seed_passwd,
        &ManagedRoster::from_system(&seed_passwd, &seed_group),
    );
    // C1 (CRITICAL): a ward-writable scan dir is adversarial input — a
    // `mkfifo` there can block a plain read forever. Three layers, and all
    // three are needed:
    //   1. `scan` refuses the adversarial shapes itself (file type + size);
    //   2. it runs on the blocking pool, so unpredictable filesystem I/O
    //      never occupies an async executor worker — and the mtime
    //      FINGERPRINT rides the SAME hop (N6), since it `stat`s the very
    //      same ward-controlled dirs and would otherwise wedge a worker on
    //      its own while the scan it gates sat safely off-executor;
    //   3. the tick refuses to wait for it forever (`INVENTORY_BUDGET`). (2)
    //      alone does NOT contain this: `.await`ing the handle means a hung
    //      path stalls the enforcement loop regardless of which thread the
    //      hang is on.
    // On no answer, start empty and let the periodic refresh fill it in — the
    // daemon comes up and enforces either way.
    let seed = inventory_within(
        INVENTORY_BUDGET,
        tokio::task::spawn_blocking(move || {
            (
                crate::app_inventory::dirs_fingerprint(&seed_homes),
                crate::app_inventory::scan(&seed_homes),
            )
        }),
    )
    .await;
    // C-C: an inventory that never answered is UNKNOWN, not EMPTY. The
    // difference is load-bearing for §2.4's user-installed learning gate: an
    // empty `user_installed` set reads "nothing on this device is
    // user-installed", so a ward who wedges the seed scan (a hung FUSE mount
    // at `~/.local/share/applications` needs no privilege) and then `flatpak
    // install --user`s a governed learning id would get FREE TIME — the exact
    // hole the gate exists to close, persisting across every backed-off
    // retry. While unknown, `focus::classify` is handed `None` and fails the
    // inventory-dependent arm CLOSED to Screen. The refresh below flips this
    // on its first answer (and, having answered, keeps the previous list on
    // any later stall — a stale inventory is still a KNOWN one).
    let mut inventory_known = inventory_answered(&seed);
    // Set when a scan times out: no further attempt until then (see
    // `INVENTORY_RETRY_AFTER_STALL_SECS`).
    let mut inv_retry_after: i64 = 0;
    if seed.is_none() {
        eprintln!(
            "charterd: app inventory did not answer within {}s at startup — \
             carrying on without it; will retry",
            INVENTORY_BUDGET.as_secs()
        );
        // A timed-out seed's spawn_blocking thread is parked for good
        // (uncancellable) — the first refresh tick must NOT immediately burn
        // a second blocking-pool thread on the same hung path. Same backoff
        // as a timed-out refresh.
        inv_retry_after = sys.clock().now_utc() as i64 + INVENTORY_RETRY_AFTER_STALL_SECS;
    }
    let (mut inv_fingerprint, mut inventory): (u64, Vec<charter_proto::status::AppRef>) =
        seed.unwrap_or_default();
    if matches!(mode, EnforceMode::Enforce) && WebContentEnforcer::is_configured(&sys) {
        let _ = web.force_lock(&sys).await;
    }

    // Independent per-child screen-time enforcement (device-only, multi-child).
    let mut multi = MultiChildEnforcer::new();
    let mut lock = LockState::default();
    let lock_bin =
        std::env::var("CHARTER_LOCK_BIN").unwrap_or_else(|_| "/usr/bin/charter-lock".into());

    // Shared queue: the broker's time.extend enactor deposits approved extensions
    // here and the enforce loop drains them into the brokered child's LIVE ledger.
    let extension_inbox: ExtensionInbox = Arc::new(Mutex::new(Vec::new()));
    // Per-child time-left, republished every tick from the LIVE MultiChildEnforcer
    // so `charter status` shows each child their real remaining time (not the
    // stale/empty single-child EnforcerRuntime).
    let time_left_snapshots: TimeLeftSnapshots =
        Arc::new(Mutex::new(std::collections::BTreeMap::new()));
    // The brokered child's subject (the pairing's sole subject) — maps an approved
    // extension to the local uid the loop applies it to.
    let pairing_opt = load_pairing(&sys);
    let brokered_subject_hex: Option<String> =
        pairing_opt.as_ref().map(|p| p.subject_pubkey.to_hex());

    // B4: a corrupted/zero-filled machine secret must not abort the process —
    // it must leave the daemon enforcing whatever clauses are already cached
    // on disk, with the failure reported rather than hidden. Either
    // construction below can fail independently (each is its own transport),
    // and either failing sets this — see `transport_unavailable` on
    // `PublishedState`/`StatusPayload`.
    let mut transport_unavailable = false;

    // A dedicated transport handle for the STATUS feed (paired only) — a second
    // stateless relay handle, so publishing status never contends with the
    // broker's own transport. `status_last` throttles per-child re-publishes.
    let (status_tx, status_machine): (Option<RealTransportFacade>, Option<PubKey>) =
        match pairing_opt.as_ref() {
            Some(p) => {
                match RealTransportFacade::try_new(secret, p.guardian_pubkey, p.relays.clone()) {
                    Ok(tx) => {
                        let m = tx.machine;
                        (Some(tx), Some(m))
                    }
                    Err(e) => {
                        eprintln!(
                        "charterd: transport unavailable ({e}) — STATUS publish disabled this run"
                    );
                        transport_unavailable = true;
                        (None, None)
                    }
                }
            }
            None => (None, None),
        };
    let mut status_last: std::collections::HashMap<u32, charter_proto::StatusPayload> =
        std::collections::HashMap::new();

    // Offline guardian unlock: derive the guardian↔machine NIP-44 conversation
    // key ONCE (paired only). The lock spawn uses it to compute the expected
    // 8-digit code for a fresh per-lock challenge, so a locked-out ward can be
    // freed by the guardian answering from Kintrinsic — no network, clock, or
    // memorized secret. `None` (unpaired, or a key that won't derive) leaves the
    // lock's device-only escape untouched.
    let unlock_conv_key: Option<[u8; 32]> = pairing_opt.as_ref().and_then(|p| {
        charter_transport::nip44::conversation_key(&secret, p.guardian_pubkey.as_bytes()).ok()
    });

    // If paired, stand up the broker for guardian grants (install/exec) + its
    // signal channel. Per-child screen-time enforcement runs either way — a
    // broker of `None` below (unpaired, OR paired but this transport failed
    // to construct, B4) still leaves every cached clause on disk enforced by
    // the rest of this loop, which reads them straight from `sys`, not
    // through the broker.
    let (broker, signal_rx): (Option<Arc<RealBroker>>, Option<_>) = match pairing_opt {
        Some(pairing) => {
            let enforcer = Arc::new(Mutex::new(EnforcerRuntime::new(&sys, "UTC")));
            match RealTransportFacade::try_new(
                secret,
                pairing.guardian_pubkey,
                pairing.relays.clone(),
            ) {
                Ok(transport) => {
                    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
                    let events: Box<dyn EventSink> = Box::new(ChannelEventSink::new(tx));
                    let broker = Arc::new(build_broker(
                        RealSystem::default(),
                        transport,
                        enforcer.clone(),
                        events,
                        pairing.subject_pubkey,
                        extension_inbox.clone(),
                    ));
                    eprintln!("charterd: paired — guardian grants + per-child limits enforced");
                    (Some(broker), Some(rx))
                }
                Err(e) => {
                    eprintln!(
                        "charterd: transport unavailable ({e}) — no broker this run; \
                         cached clauses are still enforced from disk"
                    );
                    transport_unavailable = true;
                    (None, None)
                }
            }
        }
        None => {
            eprintln!(
                "charterd: not paired — per-child device-only enforcement (limits in {})",
                config.child_limits_dir
            );
            spawn_pair_listener(secret, config.child_limits_dir.clone());
            (None, None)
        }
    };

    // D2: watch the release relays for a newer deb (signed by the pinned
    // release key) and stage it hash-verified under /var/lib/charter/updates.
    // Both modes — an unpaired device-only laptop deserves update news too.
    crate::release_check::real::spawn_release_check();

    // Serve the org.forgesworn.Charter1 surface in BOTH modes: `charter time-left`
    // / `charter status` must work for a device-only child too (the guardian-
    // brokered methods return a friendly "not paired yet" until a phone is added).
    //
    // The connection MUST be held for the daemon's lifetime. Dropping the last
    // handle tears the connection down and releases the bus name — which is
    // exactly what happened on every device-only box: the paired path parked a
    // clone inside signal_pump, but the device-only arm dropped `conn` at the
    // end of the match, so one line after "serving …" the name was gone and
    // every user surface (CLI, tray, the lock's ask) read "daemon unavailable"
    // while enforcement ran on, headless and silent.
    let _dbus_conn = match serve(broker.clone(), time_left_snapshots.clone()).await {
        Ok(conn) => {
            // The signal pump only exists when a broker emits request updates.
            if let (Some(b), Some(rx)) = (broker.clone(), signal_rx) {
                tokio::spawn(signal_pump(conn.clone(), b, rx));
            }
            eprintln!("charterd: serving org.forgesworn.Charter1 on the system bus");
            Some(conn)
        }
        Err(e) => {
            eprintln!("charterd: D-Bus serve unavailable ({e}); enforcing headless");
            None
        }
    };

    // Fail-safe self-heal: thaw anything a previous (crashed) run left frozen +
    // re-enable VT switching BEFORE re-enforcing, so a crash-restart never strands
    // the box locked. (Restart=on-failure in the unit makes this the recovery path.)
    {
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
        let roster = ManagedRoster::from_system(&passwd, &group);
        thaw_all(&sys, &roster, &mut lock).await;
    }

    // Children that have limits but aren't enrolled in charter-managed — warned
    // once each so a mis-enrolled child's un-enforced state is observable, not
    // silent (without per-tick log spam).
    let mut warned_unmanaged: std::collections::HashSet<u32> = std::collections::HashSet::new();

    let interval = Duration::from_secs(config.poll_interval_secs.max(1));
    // Enforcement (session detection, freeze/lock reconcile — all local, cheap)
    // runs on a FAST tick so a freshly-logged-in locked child gets the panel in
    // ~2s, not up to a full poll interval of usable desktop. Network/disk work
    // (relay poll, STATUS publish, web reconcile, usage persist) stays on the
    // configured slow cadence: every `slow_every`-th iteration.
    let fast = Duration::from_secs(2).min(interval);
    let slow_every = (interval.as_secs() / fast.as_secs()).max(1);
    let mut iter: u64 = 0;
    // Charge the active child by the MEASURED awake time between ticks, not
    // a fixed `poll_interval_secs` — the latter systematically under-charges by
    // each tick's processing/relay time. Suspend/hibernate must PAUSE the
    // meter exactly (closing the lid is "not using the device"), so the
    // suspended-time delta is subtracted before the clamp.
    let mut charge = AwakeCharge::new(config.poll_interval_secs);
    // Whether the named model's last tick could read the display at all. Loop
    // state, not configuration: it exists only so the transition is logged
    // ONCE in each direction. An unreadable display charges the screen
    // baseline every two seconds, and a line per tick would bury the one line
    // that says when it started.
    let mut display_unreadable = false;
    // G1: the last probed session activity, so a lock/unlock (or idle/active)
    // transition is logged once in each direction rather than every tick —
    // same reasoning as `display_unreadable` above.
    let mut last_activity = charter_schedule::Activity::Active;
    // 04-G6: was this machine running WITHOUT a warden before now? The stamp
    // below is written every tick; a hole in it bigger than a restart is the
    // cheap half of tamper-evidence — a live USB, a GRUB `init=/bin/bash`, an
    // afternoon of crash-looping and an afternoon switched off all leave the
    // same hole, and the honest reading is "nothing was enforced for N
    // seconds", not an accusation. Computed ONCE, at startup, and carried on
    // every state file this run publishes.
    let enforcement_gap_secs = crate::watchdog::gap_secs(sys.clock().now_utc() as i64);
    if let Some(gap) = enforcement_gap_secs {
        eprintln!(
            "charterd: no warden was running on this machine for {gap}s before this start              — nothing was enforced during that time (reported to the guardian)"
        );
    }
    // Relay health wiring (02b-G1/02b-B5 follow-through): consecutive SLOW
    // ticks whose broker poll reported every relay unreachable. Reset to 0 by
    // any tick that reaches a relay at all, so a climbing count means the
    // relay set itself has gone bad, not one bad poll.
    let mut consecutive_unreachable: u32 = 0;
    // Tell systemd we are up before the first tick, so `WatchdogSec` starts
    // counting from a daemon that is actually looping.
    crate::watchdog::ping();
    loop {
        let slow_tick = iter.is_multiple_of(slow_every);
        iter = iter.wrapping_add(1);
        let now = sys.clock().now_utc() as i64;
        // Awake seconds since the previous loop iteration (saturating, clamped).
        // Computed every iteration (incl. skips below) so a paused stretch never
        // accumulates into the next real tick's charge.
        let elapsed = charge.observe(now, sys.clock().suspended_secs() as i64);

        // Who may be frozen/locked this tick: charter-managed CHILDREN only —
        // never the parent/admin/root/system (the lock-out-recovery guarantee).
        let passwd = std::fs::read_to_string("/etc/passwd").unwrap_or_default();
        let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
        // FAIL-CLOSED on an unreadable identity DB: an empty passwd/group would
        // collapse the roster to "nobody is managed", which would dismiss an
        // active lock + re-enable VT switching (a fail-OPEN). Instead, preserve
        // the prior enforcement state and retry next tick. (`/etc/passwd` and
        // `/etc/group` are world-readable + rename-atomic, so empty == a transient
        // read failure, never a real state.)
        if passwd.trim().is_empty() || group.trim().is_empty() {
            eprintln!(
                "charterd: /etc/passwd or /etc/group unreadable this tick — \
                 preserving enforcement state"
            );
            // Alive and looping, so the watchdog must not kill us over a
            // transient identity-DB read — but NOT `record`ed: enforcement did
            // not actually run this tick, and the stamp means what it says.
            crate::watchdog::ping();
            tokio::time::sleep(fast).await;
            continue;
        }
        let roster = ManagedRoster::from_system(&passwd, &group);

        // Recovery pause: an admin paused enforcement -> thaw + go idle (the loop
        // stays alive, so removing the flag resumes; the broker keeps polling on
        // the slow cadence).
        if enforcement_paused() {
            thaw_all(&sys, &roster, &mut lock).await;
            // 03-G5: a pause used to `continue` past the state-file publish, so
            // the ward's console and the guardian's view simply stopped moving
            // — indistinguishable from a daemon that had died, and the two want
            // opposite reactions. Keep publishing, and say plainly that this is
            // a pause. (The STATUS wire carries no flag for it; see the module
            // note above `run`.)
            for user in crate::state_file::published_users() {
                crate::state_file::mark_paused(&user, now);
            }
            // The tick is alive and doing its job — a pause is not a hang, and
            // the watchdog must not kill a deliberately idle daemon.
            crate::watchdog::ping();
            crate::watchdog::record(now);
            if slow_tick {
                if let Some(b) = &broker {
                    b.poll_once().await;
                }
            }
            tokio::time::sleep(fast).await;
            continue;
        }

        // 1) Load each child's config and resolve their EFFECTIVE policy
        //    (Signet-first): a paired guardian's per-child clauses (cached in the
        //    ChildClauseStore by `b.poll_once()` below, shared on-disk with this
        //    `sys`) are authoritative; the local device-only limits are the
        //    fallback for a child the guardian hasn't set. Filtered to lockable
        //    children so a stray limits.d file can never enroll an admin/root.
        let mut configs = load_child_configs(&config.child_limits_dir);
        // 02b-G8: a guardian subject claimed by two children is dropped from
        // BOTH before anything reads it — not just before policy resolution,
        // but before the bucket reads, the STATUS addressing and the extension
        // routing below, every one of which keys off `cfg.subject`.
        crate::child_policy::drop_duplicate_subjects(&mut configs);
        let configs = configs;
        let policies: Vec<_> = resolve_child_policies(&sys, &passwd, &configs)
            .into_iter()
            .filter(|(uid, _)| {
                let lockable = roster.is_lockable(*uid);
                // Observability: a child with limits who isn't a charter-managed
                // member is NOT enforced (the daemon has no authority to freeze a
                // non-managed uid). Warn once so this isn't a silent fail-open.
                if !lockable && warned_unmanaged.insert(*uid) {
                    eprintln!(
                        "charterd: uid {uid} has limits but isn't in the charter-managed \
                         group — NOT enforced (re-run Kintrinsic Setup for them)"
                    );
                }
                lockable
            })
            .collect();
        multi.sync(&policies, now, load_child_ledgers);

        // 1a) Every managed child's app buckets ("Play is an hour a day"),
        //     read once from their `buckets` clause and shared by every
        //     consumer this tick: the week-roll policy just below, the
        //     gift/time.extend group routing (1c/1b), the foreground meter
        //     (2), the ward's own view (2b), and the spent-bucket sweep
        //     (2c). Read once because independent re-reads could straddle a
        //     clause arriving mid-tick and disagree with each other on the
        //     same screen. Fail-safe: no/garbage clause → no buckets, so an
        //     unreadable cap confiscates nothing.
        let buckets_by_uid: std::collections::BTreeMap<u32, charter_schedule::GrantBuckets> =
            configs
                .iter()
                .filter_map(|(user, cfg)| {
                    let subject = cfg.subject.as_deref()?;
                    let uid = uid_for_user(&passwd, user)?;
                    if !roster.is_lockable(uid) {
                        return None;
                    }
                    let json = sys
                        .child_clauses()
                        .get_child_clause(subject, ClauseKind::Buckets.store_key())
                        .ok()
                        .flatten()?;
                    Some((uid, parse_buckets(Some(&json))?))
                })
                .collect();
        // The buckets clause's own tz/weekStart is the week-roll authority
        // for a buckets-only family (no budget clause in force) — see the
        // precedence comment in `MultiChildEnforcer::tick_attributed`.
        // Refreshed for EVERY tracked child each tick, not just those with a
        // live clause right now, so a removed/expired buckets clause falls
        // back to the enforcement tz rather than sticking to a stale one.
        for (uid, _) in &policies {
            match buckets_by_uid.get(uid) {
                Some(body) => multi.set_buckets_policy(*uid, Some(&body.tz), body.week_start),
                None => multi.set_buckets_policy(*uid, None, None),
            }
        }

        // 1a-ter) Every managed child's `askFirst` apps ("ask before opening
        //     Minecraft"), resolved to labels via the installed-app inventory
        //     right here — so the tray/console can render "Ask to open" rows
        //     with no policy read of their own (state_file's DTO just carries
        //     the answer). Fail-safe like every other apps-clause read: no/
        //     garbage/paused clause, or an empty hint, yields nothing.
        let ask_first_by_uid: std::collections::BTreeMap<
            u32,
            Vec<charter_ipc::dto::AskFirstAppView>,
        > = configs
            .iter()
            .filter_map(|(user, cfg)| {
                let subject = cfg.subject.as_deref()?;
                let uid = uid_for_user(&passwd, user)?;
                if !roster.is_lockable(uid) {
                    return None;
                }
                let json = sys
                    .child_clauses()
                    .get_child_clause(subject, ClauseKind::Apps.store_key())
                    .ok()
                    .flatten()?;
                let apps =
                    crate::apps_policy::ask_first_apps_from_apps(Some(&json), &inventory, now);
                (!apps.is_empty()).then_some((uid, apps))
            })
            .collect();

        // 1a-bis) Refresh each guardian-bound child's cross-device usage view
        //     (USAGE_SYNC, B3) from the broker-verified store, so the budget
        //     dimension enforces the POOLED spend (union rule when both minute
        //     journals exist). Absent/stale views degrade to local-only.
        for (user, cfg) in &configs {
            let (Some(subject), Some(uid)) = (cfg.subject.as_deref(), uid_for_user(&passwd, user))
            else {
                continue;
            };
            multi.set_consolidated(uid, crate::usage_pool::load_consolidated(&sys, subject));
            // A guardian stand-down ("finish up now") for this child, refreshed
            // from the same verified store. Level-triggered like the pooled view:
            // None once it is lifted or lapses, so the ward comes straight back.
            multi.set_stand_down(
                uid,
                charter_spine::standdown::stand_down_now(
                    &charter_spine::standdown::ChildSlot {
                        store: sys.child_clauses(),
                        subject_hex: subject,
                    },
                    now.max(0) as u64,
                ),
            );
        }

        // 1b) Apply any guardian-approved time.extend grants the broker enacted to
        //     the brokered child's LIVE ledger (so an extension actually extends/
        //     unlocks them, not just the D-Bus readout). Mapped subject -> uid via
        //     the child config that charter-pair bound; only drained once the uid
        //     resolves, so a not-yet-set-up child never loses an extension.
        if let Some(uid) = brokered_subject_hex.as_deref().and_then(|sh| {
            configs
                .iter()
                .find(|(_, c)| c.subject.as_deref() == Some(sh))
                .and_then(|(u, _)| uid_for_user(&passwd, u))
        }) {
            let pending =
                std::mem::take(&mut *extension_inbox.lock().unwrap_or_else(|e| e.into_inner()));
            for p in pending {
                // A per-group grant (`bucket_id` set) credits that bucket's
                // own pool; the ordinary whole-device routing credits the
                // schedule/budget pool the enactor decided on.
                match (&p.bucket_id, p.dim) {
                    (Some(bucket_id), _) => {
                        multi.apply_bucket_extension(uid, p.at, &p.req_id, p.minutes, bucket_id);
                    }
                    (None, Some(dim)) => {
                        multi.apply_extension(uid, p.at, &p.req_id, p.minutes, dim);
                    }
                    (None, None) => {}
                }
            }
        }

        // 1c) Minutes the guardian GAVE, with no ask outstanding — the same
        //     ledger a granted ask lands in, so nothing downstream needs a new
        //     case. Every configured child, not just the brokered one: a gift
        //     is addressed to a child, not to whoever last asked. The clause is
        //     re-read every tick and the ledger's id keeps it to one application.
        for (user, cfg) in configs.iter() {
            let (Some(subject), Some(uid)) = (cfg.subject.as_deref(), uid_for_user(&passwd, user))
            else {
                continue;
            };
            let Ok(Some(json)) = sys
                .child_clauses()
                .get_child_clause(subject, ClauseKind::Gift.store_key())
            else {
                continue;
            };
            let Ok(body) = serde_json::from_str::<charter_proto::GiftBody>(&json) else {
                continue;
            };
            let Some(minutes) = body.minutes_now(now as u64) else {
                continue;
            };
            // A named-times gift tops up that bucket's own pool; a plain gift
            // (no groupId — every gift before named-times, and any gift a
            // guardian still addresses to the whole device) lands exactly as
            // before. Named-times gifts never touch stand-down: a stand-down
            // is checked separately by the enforcer core and a bucket pool
            // has no bearing on it. A groupId naming a bucket that no longer
            // exists (deleted, or the buckets clause never arrived) must NOT
            // vanish silently — that is exactly "Mum gave me time and
            // nothing happened" — so it falls back to the whole-device pool,
            // generously, exactly like a groupId-less gift.
            let reqid = format!("gift:{}", body.id);
            let lock_reason = multi.remaining(uid, now).and_then(|r| r.reason);
            match gift_route(&body, lock_reason, buckets_by_uid.get(&uid)) {
                GiftRoute::Bucket(group_id) => {
                    multi.apply_bucket_extension(uid, now, &reqid, minutes, &group_id);
                }
                GiftRoute::Dimension(dim) => {
                    if body.group_id.is_some() {
                        eprintln!(
                            "charterd: gift {} named unknown group {:?} for uid {uid} — \
                             routing to the whole-device pool instead",
                            body.id, body.group_id
                        );
                    }
                    multi.apply_extension(uid, now, &reqid, minutes, dim);
                }
            }
        }

        // 1d) Minutes given or TAKEN BACK at this computer, by an admin who
        //     typed their password into the console (see `local_adjust`). Keyed
        //     by uid, not subject, so this is the ONLY give/take path that
        //     works on a standalone box with no guardian phone paired — which
        //     is most of them when someone is simply setting up a family
        //     laptop. Re-read every tick; the ledger's id list keeps each
        //     entry to one application.
        for (uid, _) in &policies {
            let record = crate::local_adjust::AdjustRecord::load(*uid);
            if record.entries.is_empty() {
                continue;
            }
            let net = crate::local_adjust::apply_to(&mut multi, *uid, now, &record);
            if net != 0 {
                eprintln!("charterd: local adjustment — {net:+}m for uid {uid}");
            }
        }

        // 2) Charge the active (foreground) child; judge every child on their own.
        //    The tick is attributed to a bucket by foreground-window identity:
        //    inside a guardian-designated learning app, time is time-free. The
        //    probe only runs for a child with a live learning/buckets/appRules/
        //    apps clause (I5 — empty everywhere short-circuits before any X
        //    traffic; an appRules-only family must still probe, since that's
        //    exactly the family whose renamed-binary bypass §2.3 exists for).
        // Session detection + focus-window attribution shell out to
        // loginctl/xprop and scan /proc — all synchronous, and on a wedged
        // X/logind each blocks up to ~2s. Run them on the blocking pool so a
        // stalled probe can't occupy this worker and starve the D-Bus serve
        // task. Ordering is preserved: uid first, then (only when some
        // identity clause is live) the X probe, then the focus classification.
        let (active, activity) = tokio::task::spawn_blocking(|| match active_session() {
            Some((uid, sid)) => (Some(uid), session_activity(&sid)),
            None => (None, charter_schedule::Activity::Active),
        })
        .await
        // B6: no answer this tick — nobody is charged, nothing is thawed, and
        // the next tick asks again. A panicking probe must not be a crash-loop
        // that thaws the box on every exit.
        .unwrap_or((None, charter_schedule::Activity::Active));
        // Log the TRANSITION, not the state — same discipline as the Named
        // model's `display_unreadable` line below: this branch runs every
        // couple of seconds, and the fact worth having in the journal is
        // when the meter stopped charging and when it started again.
        if activity != last_activity {
            if activity != charter_schedule::Activity::Active {
                eprintln!(
                    "charterd: the active session is {activity:?} — screen-time charging is \
                     paused until it is unlocked and active again"
                );
            } else {
                eprintln!("charterd: the active session is active again — charging resumed");
            }
            last_activity = activity;
        }
        let learning_apps = active
            .map(|uid| multi.learning_apps(uid))
            .unwrap_or_default();
        // `buckets_by_uid` was already read at 1a (shared with the week-roll
        // policy + gift/time.extend group routing above); this tick's
        // foreground bucket comes from that same map.
        let buckets_body = active
            .and_then(|uid| buckets_by_uid.get(&uid).cloned())
            .unwrap_or_default();
        // §2.3's governed-identity vocabulary needs appRules + the standing
        // `apps` clause too — read fresh here for the active uid only (the
        // SAME two clauses the per-app sweep below reads for every child this
        // tick; this is not a new class of I/O, just an earlier read of it).
        // Fail-safe empty on any parse/read failure, same as every other
        // clause read in this file.
        let active_subject = active.and_then(|uid| {
            configs
                .iter()
                .find(|(u, _)| uid_for_user(&passwd, u) == Some(uid))
                .and_then(|(_, c)| c.subject.clone())
        });
        let app_rule_pkgs: Vec<String> = active_subject
            .as_deref()
            .and_then(|subject| {
                sys.child_clauses()
                    .get_child_clause(subject, ClauseKind::AppRules.store_key())
                    .ok()
                    .flatten()
            })
            .and_then(|json| serde_json::from_str::<charter_schedule::GrantAppRules>(&json).ok())
            .map(|g| g.rules.into_iter().map(|r| r.pkg).collect())
            .unwrap_or_default();
        // A paused `apps` clause governs nothing right now, so its lists must
        // not silently suppress the counter either.
        let apps_clause_pkgs: Vec<String> = active_subject
            .as_deref()
            .and_then(|subject| {
                sys.child_clauses()
                    .get_child_clause(subject, ClauseKind::Apps.store_key())
                    .ok()
                    .flatten()
            })
            .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
            .and_then(|v| charter_proto::GrantApps::from_value(&v).ok())
            .filter(|g| !g.is_paused())
            .map(|g| g.blocked.into_iter().chain(g.allowed).collect())
            .unwrap_or_default();
        // One X probe answers ALL THREE questions (which meter, which bucket,
        // unrecognised) — two probes could straddle an app switch and credit
        // one app's second to another app's allowance. Skipped entirely when
        // NOTHING governs this child, so an unconfigured host never pays for it.
        let (bucket, app_bucket, unrecognised) = if learning_apps.is_empty()
            && buckets_body.buckets.is_empty()
            && app_rule_pkgs.is_empty()
            && apps_clause_pkgs.is_empty()
        {
            (charter_schedule::Bucket::Screen, None, false)
        } else {
            let session_x = tokio::task::spawn_blocking(active_session_x)
                .await
                .ok()
                .flatten();
            match session_x {
                Some((display, xauth)) => {
                    let b = buckets_body.clone();
                    // C-C: `None` while the inventory has never answered —
                    // `classify` then refuses the free-time grant that
                    // depends on it (unknown ≠ empty; see the seed comment).
                    let user_installed = user_installed_for_classify(inventory_known, &inventory);
                    let governed = crate::focus::governed_pkgs(
                        &learning_apps,
                        &b,
                        &app_rule_pkgs,
                        &apps_clause_pkgs,
                    );
                    // D1: §2.3 no longer needs to know who the ward is. The
                    // ward-authored-payload limb (`java -jar ~/x.jar`) is
                    // deleted — it could not be told apart from a system app
                    // opening the child's own document, and the counter now
                    // scopes itself to ward-owned EXECUTABLES, which
                    // `exe_uid` settles on its own.
                    tokio::task::spawn_blocking(move || {
                        crate::focus::attribute_tick(
                            &display,
                            xauth.as_deref(),
                            &learning_apps,
                            &b,
                            user_installed.as_ref(),
                            &governed,
                        )
                    })
                    .await
                    // No attribution this tick: the plain screen baseline, the
                    // same answer an unreadable display already gives.
                    .unwrap_or((
                        charter_schedule::Bucket::Screen,
                        None,
                        false,
                    ))
                }
                None => (charter_schedule::Bucket::Screen, None, false),
            }
        };
        // WHAT the active child's allowance is spent on. Read from the child
        // actually at the machine — the model is a property of one charter, and
        // only the active session accrues.
        let active_model = active
            .and_then(|uid| policies.iter().find(|(u, _)| *u == uid))
            .map(|(_, pol)| charter_schedule::TimeModel::of(pol.budget.as_ref()))
            .unwrap_or(charter_schedule::TimeModel::Session);
        let decisions = match active_model {
            charter_schedule::TimeModel::Session => {
                // Nobody is metering from the display this tick (this child is
                // on the session model, or there is no active child at all), so
                // the Named fallback's remembered state is about a session that
                // is no longer in front. Clear it silently — keeping it would
                // swallow the FIRST line of the next Named child's outage,
                // which is the one line worth having.
                display_unreadable = false;
                multi.tick_attributed_with(
                    activity,
                    active,
                    now,
                    elapsed,
                    bucket,
                    app_bucket.as_deref(),
                    unrecognised,
                )
            }
            // Named: every allowance with a window open, not just the focused
            // one. A second probe of the SAME display in the same tick is safe
            // here in a way a second focus probe never was — two focus probes
            // could straddle an app switch and credit one app's second to
            // another's allowance, whereas the open-window set is what it is
            // regardless of which member of it happens to be in front.
            charter_schedule::TimeModel::Named => {
                let open = match tokio::task::spawn_blocking(active_session_x)
                    .await
                    .ok()
                    .flatten()
                {
                    Some((display, xauth)) => {
                        let b = buckets_body.clone();
                        // Named is read off the ACTIVE child's own budget, so
                        // `active` is Some here by construction. `u32::MAX`
                        // owns no process, so even an impossible None can only
                        // lose the unidentified-window fallback, never widen
                        // it to somebody else's processes.
                        let ward_uid = active.unwrap_or(u32::MAX);
                        tokio::task::spawn_blocking(move || {
                            crate::focus::open_bucket_ids_tick(
                                &display,
                                xauth.as_deref(),
                                &b,
                                ward_uid,
                            )
                        })
                        .await
                        // `None` already means "this display could not be
                        // read", which is exactly what a probe that did not
                        // answer amounts to — and it charges the Session
                        // baseline rather than nothing.
                        .unwrap_or(None)
                    }
                    // No display to resolve at all (a Wayland seat, no Xorg on
                    // the active VT). Indistinguishable, for metering, from a
                    // display we found and could not read.
                    None => None,
                };
                // Log the TRANSITION, not the state: this branch runs every
                // two seconds, and the fact worth having in the journal is
                // when the meter started falling back and when it stopped.
                if open.is_none() != display_unreadable {
                    display_unreadable = open.is_none();
                    if display_unreadable {
                        eprintln!(
                            "charterd: warning — the active display cannot be read, so the named \
                             time model cannot see what is open. Charging the screen baseline \
                             each tick (named allowances are NOT being spent) until it can."
                        );
                    } else {
                        eprintln!(
                            "charterd: the active display is readable again — named allowances \
                             are being metered normally"
                        );
                    }
                }
                multi.tick_open_with(
                    activity,
                    active,
                    now,
                    elapsed,
                    bucket,
                    open.as_deref(),
                    unrecognised,
                )
            }
        };

        // 2b) Republish each child's time-left so `charter status` (served over
        //     D-Bus, keyed by the calling uid) reflects their REAL remaining time,
        //     AND write it to the world-readable state file the consoles read
        //     (see `state_file`: D-Bus answers for the CALLER, which is no use
        //     to a guardian asking about their child).
        {
            // B6: a poisoned lock is recovered, never re-panicked. A panic
            // anywhere in the tick while this was held used to poison it and
            // take `TimeLeft()` down inside the D-Bus handler too.
            let mut snap = time_left_snapshots
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            snap.clear();
            for (uid, pol) in &policies {
                if let Some(r) = multi.remaining(*uid, now) {
                    let mut view = remaining_to_view(&r, now);
                    if pol.learning.as_ref().is_some_and(|l| !l.is_paused()) {
                        view.learning_today_seconds = multi.learning_today(*uid, now);
                    }
                    view.used_today_seconds = multi.used_today(*uid, now);
                    // Each named bucket's own day ("Play: 22 of 60 used"), so a
                    // ward can see which allowance is nearly gone instead of
                    // discovering it when an app refuses to open.
                    if let Some(body) = buckets_by_uid.get(uid) {
                        view.buckets = body
                            .buckets
                            .iter()
                            .map(|b| {
                                // Raw meters (what the ward actually spent) —
                                // `used_seconds` always shows the true figure,
                                // paused or not, and `spent` below carries the
                                // SAME raw totals: M-4 (2026-08-03) found that
                                // subtracting the extra from spent instead of
                                // adding it to the cap saturates at zero for as
                                // long as spent < extra, which freezes the
                                // ward's displayed remaining at the base cap
                                // the whole time the surplus is being spent —
                                // "15m of 15m left" hardware-proven unmoving
                                // through 18m24s of play. `bucket_view` now
                                // does the cap-plus-extra math itself.
                                let day_used =
                                    multi.app_bucket_today(*uid, &b.id, now).unwrap_or(0);
                                let week_used =
                                    multi.app_bucket_week(*uid, &b.id, now).unwrap_or(0);
                                // A `time.extend`/gift grant to this bucket
                                // lifts BOTH walls today — the extra is a
                                // single today-only pool shared by the day and
                                // week axes (see `ExtensionLedger::apply_bucket`).
                                let extra = multi.bucket_extra(*uid, &b.id, now).unwrap_or(0);
                                let spent = charter_schedule::BucketSpent {
                                    day_secs: day_used,
                                    week_secs: week_used,
                                };
                                bucket_view(b, body.is_paused(), day_used, spent, extra)
                            })
                            .collect();
                    }
                    // The `askFirst` apps still gated right now, so the same
                    // tray/console reading this view can offer "Ask to open"
                    // without a policy read of their own.
                    if let Some(apps) = ask_first_by_uid.get(uid) {
                        view.ask_first = apps.clone();
                    }

                    if let Some(user) = user_for_uid(&passwd, *uid) {
                        let paired = configs
                            .iter()
                            .any(|(u, c)| u == &user && c.subject.is_some());
                        crate::state_file::publish(&crate::state_file::PublishedState {
                            v: crate::state_file::STATE_VERSION,
                            user,
                            at: now,
                            time_left: view.clone(),
                            local_adjust_minutes: crate::local_adjust::AdjustRecord::load(*uid)
                                .net_minutes_since(now - 24 * 3600),
                            paired,
                            paused_by_admin: false,
                            enforcement_gap_secs,
                            transport_unavailable,
                        });
                    }
                    snap.insert(*uid, view);
                }
            }
        }

        // 2c) Per-app rules: the guardian's `appRules` clause (kind 6, routed
        //     per-child into the child clause store just like `learning`) can
        //     block specific apps outright or outside their own allowed hours.
        //     Read each managed child's clause and compute the `pkg`s blocked
        //     NOW (fail-safe: no/garbage clause → empty). Inert when unset — an
        //     empty map short-circuits the /proc sweep below, so an unconfigured
        //     host never pays for it.
        let mut blocked_by_uid: std::collections::BTreeMap<u32, Vec<String>> =
            std::collections::BTreeMap::new();

        // 2b-bis) Every managed child's ACTIVE SITE apps. Kintrinsic installs
        //     Chromium so these can run resolver-pinned; that makes it a
        //     runtime, not a browser. There is no Chromium managed-policy
        //     renderer anywhere in Kintrinsic — Firefox is the browser we can
        //     govern — so a Chromium the ward can drive freely would be an
        //     unmanaged browser sitting beside a managed one. While a child has
        //     any site app in force, an unsanctioned runtime process of theirs
        //     is therefore swept.
        //
        //     Implied by turning a site app on, deliberately: a separate "also
        //     block Chromium" toggle is one a guardian can forget, and
        //     forgetting it is exactly the hole. Native learning apps do not
        //     trigger it (no Chromium involved), and unticking every site app
        //     hands the browser straight back.
        //
        //     Not gated on `cfg.subject`: a device-local learning list (an
        //     un-governed child, see `child_policy`'s precedence) materialises
        //     the same launchers and needs the same lockdown.
        let mut site_apps_by_uid: std::collections::BTreeMap<u32, Vec<charter_proto::LearningApp>> =
            std::collections::BTreeMap::new();
        // Whether a runtime exists to host them at all — three path checks,
        // read here rather than taken from the enactor's result because STATUS
        // is built before the launcher reconcile runs, and reporting last
        // tick's answer would lag a fresh `apt install chromium` by a minute.
        let site_runtime_present =
            crate::enactors::learning_apps::LearnFs::runtime_path(&learn_fs).is_some();
        for (user, _cfg) in &configs {
            let Some(uid) = uid_for_user(&passwd, user) else {
                continue;
            };
            if !roster.is_lockable(uid) {
                continue;
            }
            let sites: Vec<charter_proto::LearningApp> = multi
                .learning_apps(uid)
                .into_iter()
                .filter(|a| a.kind == charter_proto::LearningAppKind::Site)
                .collect();
            if !sites.is_empty() {
                site_apps_by_uid.insert(uid, sites);
            }
        }

        // The inventory identities, for the `apps` clause's allowlist posture.
        // Computed once per tick outside the child loop — it is machine-wide.
        let inventory_pkgs: Vec<String> = inventory.iter().map(|a| a.pkg.clone()).collect();
        for (user, cfg) in &configs {
            let Some(subject) = cfg.subject.as_deref() else {
                continue; // no guardian binding → no per-child app clauses
            };
            let Some(uid) = uid_for_user(&passwd, user) else {
                continue;
            };
            // Defense-in-depth: never target a non-lockable uid (admin/root/system).
            if !roster.is_lockable(uid) {
                continue;
            }
            let clause_json = sys
                .child_clauses()
                .get_child_clause(subject, ClauseKind::AppRules.store_key())
                .ok()
                .flatten();
            let mut blocked = crate::app_rules::blocked_pkgs_now(clause_json.as_deref(), now);

            // …plus the STANDING per-app policy (the `apps` clause, kind 4) —
            // the toggles and time-boxed holds in Kintrinsic's Apps list. Stored
            // by the broker since D3 but never read here until 0.7.1, so a
            // laptop silently ignored what a phone enforced. Holds are resolved
            // inside (shared `GrantApps::effective_at`), so "allow it for an
            // hour" ends at the same instant on every platform. Fail-safe like
            // the rest: absent/malformed/paused ⇒ nothing added.
            let apps_json = sys
                .child_clauses()
                .get_child_clause(subject, ClauseKind::Apps.store_key())
                .ok()
                .flatten();
            blocked.extend(crate::apps_policy::blocked_pkgs_from_apps(
                apps_json.as_deref(),
                &inventory_pkgs,
                now,
            ));

            // …plus any app whose BUCKET allowance is spent for today ("Play"
            // is gone → the games in it stop). This closes the bucket, NOT the
            // device: the ward keeps the rest of their day, and only these
            // apps are swept. Fail-safe: an unreadable clause caps nothing
            // (absent from the map entirely). A `time.extend`/gift grant to a
            // bucket lifts BOTH the day and week walls today (one shared pool
            // — `ExtensionLedger::apply_bucket`), so the extra is subtracted
            // from each meter before the caps are judged here — the VIEW
            // instead ADDS the same extra onto the displayed cap
            // (`groupProgressLine`/`bucket_day_remaining_secs`'s callers).
            // `spent - extra >= cap` and `spent >= cap + extra` are the same
            // inequality algebraically, so the two paths render the same
            // verdict — EXCEPT that `saturating_sub` floors `spent - extra` at
            // 0 rather than going negative when a gift exceeds what was
            // spent. That floor only reads as "still gone" if `cap` is itself
            // 0 (`0 >= 0`); `GrantBuckets::is_valid` rejects a 0-minute cap by
            // construction, so that never happens here. This dependency is
            // load-bearing: relaxing the >=1-minute floor on a bucket's cap
            // would silently reopen a screen/sweep disagreement on any bucket
            // a guardian has gifted more than its ward has yet spent.
            if let Some(body) = buckets_by_uid.get(&uid) {
                let spent = charter_schedule::spent_bucket_apps(body, |id| {
                    let extra = multi.bucket_extra(uid, id, now).unwrap_or(0);
                    charter_schedule::BucketSpent {
                        day_secs: multi
                            .app_bucket_today(uid, id, now)
                            .unwrap_or(0)
                            .saturating_sub(extra),
                        week_secs: multi
                            .app_bucket_week(uid, id, now)
                            .unwrap_or(0)
                            .saturating_sub(extra),
                    }
                });
                blocked.extend(spent);
            }

            // One tidy for all three sources, so no order of extends can leave
            // a duplicate pkg to be logged (and killed) twice.
            blocked.sort();
            blocked.dedup();
            if !blocked.is_empty() {
                blocked_by_uid.insert(uid, blocked);
            }
        }

        // 3) Per-child freeze + the active child's lock screen — gated by mode.
        //    observe: log only; freeze-only: freeze without lock; enforce: full.
        if mode.applies_effects() {
            // The panel's schedule/usage block for any locked child (built here
            // — only the loop holds the policy + ledgers).
            let mut lock_infos = std::collections::BTreeMap::new();
            for d in decisions.iter().filter(|d| d.locked) {
                if let Some(pol) = policies.iter().find(|(u, _)| *u == d.uid).map(|(_, p)| p) {
                    let rem = multi.remaining(d.uid, now);
                    lock_infos.insert(
                        d.uid,
                        crate::lock_info::lock_info(
                            pol.schedule.as_ref(),
                            pol.budget.as_ref(),
                            rem.and_then(|r| r.next_open_secs),
                            multi.used_today(d.uid, now).unwrap_or(0),
                            now,
                        ),
                    );
                }
            }
            apply_child_decisions(
                &sys,
                &decisions,
                &LockSpawnCtx {
                    passwd: &passwd,
                    lock_bin: &lock_bin,
                    infos: &lock_infos,
                    paired: broker.is_some(),
                    unlock_conv_key,
                },
                &roster,
                &mut lock,
                mode.shows_lock(),
            )
            .await;

            // Per-app enforcement: SIGTERM any blocked app the child has open.
            // The /proc sweep + kill run on the blocking pool (like the other
            // blocking scans in this loop); skipped entirely when nothing is
            // blocked this tick. Strictly scoped to the child's own uid inside
            // the sweep (owner re-checked from /proc).
            // …and the site-app runtime lockdown, which needs the sweep even
            // when nothing is blocked: a child with educational windows in
            // force has Chromium demoted from browser to runtime.
            if !blocked_by_uid.is_empty() || !site_apps_by_uid.is_empty() {
                let map = blocked_by_uid.clone();
                let sites = site_apps_by_uid.clone();
                let acted =
                    tokio::task::spawn_blocking(move || terminate_blocked_processes(&map, &sites))
                        .await
                        .unwrap_or_default();
                for (uid, pkg, pid) in acted {
                    eprintln!(
                        "charterd: per-app rule — terminated blocked {pkg} (pid {pid}) for uid {uid}"
                    );
                }
            }
        } else {
            for line in observe_lines(&decisions, &passwd) {
                eprintln!("{line}");
            }
            // Observe mode: report the intended per-app actions without acting.
            for (uid, blocked) in &blocked_by_uid {
                for pkg in blocked {
                    eprintln!(
                        "charterd: observe — would terminate blocked app {pkg} for uid {uid}"
                    );
                }
            }
        }

        // Fast iterations end here — everything below is network/disk work on
        // the slow cadence.
        // The enforcement tick got all the way through: freeze/thaw reconciled,
        // lock driven, meters charged, state published. THAT is what the
        // watchdog keep-alive vouches for — a daemon that is merely still a
        // process is exactly the failure `WatchdogSec` exists to catch — and
        // what the stamp records for the next start to compare against.
        crate::watchdog::ping();
        crate::watchdog::record(now);

        if !slow_tick {
            tokio::time::sleep(fast).await;
            continue;
        }

        // 4) Persist each child's accrued time AND the guardian's given
        //    minutes (restart durability). The extension snapshot was
        //    produced here and thrown away, which is what made a reboot
        //    refill a schedule gift in full and lose an approved
        //    `time.extend` outright.
        for (uid, usage, ext) in multi.snapshots() {
            save_child_usage(uid, &usage);
            save_child_extension(uid, &ext);
        }

        // 4b) Refresh the installed-app inventory when the launcher OR the
        //     managed users' own dirs moved (cheap mtime fingerprint) —
        //     STATUS below carries it. `passwd`/`roster` were already read
        //     fresh this tick, above.
        if now >= inv_retry_after {
            let homes = managed_home_dirs(&passwd, &roster);
            let previous = inv_fingerprint;
            let force = inventory.is_empty();
            // C1/N6: BOTH the fingerprint and the scan it gates run on the
            // blocking pool in one hop, under a hard budget — see the seed
            // call's comment for why each of the three layers is needed. The
            // rescan decision is made inside the closure so the fingerprint's
            // own `stat`s never touch the async executor either.
            let task = tokio::task::spawn_blocking(move || {
                let fp = crate::app_inventory::dirs_fingerprint(&homes);
                let rescanned =
                    (fp != previous || force).then(|| crate::app_inventory::scan(&homes));
                (fp, rescanned)
            });
            match inventory_within(INVENTORY_BUDGET, task).await {
                Some((fp, rescanned)) => {
                    inv_fingerprint = fp;
                    if let Some(fresh) = rescanned {
                        inventory = fresh;
                    }
                    // The scan ANSWERED — the inventory is now known, even if
                    // (legitimately) empty. See the seed's C-C comment.
                    inventory_known = true;
                }
                None => {
                    // Carry on with the inventory we already have. Enforcement
                    // continues; only the app LIST goes stale. The backoff is
                    // also what rate-limits this line to one per window — and
                    // it names no ward path, since which directory hung is
                    // information about the child's machine that a log has no
                    // business carrying.
                    inv_retry_after = now + INVENTORY_RETRY_AFTER_STALL_SECS;
                    eprintln!(
                        "charterd: app inventory did not answer within {}s — \
                         keeping the previous list, retrying in {}s",
                        INVENTORY_BUDGET.as_secs(),
                        INVENTORY_RETRY_AFTER_STALL_SECS
                    );
                }
            }
        }

        // 5) If paired: subscribe -> verify -> enact guardian grants (install/exec).
        if let Some(b) = &broker {
            let poll = b.poll_once().await;
            if poll.relays_unreachable {
                consecutive_unreachable = consecutive_unreachable.saturating_add(1);
            } else {
                consecutive_unreachable = 0;
            }
            if poll.released {
                // The guardian pressed Disconnect and the release authenticated
                // (S7). The pairing and every clause are already off disk; what
                // is left is a running daemon holding stale in-memory authority.
                //
                // Restart rather than unwind in place, the same way scan-to-pair
                // does in the other direction: charterd comes back up through its
                // UNPAIRED arm, `ExecStopPost=charter-recovery --thaw-all` lifts
                // the freeze on the way out, and there is no half-released state
                // for anyone to reason about.
                eprintln!(
                    "charterd: guardian RELEASE applied — this device is no longer managed; \
                     restarting unpaired"
                );
                let _ = std::process::Command::new("systemctl")
                    .args(["try-restart", "charterd.service"])
                    .status();
                return Ok(());
            }
        }

        // 5b) STATUS feed: publish each guardian-bound child's live state to the
        //     guardian (paired only), so the Kintrinsic PWA can show time-left +
        //     lock state. Numbers/enums only (no PII); throttled to displayable
        //     state-changes + a heartbeat so it isn't a per-tick relay flood.
        if let (Some(stx), Some(machine)) = (&status_tx, status_machine) {
            for (uid, pol) in &policies {
                // Only a guardian-bound child (has a `subject`) gets STATUS —
                // there is a guardian to address it to.
                let subject = configs.iter().find_map(|(u, c)| {
                    if uid_for_user(&passwd, u) == Some(*uid) {
                        c.subject.as_deref().and_then(|h| PubKey::from_hex(h).ok())
                    } else {
                        None
                    }
                });
                let (Some(subject), Some(remaining)) = (subject, multi.remaining(*uid, now)) else {
                    continue;
                };
                let tz = enforcement_tz_of(pol.schedule.as_ref(), pol.budget.as_ref());
                let mut status = crate::status_emit::build_status(
                    subject,
                    machine,
                    now as u64,
                    &remaining,
                    multi.used_today(*uid, now).unwrap_or(0),
                    None,
                    day_key(tz, now),
                    None,
                    pol.source,
                );
                // Say what the rule actually IS, not just how much is left.
                // Limits set on this machine were invisible to Kintrinsic, which
                // showed "time limit each day: not selected" while the device
                // enforced two hours — and gave no warning that a phone-side
                // limit would discard them wholesale. Read with `source`, these
                // tell the guardian both the rule and who set it.
                if let Some(b) = pol.budget.as_ref() {
                    status.daily_minutes = b.daily_minutes;
                    status.weekly_minutes = b.weekly_minutes;
                }
                if pol.learning.as_ref().is_some_and(|l| !l.is_paused()) {
                    status.learning_today_secs = multi.learning_today(*uid, now);
                }
                // Say when the educational windows CANNOT open. The enactor
                // retries a missing runtime forever and used to tell nobody, so
                // a guardian ticked Khan Academy and simply nothing happened —
                // which is how it went unnoticed that learning had never worked
                // on a Chrome-only machine at all.
                status.site_runtime_missing =
                    (site_apps_by_uid.contains_key(uid) && !site_runtime_present).then_some(true);
                // 03-G5/04-G6/B4/relay-health follow-up: the same runtime
                // facts already carried on the world-readable state file, now
                // also on the guardian-facing wire. Each absent unless it is
                // actually true/non-zero, matching every other STATUS field's
                // "absent is the ordinary state" rule.
                status.paused_by_admin = enforcement_paused().then_some(true);
                status.enforcement_gap_secs = enforcement_gap_secs.map(|g| g.max(0) as u64);
                status.relay_unreachable_polls =
                    (consecutive_unreachable > 0).then_some(consecutive_unreachable);
                status.transport_unavailable = transport_unavailable.then_some(true);
                // Per-bucket ("named time") progress, so the guardian's app
                // can show "Play: 22 of 60 used" instead of a blank. RAW
                // meters — not extra-adjusted — because this is the guardian's
                // own account of what was actually spent, unlike the ward's
                // view (which shows what still binds after any grant she
                // approved). Capped defensively at MAX_STATUS_GROUPS, though a
                // valid `buckets` clause can never exceed it.
                status.groups = buckets_by_uid.get(uid).map(|body| {
                    body.buckets
                        .iter()
                        .take(charter_proto::MAX_STATUS_GROUPS)
                        .map(|b| charter_proto::GroupSpent {
                            id: b.id.clone(),
                            day_secs: multi.app_bucket_today(*uid, &b.id, now).unwrap_or(0),
                            week_secs: multi.app_bucket_week(*uid, &b.id, now).unwrap_or(0),
                        })
                        .collect()
                });
                // The union-rule input (B3): today's active-minutes journal,
                // omitted while empty rather than sent as an all-zero bitmap.
                status.active_minutes_today = multi.minutes_today_b64(*uid, now);
                // Capped HERE too, not only on parse. `StatusPayload::from_json`
                // truncating at MAX_STATUS_APPS protects a RECEIVER; it does
                // nothing about what this daemon puts on the wire. A ward with
                // 4096 `.desktop` files in their own writable dir (the scan's
                // per-dir cap, times one dir per managed user) would otherwise
                // inflate EVERY status emit to megabytes and push it past the
                // relay's event-size limit — costing the guardian the entire
                // status feed, time-left included, to publish an app list.
                status.apps = (!inventory.is_empty()).then(|| {
                    inventory
                        .iter()
                        .take(charter_proto::MAX_STATUS_APPS)
                        .cloned()
                        .collect()
                });
                // Unrecognised-screen-time aggregate (§2.3) — ONE number,
                // never which program it was (see AppRef/StatusPayload doc
                // comments for the privacy rule). Absent while zero so
                // payloads with nothing to report stay byte-identical.
                let unrecognised = multi.unrecognised_today(*uid, now).unwrap_or(0);
                status.unrecognised_today_secs = (unrecognised > 0).then_some(unrecognised);
                // What this machine is running, so Kintrinsic can say "update
                // available" (and afterwards, that it landed). Until this,
                // charterd reported no version at all and the laptop was
                // invisible to the guardian on that score.
                status.app_version_code = Some(crate::version::version_code());
                status.app_version_name = Some(crate::version::version_name().to_string());
                if crate::status_emit::should_emit_status(
                    status_last.get(uid),
                    &status,
                    STATUS_HEARTBEAT_SECS,
                ) {
                    stx.emit_status(&status.to_json(), now as u64).await;
                    status_last.insert(*uid, status);
                }
            }
        }

        // 6) Learning-app launchers: reconcile the union of every child's
        //    active learning apps (level-triggered, inert when none).
        {
            let mut union: Vec<charter_proto::LearningApp> = Vec::new();
            let mut seen = std::collections::BTreeSet::new();
            for uid in multi.uids() {
                for app in multi.learning_apps(uid) {
                    if seen.insert(app.id.clone()) {
                        union.push(app);
                    }
                }
            }
            for act in crate::enactors::learning_apps::reconcile(&union, &mut learn_fs) {
                eprintln!("charterd: learning launcher {act:?}");
            }
        }

        // 7) Web content policy (machine-wide), fail-closed — full enforce only.
        if matches!(mode, EnforceMode::Enforce)
            && matches!(
                web.reconcile(&sys).await,
                crate::web_content::WebReconcile::Failed
            )
        {
            let _ = web.force_lock(&sys).await;
        }

        tokio::time::sleep(fast).await;
    }
}

/// Meters the AWAKE seconds between enforcement-loop iterations.
///
/// `observe(now, suspended)` takes the wall clock and the cumulative
/// suspended-time counter (`Clock::suspended_secs`, i.e. boottime − monotonic)
/// and returns the seconds to charge: the wall delta minus whatever part of it
/// the machine spent in S3 sleep / hibernate — a closed lid pauses the ward's
/// meter instead of eating his budget. The result is clamped to
/// 4 × poll interval so a wall-clock step (or any accounting surprise) is
/// never credited as a burst of active screen time; the first observation
/// charges 0.
struct AwakeCharge {
    max_elapsed: i64,
    /// (wall unix secs, cumulative suspended secs) at the previous iteration.
    last: Option<(i64, i64)>,
}

impl AwakeCharge {
    fn new(poll_interval_secs: u64) -> Self {
        AwakeCharge {
            max_elapsed: (poll_interval_secs.max(1) as i64).saturating_mul(4),
            last: None,
        }
    }

    fn observe(&mut self, now: i64, suspended: i64) -> u64 {
        let charged = match self.last {
            Some((prev_now, prev_suspended)) => {
                let slept = (suspended - prev_suspended).max(0);
                (now - prev_now - slept).clamp(0, self.max_elapsed)
            }
            None => 0,
        };
        self.last = Some((now, suspended));
        charged as u64
    }
}

/// Parse a `buckets` clause body. Fail-safe by design: `None` on absent or
/// unparseable JSON, and the caller then caps nothing — a cap we cannot read
/// must never confiscate an app the ward is entitled to. (`learning` fails the
/// other way, to Screen, because there an error must never make time free.)
fn parse_buckets(json: Option<&str>) -> Option<charter_schedule::GrantBuckets> {
    let body: charter_schedule::GrantBuckets = serde_json::from_str(json?).ok()?;
    body.is_valid().then_some(body)
}

/// Where a gift's minutes should land.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GiftRoute {
    /// A named-times gift (`groupId` set): tops up that bucket's own pool.
    Bucket(String),
    /// A plain gift (every gift before named-times, and any gift still
    /// addressed to the whole device): the ordinary whole-device dimension.
    Dimension(charter_schedule::Dimension),
}

/// Decide a gift's route, pure so the routing rule is unit-tested without a
/// live enforcer: a `groupId` naming a bucket that actually EXISTS in `buckets`
/// (the ward's current, valid `buckets` clause) always wins — a named-times
/// gift never falls back to the whole-device pool just because the child
/// also happens to be schedule-locked. A `groupId` naming a bucket that does
/// NOT exist (deleted since the gift was authored, or no buckets clause at
/// all) must not make the gift vanish — "Mum gave me time and nothing
/// happened" is exactly the failure this guards against — so it falls back
/// to the whole-device pool, generously, exactly like a groupId-less gift.
/// Absent `groupId` (or an unknown one) behaves EXACTLY as every gift did
/// before named-times — minutes on the budget pool, unless the child is
/// currently locked BY THE SCHEDULE (a closed window), in which case only a
/// schedule extension can actually unlock them.
fn gift_route(
    body: &charter_proto::GiftBody,
    lock_reason: Option<charter_schedule::LockReason>,
    buckets: Option<&charter_schedule::GrantBuckets>,
) -> GiftRoute {
    let dimension_fallback = || {
        GiftRoute::Dimension(match lock_reason {
            Some(charter_schedule::LockReason::Schedule) => charter_schedule::Dimension::Schedule,
            _ => charter_schedule::Dimension::Budget,
        })
    };
    match body.group_id.as_deref() {
        Some(group_id) => {
            let exists = buckets.is_some_and(|b| b.buckets.iter().any(|bk| bk.id == group_id));
            if exists {
                GiftRoute::Bucket(group_id.to_string())
            } else {
                dimension_fallback()
            }
        }
        None => dimension_fallback(),
    }
}

/// Build one named bucket's ward-facing view from its cap and this tick's
/// usage. `day_used` is the RAW day meter (never extra-adjusted), so "Play:
/// 22 of 60 used" always shows what was actually spent; `spent` is ALSO raw
/// (day/week totals exactly as metered, never extra-adjusted) — `extra` is
/// added onto the CAP instead, so the remainders count down honestly through
/// a granted surplus rather than freezing.
///
/// M-4 (2026-08-03, hardware-proven): the previous shape subtracted `extra`
/// from `spent` before judging the caps. That saturates at zero for as long
/// as `spent < extra`, which reads as "still exactly at the base cap" for the
/// entire time the ward is spending the granted surplus — "15m of 15m left"
/// sat frozen through 18m24s of continuous play after a 30-minute gift on a
/// spent 15-minute bucket, even though enforcement correctly allowed the
/// extra time. Adding `extra` to the cap instead means `remaining` moves on
/// every tick from the moment the grant lands, reaching 0 only once the
/// FULL cap-plus-extra has actually been spent — the same "cap + today's
/// extra" phrasing Kintrinsic's guardian-side fix (commit c078faa) already
/// uses for the equivalent freeze on her side.
///
/// A paused set still meters (the picture stays honest) but caps nothing: day
/// fields read `0` (the established day discipline — paired with
/// `capped: false`) and week fields read `-1`, the same "not set" sentinel
/// `budget_week_seconds`/`week_remaining_seconds` use everywhere else, so a
/// week field is never misread as "the week is used up" when nothing is
/// actually capped.
fn bucket_view(
    b: &charter_schedule::AppBucket,
    paused: bool,
    day_used: u64,
    spent: charter_schedule::BucketSpent,
    extra: u64,
) -> charter_ipc::dto::BucketView {
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
    charter_ipc::dto::BucketView {
        id: b.id.clone(),
        label: b.label.clone(),
        used_seconds: day_used,
        // The BINDING wall (whichever axis is tightest), not just the day
        // axis: a weekly-only bucket has NO day limit at all, and showing
        // the day figure here would read "0m left / capped" while real
        // week-minutes remain (colliding with `limit_seconds == 0` meaning
        // "paused" — dto.rs). `day_limit.or(week_limit)` so a day-only
        // bucket keeps showing its day wall unchanged, and a weekly-only
        // bucket shows its real (week) wall instead of a phantom zero.
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

#[cfg(test)]
mod tests {
    // ---- kill sweep: find_blocked_hit (honest attribution) -----------------

    fn minecraft_jvm_cmdline() -> Vec<String> {
        vec![
            "java".into(),
            "-cp".into(),
            "/home/kid/.minecraft/libs/x.jar".into(),
            "net.minecraft.client.main.Main".into(),
        ]
    }

    /// The kill sweep's blocked set includes a bare/renamed-launcher JVM by
    /// its `cmdline:` identity alone, with NO launcher ancestor in the tree
    /// at all (`lookup` returns `None` for everything) — proving the sweep
    /// does not depend on ancestry for this identity form the way it must
    /// for a path-named launcher.
    #[test]
    fn the_kill_sweeps_blocked_set_includes_a_bare_cmdline_identity() {
        let pkgs = vec!["cmdline:net.minecraft.client.main.Main".to_string()];
        let cmdline = minecraft_jvm_cmdline();
        let joined = cmdline.join(" ");
        let no_ancestor = |_: u32| None;
        assert_eq!(
            super::find_blocked_hit(
                &pkgs,
                4242,
                Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
                Some("java"),
                &cmdline,
                &joined,
                None,
                no_ancestor,
            ),
            Some("cmdline:net.minecraft.client.main.Main")
        );
    }

    /// An unrelated Java program must never be swept just because Minecraft
    /// is blocked — the whole reason `cmdline:` names the game's own main
    /// class rather than `java` itself.
    #[test]
    fn the_kill_sweep_spares_an_unrelated_java_process() {
        let pkgs = vec!["cmdline:net.minecraft.client.main.Main".to_string()];
        let cmdline = vec![
            "java".to_string(),
            "-jar".to_string(),
            "/opt/foo/foo.jar".to_string(),
        ];
        let joined = cmdline.join(" ");
        let no_ancestor = |_: u32| None;
        assert_eq!(
            super::find_blocked_hit(
                &pkgs,
                99,
                Some("/usr/lib/jvm/java-21-openjdk/bin/java"),
                Some("java"),
                &cmdline,
                &joined,
                None,
                no_ancestor,
            ),
            None
        );
    }

    /// A Wayland session can never satisfy the Xorg search, and the freeze
    /// lands regardless — so the failure must name itself rather than read as
    /// a transient miss. Everything else keeps the ordinary message.
    #[test]
    fn a_wayland_session_is_named_in_the_log_not_reported_as_a_generic_miss() {
        let note = super::display_protocol_note(Some("wayland")).expect("wayland is named");
        assert!(note.contains("WAYLAND"));
        // The two things a reader must learn: apps still freeze, and the ward
        // is left without the panel or the ask.
        assert!(note.contains("frozen"));
        assert!(note.contains("ask for more time"));
        // Case and stray whitespace come from loginctl, not from us.
        assert!(super::display_protocol_note(Some(" Wayland\n")).is_some());
        // x11 / tty / absent are the ordinary miss.
        assert!(super::display_protocol_note(Some("x11")).is_none());
        assert!(super::display_protocol_note(Some("tty")).is_none());
        assert!(super::display_protocol_note(None).is_none());
    }

    /// G1 (03b-linux-charterd-bins-matching): parses
    /// `loginctl show-session <sid> -p LockedHint -p IdleHint --value`'s
    /// two-line output. `LockedHint` (line 1) wins over `IdleHint` (line 2);
    /// anything short of a "yes" on either line, including empty/unreadable
    /// output, is Active — the fail-toward-charging direction every other
    /// probe in this file takes.
    #[test]
    fn activity_from_hints_parses_the_two_loginctl_lines() {
        use charter_schedule::Activity;
        assert_eq!(super::activity_from_hints("yes\nno"), Activity::Locked);
        assert_eq!(super::activity_from_hints("no\nyes"), Activity::Idle);
        assert_eq!(super::activity_from_hints("no\nno"), Activity::Active);
        assert_eq!(super::activity_from_hints(""), Activity::Active);
        // Case and stray whitespace come from loginctl, not from us — same
        // discipline as `display_protocol_note` above.
        assert_eq!(super::activity_from_hints("YES\n no "), Activity::Locked);
    }

    #[test]
    fn parse_xorg_auth_arg_extracts_the_cookie_path() {
        let args: Vec<String> =
            "/usr/lib/xorg/Xorg -core :1 -seat seat0 -auth /var/run/lightdm/root/:1 -nolisten tcp vt8"
                .split(' ')
                .map(String::from)
                .collect();
        assert_eq!(
            super::parse_xorg_auth_arg(&args).as_deref(),
            Some("/var/run/lightdm/root/:1")
        );
        // No -auth arg -> None (fall through to the next resolver).
        let no_auth: Vec<String> = "/usr/lib/xorg/Xorg :0"
            .split(' ')
            .map(String::from)
            .collect();
        assert_eq!(super::parse_xorg_auth_arg(&no_auth), None);
    }

    #[test]
    fn display_from_xorg_args_finds_the_bare_display_arg() {
        let args: Vec<String> =
            "/usr/lib/xorg/Xorg -core :1 -seat seat0 -auth /var/run/lightdm/root/:1 -nolisten tcp vt8"
                .split(' ')
                .map(String::from)
                .collect();
        // ":1" is picked, not the ":1" suffix inside the -auth path.
        assert_eq!(super::display_from_xorg_args(&args).as_deref(), Some(":1"));
        let none: Vec<String> = "/usr/lib/xorg/Xorg -core vt8"
            .split(' ')
            .map(String::from)
            .collect();
        assert_eq!(super::display_from_xorg_args(&none), None);
    }

    #[test]
    fn lock_text_uses_level_reason_not_just_the_edge_effect() {
        use crate::child_policy::PolicySource;
        use crate::multi_child::ChildDecision;
        // The panel usually spawns ticks AFTER the lock edge: effects are
        // empty, but the level-state reason must still pick the right copy.
        let d = ChildDecision {
            uid: 1002,
            active: true,
            locked: true,
            reason: Some(charter_schedule::LockReason::Schedule),
            effects: vec![],
            source: PolicySource::DeviceOnly,
        };
        assert_eq!(super::lock_text(&d).0, "Outside allowed hours");
        // No reason anywhere -> the generic fallback.
        let bare = ChildDecision { reason: None, ..d };
        assert_eq!(super::lock_text(&bare).0, "Time's up");
    }

    #[test]
    fn vt_from_xorg_args_parses_the_vt_number() {
        let args: Vec<String> =
            "/usr/lib/xorg/Xorg -core :1 -seat seat0 -auth /var/run/lightdm/root/:1 -nolisten tcp vt8 -novtswitch"
                .split(' ')
                .map(String::from)
                .collect();
        assert_eq!(super::vt_from_xorg_args(&args), Some(8));
        // No vtN arg -> None ("-novtswitch" must not parse as a VT).
        let none: Vec<String> = "/usr/lib/xorg/Xorg :0 -novtswitch"
            .split(' ')
            .map(String::from)
            .collect();
        assert_eq!(super::vt_from_xorg_args(&none), None);
    }

    #[test]
    fn user_for_uid_reverses_the_passwd_lookup() {
        let passwd = "root:x:0:0::/root:/bin/bash\nchild:x:1002:1002::/home/child:/bin/bash\n";
        assert_eq!(super::user_for_uid(passwd, 1002).as_deref(), Some("child"));
        assert_eq!(super::user_for_uid(passwd, 9999), None);
    }

    #[test]
    fn awake_charge_meters_wall_time_between_ticks() {
        let mut c = super::AwakeCharge::new(10);
        assert_eq!(c.observe(1000, 0), 0); // first observation: no baseline
        assert_eq!(c.observe(1002, 0), 2); // normal 2s fast tick
        assert_eq!(c.observe(1015, 0), 13); // a slow iteration still fully charged
    }

    #[test]
    fn awake_charge_does_not_bill_a_suspend_gap() {
        let mut c = super::AwakeCharge::new(10);
        c.observe(1000, 0);
        // Lid closed for an hour: wall +3605, of which 3600 suspended.
        // Only the 5 awake seconds around the sleep edges are charged.
        assert_eq!(c.observe(1000 + 3605, 3600), 5);
        // And the next ordinary tick is back to normal.
        assert_eq!(c.observe(1000 + 3607, 3600), 2);
    }

    #[test]
    fn awake_charge_clamps_clock_steps_both_ways() {
        let mut c = super::AwakeCharge::new(10);
        c.observe(1000, 0);
        assert_eq!(c.observe(90_000, 0), 40); // forward step: at most 4x poll
        assert_eq!(c.observe(500, 0), 0); // backward step: never negative

        // A suspended counter that (impossibly) ran backwards is ignored:
        assert_eq!(c.observe(503, -7), 3);
    }

    #[test]
    fn notify_argv_runs_notify_send_as_the_child_in_their_session() {
        let argv = super::notify_argv("child", 1002, ":0", "Kintrinsic", "10 minutes left");
        // Runs as the child, with their session display + bus, critical urgency.
        assert_eq!(argv[0], "runuser");
        assert!(argv.contains(&"child".to_string()));
        assert!(argv.iter().any(|a| a == "DISPLAY=:0"));
        assert!(argv
            .iter()
            .any(|a| a == "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1002/bus"));
        assert!(argv.iter().any(|a| a == "notify-send"));
        assert!(argv.contains(&"10 minutes left".to_string()));
    }

    #[test]
    fn challenge_from_bytes_maps_into_the_unambiguous_alphabet() {
        // Indices into "ABCDEFGHJKLMNPQRSTUVWXYZ23456789": 0=A, 8=J, 24=2, 31=9.
        assert_eq!(super::challenge_from_bytes(&[0, 8, 24, 31]), "AJ29");
        // The byte is reduced `% 32`, so 32 wraps back to index 0 ('A').
        assert_eq!(super::challenge_from_bytes(&[32, 40]), "AJ");
        // Every produced char is in the alphabet — no ambiguous 0/O/1/I.
        let s = super::challenge_from_bytes(&[3, 200, 77, 255]);
        assert_eq!(s.chars().count(), 4);
        assert!(s.bytes().all(|b| super::CHALLENGE_ALPHABET.contains(&b)));
    }

    #[test]
    fn random_challenge_is_four_alphabet_chars() {
        // /dev/urandom is present on the test host; if a sandbox hid it, `None`
        // is a valid (graceful) outcome — the lock keeps its device-only escape.
        if let Some(ch) = super::random_challenge() {
            assert_eq!(ch.chars().count(), 4);
            assert!(ch.bytes().all(|b| super::CHALLENGE_ALPHABET.contains(&b)));
        }
    }

    // ---- gift routing (§2 of the named-times wiring) ----------------------

    fn gift(group_id: Option<&str>) -> charter_proto::GiftBody {
        charter_proto::GiftBody {
            v: 1,
            issued_at: 1,
            id: "g1".into(),
            minutes: 15,
            expires_at: 9_999_999_999,
            group_id: group_id.map(String::from),
        }
    }

    fn play_bucket(daily: Option<u16>, weekly: Option<u16>) -> charter_schedule::AppBucket {
        charter_schedule::AppBucket {
            id: "play".into(),
            label: "Play".into(),
            apps: vec!["mc".into()],
            daily_minutes: daily,
            weekly_minutes: weekly,
        }
    }

    fn buckets_with(bs: Vec<charter_schedule::AppBucket>) -> charter_schedule::GrantBuckets {
        charter_schedule::GrantBuckets {
            v: 1,
            buckets: bs,
            paused: None,
            week_start: None,
            tz: "UTC".into(),
            issued_at: 1,
        }
    }

    #[test]
    fn a_gift_with_a_group_id_always_routes_to_that_bucket() {
        // Even while the child is schedule-locked, a named-times gift never
        // falls back to the whole-device pool — it targets exactly the
        // bucket the guardian named, as long as that bucket actually exists.
        let live = buckets_with(vec![play_bucket(Some(60), None)]);
        assert_eq!(
            super::gift_route(
                &gift(Some("play")),
                Some(charter_schedule::LockReason::Schedule),
                Some(&live)
            ),
            super::GiftRoute::Bucket("play".into())
        );
        assert_eq!(
            super::gift_route(&gift(Some("play")), None, Some(&live)),
            super::GiftRoute::Bucket("play".into())
        );
    }

    #[test]
    fn a_gift_to_a_deleted_group_falls_back_to_the_whole_device_pool() {
        // The gift names "play", but the ward's CURRENT buckets clause has
        // no such bucket (deleted since the gift was authored) — and a
        // second case with no buckets clause at all. Both must fall back
        // exactly like a groupId-less gift, never vanish.
        let other_only = buckets_with(vec![play_bucket(Some(60), None)]);
        assert_eq!(
            super::gift_route(&gift(Some("reading")), None, Some(&other_only)),
            super::GiftRoute::Dimension(charter_schedule::Dimension::Budget)
        );
        assert_eq!(
            super::gift_route(
                &gift(Some("reading")),
                Some(charter_schedule::LockReason::Schedule),
                Some(&other_only)
            ),
            super::GiftRoute::Dimension(charter_schedule::Dimension::Schedule)
        );
        assert_eq!(
            super::gift_route(&gift(Some("play")), None, None),
            super::GiftRoute::Dimension(charter_schedule::Dimension::Budget),
            "no buckets clause at all is the same as an unknown group"
        );
    }

    #[test]
    fn a_gift_without_a_group_id_routes_exactly_as_before() {
        // Byte-identical to pre-named-times behaviour: budget pool by
        // default, schedule pool only when a closed window is the block.
        assert_eq!(
            super::gift_route(&gift(None), None, None),
            super::GiftRoute::Dimension(charter_schedule::Dimension::Budget)
        );
        assert_eq!(
            super::gift_route(
                &gift(None),
                Some(charter_schedule::LockReason::Budget),
                None
            ),
            super::GiftRoute::Dimension(charter_schedule::Dimension::Budget)
        );
        assert_eq!(
            super::gift_route(
                &gift(None),
                Some(charter_schedule::LockReason::Schedule),
                None
            ),
            super::GiftRoute::Dimension(charter_schedule::Dimension::Schedule)
        );
    }

    /// A gift landing on the whole-device pool (whether groupId-less or a
    /// fallback from a deleted group) is idempotent by its `gift:{id}`
    /// reqId, exactly like every other extension — the same seam
    /// `MultiChildEnforcer::apply_extension` already guarantees.
    #[test]
    fn a_gift_to_a_deleted_group_lands_as_device_time_idempotently() {
        use crate::child_policy::{EffectivePolicy, PolicySource};
        use crate::multi_child::MultiChildEnforcer;

        const UID: u32 = 1001;
        const NOW: i64 = 1_704_499_200; // any weekday noon-ish instant

        let mut multi = MultiChildEnforcer::new();
        multi.sync(
            &[(
                UID,
                EffectivePolicy {
                    schedule: None,
                    budget: None,
                    learning: None,
                    source: PolicySource::DeviceOnly,
                },
            )],
            NOW,
            |_| (None, None),
        );

        let g = gift(Some("reading")); // "reading" does not exist below
        let live = buckets_with(vec![play_bucket(Some(60), None)]);
        let reqid = format!("gift:{}", g.id);
        let route = super::gift_route(&g, None, Some(&live));
        assert_eq!(
            route,
            super::GiftRoute::Dimension(charter_schedule::Dimension::Budget)
        );
        let super::GiftRoute::Dimension(dim) = route else {
            unreachable!()
        };

        assert!(
            multi.apply_extension(UID, NOW, &reqid, 15, dim),
            "first application lands on the whole-device budget pool"
        );
        assert!(
            !multi.apply_extension(UID, NOW, &reqid, 15, dim),
            "a replay of the SAME reqId does not double-credit"
        );
        assert_eq!(multi.remaining(UID, NOW).unwrap().extension_secs, 15 * 60);
    }

    // ---- bucket view assembly (§3 of the named-times wiring) --------------

    fn spent(day_secs: u64, week_secs: u64) -> charter_schedule::BucketSpent {
        charter_schedule::BucketSpent {
            day_secs,
            week_secs,
        }
    }

    #[test]
    fn a_paused_set_reports_no_day_limit_and_unset_week_fields() {
        // Paused caps NOTHING, but still meters: `used_seconds` shows the raw
        // spend, day fields read 0 (the established discipline, paired with
        // capped:false), and week fields read -1 ("not set"), never 0 — the
        // same sentinel `budget_week_seconds` uses everywhere else.
        let b = play_bucket(Some(60), Some(300));
        let v = super::bucket_view(&b, true, 40 * 60, spent(40 * 60, 200 * 60), 0);
        assert_eq!(v.used_seconds, 40 * 60, "raw usage still shown");
        assert_eq!(v.limit_seconds, 0);
        assert_eq!(v.remaining_seconds, 0);
        assert_eq!(v.week_limit_seconds, -1);
        assert_eq!(v.week_remaining_seconds, -1);
        assert!(!v.spent);
        assert!(!v.capped);
    }

    #[test]
    fn an_unpaused_daily_only_bucket_leaves_week_fields_unset() {
        // No weekly cap on this bucket at all: week fields read -1, not 0.
        let b = play_bucket(Some(60), None);
        let v = super::bucket_view(&b, false, 20 * 60, spent(20 * 60, 20 * 60), 0);
        assert_eq!(v.limit_seconds, 60 * 60);
        assert_eq!(v.remaining_seconds, 40 * 60);
        assert_eq!(v.week_limit_seconds, -1);
        assert_eq!(v.week_remaining_seconds, -1);
        assert!(v.capped);
    }

    #[test]
    fn a_weekly_cap_reports_the_binding_remainder_not_the_day_axis() {
        let b = play_bucket(Some(60), Some(300));
        // 20m today (headroom on the day axis ALONE) but the week is fully
        // spent: `remaining_seconds` must report the BINDING remainder
        // (min of the two axes), so it reads 0 right alongside `spent`
        // being true — never a positive "time left" next to "spent".
        let v = super::bucket_view(&b, false, 20 * 60, spent(20 * 60, 300 * 60), 0);
        assert_eq!(v.limit_seconds, 60 * 60, "the day limit is still shown");
        assert_eq!(v.week_limit_seconds, 300 * 60);
        assert_eq!(v.week_remaining_seconds, 0);
        assert_eq!(
            v.remaining_seconds, 0,
            "the week wall binds even though the day axis alone has room"
        );
        assert!(v.spent);
    }

    /// A weekly-ONLY bucket (no daily cap at all) has no day wall to show —
    /// `limit_seconds`/`remaining_seconds` must fall back to the WEEK wall
    /// instead of reading 0 (which collides with "paused") while real
    /// week-minutes remain.
    #[test]
    fn a_weekly_only_bucket_shows_the_week_wall_as_its_limit() {
        let b = play_bucket(None, Some(300));
        let v = super::bucket_view(&b, false, 40 * 60, spent(40 * 60, 100 * 60), 0);
        assert_eq!(v.limit_seconds, 300 * 60);
        assert_eq!(v.remaining_seconds, 200 * 60);
        assert_eq!(v.week_limit_seconds, 300 * 60);
        assert_eq!(v.week_remaining_seconds, 200 * 60);
        assert!(v.capped);
        assert!(!v.spent);
    }

    /// M-4: a gifted bucket must count down HONESTLY through the surplus,
    /// never freeze at the base cap. 15m cap + a 30m extra = 45m total; as
    /// raw spend climbs through the 15m..45m surplus band, `remaining_seconds`
    /// must move on every tick and reach 0 only once the full 45m is spent —
    /// not the moment the base 15m cap alone is reached (hardware-proven
    /// "15m of 15m left" frozen for 18m24s of play, 2026-08-03).
    #[test]
    fn a_gifted_bucket_counts_down_honestly_through_the_surplus_and_never_freezes() {
        let b = play_bucket(Some(15), None);
        let extra = 30 * 60; // a 30-minute gift on top of the 15m cap

        // 5 minutes into the surplus (20m raw spend, cap+extra = 45m).
        let v1 = super::bucket_view(&b, false, 20 * 60, spent(20 * 60, 20 * 60), extra);
        assert_eq!(
            v1.limit_seconds,
            45 * 60,
            "the shown cap grows by the extra"
        );
        assert_eq!(v1.remaining_seconds, 25 * 60);
        assert!(
            !v1.spent,
            "must not read spent while the surplus is unspent"
        );

        // 10 minutes into the surplus (25m raw spend) — the clock must have
        // MOVED, never sat pinned at the base-cap remainder from v1's tick.
        let v2 = super::bucket_view(&b, false, 25 * 60, spent(25 * 60, 25 * 60), extra);
        assert_eq!(v2.remaining_seconds, 20 * 60);
        assert!(
            v2.remaining_seconds < v1.remaining_seconds,
            "remaining must move between two ticks inside the surplus region: {v1:?} -> {v2:?}"
        );

        // At exactly the BASE cap alone (15m raw spend) — the old, buggy
        // shape read `spent: true, remaining: 0` here; with a live extra this
        // must still show 30m left and NOT read spent.
        let v_at_cap = super::bucket_view(&b, false, 15 * 60, spent(15 * 60, 15 * 60), extra);
        assert!(
            !v_at_cap.spent,
            "must not read spent at the base cap alone while extra remains: {v_at_cap:?}"
        );
        assert_eq!(v_at_cap.remaining_seconds, 30 * 60);

        // Only once the FULL cap+extra (45m) is spent does it reach 0/spent.
        let v_full = super::bucket_view(&b, false, 45 * 60, spent(45 * 60, 45 * 60), extra);
        assert_eq!(v_full.remaining_seconds, 0);
        assert!(v_full.spent);
    }

    // ---- the enforcer wiring end to end (weekly wall + extra) -------------

    const YOUNGER: u32 = 1001;

    fn play_group(daily: Option<u16>, weekly: Option<u16>) -> charter_schedule::GrantBuckets {
        charter_schedule::GrantBuckets {
            v: 2,
            buckets: vec![play_bucket(daily, weekly)],
            paused: None,
            week_start: None,
            tz: "UTC".into(),
            issued_at: 1,
        }
    }

    /// Reproduces the exact extra-adjusted closure the enforcement loop feeds
    /// `spent_bucket_apps` (see the `blocked_by_uid` build, ~1740), against a
    /// live `MultiChildEnforcer` — proving the wiring, not re-proving
    /// `spent_bucket_apps` itself (already covered in `charter-schedule`).
    fn blocked_now(
        multi: &crate::multi_child::MultiChildEnforcer,
        uid: u32,
        body: &charter_schedule::GrantBuckets,
        now: i64,
    ) -> Vec<String> {
        charter_schedule::spent_bucket_apps(body, |id| {
            let extra = multi.bucket_extra(uid, id, now).unwrap_or(0);
            charter_schedule::BucketSpent {
                day_secs: multi
                    .app_bucket_today(uid, id, now)
                    .unwrap_or(0)
                    .saturating_sub(extra),
                week_secs: multi
                    .app_bucket_week(uid, id, now)
                    .unwrap_or(0)
                    .saturating_sub(extra),
            }
        })
    }

    #[test]
    fn the_weekly_wall_closes_the_bucket_while_daily_headroom_remains() {
        use crate::child_policy::{EffectivePolicy, PolicySource};
        use crate::multi_child::MultiChildEnforcer;

        // A Monday 00:00 UTC start, so two consecutive days stay in the same
        // (Mon-start) week. Fully unconstrained on schedule/budget: this test
        // is about the app-bucket meter + sweep, which credit and judge
        // independently of the whole-device dimensions.
        const MON_0000: i64 = 1_704_067_200; // 2024-01-01 00:00 UTC, a Monday
        let policy = EffectivePolicy {
            schedule: None,
            budget: None,
            learning: None,
            source: PolicySource::DeviceOnly,
        };

        let mut multi = MultiChildEnforcer::new();
        multi.sync(&[(YOUNGER, policy)], MON_0000, |_| (None, None));
        let body = play_group(Some(60), Some(90)); // 60m/day, 90m/week — id "play"
                                                   // Monday: 50 minutes of Play (well under the 60m daily cap).
        multi.tick_attributed(
            Some(YOUNGER),
            MON_0000 + 3600,
            50 * 60,
            charter_schedule::Bucket::Screen,
            Some("play"),
            false,
        );
        // Tuesday: 45 more minutes — the day's OWN total (45m) still has
        // headroom against the 60m daily cap, but the week's running total
        // (50+45=95m) has now blown the 90m weekly cap.
        let tue = MON_0000 + 24 * 3600 + 3600;
        multi.tick_attributed(
            Some(YOUNGER),
            tue,
            45 * 60,
            charter_schedule::Bucket::Screen,
            Some("play"),
            false,
        );
        assert_eq!(multi.app_bucket_today(YOUNGER, "play", tue), Some(45 * 60));
        assert_eq!(multi.app_bucket_week(YOUNGER, "play", tue), Some(95 * 60));

        assert_eq!(
            blocked_now(&multi, YOUNGER, &body, tue),
            vec!["mc".to_string()],
            "the weekly wall alone must close the bucket"
        );

        // A 10-minute grant (via the same seam the gift/time.extend routing
        // uses) lifts BOTH walls today: day 45-10=35 (<60, open) and week
        // 95-10=85 (<90, open) — the whole bucket re-opens.
        assert!(multi.apply_bucket_extension(YOUNGER, tue, "req-extra", 10, "play"));
        assert!(
            blocked_now(&multi, YOUNGER, &body, tue).is_empty(),
            "the extra must re-open BOTH walls, not just the one that bit"
        );
        // Idempotent: replaying the same reqId does not stack another 10m.
        assert!(!multi.apply_bucket_extension(YOUNGER, tue, "req-extra", 10, "play"));
    }

    /// Reproduces the exact raw-spent + extra dataflow the tick loop feeds
    /// `bucket_view` (see the `view.buckets` build, ~1889) against a live
    /// `MultiChildEnforcer` — the same "prove the wiring" pattern as
    /// `blocked_now`, but for the ward's own display instead of the sweep.
    fn ward_view_now(
        multi: &crate::multi_child::MultiChildEnforcer,
        uid: u32,
        b: &charter_schedule::AppBucket,
        paused: bool,
        now: i64,
    ) -> charter_ipc::dto::BucketView {
        let day_used = multi.app_bucket_today(uid, &b.id, now).unwrap_or(0);
        let week_used = multi.app_bucket_week(uid, &b.id, now).unwrap_or(0);
        let extra = multi.bucket_extra(uid, &b.id, now).unwrap_or(0);
        let spent = charter_schedule::BucketSpent {
            day_secs: day_used,
            week_secs: week_used,
        };
        super::bucket_view(b, paused, day_used, spent, extra)
    }

    /// M-4 end to end: a real `MultiChildEnforcer` ticking real play time
    /// through a real `apply_bucket_extension` grant must show the SAME
    /// honest, moving countdown the pure `bucket_view` test above proves —
    /// this is the actual production wiring, not a re-statement of it.
    #[test]
    fn a_real_bucket_gift_counts_down_honestly_through_the_surplus() {
        use crate::child_policy::{EffectivePolicy, PolicySource};
        use crate::multi_child::MultiChildEnforcer;

        const MON_0000: i64 = 1_704_067_200; // 2024-01-01 00:00 UTC, a Monday
        let policy = EffectivePolicy {
            schedule: None,
            budget: None,
            learning: None,
            source: PolicySource::DeviceOnly,
        };
        let mut multi = MultiChildEnforcer::new();
        multi.sync(&[(YOUNGER, policy)], MON_0000, |_| (None, None));
        let b = play_bucket(Some(15), None); // 15m/day cap, id "play"

        // Spend the whole 15m cap.
        multi.tick_attributed(
            Some(YOUNGER),
            MON_0000 + 15 * 60,
            15 * 60,
            charter_schedule::Bucket::Screen,
            Some("play"),
            false,
        );
        let before = ward_view_now(&multi, YOUNGER, &b, false, MON_0000 + 15 * 60);
        assert!(before.spent, "the base cap alone must read spent");
        assert_eq!(before.remaining_seconds, 0);

        // A 30-minute gift lifts the wall; the ward keeps playing INTO it.
        assert!(multi.apply_bucket_extension(YOUNGER, MON_0000 + 15 * 60, "req-gift", 30, "play"));
        let t1 = MON_0000 + 15 * 60 + 5 * 60; // 5m into the surplus
        multi.tick_attributed(
            Some(YOUNGER),
            t1,
            5 * 60,
            charter_schedule::Bucket::Screen,
            Some("play"),
            false,
        );
        let v1 = ward_view_now(&multi, YOUNGER, &b, false, t1);
        assert!(
            !v1.spent,
            "must not read spent while the surplus is unspent"
        );
        assert_eq!(v1.remaining_seconds, 25 * 60, "45m total - 20m spent");

        let t2 = t1 + 5 * 60; // 10m into the surplus
        multi.tick_attributed(
            Some(YOUNGER),
            t2,
            5 * 60,
            charter_schedule::Bucket::Screen,
            Some("play"),
            false,
        );
        let v2 = ward_view_now(&multi, YOUNGER, &b, false, t2);
        assert!(
            v2.remaining_seconds < v1.remaining_seconds,
            "the clock must MOVE between two ticks inside the surplus, never freeze: \
             {v1:?} -> {v2:?}"
        );
        assert_eq!(v2.remaining_seconds, 20 * 60);

        // Only after the FULL 45m (cap+extra) is spent does it hit 0/spent.
        let t3 = MON_0000 + 15 * 60 + 30 * 60;
        multi.tick_attributed(
            Some(YOUNGER),
            t3,
            (t3 - t2) as u64,
            charter_schedule::Bucket::Screen,
            Some("play"),
            false,
        );
        let v3 = ward_view_now(&multi, YOUNGER, &b, false, t3);
        assert_eq!(v3.remaining_seconds, 0);
        assert!(v3.spent);
    }
}

#[cfg(test)]
mod vt_heal_tests {
    use super::*;

    /// A locked session that ends under the VT lock takes the whole display
    /// with it: the display manager cannot claim a VT for a greeter, so no X
    /// exists to switch TO and the heal finds nothing, silently, forever. Robin's
    /// laptop sat black for over three minutes on 2026-07-27 with the power
    /// button as the only way out.
    #[test]
    fn a_run_of_no_x_eventually_restarts_the_display_manager() {
        assert!(!should_restart_dm(0), "the first miss must just wait");
        assert!(
            !should_restart_dm(VT_HEAL_GIVE_UP_TICKS - 1),
            "a greeter still starting must not be interrupted"
        );
        assert!(should_restart_dm(VT_HEAL_GIVE_UP_TICKS));
        assert!(should_restart_dm(u8::MAX), "and it must stay decided");
    }

    /// A FAILED display-manager restart must not clear the heal — that
    /// recreates the original permanent black screen with exactly one log
    /// line. But it cannot re-arm forever either: on a box where the
    /// `display-manager` alias doesn't resolve, an unbounded retry is a
    /// restart storm every ~10s all night.
    #[test]
    fn a_failed_dm_restart_keeps_the_heal_armed_but_not_forever() {
        // Success: done, stand down the heal.
        assert!(!heal_stays_armed(true, 1));
        // Failure with retries left: stay armed for another round of misses.
        assert!(heal_stays_armed(false, 1));
        assert!(heal_stays_armed(false, VT_HEAL_MAX_DM_RESTARTS - 1));
        // Failure at the cap: concede — loudly, but only once per lock.
        assert!(!heal_stays_armed(false, VT_HEAL_MAX_DM_RESTARTS));
        assert!(!heal_stays_armed(false, u8::MAX));
    }

    /// Long enough to outlast a normal greeter respawn, short enough that a
    /// person staring at a black screen is not left concluding it is dead.
    #[test]
    fn the_give_up_window_is_seconds_not_minutes() {
        let secs = u32::from(VT_HEAL_GIVE_UP_TICKS) * 2; // the 2s fast tick
        assert!((6..=20).contains(&secs), "give-up window was {secs}s");
    }
}

/// N6: the app-inventory scan must never be able to stall the enforcement
/// loop. `spawn_blocking` alone does not achieve that — the tick still awaits
/// the handle — so the budget in [`inventory_within`] is the layer that
/// actually contains a hung ward-controlled path.
#[cfg(test)]
mod inventory_budget_tests {
    use super::*;

    /// The ordinary case: a scan that answers is used.
    #[tokio::test]
    async fn a_scan_that_answers_is_used() {
        let got = inventory_within(
            Duration::from_secs(5),
            std::future::ready(Ok::<u8, tokio::task::JoinError>(7)),
        )
        .await;
        assert_eq!(got, Some(7));
    }

    /// A scan that NEVER returns — a `stat`/`read_dir` on a ward-mounted hung
    /// FUSE filesystem at `~/.local/share/applications`, which takes no
    /// privilege to set up — must give the tick its thread back. Before the
    /// budget existed this `.await` never completed: no lock, no sweep, no
    /// STATUS, indefinitely, which is the C1 DoS with an extra step. The test
    /// itself would hang without the fix, so a regression fails loudly rather
    /// than quietly passing.
    #[tokio::test]
    async fn a_scan_that_never_answers_gives_the_tick_its_thread_back() {
        let hung = std::future::pending::<Result<u8, tokio::task::JoinError>>();
        let got = inventory_within(Duration::from_millis(20), hung).await;
        assert_eq!(
            got, None,
            "a hung scan must yield 'no answer', so the caller keeps the \
             previous inventory and carries on enforcing"
        );
    }

    /// A PANICKED scan task is the same answer as a hung one. It used to be
    /// an `.expect()`, i.e. a ward-writable directory could take the whole
    /// daemon down.
    #[tokio::test]
    async fn a_panicking_scan_does_not_take_the_daemon_down() {
        let task = tokio::task::spawn_blocking(|| panic!("scan blew up"));
        assert_eq!(inventory_within(Duration::from_secs(5), task).await, None);
    }

    /// Hours a PERMANENTLY hung path takes to exhaust a blocking pool of
    /// `pool` threads when retried every `retry_secs`. A timed-out
    /// `spawn_blocking` task cannot be cancelled — dropping the handle
    /// detaches it and the thread stays parked on the hung path for good — so
    /// the retry window, not the timeout, is what bounds the leak.
    fn hours_to_exhaust(pool: i64, retry_secs: i64) -> i64 {
        pool * retry_secs / 3600
    }

    #[test]
    fn the_stall_backoff_cannot_exhaust_the_blocking_pool() {
        const POOL: i64 = 512; // tokio's default max_blocking_threads
        const FAST_TICK_SECS: i64 = 2;
        // Retrying on every fast tick would burn the whole pool inside an
        // hour — which is exactly why a backoff window exists at all.
        assert!(hours_to_exhaust(POOL, FAST_TICK_SECS) < 1);
        // With the real window, a permanently wedged path costs a thread a
        // day rather than the daemon.
        let hours = hours_to_exhaust(POOL, INVENTORY_RETRY_AFTER_STALL_SECS);
        assert!(hours >= 24, "pool would be exhausted in {hours}h");
    }

    /// The budget is about STALENESS, not patience: a real scan is
    /// milliseconds, so the app list may lag a few seconds, never minutes.
    #[test]
    fn the_budget_is_seconds_not_minutes() {
        let secs = INVENTORY_BUDGET.as_secs();
        assert!((1..=10).contains(&secs), "budget was {secs}s");
    }

    // ---- D3: unknown inventory ≠ empty inventory ---------------------------

    /// A timed-out seed leaves the inventory UNKNOWN. (Mutating
    /// `inventory_answered` to a bare `true` — which is exactly what an
    /// inlined `.is_some()` invites a refactor to do — must fail here.)
    #[test]
    fn a_timed_out_seed_leaves_the_inventory_unknown() {
        let answered: Option<(u64, Vec<charter_proto::status::AppRef>)> = Some((0, Vec::new()));
        let timed_out: Option<(u64, Vec<charter_proto::status::AppRef>)> = None;
        assert!(
            inventory_answered(&answered),
            "a scan that answered is known"
        );
        assert!(
            !inventory_answered(&timed_out),
            "a scan that never answered must NOT count as known"
        );
        // …and an EMPTY answer is still an answer: a device with no launchers
        // is a real, knowable state, not the hung-FUSE one.
        assert!(inventory_answered(&answered));
    }

    /// **The hole, reproduced end to end.** A ward wedges the seed scan (hung
    /// FUSE at `~/.local/share/applications`, no privilege), reboots, then
    /// `flatpak install --user`s a GOVERNED learning app id. If the unknown
    /// inventory were passed to `classify` as an empty set, §2.4's gate would
    /// read "nothing is user-installed" and hand over free learning time —
    /// permanently, since every retry times out and backs off 300 s.
    ///
    /// This is the composition the mutation survived before: it asserts the
    /// VERDICT, not the flag, so flipping `inventory_answered` to `true` (or
    /// `user_installed_for_classify` to always-`Some`) fails right here.
    #[test]
    fn an_unknown_inventory_never_hands_a_user_flatpak_free_time() {
        let learning = [charter_proto::LearningApp {
            id: "gcompris".into(),
            label: "GCompris".into(),
            kind: charter_proto::LearningAppKind::Native,
            domains: vec![],
            url: None,
            exec: Some("org.kde.gcompris".into()),
            trusted: false,
            free: None,
        }];
        // The impostor: a genuine, root-owned bwrap running the governed app
        // id — indistinguishable from a system flatpak by the process alone.
        let impostor = crate::focus::FocusedProcess {
            cmdline: vec!["/usr/bin/bwrap".into(), "org.kde.gcompris".into()],
            exe: Some("/usr/bin/bwrap".into()),
            exe_uid: Some(0),
            cgroup: None,
            pid: 0,
        };

        // Seed timed out ⇒ unknown ⇒ the arm fails CLOSED.
        let unknown = user_installed_for_classify(inventory_answered(&None::<()>), &[]);
        assert_eq!(unknown, None, "an unanswered scan must not become a set");
        assert_eq!(
            crate::focus::classify(&impostor, &learning, unknown.as_ref()),
            charter_schedule::Bucket::Screen,
            "an unknown inventory must never grant the §2.4 free-time exemption"
        );

        // Contrast: a scan that ANSWERED, finding nothing user-installed, is
        // a real state and does grant it (this is the pre-existing rule, and
        // proves the test above is not passing for a trivial reason).
        let known_empty = user_installed_for_classify(inventory_answered(&Some(())), &[]);
        assert_eq!(known_empty, Some(std::collections::BTreeSet::new()));
        assert_eq!(
            crate::focus::classify(&impostor, &learning, known_empty.as_ref()),
            charter_schedule::Bucket::Learning
        );
    }

    // ---- 03-G5: a recovery pause expires ----

    /// Back-date a file's mtime by `secs`, so the pause can be aged without
    /// waiting four hours for it.
    fn age_file(path: &str, secs: i64) {
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(secs as u64);
        let unix = when
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_secs() as libc::time_t;
        let tv = libc::timeval {
            tv_sec: unix,
            tv_usec: 0,
        };
        let times = [tv, tv];
        let c = std::ffi::CString::new(path).expect("path");
        // SAFETY: a NUL-terminated path and a two-element `timeval` array,
        // exactly as `utimes(2)` wants.
        let rc = unsafe { libc::utimes(c.as_ptr(), times.as_ptr()) };
        assert_eq!(rc, 0, "utimes failed on {path}");
    }

    /// A pause had no expiry at all: it self-healed only on reboot, so on a
    /// box nobody reboots, one click left a child with no limits indefinitely
    /// while the guardian's app kept showing the last pre-pause numbers.
    ///
    /// One test, because `CHARTER_PAUSE_FLAG` is process-global.
    #[test]
    fn a_recovery_pause_is_honoured_then_lapses_and_can_be_reissued() {
        let dir = std::env::temp_dir().join(format!("charterd-pause-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        let flag = dir.join("paused").to_string_lossy().into_owned();
        std::env::set_var("CHARTER_PAUSE_FLAG", &flag);

        assert!(!enforcement_paused(), "no flag, no pause");

        std::fs::write(&flag, "").expect("write flag");
        assert!(enforcement_paused(), "a fresh pause is honoured");

        // Just inside the window.
        age_file(&flag, PAUSE_MAX_SECS - 60);
        assert!(enforcement_paused(), "a pause under 4h is still in force");
        assert!(std::path::Path::new(&flag).exists());

        // Past it: the pause lapses AND the flag is cleared, so it cannot
        // quietly re-assert itself on the next tick.
        age_file(&flag, PAUSE_MAX_SECS + 60);
        assert!(!enforcement_paused(), "a pause over 4h has lapsed");
        assert!(
            !std::path::Path::new(&flag).exists(),
            "the lapsed flag is removed, not merely ignored"
        );
        assert!(!enforcement_paused(), "and it stays lapsed");

        // Re-issuing is one click, and it refreshes the whole window.
        std::fs::write(&flag, "").expect("re-issue");
        assert!(enforcement_paused());

        std::env::remove_var("CHARTER_PAUSE_FLAG");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- 03-B6: a poisoned shared mutex must not take the daemon down ----

    /// The two shared mutexes in the loop used to be `.expect()`ed. A panic
    /// anywhere in the tick while `time_left_snapshots` was held poisoned the
    /// lock, after which `TimeLeft()` panicked inside the D-Bus handler too —
    /// so one input-dependent panic became a crash on every later read. This
    /// pins the recovery, which is the whole of the decision.
    #[test]
    fn a_poisoned_lock_is_recovered_rather_than_re_panicked() {
        use std::sync::{Arc, Mutex};
        let m: Arc<Mutex<Vec<u32>>> = Arc::new(Mutex::new(vec![1]));
        let m2 = Arc::clone(&m);
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("a probe blew up while holding the lock");
        })
        .join();
        assert!(m.lock().is_err(), "the lock really is poisoned");
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(*g, vec![1], "and the data behind it is intact");
        g.push(2);
    }
}
