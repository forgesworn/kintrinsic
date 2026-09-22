//! The relay transport port — finalized by Phase 2. Multi-relay publish +
//! EOSE-bounded query over websockets in the real impl; an in-memory store in
//! the mock. Delivery-only: the relay never verifies (a hostile relay can
//! delay/drop but never forge — forgery is caught by `charter-verify`).

use async_trait::async_trait;

use charter_primitives::{NostrEvent, PubKey};

use crate::error::SysResult;

/// A relay websocket URL (e.g. `wss://relay.example`).
pub type RelayUrl = String;

/// Per-relay publish outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishOutcome {
    Ok,
    Failed(String),
}

/// A subscription filter (the subset Charter uses).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    pub kinds: Vec<u16>,
    pub authors: Vec<PubKey>,
    /// `#p` tag values (gift-wrap recipient).
    pub p_tags: Vec<PubKey>,
    pub since: Option<u64>,
    pub limit: Option<usize>,
}

impl Filter {
    /// Does `ev` match this filter?
    pub fn matches(&self, ev: &NostrEvent) -> bool {
        if !self.kinds.is_empty() && !self.kinds.contains(&ev.kind) {
            return false;
        }
        if !self.authors.is_empty() && !self.authors.contains(&ev.pubkey) {
            return false;
        }
        if !self.p_tags.is_empty() {
            let has = ev.tags.iter().any(|t| {
                t.len() >= 2 && t[0] == "p" && self.p_tags.iter().any(|p| p.to_hex() == t[1])
            });
            if !has {
                return false;
            }
        }
        if let Some(since) = self.since {
            if ev.created_at < since {
                return false;
            }
        }
        true
    }
}

/// Relay IO failure (the whole query failed, distinct from a per-relay publish
/// failure).
///
/// [`RelayIoError::Unreachable`] is returned when **every** relay in the set
/// failed before it could serve anything — a connect timeout, a DNS/TLS
/// failure, a refused (non-`wss://`) URL, or a `REQ` that could not be sent.
/// It is deliberately distinct from `Ok(vec![])`, which means the relays were
/// reached and had nothing matching: "the network is down" and "the guardian
/// has published nothing" must never look the same to the caller, or a device
/// whose relay set has gone bad reports itself perfectly healthy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayIoError {
    Unreachable(String),
}

/// The relay transport port. Multi-relay: publish fans out (per-relay outcome),
/// query dedups by event id across relays.
#[async_trait]
pub trait RelayTransport: Send + Sync {
    async fn publish(&self, relays: &[RelayUrl], ev: NostrEvent)
        -> Vec<(RelayUrl, PublishOutcome)>;
    async fn query(
        &self,
        relays: &[RelayUrl],
        filter: Filter,
    ) -> Result<Vec<NostrEvent>, RelayIoError>;
    /// Optional: persist nothing — relays are stateless to us.
    fn noop(&self) -> SysResult<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Mock: a functional in-memory relay. Cloning shares the store so a published
// event is queryable (store-and-forward).
// ---------------------------------------------------------------------------

#[cfg(feature = "mock")]
mod mock {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// In-memory relay. `fail_relays` publish-fail; otherwise events land in a
    /// shared store and are returned (deduped by id) by `query`.
    #[derive(Clone, Default)]
    pub struct MockRelayTransport {
        events: Arc<Mutex<Vec<NostrEvent>>>,
        fail_relays: Arc<Mutex<Vec<RelayUrl>>>,
    }

    impl MockRelayTransport {
        pub fn new() -> Self {
            Self::default()
        }

        /// Mark a relay url as always failing to publish (partial-relay tests).
        pub fn with_failing_relay(self, url: &str) -> Self {
            self.fail_relays.lock().expect("lock").push(url.to_string());
            self
        }

        /// Directly inject an event (simulates an event a hostile relay serves).
        pub fn inject(&self, ev: NostrEvent) {
            self.events.lock().expect("lock").push(ev);
        }

        /// Count of stored events.
        pub fn len(&self) -> usize {
            self.events.lock().expect("lock").len()
        }

        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    #[async_trait]
    impl RelayTransport for MockRelayTransport {
        async fn publish(
            &self,
            relays: &[RelayUrl],
            ev: NostrEvent,
        ) -> Vec<(RelayUrl, PublishOutcome)> {
            let failing = self.fail_relays.lock().expect("lock").clone();
            let mut accepted = false;
            let mut out = Vec::with_capacity(relays.len());
            for r in relays {
                if failing.contains(r) {
                    out.push((r.clone(), PublishOutcome::Failed("relay down".into())));
                } else {
                    accepted = true;
                    out.push((r.clone(), PublishOutcome::Ok));
                }
            }
            if accepted {
                self.events.lock().expect("lock").push(ev);
            }
            out
        }

        async fn query(
            &self,
            _relays: &[RelayUrl],
            filter: Filter,
        ) -> Result<Vec<NostrEvent>, RelayIoError> {
            let events = self.events.lock().expect("lock");
            let mut seen = Vec::new();
            let mut out = Vec::new();
            for ev in events.iter() {
                if filter.matches(ev) && !seen.contains(&ev.id) {
                    seen.push(ev.id);
                    out.push(ev.clone());
                }
            }
            if let Some(limit) = filter.limit {
                out.truncate(limit);
            }
            Ok(out)
        }
    }
}

#[cfg(feature = "mock")]
pub use mock::MockRelayTransport;

// ---------------------------------------------------------------------------
// Real: a multi-relay nostr websocket client (rustls-backed wss). The wire
// framing + parsing is a pure core, verified headlessly; the live socket IO is
// network/VM-verified. Delivery-only — the relay is never trusted.
// ---------------------------------------------------------------------------

#[cfg(feature = "real-relay")]
mod real {
    use std::time::Duration;

    use futures_util::{SinkExt, StreamExt};
    use serde_json::{json, Value};
    use tokio_tungstenite::connect_async_with_config;
    use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

    use super::*;

    /// The live socket type both `publish_one` and `query_one` work over.
    type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

    /// A single, fixed subscription id — each query opens a fresh connection, so
    /// the relay only ever sees one live subscription per socket.
    const SUB_ID: &str = "charter";

    /// Hard ceiling on events a single [`RealRelayTransport::query_one`] call
    /// will buffer from one relay before it stops reading — regardless of how
    /// many `EVENT` frames the relay streams ahead of `EOSE`. Delivery-only means
    /// the relay is never trusted: a hostile/compromised one can stream unbounded
    /// (or a few very large) fake `EVENT`s inside a single timeout window to
    /// exhaust the daemon's memory/CPU before the downstream `.take(max_inbound)`
    /// ever runs. A well-behaved relay sends only the handful of matching wraps
    /// then `EOSE`, so this never bites in normal operation — it is a *safety
    /// ceiling*, not a functional limit. `charter-transport` examines everything
    /// fetched (its `max_inbound` default sits above this ceiling on purpose: a
    /// lower cap, applied before unwrapping, let junk wraps crowd out the
    /// guardian's), so THIS is the bound on per-poll work. Per-relay: `query`
    /// dedups across relays afterward. It is also the remaining flood surface —
    /// a relay asked for "everything p-tagged to me" returns its newest 1024,
    /// and NIP-59's ephemeral outer author leaves nothing to filter on.
    const MAX_EVENTS_PER_QUERY: usize = 1024;

    /// Cumulative **byte** budget for one `query_one` call, alongside (not
    /// instead of) [`MAX_EVENTS_PER_QUERY`]. The event count alone bounds
    /// nothing useful: `NostrEvent.content` is an unbounded `String`, so 1024
    /// events of tens of MB each fit comfortably under the count ceiling and
    /// are limited only by the link. A Charter gift-wrap is kilobytes, so 8 MiB
    /// of accepted payload per relay per poll is orders of magnitude above any
    /// honest traffic and still bounded memory against a hostile relay.
    const MAX_BYTES_PER_QUERY: usize = 8 * 1024 * 1024;

    /// Websocket message/frame ceiling. tungstenite's defaults are 64 MiB per
    /// message and 16 MiB per frame — i.e. a single frame can cost the daemon
    /// 16 MiB before any of *our* accounting runs, because the library has
    /// already buffered it by the time we see a `Message`. A gift-wrap is
    /// kilobytes; 256 KiB leaves generous headroom and caps the per-frame cost.
    const MAX_WS_MESSAGE_BYTES: usize = 256 * 1024;

    /// Set to `1` to allow plaintext `ws://` relay URLs (a local test relay).
    /// Absent — i.e. in production — a non-`wss://` relay is refused at connect.
    const ALLOW_INSECURE_RELAY_ENV: &str = "CHARTER_ALLOW_INSECURE_RELAY";

    /// The websocket config every Charter socket is opened with.
    fn ws_config() -> WebSocketConfig {
        WebSocketConfig {
            max_message_size: Some(MAX_WS_MESSAGE_BYTES),
            max_frame_size: Some(MAX_WS_MESSAGE_BYTES),
            ..Default::default()
        }
    }

    /// The memory an accepted event costs us: the two unbounded fields. The
    /// fixed-width id/pubkey/sig/kind/created_at are noise beside them.
    fn event_weight(ev: &NostrEvent) -> usize {
        ev.content.len()
            + ev.tags
                .iter()
                .flat_map(|t| t.iter())
                .map(|s| s.len())
                .sum::<usize>()
    }

    /// Is `url` one we are willing to open a socket to? Only `wss://` unless
    /// insecure relays are explicitly opted into.
    ///
    /// Pairing already refuses a `bunker://…?relay=ws://…` URI in
    /// `charter-transport::pairing` and `charter-cli::pairing`, but that is the
    /// *entry* check: a relay list edited on disk, restored from a backup, or
    /// written by an older build never passes through it. The check belongs
    /// here too, at the one place every relay URL actually becomes a socket, so
    /// it holds regardless of how the URL arrived.
    fn relay_url_allowed(url: &str, allow_insecure: bool) -> bool {
        let u = url.trim();
        if u.len() >= 6 && u[..6].eq_ignore_ascii_case("wss://") {
            return true;
        }
        allow_insecure && u.len() >= 5 && u[..5].eq_ignore_ascii_case("ws://")
    }

    /// Read the insecure-relay opt-in from the environment (per call: a daemon
    /// that is restarted with the flag must not need a rebuild to pick it up).
    fn insecure_relays_allowed() -> bool {
        std::env::var(ALLOW_INSECURE_RELAY_ENV).map(|v| v == "1") == Ok(true)
    }

    /// The pure "should [`RealRelayTransport::query_one`] buffer this
    /// relay-served event?" decision, factored out so it is unit-testable without
    /// a live socket. Three independent guards, all required:
    ///   1. Re-apply the subscription [`Filter`] locally — a relay may serve
    ///      events that do not match the `REQ` (wrong kind/author/since), by bug
    ///      or malice; the mock transport already filters on query, so this brings
    ///      the real transport in line and drops junk before it is buffered.
    ///   2. Enforce the hard [`MAX_EVENTS_PER_QUERY`] ceiling (`accepted` = events
    ///      already buffered this call) so a flood cannot grow the buffer without
    ///      bound.
    ///   3. Enforce the cumulative [`MAX_BYTES_PER_QUERY`] budget (`bytes` =
    ///      [`event_weight`] already buffered this call), because (2) counts
    ///      events and an event has no intrinsic size.
    fn accept_event(filter: &Filter, ev: &NostrEvent, accepted: usize, bytes: usize) -> bool {
        accepted < MAX_EVENTS_PER_QUERY
            && bytes.saturating_add(event_weight(ev)) <= MAX_BYTES_PER_QUERY
            && filter.matches(ev)
    }

    /// The pure publish verdict, factored out of `publish_one` so the one
    /// decision that matters is testable without a live socket. `waited` is the
    /// OK wait: `None` = the deadline passed, `Some(None)` = the stream ended
    /// with no matching OK, `Some(Some(accepted))` = the relay answered.
    ///
    /// A **timeout** stays `Ok`: the frame was written to a socket the relay is
    /// still holding open, and plenty of relays simply do not send OKs
    /// promptly. A **closed stream** is `Failed`: the relay accepted the TCP/TLS
    /// connection and then dropped it, or the buffered write failed after
    /// `send` returned — "the bytes were sent" is not true there, and reporting
    /// it as delivered is how a STATUS the guardian never received, an
    /// `exec.allow` that never reached the inbox and a never-published audit
    /// event all become indistinguishable from success.
    fn publish_outcome(waited: Option<Option<bool>>) -> PublishOutcome {
        match waited {
            Some(Some(true)) | None => PublishOutcome::Ok,
            Some(Some(false)) => PublishOutcome::Failed("rejected by relay".into()),
            Some(None) => PublishOutcome::Failed("relay closed the socket before the OK".into()),
        }
    }

    /// Render a [`Filter`] as a NIP-01 filter object (omitting empty fields).
    fn filter_to_json(f: &Filter) -> Value {
        let mut m = serde_json::Map::new();
        if !f.kinds.is_empty() {
            m.insert("kinds".into(), json!(f.kinds));
        }
        if !f.authors.is_empty() {
            let hex: Vec<String> = f.authors.iter().map(|a| a.to_hex()).collect();
            m.insert("authors".into(), json!(hex));
        }
        if !f.p_tags.is_empty() {
            let hex: Vec<String> = f.p_tags.iter().map(|p| p.to_hex()).collect();
            m.insert("#p".into(), json!(hex));
        }
        if let Some(since) = f.since {
            m.insert("since".into(), json!(since));
        }
        if let Some(limit) = f.limit {
            m.insert("limit".into(), json!(limit));
        }
        Value::Object(m)
    }

    /// `["REQ", subid, filter]` client message.
    fn req_message(sub_id: &str, f: &Filter) -> String {
        json!(["REQ", sub_id, filter_to_json(f)]).to_string()
    }

    /// `["EVENT", event]` client publish message.
    fn event_message(ev: &NostrEvent) -> String {
        json!(["EVENT", ev]).to_string()
    }

    /// `["CLOSE", subid]` client message.
    fn close_message(sub_id: &str) -> String {
        json!(["CLOSE", sub_id]).to_string()
    }

    /// The relay->client messages Charter acts on.
    #[derive(Debug, PartialEq, Eq)]
    enum RelayMsg {
        Event(String, Box<NostrEvent>),
        Eose(String),
        /// `["OK", id, accepted, msg]`
        Ok(String, bool),
        /// NOTICE / CLOSED / anything else.
        Other,
    }

    /// Parse a relay text frame into a [`RelayMsg`] (None if not valid JSON / not
    /// an array / unknown shape that we can't act on).
    fn parse_relay_message(text: &str) -> Option<RelayMsg> {
        let v: Value = serde_json::from_str(text).ok()?;
        let arr = v.as_array()?;
        match arr.first()?.as_str()? {
            "EVENT" => {
                let sub = arr.get(1)?.as_str()?.to_string();
                let ev: NostrEvent = serde_json::from_value(arr.get(2)?.clone()).ok()?;
                Some(RelayMsg::Event(sub, Box::new(ev)))
            }
            "EOSE" => Some(RelayMsg::Eose(arr.get(1)?.as_str()?.to_string())),
            "OK" => {
                let id = arr.get(1)?.as_str()?.to_string();
                let accepted = arr.get(2)?.as_bool()?;
                Some(RelayMsg::Ok(id, accepted))
            }
            _ => Some(RelayMsg::Other),
        }
    }

    /// Real multi-relay websocket transport. `timeout` bounds each connect +
    /// each EOSE/OK wait so a slow or hostile relay can never hang the broker.
    #[derive(Clone)]
    pub struct RealRelayTransport {
        timeout: Duration,
    }
    impl Default for RealRelayTransport {
        fn default() -> Self {
            Self {
                timeout: Duration::from_secs(8),
            }
        }
    }

    impl RealRelayTransport {
        /// Override the per-relay IO timeout.
        pub fn with_timeout(timeout: Duration) -> Self {
            Self { timeout }
        }

        /// Open one socket: refuse the URL outright unless it is `wss://` (or
        /// the insecure opt-in is set), then connect under the message/frame
        /// ceiling of [`ws_config`]. `Err` carries a reason for the caller to
        /// report — it is never swallowed.
        async fn connect_one(&self, url: &RelayUrl) -> Result<Ws, String> {
            if !relay_url_allowed(url, insecure_relays_allowed()) {
                return Err(format!(
                    "refusing a non-wss relay url (set {ALLOW_INSECURE_RELAY_ENV}=1 to allow it)"
                ));
            }
            match tokio::time::timeout(
                self.timeout,
                connect_async_with_config(url, Some(ws_config()), false),
            )
            .await
            {
                Ok(Ok((ws, _))) => Ok(ws),
                Ok(Err(e)) => Err(format!("connect: {e}")),
                Err(_) => Err("connect timeout".into()),
            }
        }

        async fn publish_one(&self, url: &RelayUrl, ev: &NostrEvent) -> PublishOutcome {
            let mut ws = match self.connect_one(url).await {
                Ok(ws) => ws,
                Err(e) => return PublishOutcome::Failed(e),
            };
            if let Err(e) = ws.send(Message::text(event_message(ev))).await {
                return PublishOutcome::Failed(format!("send: {e}"));
            }
            // Wait for the matching OK. See `publish_outcome` for why a timeout
            // is delivered and a closed socket is not.
            let id_hex = ev.id.to_hex();
            let waited = tokio::time::timeout(self.timeout, async {
                while let Some(Ok(msg)) = ws.next().await {
                    if let Message::Text(t) = msg {
                        if let Some(RelayMsg::Ok(id, accepted)) = parse_relay_message(t.as_str()) {
                            if id == id_hex {
                                return Some(accepted);
                            }
                        }
                    }
                }
                None
            })
            .await;
            let _ = ws.close(None).await;
            publish_outcome(waited.ok())
        }

        /// Query one relay. `Err(Unreachable)` means we never got as far as a
        /// subscription (refused url / connect failure / the `REQ` would not
        /// send); `Ok(vec![])` means the relay was reached and served nothing.
        /// Collapsing the two — as this used to — is what made a dead relay set
        /// indistinguishable from a quiet guardian.
        async fn query_one(
            &self,
            url: &RelayUrl,
            filter: &Filter,
        ) -> Result<Vec<NostrEvent>, RelayIoError> {
            let mut ws = self
                .connect_one(url)
                .await
                .map_err(|e| RelayIoError::Unreachable(format!("{url}: {e}")))?;
            if let Err(e) = ws.send(Message::text(req_message(SUB_ID, filter))).await {
                return Err(RelayIoError::Unreachable(format!("{url}: send REQ: {e}")));
            }
            let mut out = Vec::new();
            let mut bytes = 0usize;
            let _ = tokio::time::timeout(self.timeout, async {
                while let Some(Ok(msg)) = ws.next().await {
                    if let Message::Text(t) = msg {
                        match parse_relay_message(t.as_str()) {
                            Some(RelayMsg::Event(sub, ev)) if sub == SUB_ID => {
                                // Hard caps: stop reading once either ceiling is
                                // hit rather than keep buffering a hostile
                                // relay's flood (a well-behaved relay never
                                // reaches either).
                                if out.len() >= MAX_EVENTS_PER_QUERY || bytes >= MAX_BYTES_PER_QUERY
                                {
                                    break;
                                }
                                // Re-validate against our own REQ filter; drop any
                                // event the relay served that does not match.
                                if accept_event(filter, &ev, out.len(), bytes) {
                                    bytes = bytes.saturating_add(event_weight(&ev));
                                    out.push(*ev);
                                }
                            }
                            Some(RelayMsg::Eose(sub)) if sub == SUB_ID => break,
                            _ => {}
                        }
                    }
                }
            })
            .await;
            let _ = ws.send(Message::text(close_message(SUB_ID))).await;
            let _ = ws.close(None).await;
            Ok(out)
        }
    }

    #[async_trait]
    impl RelayTransport for RealRelayTransport {
        async fn publish(
            &self,
            relays: &[RelayUrl],
            ev: NostrEvent,
        ) -> Vec<(RelayUrl, PublishOutcome)> {
            let mut out = Vec::with_capacity(relays.len());
            for r in relays {
                out.push((r.clone(), self.publish_one(r, &ev).await));
            }
            out
        }

        async fn query(
            &self,
            relays: &[RelayUrl],
            filter: Filter,
        ) -> Result<Vec<NostrEvent>, RelayIoError> {
            let mut seen = Vec::new();
            let mut out = Vec::new();
            let mut reached = 0usize;
            let mut why: Vec<String> = Vec::new();
            for r in relays {
                match self.query_one(r, &filter).await {
                    Ok(events) => {
                        reached += 1;
                        for ev in events {
                            if !seen.contains(&ev.id) {
                                seen.push(ev.id);
                                out.push(ev);
                            }
                        }
                    }
                    Err(RelayIoError::Unreachable(e)) => why.push(e),
                }
            }
            // EVERY relay failed before serving anything: that is a broken relay
            // set, not a quiet guardian, and the caller must be able to tell.
            // An EMPTY relay list keeps the old `Ok(vec![])` — there is nothing
            // to be unreachable, and callers pass one on purpose.
            if reached == 0 && !relays.is_empty() {
                return Err(RelayIoError::Unreachable(format!(
                    "no relay reachable ({} tried): {}",
                    relays.len(),
                    why.join("; ")
                )));
            }
            if let Some(limit) = filter.limit {
                out.truncate(limit);
            }
            Ok(out)
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use charter_primitives::{EventId, PubKey, Sig};

        fn sample_event() -> NostrEvent {
            NostrEvent {
                id: EventId::from_hex(&"ab".repeat(32)).unwrap(),
                pubkey: PubKey::from_hex(&"cd".repeat(32)).unwrap(),
                created_at: 1_700_000_000,
                kind: 31112,
                tags: vec![vec!["p".into(), "ef".repeat(32)]],
                content: "ciphertext".into(),
                sig: Sig::from_hex(&"12".repeat(64)).unwrap(),
            }
        }

        #[test]
        fn req_message_carries_filter_fields() {
            let f = Filter {
                kinds: vec![1059, 31112],
                authors: vec![PubKey::from_hex(&"cd".repeat(32)).unwrap()],
                p_tags: vec![PubKey::from_hex(&"ef".repeat(32)).unwrap()],
                since: Some(42),
                limit: Some(10),
            };
            let v: Value = serde_json::from_str(&req_message("charter", &f)).unwrap();
            assert_eq!(v[0], "REQ");
            assert_eq!(v[1], "charter");
            assert_eq!(v[2]["kinds"], json!([1059, 31112]));
            assert_eq!(v[2]["authors"][0], "cd".repeat(32));
            assert_eq!(v[2]["#p"][0], "ef".repeat(32));
            assert_eq!(v[2]["since"], 42);
            assert_eq!(v[2]["limit"], 10);
        }

        #[test]
        fn empty_filter_omits_fields() {
            let v: Value =
                serde_json::from_str(&req_message("charter", &Filter::default())).unwrap();
            assert!(v[2].as_object().unwrap().is_empty());
        }

        #[test]
        fn event_message_roundtrips_through_parse() {
            let ev = sample_event();
            let wire = event_message(&ev);
            let v: Value = serde_json::from_str(&wire).unwrap();
            assert_eq!(v[0], "EVENT");
            // A relay echoing this back as ["EVENT", sub, ev] parses to our event.
            let echoed = json!(["EVENT", "charter", v[1]]).to_string();
            match parse_relay_message(&echoed).unwrap() {
                RelayMsg::Event(sub, got) => {
                    assert_eq!(sub, "charter");
                    assert_eq!(*got, ev);
                }
                other => panic!("expected Event, got {other:?}"),
            }
        }

        #[test]
        fn parse_eose_ok_and_notice() {
            assert_eq!(
                parse_relay_message(r#"["EOSE","charter"]"#),
                Some(RelayMsg::Eose("charter".into()))
            );
            assert_eq!(
                parse_relay_message(r#"["OK","abcd",true,""]"#),
                Some(RelayMsg::Ok("abcd".into(), true))
            );
            assert_eq!(
                parse_relay_message(r#"["OK","abcd",false,"invalid: bad sig"]"#),
                Some(RelayMsg::Ok("abcd".into(), false))
            );
            assert_eq!(
                parse_relay_message(r#"["NOTICE","hi"]"#),
                Some(RelayMsg::Other)
            );
            assert_eq!(parse_relay_message("not json"), None);
            assert_eq!(parse_relay_message(r#"{"not":"array"}"#), None);
        }

        #[test]
        fn close_message_shape() {
            let v: Value = serde_json::from_str(&close_message("charter")).unwrap();
            assert_eq!(v, json!(["CLOSE", "charter"]));
        }

        #[test]
        fn accept_event_drops_events_not_matching_the_filter() {
            // The query REQ pins one kind. An event of that kind is accepted; an
            // event of another kind (a hostile relay ignoring our REQ) is dropped
            // BEFORE it is buffered/deserialized-further — Fix 1(a).
            let filter = Filter {
                kinds: vec![31112],
                ..Default::default()
            };
            let mut matching = sample_event();
            matching.kind = 31112;
            let mut wrong_kind = sample_event();
            wrong_kind.kind = 1; // not in the filter
            assert!(
                accept_event(&filter, &matching, 0, 0),
                "an event matching the REQ filter is accepted"
            );
            assert!(
                !accept_event(&filter, &wrong_kind, 0, 0),
                "an event the relay served that does not match the REQ is dropped"
            );
        }

        #[test]
        fn accept_event_enforces_the_hard_cap() {
            // An empty filter matches everything, so only the cap can bound it —
            // Fix 1(b). Events are accepted while fewer than the ceiling are
            // already buffered, then refused, so a flood cannot grow the buffer
            // without bound.
            let filter = Filter::default();
            let ev = sample_event();
            assert!(accept_event(&filter, &ev, 0, 0));
            assert!(
                accept_event(&filter, &ev, MAX_EVENTS_PER_QUERY - 1, 0),
                "the last slot under the ceiling is accepted"
            );
            assert!(
                !accept_event(&filter, &ev, MAX_EVENTS_PER_QUERY, 0),
                "at the ceiling, further events are refused"
            );
            assert!(
                !accept_event(&filter, &ev, MAX_EVENTS_PER_QUERY + 1, 0),
                "past the ceiling, further events are refused"
            );
        }

        // ---- G2: the byte budget -----------------------------------------

        #[test]
        fn accept_event_enforces_the_cumulative_byte_budget() {
            // The event COUNT is nowhere near its ceiling here; only the byte
            // budget can refuse these. A hostile relay streaming a handful of
            // multi-MB events is the case the count ceiling never saw.
            let filter = Filter::default();
            let mut fat = sample_event();
            fat.content = "x".repeat(MAX_BYTES_PER_QUERY / 4);
            let weight = event_weight(&fat);
            assert!(accept_event(&filter, &fat, 0, 0));
            assert!(
                accept_event(&filter, &fat, 3, MAX_BYTES_PER_QUERY - weight),
                "the last event that exactly fills the budget is accepted"
            );
            assert!(
                !accept_event(&filter, &fat, 3, MAX_BYTES_PER_QUERY - weight + 1),
                "one byte over the budget and the event is refused"
            );
            assert!(
                !accept_event(&filter, &fat, 4, MAX_BYTES_PER_QUERY),
                "at the budget, further events are refused however few were counted"
            );
        }

        #[test]
        fn event_weight_counts_content_and_tags() {
            let mut ev = sample_event();
            ev.content = "abcd".into();
            ev.tags = vec![vec!["p".into(), "xy".into()]];
            assert_eq!(event_weight(&ev), 4 + 1 + 2);
        }

        #[test]
        fn ws_config_caps_message_and_frame_size() {
            // Left at tungstenite's defaults this is 64 MiB / 16 MiB — buffered
            // by the library before any of our accounting can run.
            let cfg = ws_config();
            assert_eq!(cfg.max_message_size, Some(MAX_WS_MESSAGE_BYTES));
            assert_eq!(cfg.max_frame_size, Some(MAX_WS_MESSAGE_BYTES));
            // ...and tighter than what the library would have used.
            let library_default = WebSocketConfig::default();
            assert!(
                cfg.max_message_size < library_default.max_message_size,
                "the ceiling must be tighter than tungstenite's 64 MiB default"
            );
        }

        #[test]
        fn only_wss_urls_are_connected_to_unless_opted_in() {
            assert!(relay_url_allowed("wss://relay.example", false));
            assert!(relay_url_allowed("WSS://relay.example", false));
            assert!(
                relay_url_allowed("  wss://relay.example  ", false),
                "surrounding whitespace is not a downgrade"
            );
            assert!(
                !relay_url_allowed("ws://relay.example", false),
                "a plaintext relay is refused in production"
            );
            assert!(
                !relay_url_allowed("http://relay.example", false),
                "a non-websocket scheme is refused"
            );
            assert!(!relay_url_allowed("wss:/relay.example", false));
            assert!(!relay_url_allowed("", false));
            assert!(
                relay_url_allowed("ws://127.0.0.1:7777", true),
                "the explicit opt-in allows a local test relay"
            );
            assert!(
                !relay_url_allowed("http://relay.example", true),
                "the opt-in is for ws://, not for anything at all"
            );
        }

        // ---- B5: the publish verdict -------------------------------------

        #[test]
        fn a_closed_socket_is_not_a_delivered_publish() {
            // `Some(None)`: the relay took the connection and then dropped it
            // without an OK. Reported as delivered, this is a STATUS the
            // guardian never receives that nothing anywhere records as lost.
            assert_eq!(
                publish_outcome(Some(None)),
                PublishOutcome::Failed("relay closed the socket before the OK".into())
            );
        }

        #[test]
        fn a_timeout_is_still_a_delivered_publish() {
            // The frame was written to a socket the relay is still holding
            // open; many relays simply do not OK promptly.
            assert_eq!(publish_outcome(None), PublishOutcome::Ok);
        }

        #[test]
        fn an_explicit_ok_and_an_explicit_rejection_are_unchanged() {
            assert_eq!(publish_outcome(Some(Some(true))), PublishOutcome::Ok);
            assert_eq!(
                publish_outcome(Some(Some(false))),
                PublishOutcome::Failed("rejected by relay".into())
            );
        }

        // ---- G1: unreachable ---------------------------------------------

        #[tokio::test]
        async fn a_relay_set_that_cannot_be_reached_is_an_error_not_an_empty_result() {
            // Every url is refused before a socket is even opened (the non-wss
            // guard), which is the cheapest deterministic "no relay reached"
            // there is — no network, no listener, no timeout to wait out.
            let t = RealRelayTransport::with_timeout(Duration::from_millis(50));
            let relays = vec!["ws://insecure.example".to_string()];
            match t.query(&relays, Filter::default()).await {
                Err(RelayIoError::Unreachable(why)) => {
                    assert!(why.contains("no relay reachable"), "got {why}");
                    assert!(why.contains("insecure.example"), "names the relay: {why}");
                }
                other => panic!("expected Unreachable, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn an_empty_relay_list_is_still_simply_empty() {
            // Nothing to be unreachable — callers pass an empty set on purpose.
            let t = RealRelayTransport::with_timeout(Duration::from_millis(50));
            assert_eq!(t.query(&[], Filter::default()).await.unwrap(), vec![]);
        }

        #[tokio::test]
        async fn a_refused_url_is_a_publish_failure_not_a_silent_success() {
            let t = RealRelayTransport::with_timeout(Duration::from_millis(50));
            let relays = vec!["ws://insecure.example".to_string()];
            let out = t.publish(&relays, sample_event()).await;
            assert_eq!(out.len(), 1);
            assert_eq!(out[0].0, "ws://insecure.example");
            match &out[0].1 {
                PublishOutcome::Failed(why) => assert!(why.contains("non-wss"), "got {why}"),
                other => panic!("expected Failed, got {other:?}"),
            }
        }
    }
}

#[cfg(feature = "real-relay")]
pub use real::RealRelayTransport;
