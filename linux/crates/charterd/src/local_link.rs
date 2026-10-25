//! The Linux ward's end of the local link (v1): direct delivery between this
//! warden and the guardian carrier on the same home network.
//!
//! The ward is a CLIENT. It discovers the carrier through avahi-daemon
//! (`_kintrinsic._tcp`, browse only — this machine never advertises and never
//! listens) and connects out with `charter_link::WardLink`. Everything the
//! link carries is the same NIP-59 gift wrap as its relay copy: guardian
//! wraps go through [`Broker::ingest_local`], the same unwrap and handlers as
//! a relay poll, and every machine→guardian wrap the broker or the STATUS
//! feed builds is offered to the link's outbox before relay IO starts. See
//! `docs/local-network-design.md` and `spec/contract.md`, "Local link (v1)".
//!
//! What lives here, and why it is not in `runtime.rs`:
//!
//! - [`LateSink`]: the link needs the broker (to ingest) and the broker's
//!   transport needs the link (to offer), so one side is bound late.
//! - [`LinkControl`] + [`BrokerIngest`]: the link's ingest over the real
//!   broker, and the release signal both ingest routes share.
//! - [`BrokerPoller`]: the relay poll, off the enforcement loop.
//! - [`DiscoverySet`] and the avahi parsing helpers (pure, unit-tested), and
//!   the avahi D-Bus browser itself (`avahi`, `real` only).
//!
//! All of it except the D-Bus browser is generic over the broker's ports, so
//! the mock integration tests compose exactly what production runs.

use std::collections::BTreeMap;
use std::future::Future;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use charter_link::{AckResult, LocalOffer, LocalSink, WardIngest};
use charter_primitives::NostrEvent;
use charter_spine::broker::PollCounts;
use charter_spine::{Broker, LocalIngestOptions, TransportFacade};
use charter_sys::SystemLayer;
use charter_transport::Entropy;

/// The Linux warden applies a RELEASE itself, exactly as on the poll path.
pub const INGEST_OPTIONS: LocalIngestOptions = LocalIngestOptions {
    handle_release: true,
};

// ---------------------------------------------------------------------------
// Late-bound sink
// ---------------------------------------------------------------------------

/// A [`LocalSink`] bound after construction. Attached to every transport that
/// publishes machine→guardian wraps (the broker's facade and the STATUS
/// facade) before the broker exists; bound to the link's outbox once the link
/// has been spawned over that broker. Offers before binding (none in
/// practice: binding follows construction immediately) are dropped, which is
/// exactly "no local link": the relay copy is unaffected.
#[derive(Clone, Default)]
pub struct LateSink(Arc<OnceLock<Arc<dyn LocalSink>>>);

impl LateSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind the real sink. `false` if one was already bound (the first wins).
    pub fn bind(&self, sink: Arc<dyn LocalSink>) -> bool {
        self.0.set(sink).is_ok()
    }

    pub fn is_bound(&self) -> bool {
        self.0.get().is_some()
    }

    /// This late sink as the trait object a transport takes.
    pub fn as_sink(&self) -> Arc<dyn LocalSink> {
        Arc::new(self.clone())
    }
}

impl LocalSink for LateSink {
    fn offer(&self, offer: LocalOffer) {
        if let Some(sink) = self.0.get() {
            sink.offer(offer);
        }
    }
}

// ---------------------------------------------------------------------------
// Release signal
// ---------------------------------------------------------------------------

/// What the two ingest routes (the relay poll task and the local link) share
/// with the enforcement loop: whether a guardian RELEASE has been applied on
/// either of them (S7).
///
/// The broker itself latches a release inside the handler turn, so once one
/// applies, no later handler on either route stores anything (see
/// `Broker::on_release`). This flag only carries the news to the loop, which
/// restarts the daemon unpaired, and stops the poll task from starting
/// another round in the meantime.
#[derive(Debug, Default)]
pub struct LinkControl {
    released: AtomicBool,
}

impl LinkControl {
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// A guardian RELEASE authenticated and was applied, on either route.
    pub fn released(&self) -> bool {
        self.released.load(Ordering::SeqCst)
    }

    pub fn mark_released(&self) {
        self.released.store(true, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// Ingest
// ---------------------------------------------------------------------------

/// The link's [`WardIngest`] over the real broker: one wrap at a time through
/// [`Broker::ingest_local`], whose result is what the signed ACK reports.
pub struct BrokerIngest<S: SystemLayer, T: TransportFacade, E: Entropy> {
    broker: Arc<Broker<S, T, E>>,
    control: Arc<LinkControl>,
}

impl<S: SystemLayer, T: TransportFacade, E: Entropy> BrokerIngest<S, T, E> {
    pub fn new(broker: Arc<Broker<S, T, E>>, control: Arc<LinkControl>) -> Arc<Self> {
        Arc::new(BrokerIngest { broker, control })
    }
}

impl<S, T, E> WardIngest for BrokerIngest<S, T, E>
where
    S: SystemLayer + Send + Sync + 'static,
    T: TransportFacade + 'static,
    E: Entropy + 'static,
{
    fn ingest(&self, wrap: NostrEvent) -> Pin<Box<dyn Future<Output = AckResult> + Send>> {
        let broker = self.broker.clone();
        let control = self.control.clone();
        Box::pin(async move {
            let out = broker.ingest_local(&wrap, INGEST_OPTIONS).await;
            if out.released {
                control.mark_released();
            }
            out.result
        })
    }
}

// ---------------------------------------------------------------------------
// The relay poll, off the enforcement loop
// ---------------------------------------------------------------------------

/// A relay poll that has run this long is reported: it is the `WatchdogSec`
/// window, i.e. how long a poll inline in the loop could take before systemd
/// killed the daemon for it. Relay IO carries its own timeouts, so a poll
/// this old is a wedged connection or a very long enactor.
pub const POLL_OVERDUE: Duration = Duration::from_secs(180);

/// Whether a poll that has been in flight for `age` is overdue.
pub fn poll_overdue(age: Option<Duration>) -> bool {
    age.is_some_and(|a| a >= POLL_OVERDUE)
}

/// Runs `Broker::poll_once` on its own task, one round at a time, every
/// `every` after the previous round finished.
///
/// Before the local link, the poll ran inline in the enforcement loop: every
/// relay query (up to ~16 s per relay) stalled the 2 s fast tick, and a
/// change that arrived over the link could wait out a whole poll round before
/// the loop re-read the store. Here the loop only drains finished rounds.
///
/// Watchdog: relay operations ping systemd only from the enforcement task
/// (`watchdog::ping_from_enforcement_task`); this task is outside it, so a
/// poll never vouches for the loop. The loop pings after each enforcement
/// pass, as it always has. A poll that wedges therefore leaves the daemon
/// enforcing cached clauses (it is reported via [`Self::in_flight_for`]); it
/// can no longer get the daemon killed and `--thaw-all`ed.
pub struct BrokerPoller {
    results: tokio::sync::mpsc::UnboundedReceiver<PollCounts>,
    in_flight: Arc<Mutex<Option<Instant>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl BrokerPoller {
    pub fn spawn<S, T, E>(
        broker: Arc<Broker<S, T, E>>,
        control: Arc<LinkControl>,
        every: Duration,
    ) -> Self
    where
        S: SystemLayer + Send + Sync + 'static,
        T: TransportFacade + 'static,
        E: Entropy + 'static,
    {
        let (tx, results) = tokio::sync::mpsc::unbounded_channel();
        let in_flight: Arc<Mutex<Option<Instant>>> = Arc::default();
        let flight = in_flight.clone();
        let handle = tokio::spawn(async move {
            while !control.released() {
                set(&flight, Some(Instant::now()));
                let counts = broker.poll_once().await;
                set(&flight, None);
                if counts.released {
                    control.mark_released();
                }
                if tx.send(counts).is_err() || counts.released {
                    return;
                }
                tokio::time::sleep(every).await;
            }
        });
        BrokerPoller {
            results,
            in_flight,
            handle,
        }
    }

    /// Every round finished since the last call, oldest first.
    pub fn drain(&mut self) -> Vec<PollCounts> {
        let mut out = Vec::new();
        while let Ok(c) = self.results.try_recv() {
            out.push(c);
        }
        out
    }

    /// How long the current round has been running, if one is.
    pub fn in_flight_for(&self) -> Option<Duration> {
        self.in_flight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .map(|t| t.elapsed())
    }

    /// Stop polling (release / shutdown). A round in flight is cancelled at
    /// its next await point; the daemon restarts right after.
    pub fn abort(&self) {
        self.handle.abort();
    }
}

fn set(slot: &Mutex<Option<Instant>>, v: Option<Instant>) {
    *slot.lock().unwrap_or_else(|e| e.into_inner()) = v;
}

/// Whether the cumulative suspended-time counter moved enough between two
/// loop iterations to mean the machine was asleep (and the network may have
/// changed under it): the link resets its back-off and retries at once.
pub fn resumed_from_suspend(prev_suspended_secs: u64, now_suspended_secs: u64) -> bool {
    now_suspended_secs >= prev_suspended_secs.saturating_add(2)
}

// ---------------------------------------------------------------------------
// Discovery (avahi) — pure parts
// ---------------------------------------------------------------------------

/// The DNS-SD domain the carrier registers in.
pub const SERVICE_DOMAIN: &str = "local";

/// The current discovery results, keyed by avahi's `(interface, name)`: the
/// same instance seen on two interfaces is two entries, and an `ItemRemove`
/// names exactly the entry it removes.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DiscoverySet {
    found: BTreeMap<(i32, String), SocketAddrV4>,
}

impl DiscoverySet {
    /// Record a resolved instance; `true` if the address set changed.
    pub fn insert(&mut self, interface: i32, name: &str, addr: SocketAddrV4) -> bool {
        let before = self.addrs();
        self.found.insert((interface, name.to_string()), addr);
        self.addrs() != before
    }

    /// Forget an instance; `true` if the address set changed.
    pub fn remove(&mut self, interface: i32, name: &str) -> bool {
        let before = self.addrs();
        self.found.remove(&(interface, name.to_string()));
        self.addrs() != before
    }

    /// Forget everything; `true` if anything was known.
    pub fn clear(&mut self) -> bool {
        let had = !self.found.is_empty();
        self.found.clear();
        had
    }

    /// The distinct addresses, in a stable order, for `WardLink::set_discovered`.
    pub fn addrs(&self) -> Vec<SocketAddrV4> {
        let mut v: Vec<SocketAddrV4> = self.found.values().copied().collect();
        v.sort();
        v.dedup();
        v
    }
}

/// Whether an avahi browse item is the carrier's service (avahi already
/// filters by type and domain; this guards against a daemon that does not).
pub fn is_carrier_service(service_type: &str, domain: &str) -> bool {
    service_type.eq_ignore_ascii_case(charter_link::SERVICE_TYPE)
        && (domain.is_empty()
            || domain
                .trim_end_matches('.')
                .eq_ignore_ascii_case(SERVICE_DOMAIN))
}

/// A resolved `(address, port)` from `ResolveService` as an IPv4 socket
/// address. IPv6 (including a scoped link-local `fe80::…%eth0`), an empty or
/// garbage address and port 0 are all `None`: the link is IPv4 only.
pub fn parse_resolved(address: &str, port: u16) -> Option<SocketAddrV4> {
    if port == 0 {
        return None;
    }
    let ip: Ipv4Addr = address.trim().parse().ok()?;
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() {
        return None;
    }
    Some(SocketAddrV4::new(ip, port))
}

/// D-Bus error names that mean "this avahi has no such method/interface"
/// (avahi < 0.8 has no `Server2`), so the browser falls back to
/// `Server.ServiceBrowserNew`.
pub fn is_unknown_member(error_name: &str) -> bool {
    matches!(
        error_name,
        "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownObject"
    )
}

/// Logs a recurring failure once, not every retry: a laptop with no
/// avahi-daemon would otherwise write the same line every minute forever.
#[derive(Debug, Default)]
pub struct OnceLog {
    last: Option<String>,
}

impl OnceLog {
    /// Whether `msg` should be logged (it differs from the last one logged).
    pub fn should_log(&mut self, msg: &str) -> bool {
        if self.last.as_deref() == Some(msg) {
            return false;
        }
        self.last = Some(msg.to_string());
        true
    }

    /// The condition cleared: the next failure is news again.
    pub fn reset(&mut self) {
        self.last = None;
    }
}

// ---------------------------------------------------------------------------
// Discovery (avahi) — the D-Bus browser
// ---------------------------------------------------------------------------

/// Browses `_kintrinsic._tcp` through avahi-daemon on the system bus and keeps
/// the link's discovered set current. Detached; never blocks or crashes the
/// daemon. Without avahi (minimal installs) only the link's remembered
/// address is tried, and that is logged once.
#[cfg(feature = "real")]
pub mod avahi {
    use std::sync::Arc;
    use std::time::Duration;

    use charter_link::WardLink;
    use zbus::export::futures_util::StreamExt;
    use zbus::zvariant::OwnedObjectPath;

    use super::{is_carrier_service, is_unknown_member, parse_resolved, DiscoverySet, OnceLog};

    const AVAHI: &str = "org.freedesktop.Avahi";
    const SERVER: &str = "org.freedesktop.Avahi.Server";
    const SERVER2: &str = "org.freedesktop.Avahi.Server2";
    const BROWSER: &str = "org.freedesktop.Avahi.ServiceBrowser";
    /// `AVAHI_IF_UNSPEC`, `AVAHI_PROTO_INET`.
    const IF_UNSPEC: i32 = -1;
    const PROTO_INET: i32 = 0;
    /// After a failure (no avahi, no bus, a browser `Failure`).
    const RETRY: Duration = Duration::from_secs(60);
    /// After avahi-daemon itself restarted: it is back, browse again soon.
    const RETRY_AFTER_RESTART: Duration = Duration::from_secs(5);
    /// avahi resolves with its own timeout; this only bounds a stuck reply.
    const RESOLVE_BUDGET: Duration = Duration::from_secs(10);

    /// Why one browse session ended.
    enum Ended {
        Failed(String),
        AvahiRestarted,
    }

    /// Start the browser for `link` on the current runtime.
    pub fn spawn(link: Arc<WardLink>) {
        tokio::spawn(run(link));
    }

    async fn run(link: Arc<WardLink>) {
        let mut log = OnceLog::default();
        let mut set = DiscoverySet::default();
        loop {
            let wait = match browse(&link, &mut set, &mut log).await {
                Ended::Failed(e) => {
                    if log.should_log(&e) {
                        eprintln!(
                            "charterd: local link discovery unavailable ({e}) — only the \
                             last guardian address that worked will be tried; retrying \
                             every {}s",
                            RETRY.as_secs()
                        );
                    }
                    RETRY
                }
                Ended::AvahiRestarted => RETRY_AFTER_RESTART,
            };
            tokio::time::sleep(wait).await;
        }
    }

    fn describe(e: &zbus::Error) -> String {
        match e {
            zbus::Error::MethodError(name, detail, _) => match detail {
                Some(d) => format!("{}: {d}", name.as_str()),
                None => name.as_str().to_string(),
            },
            other => other.to_string(),
        }
    }

    fn unknown_member(e: &zbus::Error) -> bool {
        match e {
            zbus::Error::MethodError(name, _, _) => is_unknown_member(name.as_str()),
            zbus::Error::FDO(f) => matches!(
                **f,
                zbus::fdo::Error::UnknownMethod(_)
                    | zbus::fdo::Error::UnknownInterface(_)
                    | zbus::fdo::Error::UnknownObject(_)
            ),
            _ => false,
        }
    }

    async fn browse(link: &WardLink, set: &mut DiscoverySet, log: &mut OnceLog) -> Ended {
        match browse_inner(link, set, log).await {
            Ok(ended) => ended,
            Err(e) => Ended::Failed(describe(&e)),
        }
    }

    async fn browse_inner(
        link: &WardLink,
        set: &mut DiscoverySet,
        log: &mut OnceLog,
    ) -> zbus::Result<Ended> {
        let conn = zbus::Connection::system().await?;
        // avahi-daemon going away or restarting: its browser objects die with
        // it and their signals simply stop, so watch the name itself.
        let dbus = zbus::fdo::DBusProxy::new(&conn).await?;
        let mut owner = dbus
            .receive_name_owner_changed_with_args(&[(0, AVAHI)])
            .await?;

        let args = (
            IF_UNSPEC,
            PROTO_INET,
            charter_link::SERVICE_TYPE,
            super::SERVICE_DOMAIN,
            0u32,
        );
        let server2 = zbus::Proxy::new(&conn, AVAHI, "/", SERVER2).await?;
        // Prefer Prepare + Start (avahi >= 0.8): subscribe first, then start,
        // so no ItemNew can fire before we listen.
        let (path, prepared): (OwnedObjectPath, bool) =
            match server2.call("ServiceBrowserPrepare", &args).await {
                Ok(p) => (p, true),
                Err(e) if unknown_member(&e) => {
                    let server = zbus::Proxy::new(&conn, AVAHI, "/", SERVER).await?;
                    (server.call("ServiceBrowserNew", &args).await?, false)
                }
                Err(e) => return Err(e),
            };
        let browser = zbus::Proxy::new(&conn, AVAHI, path.clone(), BROWSER).await?;
        let mut new = browser.receive_signal("ItemNew").await?;
        let mut gone = browser.receive_signal("ItemRemove").await?;
        let mut failure = browser.receive_signal("Failure").await?;
        if prepared {
            browser.call_method("Start", &()).await?;
        }
        // A fresh browse re-announces everything still present.
        if set.clear() {
            link.set_discovered(Vec::new());
        }
        log.reset();
        let server = zbus::Proxy::new(&conn, AVAHI, "/", SERVER).await?;

        let ended = loop {
            tokio::select! {
                m = new.next() => {
                    let Some(m) = m else { break Ended::Failed("browser signals ended".into()) };
                    let Ok((iface, proto, name, stype, domain, _flags)) =
                        m.body().deserialize::<(i32, i32, String, String, String, u32)>()
                    else {
                        continue;
                    };
                    if !is_carrier_service(&stype, &domain) {
                        continue;
                    }
                    let reply = tokio::time::timeout(
                        RESOLVE_BUDGET,
                        server.call::<_, _, (
                            i32, i32, String, String, String, String, i32, String, u16,
                            Vec<Vec<u8>>, u32,
                        )>(
                            "ResolveService",
                            &(iface, proto, name.as_str(), stype.as_str(), domain.as_str(),
                              PROTO_INET, 0u32),
                        ),
                    )
                    .await;
                    let Ok(Ok(r)) = reply else { continue };
                    if let Some(addr) = parse_resolved(&r.7, r.8) {
                        if set.insert(iface, &name, addr) {
                            link.set_discovered(set.addrs());
                        }
                    }
                }
                m = gone.next() => {
                    let Some(m) = m else { break Ended::Failed("browser signals ended".into()) };
                    if let Ok((iface, _proto, name, _t, _d, _f)) =
                        m.body().deserialize::<(i32, i32, String, String, String, u32)>()
                    {
                        if set.remove(iface, &name) {
                            link.set_discovered(set.addrs());
                        }
                    }
                }
                m = failure.next() => {
                    let why = m
                        .and_then(|m| m.body().deserialize::<String>().ok())
                        .unwrap_or_else(|| "browser failed".into());
                    break Ended::Failed(format!("avahi browser failure: {why}"));
                }
                _ = owner.next() => break Ended::AvahiRestarted,
            }
        };
        // Best-effort: a dead daemon frees its own objects.
        let _ = browser.call_method("Free", &()).await;
        Ok(ended)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a(s: &str) -> SocketAddrV4 {
        s.parse().unwrap()
    }

    #[test]
    fn resolved_ipv4_addresses_parse_and_everything_else_is_refused() {
        assert_eq!(
            parse_resolved("192.168.1.5", 47615),
            Some(a("192.168.1.5:47615"))
        );
        assert_eq!(parse_resolved(" 10.0.0.2 ", 9), Some(a("10.0.0.2:9")));
        assert_eq!(parse_resolved("192.168.1.5", 0), None);
        assert_eq!(parse_resolved("fe80::1%wlp2s0", 47615), None);
        assert_eq!(parse_resolved("2001:db8::1", 47615), None);
        assert_eq!(parse_resolved("", 47615), None);
        assert_eq!(parse_resolved("not-an-ip", 47615), None);
        assert_eq!(parse_resolved("0.0.0.0", 47615), None);
        assert_eq!(parse_resolved("255.255.255.255", 47615), None);
        assert_eq!(parse_resolved("224.0.0.251", 5353), None);
    }

    #[test]
    fn the_set_is_keyed_by_interface_and_name_and_reports_real_changes_only() {
        let mut s = DiscoverySet::default();
        assert!(s.insert(2, "Kintrinsic ab12", a("192.168.1.5:47615")));
        // The same answer again changes nothing.
        assert!(!s.insert(2, "Kintrinsic ab12", a("192.168.1.5:47615")));
        // The same instance on a second interface, same address: no new address.
        assert!(!s.insert(3, "Kintrinsic ab12", a("192.168.1.5:47615")));
        // A moved address replaces the old one.
        assert!(s.insert(2, "Kintrinsic ab12", a("192.168.1.9:47615")));
        assert_eq!(
            s.addrs(),
            vec![a("192.168.1.5:47615"), a("192.168.1.9:47615")]
        );
        // Removing one interface's entry leaves the other.
        assert!(s.remove(2, "Kintrinsic ab12"));
        assert_eq!(s.addrs(), vec![a("192.168.1.5:47615")]);
        assert!(!s.remove(2, "Kintrinsic ab12"), "already gone");
        assert!(!s.remove(3, "Kintrinsic ffff"), "never seen");
        assert!(s.remove(3, "Kintrinsic ab12"));
        assert!(s.addrs().is_empty());
        assert!(!s.clear());
        s.insert(1, "x", a("10.0.0.1:1"));
        assert!(s.clear());
        assert!(s.addrs().is_empty());
    }

    #[test]
    fn duplicate_addresses_from_two_instances_are_offered_once_in_order() {
        let mut s = DiscoverySet::default();
        s.insert(2, "b", a("192.168.1.9:1"));
        s.insert(2, "a", a("192.168.1.5:1"));
        s.insert(3, "c", a("192.168.1.9:1"));
        assert_eq!(s.addrs(), vec![a("192.168.1.5:1"), a("192.168.1.9:1")]);
    }

    #[test]
    fn only_the_carrier_service_in_the_local_domain_is_taken() {
        assert!(is_carrier_service("_kintrinsic._tcp", "local"));
        assert!(is_carrier_service("_kintrinsic._tcp", "local."));
        assert!(is_carrier_service("_KINTRINSIC._tcp", "LOCAL"));
        assert!(is_carrier_service("_kintrinsic._tcp", ""));
        assert!(!is_carrier_service("_http._tcp", "local"));
        assert!(!is_carrier_service("_kintrinsic._udp", "local"));
        assert!(!is_carrier_service("_kintrinsic._tcp", "example.com"));
    }

    #[test]
    fn only_missing_member_errors_trigger_the_fallback_browser() {
        assert!(is_unknown_member(
            "org.freedesktop.DBus.Error.UnknownMethod"
        ));
        assert!(is_unknown_member(
            "org.freedesktop.DBus.Error.UnknownInterface"
        ));
        assert!(is_unknown_member(
            "org.freedesktop.DBus.Error.UnknownObject"
        ));
        assert!(!is_unknown_member(
            "org.freedesktop.DBus.Error.ServiceUnknown"
        ));
        assert!(!is_unknown_member(
            "org.freedesktop.Avahi.NotSupportedError"
        ));
    }

    #[test]
    fn a_recurring_failure_is_logged_once_until_it_clears() {
        let mut l = OnceLog::default();
        assert!(l.should_log("no avahi"));
        assert!(!l.should_log("no avahi"));
        assert!(l.should_log("bus down"));
        assert!(!l.should_log("bus down"));
        l.reset();
        assert!(l.should_log("bus down"));
    }

    #[test]
    fn a_suspend_is_seen_as_the_counter_moving_two_seconds_or_more() {
        assert!(!resumed_from_suspend(100, 100));
        assert!(!resumed_from_suspend(100, 101));
        assert!(resumed_from_suspend(100, 102));
        assert!(resumed_from_suspend(0, 3600));
        assert!(
            !resumed_from_suspend(100, 50),
            "a counter going back is not a resume"
        );
    }

    #[test]
    fn a_poll_is_overdue_only_after_the_watchdog_window() {
        assert!(!poll_overdue(None));
        assert!(!poll_overdue(Some(Duration::from_secs(179))));
        assert!(poll_overdue(Some(POLL_OVERDUE)));
        assert!(poll_overdue(Some(Duration::from_secs(3600))));
    }

    #[test]
    fn a_late_sink_forwards_only_once_bound_and_the_first_binding_wins() {
        struct Rec(Mutex<Vec<u16>>);
        impl LocalSink for Rec {
            fn offer(&self, o: LocalOffer) {
                self.0.lock().unwrap().push(o.inner_kind);
            }
        }
        let ev = NostrEvent {
            id: charter_primitives::EventId::from_bytes([1; 32]),
            pubkey: charter_primitives::PubKey::from_bytes([2; 32]),
            created_at: 1,
            kind: charter_primitives::kinds::GIFT_WRAP,
            tags: vec![],
            content: String::new(),
            sig: charter_primitives::Sig::from_bytes([0; 64]),
        };
        let offer = |k| LocalOffer {
            inner_kind: k,
            key: None,
            wrap: ev.clone(),
        };
        let late = LateSink::new();
        let as_sink = late.as_sink();
        as_sink.offer(offer(1));
        assert!(!late.is_bound());
        let first = Arc::new(Rec(Mutex::new(Vec::new())));
        let second = Arc::new(Rec(Mutex::new(Vec::new())));
        assert!(late.bind(first.clone()));
        assert!(!late.bind(second.clone()));
        as_sink.offer(offer(2));
        assert_eq!(*first.0.lock().unwrap(), vec![2]);
        assert!(second.0.lock().unwrap().is_empty());
    }
}
