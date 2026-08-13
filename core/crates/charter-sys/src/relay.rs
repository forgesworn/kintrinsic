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
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;

    use super::*;

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
    /// ceiling*, not a functional limit. It sits well above `charter-transport`'s
    /// `max_inbound` default (256, the functional per-poll cap applied downstream
    /// via `.take`), so a legitimate backlog is never truncated here; only a
    /// flood is bounded. Per-relay: `query` dedups across relays afterward.
    const MAX_EVENTS_PER_QUERY: usize = 1024;

    /// The pure "should [`RealRelayTransport::query_one`] buffer this
    /// relay-served event?" decision, factored out so it is unit-testable without
    /// a live socket. Two independent guards, both required:
    ///   1. Re-apply the subscription [`Filter`] locally — a relay may serve
    ///      events that do not match the `REQ` (wrong kind/author/since), by bug
    ///      or malice; the mock transport already filters on query, so this brings
    ///      the real transport in line and drops junk before it is buffered.
    ///   2. Enforce the hard [`MAX_EVENTS_PER_QUERY`] ceiling (`accepted` = events
    ///      already buffered this call) so a flood cannot grow the buffer without
    ///      bound.
    fn accept_event(filter: &Filter, ev: &NostrEvent, accepted: usize) -> bool {
        accepted < MAX_EVENTS_PER_QUERY && filter.matches(ev)
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

        async fn publish_one(&self, url: &RelayUrl, ev: &NostrEvent) -> PublishOutcome {
            let mut ws = match tokio::time::timeout(self.timeout, connect_async(url)).await {
                Ok(Ok((ws, _))) => ws,
                Ok(Err(e)) => return PublishOutcome::Failed(format!("connect: {e}")),
                Err(_) => return PublishOutcome::Failed("connect timeout".into()),
            };
            if let Err(e) = ws.send(Message::text(event_message(ev))).await {
                return PublishOutcome::Failed(format!("send: {e}"));
            }
            // Briefly wait for the matching OK; a missing OK before the deadline
            // is treated as delivered (bytes were sent), an OK(false) as rejected.
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
            match waited {
                Ok(Some(true)) | Err(_) | Ok(None) => PublishOutcome::Ok,
                Ok(Some(false)) => PublishOutcome::Failed("rejected by relay".into()),
            }
        }

        async fn query_one(&self, url: &RelayUrl, filter: &Filter) -> Vec<NostrEvent> {
            let mut ws = match tokio::time::timeout(self.timeout, connect_async(url)).await {
                Ok(Ok((ws, _))) => ws,
                _ => return Vec::new(),
            };
            if ws
                .send(Message::text(req_message(SUB_ID, filter)))
                .await
                .is_err()
            {
                return Vec::new();
            }
            let mut out = Vec::new();
            let _ = tokio::time::timeout(self.timeout, async {
                while let Some(Ok(msg)) = ws.next().await {
                    if let Message::Text(t) = msg {
                        match parse_relay_message(t.as_str()) {
                            Some(RelayMsg::Event(sub, ev)) if sub == SUB_ID => {
                                // Hard cap: stop reading once the ceiling is hit
                                // rather than keep buffering a hostile relay's
                                // flood (a well-behaved relay never reaches this).
                                if out.len() >= MAX_EVENTS_PER_QUERY {
                                    break;
                                }
                                // Re-validate against our own REQ filter; drop any
                                // event the relay served that does not match.
                                if accept_event(filter, &ev, out.len()) {
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
            out
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
            for r in relays {
                for ev in self.query_one(r, &filter).await {
                    if !seen.contains(&ev.id) {
                        seen.push(ev.id);
                        out.push(ev);
                    }
                }
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
                accept_event(&filter, &matching, 0),
                "an event matching the REQ filter is accepted"
            );
            assert!(
                !accept_event(&filter, &wrong_kind, 0),
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
            assert!(accept_event(&filter, &ev, 0));
            assert!(
                accept_event(&filter, &ev, MAX_EVENTS_PER_QUERY - 1),
                "the last slot under the ceiling is accepted"
            );
            assert!(
                !accept_event(&filter, &ev, MAX_EVENTS_PER_QUERY),
                "at the ceiling, further events are refused"
            );
            assert!(
                !accept_event(&filter, &ev, MAX_EVENTS_PER_QUERY + 1),
                "past the ceiling, further events are refused"
            );
        }
    }
}

#[cfg(feature = "real-relay")]
pub use real::RealRelayTransport;
