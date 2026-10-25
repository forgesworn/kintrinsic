# Local network delivery: design

Paired guardian and ward devices on the same home network exchange requests,
answers and clause changes directly, without a relay. Relays stay in use for
remote and deferred delivery. This document records the design decisions, the
local-link protocol, the routing and acknowledgement rules, the platform
support matrix and the hardware test plan. The wire-level definition lives in
[the contract](../spec/contract.md) under **Local link (v1)**.

## Decisions

| Question | Decision | Why |
| --- | --- | --- |
| Who listens? | The **guardian carrier** (Android guardian app) listens and advertises. **Wards connect out.** | Children's devices never advertise a service or open a listening port, so a ward laptop or phone on school or café Wi-Fi does not announce that it is supervised. Linux wards need no firewall change (outbound only). The carrier already runs a persistent foreground service and already holds the guardian key natively for relay authentication and request notifications, so no new key copies are made. |
| Discovery | DNS-SD `_kintrinsic._tcp.local`, registered with Android `NsdManager` on the carrier. Linux wards browse through avahi-daemon on the system D-Bus; Android wards browse with `NsdManager`. Wards also retry the last address that authenticated. | Native, no router changes, no household server. Discovery is only a hint: identity comes from the handshake. |
| Transport | Plain TCP, IPv4, length-prefixed JSON frames. No TLS. | Every payload is already a NIP-59 gift wrap (encrypted end to end), every guardian instruction is a guardian-signed inner event, every acknowledgement is signed by the ward's machine key. The session handshake gates access and identifies the peer; integrity never rests on the channel. This avoids self-signed certificates entirely. |
| Session authentication | Mutual BIP-340 challenge–response bound to fresh 32-byte challenges from both sides. The guardian proves first; the ward reveals its machine key only to its pinned guardian. | Reuses the existing guardian key and machine key and the existing `charter-crypto` Schnorr implementation. A stranger probing the carrier learns only the guardian public key, which relays already expose as a `p` tag. |
| Ingestion on the ward | A new broker entry point, `Broker::ingest_local`, unwraps with the same NIP-59 code as relay polling and runs the same `on_clause` / `on_grant` / release / usage-sync handlers. | Local delivery cannot bypass verification, pinning, freshness, nonce or ordering checks; it only replaces how the wrap arrives. |
| Relay copies | Both sides publish every wrap to relays **immediately and in parallel** with the local send, using the **identical wrap** (same event id). Neither side waits for relays when the local route succeeds. | Durable delivery for other networks and offline peers is retained; the relay copy is idempotent downstream (same inner event, same `reqId`, per-kind `issuedAt` floors). |
| Acknowledgement | A signed `ACK` frame from the ward per guardian wrap received over the link, reporting what the broker actually did. | Separates local receipt (`OK`), relay acceptance and application on the device. No new Nostr kind. |
| Browser PWA | Relay only, with a visible note. | The guardian end listens; a web page cannot accept connections, browse DNS-SD or open raw TCP. See the support matrix. |

## Roles and data flow

```
 ward (Linux charterd / Android ward app)            guardian carrier (Android)
 ─────────────────────────────────────────           ──────────────────────────────
 discovery (avahi / NsdManager) ──hint──▶  _kintrinsic._tcp  ◀── NsdManager register
 WardLink client  ── TCP connect ──────────────────────────▶ GuardianLink listener
                  ◀─ HELLO / CHALLENGE / AUTH / READY ─────▶  (jni-guardian, Rust)
 outbox (REQUEST, AUDIT, STATUS wraps) ── EVENT ──────────▶  inbound store ─▶ notify
                                                              │             (classify)
                                                              ▼
                                                   console drains localInbox()
                                                   → same ingest as relay poll
 Broker::ingest_local ◀── EVENT (GRANT/CLAUSE/RELEASE wraps) ◀─ localSend() from console
          │                                                   outbound queue per ward
          └── ACK (signed result) ─────────────────────────▶  delivery state ─▶ console
```

Relays carry identical copies in both directions.

## Local link protocol (v1)

The normative definition is in the contract. In short:

1. **Framing.** Each frame is a 4-byte big-endian length `N` (1 ≤ `N` ≤
   262 144) followed by `N` bytes of UTF-8 JSON, which must be an array whose
   first element is a frame-type string. A zero, oversized or unparsable frame
   closes the connection.
2. **Handshake** (deadline 5 s from TCP connect or accept):
   1. ward → `["HELLO", {"v":1, "challenge": cw}]`
   2. guardian → `["CHALLENGE", {"v":1, "guardian": G, "challenge": cg, "sig": sg}]`,
      `sg = BIP340(G, SHA256("kintrinsic-local-v1/guardian\n" ‖ cw ‖ "\n" ‖ cg ‖ "\n" ‖ G))`
   3. ward checks `G` is its pinned guardian and `sg` verifies, then sends
      `["AUTH", {"machine": M, "sig": sw}]`,
      `sw = BIP340(M, SHA256("kintrinsic-local-v1/ward\n" ‖ cw ‖ "\n" ‖ cg ‖ "\n" ‖ G ‖ "\n" ‖ M))`
   4. guardian checks `M` is a paired device and `sw` verifies, then sends
      `["READY", {"v":1}]`.

   All hex is lowercase; challenges are 32 random bytes from the OS. Any
   failure closes the connection without explanation.
3. **Session frames.** `["EVENT", wrap]` in either direction (kind 1059, a
   single `p` tag naming the peer, valid id and outer signature);
   `["OK", wrapId, accepted, reason]` for receipt; `["ACK", {...}]` from the
   ward only; `["PING", n]` / `["PONG", n]` keep-alive. Unknown frame types
   are ignored but count towards rate limits.
4. **ACK.** `{"wrap": id, "result": R, "ts": t, "sig": s}`,
   `s = BIP340(M, SHA256("kintrinsic-local-v1/ack\n" ‖ G ‖ "\n" ‖ M ‖ "\n" ‖ id ‖ "\n" ‖ R ‖ "\n" ‖ t))`
   with `t` in decimal. `R` is one of:

   | Result | Meaning on the ward |
   | --- | --- |
   | `applied` | Verified, and this delivery made it the device's state: a clause stored as the effective clause for its subject and kind; a grant moved its request to *Enacted* or *Denied*; a release applied. |
   | `unchanged` | Verified, and the device already held it: the same inner event is the stored clause, or the request was already answered by an earlier copy. |
   | `superseded` | An authentic clause older than (or tied with a different event at) the one in effect. The newer clause stays effective. |
   | `expired` | A grant past its validity window, or for an ask the device has already expired or closed. |
   | `refused` | Failed verification or validation (forged, wrong guardian, malformed, parameter mismatch, unknown request). |
   | `failed` | Authentic but the device could not record or enact it. Retrying may help. |
   | `ignored` | A kind the device does not act on over the local link. |

   `applied` for an install approval means the approval was recorded and the
   install queued, not that the operating system finished installing. The
   carrier verifies every ACK against the session's authenticated machine key
   and the wrap's recipient before recording it.

### Bounds

| Limit | Guardian listener | Ward client |
| --- | --- | --- |
| Unauthenticated connections | 8 concurrent | — |
| Authenticated sessions | 16 total, 2 per machine (a third closes the oldest) | 4 guardian sessions |
| Inbound frames | 120 per minute per session, else close | 120 per minute per session, else close |
| Inbound wraps held | 256 (inbound store) | 32 queued for ingest per session; reading pauses when full |
| Outbound held | 64 per ward, 48 h | REQUEST 64 (24 h), AUDIT 64 (48 h), latest STATUS per subject |
| Keep-alive | closes after 330 s silence | PING every 120 s, closes after 330 s silence |
| Accepted peers | IPv4 private and link-local source addresses only | — |

### Discovery details

- Instance name `Kintrinsic <4 random hex>`, generated once per install. TXT
  record `v=1` only. No names, keys or family details are broadcast.
- The carrier listens on IPv4 `0.0.0.0`, preferring port 47615 and falling
  back to an ephemeral port, and registers the service only while a Wi-Fi or
  Ethernet network is active.
- Targeting Android 17 (API 37) will require `ACCESS_LOCAL_NETWORK` on both
  Android apps; both currently target API 35, where `INTERNET` implies local
  access. Raise this before any target-SDK bump.
- Wards keep up to 8 candidate addresses: those discovered, plus the last
  address that completed a handshake (persisted), which covers a restart when
  multicast discovery is slow or blocked. Candidates that fail back off from
  2 s to 60 s. A new discovery result or network change resets the back-off.
- A changed IP address is handled by rediscovery; no address is configured by
  hand.

## Routing and fallback

**Guardian → ward** (clauses, grants, releases):

1. The console builds the wrap as today.
2. It starts the relay publish and calls `localSend(recipient, wrap)` at the
   same time.
3. If the carrier reports `sent`, the console waits up to 3 s for the ward's
   `OK`; receipt completes the publish without waiting for relays. The relay
   publish continues in the background.
4. Otherwise the relay result decides, as before. If relays also fail but the
   carrier has queued the wrap for a ward that is not connected, the console
   reports **not sent yet, waiting for the home network**, rather than success
   or a hard failure. The queued wrap goes out when the ward connects. Expired
   grants are refused by the ward when they arrive.
5. A later `ACK` updates the delivery state to *applied on the device*.

**Ward → guardian** (requests, audits, status): the ward offers each wrap to
its local outbox before the relay publish starts. Connected guardian sessions
receive it at once; a guardian that connects later receives everything still
in the outbox. Relay publication and the existing relay outbox are unchanged,
so durable remote delivery is retained.

**Timeouts.** Local work never waits on a relay. A ward without a reachable
guardian simply has no session; relay polling continues on its own schedule.
Connection attempts are bounded (TCP 3 s, handshake 5 s).

**Both routes down.** Verified cached clauses keep enforcing. Pending asks and
undelivered changes stay pending and expire under the existing rules: a grant
that arrives after its `exp`, or after the ask aged out, is never enacted.

## Deduplication and ordering

- The same wrap via both routes has the same id, so the carrier's inbound
  store, the carrier's notification dedupe (by `reqId`) and the console's
  wrap and `reqId` dedupe all collapse it to one entry and one notification.
- On the ward, duplicates are absorbed by existing state: a grant only acts
  on a *Pending* request, its `reqId` is consumed, and `time.extend` is
  idempotent by `reqId` in the extension ledger. Clause ordering is the
  existing per-subject, per-kind `issuedAt` floor. All of this state is on
  disk and survives restarts, so a relay copy of an older clause arriving
  after a newer local one is refused as stale.
- Ingestion from the local link and from relay polling is serialised inside
  the broker (one handler at a time), not by holding locks across relay IO.

## Delivery state shown to the guardian

Per wrap and per ward device: `queued` (held by the carrier, ward not
connected), `sent` (written to an authenticated session), `received` (ward
`OK`), relay accepted, then the ACK result. An app-open approval counts as
applied on a device only when both the grant and that device's `apps` clause
report `applied` or `unchanged`. A device with a live session is shown as
**on the home network**, independently of relay reachability, so a working
LAN is never presented as total disconnection.

## Support matrix

| Surface | Local delivery | Notes |
| --- | --- | --- |
| Android guardian carrier | **Yes** (listener) | Holds a `MulticastLock` while registered on Wi-Fi so a pocketed phone still answers discovery (battery cost measured in the hardware gates). Needs the local guardian key, which the carrier already holds when parent approval is set up on the phone. A remote NIP-46 signer does not work offline. Foreground service `specialUse`. Older consoles and older APKs feature-detect the bridge and stay on relays. |
| Browser PWA (no carrier) | **No**, relays only | A page cannot listen for connections, browse DNS-SD or open raw TCP (Direct Sockets is limited to Isolated Web Apps on ChromeOS). Chrome 147 allows `ws://` to a LAN IP literal from an `https` page only after a Local Network Access prompt, Android WebView does not support that prompt, and the ward would still need a reachable listener. The app says so where delivery is discussed. |
| Android ward | **Yes** (client) | `NsdManager` discovery while no session is up, holding its own `MulticastLock` for the duration (a foreground-service process does not get the system's automatic lock). Outbound TCP from the native library, unaffected by the cleartext policy and outside the ward's split-tunnel DNS VPN. A foreground service keeps network access in Doze, but whether an incoming packet wakes a dozing phone is not documented: a locked, dark phone may process a change only at its next liveness wake, and always on screen-on. Measured in the hardware gates. |
| Linux ward (`charterd`) | **Yes** (client) | Needs `avahi-daemon` for discovery (installed by default on Linux Mint and Ubuntu desktop); without it, only the remembered last address is tried. Outbound only: no firewall change. |
| Older wards, older carriers | Relays only | No behaviour change. |

Platform facts and their sources are in [local-network-platform-notes.md](local-network-platform-notes.md).

## Security notes

- A local message is accepted only after the same verification as its relay
  copy. The link never grants authority: a CLAUSE or GRANT is acted on only if
  its inner event is signed by the pinned guardian, and a REQUEST is accepted
  by the console only if its seal is signed by the machine key it names.
- The carrier accepts only machine keys of devices in the console's roster,
  and wards accept only their pinned guardian.
- An on-path attacker on the LAN can drop or delay frames (as a relay can) but
  cannot forge a clause, grant, request or ACK, and cannot read payloads.
- Bounds above cap memory, connections and work from unrelated LAN clients.
- Remote NIP-46 signers are unaffected and gain no offline ability. Consumer
  apps keep using their NIP-46 bunker path.

## Implementation interfaces

These are the agreed seams between the components. Names are binding;
private helpers are free.

### `core/crates/charter-link` (new leaf crate)

Depends on `charter-primitives`, `charter-crypto`, `charter-verify`
(`default-features = false`), `serde`, `serde_json`, `zeroize`; the `net`
feature adds `tokio` (net, io-util, time, sync, rt). Default features: none.

```rust
pub const PROTOCOL_VERSION: u8 = 1;
pub const MAX_FRAME: usize = 262_144;
pub const SERVICE_TYPE: &str = "_kintrinsic._tcp";
pub const PREFERRED_PORT: u16 = 47_615;

pub enum AckResult { Applied, Unchanged, Superseded, Expired, Refused, Failed, Ignored } // wire: lowercase
pub struct LocalOffer { pub inner_kind: u16, pub key: Option<String>, pub wrap: NostrEvent }
pub trait LocalSink: Send + Sync { fn offer(&self, offer: LocalOffer); } // never blocks on the network

// proto (always compiled)
pub fn guardian_proof_digest(cw: &[u8; 32], cg: &[u8; 32], guardian: &PubKey) -> [u8; 32];
pub fn ward_proof_digest(cw: &[u8; 32], cg: &[u8; 32], guardian: &PubKey, machine: &PubKey) -> [u8; 32];
pub fn ack_digest(guardian: &PubKey, machine: &PubKey, wrap: &EventId, result: AckResult, ts: u64) -> [u8; 32];
pub struct Ack { pub wrap: EventId, pub result: AckResult, pub ts: u64, pub sig: Sig }
pub fn sign_ack(machine_sk: &[u8; 32], guardian: &PubKey, wrap: &EventId, result: AckResult, ts: u64) -> Ack;
pub fn verify_ack(ack: &Ack, guardian: &PubKey, machine: &PubKey) -> bool;
pub enum Frame { Hello { .. }, Challenge { .. }, Auth { .. }, Ready { .. }, Event(NostrEvent),
                 Ok { wrap: EventId, accepted: bool, reason: String }, Ack(Ack), Ping(u64), Pong(u64), Unknown(String) }
pub fn encode_frame(frame: &Frame) -> Vec<u8>;            // includes the 4-byte length prefix
pub fn decode_frame(body: &[u8]) -> Result<Frame, FrameError>; // body without the prefix
pub fn validate_wrap_for(wrap: &NostrEvent, recipient: &PubKey) -> Result<(), WrapError>;

// ward role (feature "net")
pub trait WardIngest: Send + Sync + 'static {
    fn ingest(&self, wrap: NostrEvent) -> Pin<Box<dyn Future<Output = AckResult> + Send>>;
}
pub struct WardLinkConfig { pub machine_sk: Zeroizing<[u8; 32]>, pub guardian: PubKey, pub state_dir: PathBuf }
impl WardLink {
    pub fn spawn(cfg: WardLinkConfig, ingest: Arc<dyn WardIngest>, rt: &tokio::runtime::Handle) -> io::Result<Arc<Self>>;
    pub fn set_discovered(&self, addrs: Vec<SocketAddrV4>); // replaces the discovered set; the remembered address stays
    pub fn kick(&self);                                     // network change, screen on, alarm: reset back-off, try now
    pub fn sink(self: &Arc<Self>) -> Arc<dyn LocalSink>;    // the ward outbox
    pub fn snapshot(&self) -> WardLinkSnapshot;             // serde: sessions, candidates, last error
    pub fn shutdown(&self);
}

// guardian role (feature "net")
pub struct GuardianLinkConfig { pub guardian_sk: Zeroizing<[u8; 32]>, pub state_dir: PathBuf,
                                pub preferred_port: u16, pub allow_loopback: bool }
impl GuardianLink {
    pub fn start(cfg: GuardianLinkConfig, allowed: Vec<PubKey>, rt: &tokio::runtime::Handle) -> io::Result<Arc<Self>>;
    pub fn port(&self) -> u16;
    pub fn set_allowed(&self, allowed: Vec<PubKey>);          // closes sessions no longer allowed
    pub fn send(&self, recipient: &PubKey, wrap: NostrEvent) -> SendState; // Sent | Queued | Rejected(reason)
    pub fn next_events(&self, timeout: Duration) -> Vec<GuardianEvent>;    // blocking, for a dedicated JNI thread
    pub fn inbox(&self) -> Vec<InboxEntry>;
    pub fn consume(&self, ids: &[EventId]);
    pub fn delivery(&self, ids: &[EventId]) -> BTreeMap<EventId, DeliveryRecord>;
    pub fn sessions(&self) -> Vec<SessionInfo>;
    pub fn shutdown(&self);
}
pub enum GuardianEvent { Wrap { machine: PubKey, wrap: NostrEvent }, Delivery { wrap: EventId, machine: PubKey, state: DeliveryState },
                         Session { machine: PubKey, up: bool } }
pub enum DeliveryState { Queued, Sent, Received, Rejected, Acked(AckResult) } // bridge strings: queued, sent, received, rejected, or the ACK result
```

### `charter-transport` and `charter-spine`

```rust
// charter-transport: CharterTransport
pub fn unwrap_inbound(&self, wrap: &NostrEvent, now: u64) -> Option<(NostrEvent, PubKey)>; // same code path as poll_inner
pub fn set_local_sink(&mut self, sink: Arc<dyn charter_link::LocalSink>);                  // every machine→guardian wrap is offered before relay IO

// charter-spine
pub enum LocalInbound { Clause(ReceivedClause), Grant(ReceivedGrant), Release(NostrEvent, PubKey),
                        UsageSync(NostrEvent), Other { inner_kind: u16 } }
// TransportFacade gains, with a default of None:
fn unwrap_local(&self, wrap: &NostrEvent, now: u64) -> Option<LocalInbound>;
pub struct LocalIngestOptions { pub handle_release: bool }
pub struct LocalIngestOutcome { pub result: AckResult, pub inner_kind: Option<u16>, pub inner_id: Option<EventId>,
                                pub req_id: Option<ReqId>, pub released: bool, pub release: Option<(NostrEvent, PubKey)> }
impl Broker { pub async fn ingest_local(&self, wrap: &NostrEvent, opts: LocalIngestOptions) -> LocalIngestOutcome; }
```

`ingest_local` and the handler stage of `poll_once` take the same internal
async mutex around each handler call, never across relay IO. With
`handle_release: false` (Android) a verified-shape release is handed back in
`release` for the warden's existing `try_apply_releases` path.

### Native library entry points (both ABIs move from 2 to 3)

Ward (`android/jni`, Kotlin `CharterNative`):
`charterLinkDiscovered(addrsJson: String): String` (`{"addrs":["192.168.1.5:47615"]}`),
`charterLinkKick(): String`, `charterLinkState(): String`.

Guardian (`android/jni-guardian`, Kotlin `GuardianNative`):
`guardianLinkStart(stateDir: String, guardianSk: ByteArray, allowedJson: String, preferredPort: Int): String`
(`{"ok":true,"port":N}`), `guardianLinkStop(): String`, `guardianLinkAllow(allowedJson: String): String`,
`guardianLinkSend(recipientHex: String, wrapJson: String): String` (`{"state":"sent"|"queued"|"rejected","reason"?}`),
`guardianLinkNext(timeoutMs: Int): String` (JSON array of events), `guardianLinkInbox(): String`,
`guardianLinkConsume(idsJson: String): String`, `guardianLinkDelivery(idsJson: String): String`,
`guardianLinkSessions(): String`. The Rust side owns the sockets on its own runtime thread; a dedicated
Kotlin thread loops on `guardianLinkNext`. Kotlin does no cryptography.

### Carrier bridge (`window.CharterCarrier`, every addition optional and feature-detected)

| Method | Returns |
| --- | --- |
| `localInfo()` | `{"v":1,"available":bool,"listening":bool,"sessions":[{"machine":hex,"since":unix}]}` |
| `localSend(recipientHex, wrapJson)` | `{"state":"sent"\|"queued"\|"rejected"\|"unavailable"}` |
| `localDelivery(wrapIdsJson)` | `{"<wrapId>":{"state":…,"at":unix,"machine":hex}}` |
| `localInbox()` | `[{"id":wrapId,"machine":hex,"wrap":{…},"receivedAt":unix}]` |
| `localConsume(idsJson)` | nothing |

Roster entries gain `paired: boolean` (absent means paired, for older consoles);
only paired machines are admitted at the handshake.

## Hardware test plan

See [local-network-testing.md](local-network-testing.md) for the exact builds,
commands, network conditions and expected results for each acceptance case.
