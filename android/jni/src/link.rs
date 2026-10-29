//! The ward's local link (v1): a client that connects out to the guardian
//! carrier on the home network (`docs/local-network-design.md`). The ward
//! never listens. Kotlin discovers the carrier with `NsdManager` and hands the
//! addresses in; the socket work runs here, on a runtime of its own.
//!
//! The link only changes how a gift wrap arrives. Every inbound wrap goes
//! through [`charter_spine::Broker::ingest_local`], the same unwrap and
//! verification as its relay copy; a RELEASE comes back to the warden's own
//! [`crate::warden::Warden`] release path (`try_apply_releases`), exactly as a
//! polled one does.
//!
//! Locking: the warden lock is taken twice per inbound wrap, briefly — once to
//! snapshot the broker, once to absorb the outcome — and never across the
//! ingest await or any network IO. Both steps run on the blocking pool, so a
//! warden lock held for a relay round (`submit_request`) does not stall the
//! link's other sessions; that wrap's own ACK does wait for the lock.
//!
//! Routing: Wi-Fi without internet is not Android's default network while
//! mobile data is up, and a socket on default routing would send the private
//! guardian address out over mobile data. Kotlin passes the handle of the
//! Wi-Fi network the guardian was discovered on (`Network.getNetworkHandle`)
//! with the address set, and every outbound socket is bound to it with
//! `android_setsocknetwork` before it connects.

use std::future::Future;
use std::net::SocketAddrV4;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use charter_link::{AckResult, LocalSink, WardIngest, WardLink, WardLinkConfig};
use charter_primitives::NostrEvent;
use charter_spine::LocalIngestOptions;

use crate::warden::Warden;

/// How long a retired link's runtime waits for in-flight ingests to finish
/// before it is dropped (which cancels whatever is still running).
const DRAIN_LIMIT: Duration = Duration::from_secs(30);

/// The warden lock the link reaches back into: `&WARDEN` in production, a
/// leaked test mutex in host tests.
pub type WardenHome = &'static Mutex<Option<Warden>>;

fn guard(home: WardenHome) -> std::sync::MutexGuard<'static, Option<Warden>> {
    home.lock().unwrap_or_else(|p| p.into_inner())
}

/// A running [`WardLink`] and the runtime that drives it. Dropping (or
/// [`LinkHandle::retire`]) stops the link without ever blocking the caller,
/// which usually holds the warden lock.
pub struct LinkHandle {
    link: Arc<WardLink>,
    rt: Option<tokio::runtime::Runtime>,
    inflight: Arc<AtomicUsize>,
    /// Which warden generation this link belongs to: an ingest that finishes
    /// after the warden rebuilt or released its transport is not absorbed
    /// into the new one.
    gen: u64,
    state_dir: PathBuf,
    /// The `Network.getNetworkHandle()` every outbound socket is bound to;
    /// 0 (`NETWORK_UNSPECIFIED`) leaves it on default routing.
    net_handle: Arc<AtomicU64>,
}

/// Bind a socket to an Android network (NDK, libandroid, API 23+).
#[cfg(target_os = "android")]
mod netbind {
    use std::os::fd::RawFd;

    #[link(name = "android")]
    extern "C" {
        /// `int android_setsocknetwork(net_handle_t network, int fd)`, with
        /// `net_handle_t` a `uint64_t` (`<android/multinetwork.h>`).
        fn android_setsocknetwork(network: u64, fd: std::os::raw::c_int) -> std::os::raw::c_int;
    }

    pub fn bind(network: u64, fd: RawFd) -> std::io::Result<()> {
        // SAFETY: a plain syscall wrapper over an fd the caller owns for the
        // duration of the call; no pointers cross.
        if unsafe { android_setsocknetwork(network, fd) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    }
}

/// The socket hook: bind to the current network handle, if one is set.
#[cfg(target_os = "android")]
fn prepare_socket(net: Arc<AtomicU64>) -> Option<charter_link::PrepareSocket> {
    Some(Arc::new(move |fd| match net.load(Ordering::SeqCst) {
        0 => Ok(()),
        h => netbind::bind(h, fd)
            .map_err(|e| std::io::Error::new(e.kind(), format!("bind to network {h}: {e}"))),
    }))
}

/// Host builds route on the default network (there is only one).
#[cfg(not(target_os = "android"))]
fn prepare_socket(_net: Arc<AtomicU64>) -> Option<charter_link::PrepareSocket> {
    None
}

impl LinkHandle {
    /// Start a link to the keys in `signers` with its state (outbox, remembered
    /// address) under `state_dir`. `allow_loopback` is for host tests (a
    /// carrier on 127.0.0.1); production passes `false`.
    pub fn spawn(
        home: WardenHome,
        gen: u64,
        machine_sk: [u8; 32],
        signers: Arc<dyn charter_primitives::SignerSet>,
        state_dir: PathBuf,
        allow_loopback: bool,
    ) -> Result<LinkHandle, String> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(4)
            .thread_name("charter-link")
            .enable_all()
            .build()
            .map_err(|e| format!("link runtime: {e}"))?;
        let inflight = Arc::new(AtomicUsize::new(0));
        let net_handle = Arc::new(AtomicU64::new(0));
        let ingest = Arc::new(WardenIngest {
            home,
            gen,
            inflight: inflight.clone(),
        });
        let link = WardLink::spawn(
            WardLinkConfig {
                machine_sk: zeroize::Zeroizing::new(machine_sk),
                // The live signer set: any key in it may complete the
                // handshake (contract, *Other places that name the guardian*),
                // and an approval or a departure is followed as it lands.
                signers,
                state_dir: state_dir.clone(),
                allow_loopback,
                prepare_socket: prepare_socket(net_handle.clone()),
            },
            ingest,
            rt.handle(),
        )
        .map_err(|e| format!("link: {e}"))?;
        Ok(LinkHandle {
            link,
            rt: Some(rt),
            inflight,
            gen,
            state_dir,
            net_handle,
        })
    }

    pub fn gen(&self) -> u64 {
        self.gen
    }

    /// The ward outbox, for the transports to offer machine→guardian wraps.
    pub fn sink(&self) -> Arc<dyn LocalSink> {
        self.link.sink()
    }

    /// The full discovered set, and the network to bind to when Kotlin
    /// named one (`None` keeps the current handle).
    pub fn set_discovered(&self, found: Discovered) {
        if let Some(h) = found.net_handle {
            self.net_handle.store(h, Ordering::SeqCst);
        }
        self.link.set_discovered(found.addrs);
    }

    /// The network handle outbound sockets are bound to (0: none).
    #[cfg(test)]
    pub fn net_handle(&self) -> u64 {
        self.net_handle.load(Ordering::SeqCst)
    }

    pub fn kick(&self) {
        self.link.kick();
    }

    pub fn snapshot(&self) -> charter_link::WardLinkSnapshot {
        self.link.snapshot()
    }

    /// Stop the link. With `grace` zero the sessions close now; otherwise they
    /// stay up that long first (so the ACK for a release that just unpaired
    /// the device still reaches the guardian). Never blocks the caller: the
    /// runtime is wound down on a thread of its own once in-flight ingests
    /// have finished (bounded by [`DRAIN_LIMIT`]).
    ///
    /// `forget` (an unpair) also deletes the link's outbox and remembered
    /// address once it has stopped, so nothing addressed to the old guardian
    /// is offered to the next one. A re-pair inside `grace` would lose its
    /// first moments of link state; relays still carry everything.
    pub fn retire(mut self, grace: Duration, forget: bool) {
        self.retire_inner(grace, forget);
    }

    fn retire_inner(&mut self, grace: Duration, forget: bool) {
        let Some(rt) = self.rt.take() else { return };
        let link = self.link.clone();
        let wipe = forget.then(|| self.state_dir.clone());
        let forget_files = move |dir: &Option<PathBuf>| {
            if let Some(dir) = dir {
                // charter-link's own list of what it keeps (outbox and the
                // remembered guardian address).
                for f in WardLink::state_files(dir) {
                    let _ = std::fs::remove_file(f);
                }
            }
        };
        if grace.is_zero() {
            link.shutdown();
        }
        let inflight = self.inflight.clone();
        // The runtime must never be dropped here: the caller may be on the
        // link's own blocking pool (a release absorbed over the link), where
        // dropping it would panic or wait on itself.
        let slot = Arc::new(Mutex::new(Some(rt)));
        let thread_slot = slot.clone();
        let spawned = std::thread::Builder::new()
            .name("charter-link-retire".into())
            .spawn(move || {
                if !grace.is_zero() {
                    std::thread::sleep(grace);
                    link.shutdown();
                }
                // Closed: the link persists nothing more.
                forget_files(&wipe);
                let deadline = Instant::now() + DRAIN_LIMIT;
                while inflight.load(Ordering::SeqCst) > 0 && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
                drop(link);
                let rt = thread_slot.lock().unwrap_or_else(|p| p.into_inner()).take();
                if let Some(rt) = rt {
                    rt.shutdown_timeout(Duration::from_secs(1));
                }
            });
        if let Err(e) = spawned {
            // No thread to wait on: stop now, without blocking.
            eprintln!("charter-link: retire thread: {e}");
            self.link.shutdown();
            if forget {
                forget_files(&Some(self.state_dir.clone()));
            }
            let rt = slot.lock().unwrap_or_else(|p| p.into_inner()).take();
            if let Some(rt) = rt {
                rt.shutdown_background();
            }
        }
    }
}

impl Drop for LinkHandle {
    fn drop(&mut self) {
        self.retire_inner(Duration::ZERO, false);
    }
}

/// Decrements the in-flight count however the ingest task ends.
struct Inflight(Arc<AtomicUsize>);

impl Drop for Inflight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// [`WardIngest`] over the warden's broker.
struct WardenIngest {
    home: WardenHome,
    gen: u64,
    inflight: Arc<AtomicUsize>,
}

impl WardIngest for WardenIngest {
    fn ingest(&self, wrap: NostrEvent) -> Pin<Box<dyn Future<Output = AckResult> + Send>> {
        let (home, gen) = (self.home, self.gen);
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let token = Inflight(self.inflight.clone());
        // A task of its own, so the session closing mid-ingest (shutdown, a
        // dropped connection) cannot cancel a broker handler half way.
        let inflight = self.inflight.clone();
        let task = tokio::spawn(async move {
            let _token = token;
            ingest_one(home, gen, wrap, inflight).await
        });
        Box::pin(async move { task.await.unwrap_or(AckResult::Failed) })
    }
}

async fn ingest_one(
    home: WardenHome,
    gen: u64,
    wrap: NostrEvent,
    inflight: Arc<AtomicUsize>,
) -> AckResult {
    // Locked, briefly: which broker serves this link generation.
    let broker =
        tokio::task::spawn_blocking(move || guard(home).as_ref().and_then(|w| w.link_broker(gen)))
            .await
            .ok()
            .flatten();
    let Some(broker) = broker else {
        // Unpaired or rebuilt since this session came up: retrying may help.
        return AckResult::Failed;
    };
    // Unlocked: the broker's own verification, under its own handler turn.
    let (out, audits) = broker
        .ingest_local_deferred(
            &wrap,
            LocalIngestOptions {
                handle_release: false,
            },
        )
        .await;
    // The audits are relay IO (seconds with the relays down, which is when
    // the link matters): they go out on a task of their own, so the ACK does
    // not wait for them. Counted in flight, so a retiring link lets them
    // finish (up to `DRAIN_LIMIT`).
    if audits.is_empty() {
        drop(broker);
    } else {
        inflight.fetch_add(1, Ordering::SeqCst);
        let token = Inflight(inflight);
        tokio::spawn(async move {
            let _token = token;
            broker.emit_local_audits(audits).await;
        });
    }
    // Locked, briefly: the release path, the clause mirror, the applied count.
    tokio::task::spawn_blocking(move || match guard(home).as_mut() {
        Some(w) => w.absorb_local(gen, out, now_unix()),
        None => AckResult::Failed,
    })
    .await
    .unwrap_or(AckResult::Failed)
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// What Kotlin's discovery reported (`charterLinkDiscovered`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Discovered {
    pub addrs: Vec<SocketAddrV4>,
    /// `Network.getNetworkHandle()` of the Wi-Fi/Ethernet network the
    /// addresses were found on (0: default routing); `None` when the body
    /// carried none (an older caller), which keeps the current handle.
    pub net_handle: Option<u64>,
}

/// `{"addrs":["192.168.1.5:47615", …],"netHandle":<long>}` → the IPv4 socket
/// addresses in it and the network to bind to. Anything in `addrs` that is
/// not an `ip:port` IPv4 string is skipped (the link drops public and port-0
/// addresses itself); `netHandle` is optional, and one that is not a
/// non-negative integer is ignored; a body that is not that shape at all is
/// an error.
pub fn parse_discovered(json: &str) -> Result<Discovered, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("bad json: {e}"))?;
    let arr = v
        .get("addrs")
        .and_then(|a| a.as_array())
        .ok_or("expected {\"addrs\":[…]}")?;
    Ok(Discovered {
        addrs: arr
            .iter()
            .filter_map(|a| a.as_str())
            .filter_map(|s| s.trim().parse::<SocketAddrV4>().ok())
            .collect(),
        net_handle: v.get("netHandle").and_then(|h| h.as_u64()),
    })
}

/// The `charterLinkState` body: the link snapshot plus the warden's count of
/// locally ingested wraps that were `applied` and the link's generation
/// (`gen`, process-unique: it moves whenever a new link starts, which has
/// been told no discovered address yet), or `{"running":false}`.
pub fn state_json(
    snapshot: Option<(charter_link::WardLinkSnapshot, u64)>,
    applied_seq: u64,
) -> String {
    let gen = snapshot.as_ref().map(|(_, g)| *g);
    let mut v = match snapshot.and_then(|(s, _)| serde_json::to_value(s).ok()) {
        Some(serde_json::Value::Object(m)) => m,
        _ => {
            let mut m = serde_json::Map::new();
            m.insert("running".into(), false.into());
            m.insert("connected".into(), false.into());
            m.insert("appliedSeq".into(), applied_seq.into());
            return serde_json::Value::Object(m).to_string();
        }
    };
    v.insert("running".into(), true.into());
    v.insert("appliedSeq".into(), applied_seq.into());
    if let Some(g) = gen {
        v.insert("gen".into(), g.into());
    }
    serde_json::Value::Object(v).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovered_addresses_parse_and_skip_junk() {
        let a = parse_discovered(
            r#"{"addrs":["192.168.1.5:47615"," 10.0.0.2:1 ","fe80::1","nope",7,"1.2.3.4"]}"#,
        )
        .unwrap();
        assert_eq!(a.net_handle, None, "an older body names no network");
        assert_eq!(
            a.addrs,
            vec![
                "192.168.1.5:47615".parse().unwrap(),
                "10.0.0.2:1".parse().unwrap()
            ]
        );
        assert!(parse_discovered(r#"{"addrs":[]}"#)
            .unwrap()
            .addrs
            .is_empty());
        assert!(parse_discovered("[]").is_err());
        assert!(parse_discovered("not json").is_err());
    }

    #[test]
    fn the_discovered_network_handle_parses() {
        let d = parse_discovered(r#"{"addrs":["192.168.1.5:47615"],"netHandle":432902426637}"#)
            .unwrap();
        assert_eq!(d.net_handle, Some(432_902_426_637));
        assert_eq!(d.addrs, vec!["192.168.1.5:47615".parse().unwrap()]);
        // No network: default routing.
        let d = parse_discovered(r#"{"addrs":[],"netHandle":0}"#).unwrap();
        assert_eq!(d.net_handle, Some(0));
        // Junk is ignored, not an error.
        for junk in [r#""12""#, "-5", "1.5", "null"] {
            let d = parse_discovered(&format!(r#"{{"addrs":[],"netHandle":{junk}}}"#)).unwrap();
            assert_eq!(d.net_handle, None, "{junk}");
        }
    }

    #[test]
    fn state_with_a_link_carries_its_generation() {
        let snap = charter_link::WardLinkSnapshot {
            connected: false,
            sessions: vec![],
            candidates: 1,
            remembered: None,
            outbox: 0,
            last_error: None,
        };
        let v: serde_json::Value = serde_json::from_str(&state_json(Some((snap, 9)), 2)).unwrap();
        assert_eq!(v["running"], true);
        assert_eq!(v["gen"], 9);
        assert_eq!(v["appliedSeq"], 2);
        assert_eq!(v["candidates"], 1);
    }

    #[test]
    fn state_without_a_link_says_not_running() {
        let v: serde_json::Value = serde_json::from_str(&state_json(None, 3)).unwrap();
        assert_eq!(v["running"], false);
        assert_eq!(v["connected"], false);
        assert_eq!(v["appliedSeq"], 3);
    }
}
