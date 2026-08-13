//! A live test guardian for on-metal rounds: mints a throwaway guardian key,
//! prints its `bunker://` pairing link, signs schedule clauses, gift-wraps
//! them to a machine over the REAL relay, and watches the machine's STATUS
//! heartbeat — the guardian half of the §6.5 live round, runnable headless.
//!
//!   cargo run --example live_guardian --features mock -- uri
//!   cargo run --example live_guardian --features mock -- send-clause <machine_pk_hex> <open|paused|budget1>
//!   cargo run --example live_guardian --features mock -- watch-status
//!   cargo run --example live_guardian --features mock -- approve   # grant every pending ask 30 min
//!   cargo run --example live_guardian --features mock -- deny      # deny every pending ask (signed 0-min grant)
//!   cargo run --example live_guardian --features mock -- install-approve <package_name> <cert_sha256_hex>
//!   cargo run --example live_guardian --features mock -- release <machine_pk_hex>  # parent-gated unpair
//!
//! The key persists in ./target/live-guardian.key (a THROWAWAY — never a real
//! guardian). Relay: wss://relay.trotters.cc (the deployed default).

use std::time::{SystemTime, UNIX_EPOCH};

use charter_primitives::{kinds, PubKey};
use charter_sys::relay::{Filter, RealRelayTransport, RelayTransport};
use charter_transport::nip59::{self, Rumor, WrapRandomness};
use charter_verify::test_support::{ClauseBuilder, TestGuardian};

const RELAY: &str = "wss://relay.trotters.cc";
const KEY_PATH: &str = "target/live-guardian.key";

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_secs()
}

fn rand32() -> [u8; 32] {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("entropy");
    b
}

/// Load or mint the throwaway guardian secret.
fn guardian_sk() -> [u8; 32] {
    if let Ok(hex) = std::fs::read_to_string(KEY_PATH) {
        let hex = hex.trim();
        if hex.len() == 64 {
            let mut b = [0u8; 32];
            for i in 0..32 {
                b[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex key");
            }
            return b;
        }
    }
    let sk = rand32();
    let hex: String = sk.iter().map(|b| format!("{b:02x}")).collect();
    std::fs::write(KEY_PATH, hex).expect("persist throwaway key");
    sk
}

fn guardian_pk(sk: &[u8; 32]) -> PubKey {
    PubKey::from_bytes(charter_crypto::xonly_pubkey(sk).expect("valid sk"))
}

fn budget_body(daily_minutes: u32, issued_at: u64) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "tz": "Europe/London", "dailyMinutes": daily_minutes,
        "issuedAt": issued_at
    })
}

fn schedule_body(paused: bool, issued_at: u64) -> serde_json::Value {
    if paused {
        serde_json::json!({
            "v": 1, "tz": "Europe/London", "paused": true,
            "weekly": {}, "issuedAt": issued_at
        })
    } else {
        // A generous open window every day — configured, but unlocked now.
        let win = [{ serde_json::json!({ "start": "06:00", "end": "22:00" }) }];
        serde_json::json!({
            "v": 1, "tz": "Europe/London",
            "weekly": {
                "mon": win, "tue": win, "wed": win, "thu": win,
                "fri": win, "sat": win, "sun": win
            },
            "issuedAt": issued_at
        })
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let sk = guardian_sk();
    let pk = guardian_pk(&sk);

    match args.first().map(String::as_str) {
        Some("uri") => {
            println!("guardian pubkey: {}", pk.to_hex());
            println!("bunker://{}?relay={RELAY}&kind=charter", pk.to_hex());
        }
        Some("send-clause") => {
            let machine = PubKey::from_hex(args.get(1).expect("machine_pk_hex arg"))
                .expect("valid machine pubkey hex");
            let mode = args.get(2).map(String::as_str).unwrap_or("open");
            let issued_at = now();

            // Sign the inner CLAUSE with the throwaway guardian. TestGuardian
            // signs with seed 0x11 by default — rebind it to OUR key.
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let ev = match mode {
                m if m.starts_with("budget") => ClauseBuilder::budget(issued_at)
                    .body(budget_body(
                        m.trim_start_matches("budget").parse().unwrap_or(1),
                        issued_at,
                    ))
                    .build_signed_by(&guardian.signer),
                paused_or_open => ClauseBuilder::schedule(issued_at)
                    .body(schedule_body(paused_or_open == "paused", issued_at))
                    .build_signed_by(&guardian.signer),
            };

            let rumor = Rumor::from_signed_event(&ev);
            let wrap = nip59::wrap(
                &rumor,
                &sk,
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: rand32(),
                    seal_nonce: rand32(),
                    wrap_nonce: rand32(),
                    seal_created_at: now(),
                    wrap_created_at: now(),
                },
            )
            .expect("wrap");

            let relay = RealRelayTransport::default();
            let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
            println!(
                "published {} clause (issuedAt {issued_at}) to {machine:?}: {outcomes:?}",
                mode,
                machine = machine.to_hex()
            );
        }
        // Publish a web-content clause: `on` = blocklist blocking youtube.com +
        // allowing wikipedia.org (YouTube strict, SafeSearch on); `off` = revoked
        // (filtering lifted). For the Linux web-policy on-metal round.
        Some("send-content") => {
            let machine = PubKey::from_hex(args.get(1).expect("machine_pk_hex arg"))
                .expect("valid machine pubkey hex");
            let mode = args.get(2).map(String::as_str).unwrap_or("on");
            let issued_at = now();
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let body = if mode == "off" {
                serde_json::json!({
                    "v": 1, "posture": "blocklist", "ageTier": "older",
                    "revoked": true, "issuedAt": issued_at
                })
            } else {
                serde_json::json!({
                    "v": 1, "posture": "blocklist", "ageTier": "older",
                    "parentAllow": ["wikipedia.org"], "parentDeny": ["youtube.com"],
                    "safeSearch": true, "youtubeRestrict": "strict", "issuedAt": issued_at
                })
            };
            let ev = ClauseBuilder::content(issued_at)
                .body(body)
                .build_signed_by(&guardian.signer);
            let wrap = nip59::wrap(
                &Rumor::from_signed_event(&ev),
                &sk,
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: rand32(),
                    seal_nonce: rand32(),
                    wrap_nonce: rand32(),
                    seal_created_at: now(),
                    wrap_created_at: now(),
                },
            )
            .expect("wrap");
            let relay = RealRelayTransport::default();
            let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
            println!(
                "published content ({mode}) to {}: {outcomes:?}",
                machine.to_hex()
            );
        }
        // Approve (or deny) every pending ask visible on the relay: unwrap
        // kind-31111 REQUESTs addressed to this guardian, sign a GRANT echoing
        // reqId/nonce/limitHit — allow grants 30 minutes; deny is the signed
        // 0-minute grant (the device treats it as an answered "not now").
        Some(cmd @ ("approve" | "deny")) => {
            use charter_verify::test_support::GrantBuilder;
            let relay = RealRelayTransport::default();
            let filter = Filter {
                kinds: vec![kinds::GIFT_WRAP],
                p_tags: vec![pk],
                since: Some(now() - 60 * 60),
                ..Default::default()
            };
            let wraps = relay
                .query(&[RELAY.to_string()], filter)
                .await
                .expect("query");
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let mut approved = 0;
            for w in wraps {
                let Ok((rumor, author)) =
                    nip59::unwrap_with_author(&w, &sk, now(), nip59::MAX_JITTER_SECS)
                else {
                    continue;
                };
                if rumor.kind != kinds::CHARTER_DEVICE_REQUEST {
                    continue;
                }
                let Ok(req) = charter_proto::RequestPayload::from_json(&rumor.content) else {
                    continue;
                };
                let limit_hit = req
                    .params
                    .get("limitHit")
                    .and_then(|v| v.as_str())
                    .unwrap_or("budget")
                    .to_string();
                println!(
                    "ask from {}: {} ({} min, {limit_hit}) — answering",
                    author.to_hex(),
                    req.req_id.to_hex(),
                    req.params
                        .get("minutesRequested")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0),
                );
                let g_now = now();
                let minutes = if cmd == "deny" { 0 } else { 30 };
                let mut gb = GrantBuilder::install_allow(req.req_id, req.nonce)
                    .op(charter_proto::OpType::TimeExtend)
                    .params(serde_json::json!({"minutesGranted": minutes, "limitHit": limit_hit}))
                    .ts(g_now)
                    .exp(g_now + 240);
                if cmd == "deny" {
                    gb = gb.deny();
                }
                gb.created_at = g_now;
                let grant = gb.build(&guardian);
                let wrap = nip59::wrap(
                    &Rumor::from_signed_event(&grant),
                    &sk,
                    req.machine.as_bytes(),
                    &WrapRandomness {
                        ephemeral_secret: rand32(),
                        seal_nonce: rand32(),
                        wrap_nonce: rand32(),
                        seal_created_at: g_now,
                        wrap_created_at: g_now,
                    },
                )
                .expect("wrap");
                let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
                println!("  grant published: {outcomes:?}");
                approved += 1;
            }
            println!("{approved} ask(s) granted");
        }
        // Approve a pending install.apk ask: unwrap kind-31111 REQUESTs for
        // this guardian, find the one for <package_name>, and sign an InstallApk
        // GRANT that pins <cert_sha256> (the exact provenance the device
        // re-verifies against the staged archive BEFORE it commits — §3.5). Pass
        // a deliberately WRONG digest to prove the device refuses the swap.
        //   install-approve <package_name> <cert_sha256_hex>
        Some("install-approve") => {
            use charter_verify::test_support::GrantBuilder;
            let pkg = args.get(1).expect("package_name arg").to_string();
            let cert = args
                .get(2)
                .expect("cert_sha256_hex arg (64 hex)")
                .to_lowercase();
            let relay = RealRelayTransport::default();
            let filter = Filter {
                kinds: vec![kinds::GIFT_WRAP],
                p_tags: vec![pk],
                since: Some(now() - 60 * 60),
                ..Default::default()
            };
            let wraps = relay
                .query(&[RELAY.to_string()], filter)
                .await
                .expect("query");
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let mut granted = 0;
            for w in wraps {
                let Ok((rumor, author)) =
                    nip59::unwrap_with_author(&w, &sk, now(), nip59::MAX_JITTER_SECS)
                else {
                    continue;
                };
                if rumor.kind != kinds::CHARTER_DEVICE_REQUEST {
                    continue;
                }
                let Ok(req) = charter_proto::RequestPayload::from_json(&rumor.content) else {
                    continue;
                };
                if req.op != charter_proto::OpType::InstallApk {
                    continue;
                }
                let req_pkg = req
                    .params
                    .get("packageName")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if req_pkg != pkg {
                    continue;
                }
                println!(
                    "install ask from {}: {} for {req_pkg} — approving (pin {cert})",
                    author.to_hex(),
                    req.req_id.to_hex(),
                );
                let g_now = now();
                let mut gb = GrantBuilder::install_allow(req.req_id, req.nonce)
                    .op(charter_proto::OpType::InstallApk)
                    .params(serde_json::json!({
                        "packageName": pkg,
                        "signerCertSha256": cert,
                        "source": "staged"
                    }))
                    .ts(g_now)
                    .exp(g_now + 240);
                gb.created_at = g_now;
                let grant = gb.build(&guardian);
                let wrap = nip59::wrap(
                    &Rumor::from_signed_event(&grant),
                    &sk,
                    req.machine.as_bytes(),
                    &WrapRandomness {
                        ephemeral_secret: rand32(),
                        seal_nonce: rand32(),
                        wrap_nonce: rand32(),
                        seal_created_at: g_now,
                        wrap_created_at: g_now,
                    },
                )
                .expect("wrap");
                let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
                println!("  install grant published: {outcomes:?}");
                granted += 1;
            }
            println!("{granted} install ask(s) granted for {pkg}");
        }
        // Publish a per-app policy clause: `send-apps <machine> <package>` blocks
        // that package (blocklist); `send-apps <machine> off` clears it. The
        // device suspends the blocked app even during allowed screen time (D3).
        Some("send-apps") => {
            let machine = PubKey::from_hex(args.get(1).expect("machine_pk_hex arg"))
                .expect("valid machine pubkey hex");
            let target = args.get(2).map(String::as_str).unwrap_or("off");
            let issued_at = now();
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let body = if target == "off" {
                serde_json::json!({
                    "v": 1, "posture": "blocklist", "blocked": [], "issuedAt": issued_at
                })
            } else {
                serde_json::json!({
                    "v": 1, "posture": "blocklist", "blocked": [target], "issuedAt": issued_at
                })
            };
            let ev = ClauseBuilder::apps(issued_at)
                .body(body)
                .build_signed_by(&guardian.signer);
            let wrap = nip59::wrap(
                &Rumor::from_signed_event(&ev),
                &sk,
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: rand32(),
                    seal_nonce: rand32(),
                    wrap_nonce: rand32(),
                    seal_created_at: now(),
                    wrap_created_at: now(),
                },
            )
            .expect("wrap");
            let relay = RealRelayTransport::default();
            let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
            println!(
                "published apps ({target}) to {}: {outcomes:?}",
                machine.to_hex()
            );
        }
        // Publish a lifeline clause (spec D9): `send-lifeline <machine_pk>`
        // ships two test numbers ("Mum"/"Dad"); `send-lifeline <machine_pk> off`
        // ships an empty... no — empty is invalid (fail-closed); `off` ships a
        // single obviously-test number. For the lock-screen call-button round.
        Some("send-lifeline") => {
            let machine = PubKey::from_hex(args.get(1).expect("machine_pk_hex arg"))
                .expect("valid machine pubkey hex");
            let issued_at = now();
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let body = serde_json::json!({
                "v": 1, "issuedAt": issued_at,
                "numbers": [
                    {"label": "Mum", "number": "07700 900123"},
                    {"label": "Dad", "number": "+44 7700 900456"}
                ]
            });
            // No `.subject(...)`: like send-clause, the absent subject routes
            // to the pairing's sole ward (the machine pk is NOT the subject).
            let ev = ClauseBuilder {
                kind: charter_proto::ClauseKind::Lifeline,
                issued_at,
                body: serde_json::json!({}),
                created_at: issued_at,
                subject: None,
            }
            .body(body)
            .build_signed_by(&guardian.signer);
            let wrap = nip59::wrap(
                &Rumor::from_signed_event(&ev),
                &sk,
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: rand32(),
                    seal_nonce: rand32(),
                    wrap_nonce: rand32(),
                    seal_created_at: now(),
                    wrap_created_at: now(),
                },
            )
            .expect("wrap");
            let relay = RealRelayTransport::default();
            let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
            println!("published lifeline to {}: {outcomes:?}", machine.to_hex());
        }
        // Publish a guardian-signed RELEASE to a machine — the parent-gated
        // unpair. The device drops its pairing + all enforcement.
        Some("release") => {
            let machine = PubKey::from_hex(args.get(1).expect("machine_pk_hex arg"))
                .expect("valid machine pubkey hex");
            let issued_at = now();
            let payload = charter_proto::ReleasePayload {
                v: 1,
                machine,
                issued_at,
            };
            let guardian = TestGuardian {
                signer: charter_sys::signer::SeedSigner::from_secret(sk),
            };
            let ev = charter_verify::test_support::sign_event(
                &guardian.signer,
                kinds::CHARTER_DEVICE_RELEASE,
                issued_at,
                vec![kinds::marker_tag()],
                payload.to_json(),
            );
            let wrap = nip59::wrap(
                &Rumor::from_signed_event(&ev),
                &sk,
                machine.as_bytes(),
                &WrapRandomness {
                    ephemeral_secret: rand32(),
                    seal_nonce: rand32(),
                    wrap_nonce: rand32(),
                    seal_created_at: issued_at,
                    wrap_created_at: issued_at,
                },
            )
            .expect("wrap");
            let relay = RealRelayTransport::default();
            let outcomes = relay.publish(&[RELAY.to_string()], wrap).await;
            println!(
                "released {machine:?}: {outcomes:?}",
                machine = machine.to_hex()
            );
        }
        Some("watch-status") => {
            let relay = RealRelayTransport::default();
            let filter = Filter {
                kinds: vec![kinds::GIFT_WRAP],
                p_tags: vec![pk],
                since: Some(now() - 15 * 60),
                ..Default::default()
            };
            let wraps = relay
                .query(&[RELAY.to_string()], filter)
                .await
                .expect("query");
            println!(
                "{} wraps addressed to the guardian in the last 15m",
                wraps.len()
            );
            for w in wraps {
                if let Ok((rumor, author)) =
                    nip59::unwrap_with_author(&w, &sk, now(), nip59::MAX_JITTER_SECS)
                {
                    if rumor.kind == kinds::CHARTER_DEVICE_STATUS {
                        println!("STATUS from {}: {}", author.to_hex(), rumor.content);
                    } else {
                        println!("(kind {} from {})", rumor.kind, author.to_hex());
                    }
                }
            }
        }
        _ => eprintln!(
            "usage: live_guardian <uri | send-clause <machine_pk> <open|paused|budgetN> | \
             approve | deny | install-approve <package_name> <cert_sha256> | \
             release <machine_pk> | watch-status>"
        ),
    }
}
