//! The local link (v1) wire protocol: frame types and codec, the handshake
//! and ACK digests, ACK signing and verification, wrap validation and the
//! peer-address classifier. Always compiled; no sockets here.

use std::net::IpAddr;

use charter_crypto::{schnorr_sign, schnorr_verify, sha256, xonly_pubkey};
use charter_primitives::{kinds, EventId, NostrEvent, PubKey, Sig};
use charter_verify::event::{id_is_consistent, signature_is_valid};
use serde_json::{json, Value};

use crate::AckResult;

/// The local link protocol version carried in HELLO, CHALLENGE and READY.
pub const PROTOCOL_VERSION: u8 = 1;
/// Largest frame body (the JSON after the 4-byte length prefix), in bytes.
pub const MAX_FRAME: usize = 262_144;
/// The DNS-SD service type the guardian carrier registers.
pub const SERVICE_TYPE: &str = "_kintrinsic._tcp";
/// The port the guardian carrier tries first.
pub const PREFERRED_PORT: u16 = 47_615;

const GUARDIAN_DOMAIN: &str = "kintrinsic-local-v1/guardian";
const WARD_DOMAIN: &str = "kintrinsic-local-v1/ward";
const ACK_DOMAIN: &str = "kintrinsic-local-v1/ack";

// ---------------------------------------------------------------------------
// Hex helpers
// ---------------------------------------------------------------------------

pub(crate) fn hex32(b: &[u8; 32]) -> String {
    // EventId is a plain 32-byte hex newtype; borrow its encoder.
    EventId::from_bytes(*b).to_hex()
}

fn unhex32(s: &str) -> Option<[u8; 32]> {
    EventId::from_hex(s).ok().map(|e| *e.as_bytes())
}

// ---------------------------------------------------------------------------
// Digests and proofs
// ---------------------------------------------------------------------------

/// The exact UTF-8 preimage of the guardian proof digest.
pub fn guardian_proof_preimage(cw: &[u8; 32], cg: &[u8; 32], guardian: &PubKey) -> String {
    format!(
        "{GUARDIAN_DOMAIN}\n{}\n{}\n{}",
        hex32(cw),
        hex32(cg),
        guardian.to_hex()
    )
}

/// The exact UTF-8 preimage of the ward proof digest.
pub fn ward_proof_preimage(
    cw: &[u8; 32],
    cg: &[u8; 32],
    guardian: &PubKey,
    machine: &PubKey,
) -> String {
    format!(
        "{WARD_DOMAIN}\n{}\n{}\n{}\n{}",
        hex32(cw),
        hex32(cg),
        guardian.to_hex(),
        machine.to_hex()
    )
}

/// The exact UTF-8 preimage of the ACK digest (`ts` in decimal).
pub fn ack_preimage(
    guardian: &PubKey,
    machine: &PubKey,
    wrap: &EventId,
    result: AckResult,
    ts: u64,
) -> String {
    format!(
        "{ACK_DOMAIN}\n{}\n{}\n{}\n{}\n{ts}",
        guardian.to_hex(),
        machine.to_hex(),
        wrap.to_hex(),
        result.as_str()
    )
}

/// SHA-256 of [`guardian_proof_preimage`]: what the guardian signs in CHALLENGE.
pub fn guardian_proof_digest(cw: &[u8; 32], cg: &[u8; 32], guardian: &PubKey) -> [u8; 32] {
    sha256(guardian_proof_preimage(cw, cg, guardian).as_bytes())
}

/// SHA-256 of [`ward_proof_preimage`]: what the ward signs in AUTH.
pub fn ward_proof_digest(
    cw: &[u8; 32],
    cg: &[u8; 32],
    guardian: &PubKey,
    machine: &PubKey,
) -> [u8; 32] {
    sha256(ward_proof_preimage(cw, cg, guardian, machine).as_bytes())
}

/// SHA-256 of [`ack_preimage`]: what the ward signs in ACK.
pub fn ack_digest(
    guardian: &PubKey,
    machine: &PubKey,
    wrap: &EventId,
    result: AckResult,
    ts: u64,
) -> [u8; 32] {
    sha256(ack_preimage(guardian, machine, wrap, result, ts).as_bytes())
}

/// The x-only public key for a secret, or `None` for an invalid scalar.
pub fn public_key(sk: &[u8; 32]) -> Option<PubKey> {
    xonly_pubkey(sk).ok().map(PubKey::from_bytes)
}

fn sign(sk: &[u8; 32], digest: &[u8; 32]) -> Option<Sig> {
    schnorr_sign(sk, digest).ok().map(Sig::from_bytes)
}

fn verify(pk: &PubKey, digest: &[u8; 32], sig: &Sig) -> bool {
    schnorr_verify(pk.as_bytes(), digest, sig.as_bytes())
}

/// The guardian's CHALLENGE signature (its key `G` is derived from the
/// secret). `None` only for an invalid secret.
pub fn guardian_proof_sign(guardian_sk: &[u8; 32], cw: &[u8; 32], cg: &[u8; 32]) -> Option<Sig> {
    let g = public_key(guardian_sk)?;
    sign(guardian_sk, &guardian_proof_digest(cw, cg, &g))
}

/// Check a CHALLENGE signature by `guardian`.
pub fn guardian_proof_verify(cw: &[u8; 32], cg: &[u8; 32], guardian: &PubKey, sig: &Sig) -> bool {
    verify(guardian, &guardian_proof_digest(cw, cg, guardian), sig)
}

/// The ward's AUTH signature (its machine key `M` is derived from the
/// secret). `None` only for an invalid secret.
pub fn ward_proof_sign(
    machine_sk: &[u8; 32],
    cw: &[u8; 32],
    cg: &[u8; 32],
    guardian: &PubKey,
) -> Option<Sig> {
    let m = public_key(machine_sk)?;
    sign(machine_sk, &ward_proof_digest(cw, cg, guardian, &m))
}

/// Check an AUTH signature by `machine`.
pub fn ward_proof_verify(
    cw: &[u8; 32],
    cg: &[u8; 32],
    guardian: &PubKey,
    machine: &PubKey,
    sig: &Sig,
) -> bool {
    verify(machine, &ward_proof_digest(cw, cg, guardian, machine), sig)
}

// ---------------------------------------------------------------------------
// ACK
// ---------------------------------------------------------------------------

/// A ward's signed report of what it did with one guardian wrap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ack {
    pub wrap: EventId,
    pub result: AckResult,
    pub ts: u64,
    pub sig: Sig,
}

/// Sign an ACK with the ward's machine key. An invalid secret yields an ACK
/// with an all-zero signature, which never verifies.
pub fn sign_ack(
    machine_sk: &[u8; 32],
    guardian: &PubKey,
    wrap: &EventId,
    result: AckResult,
    ts: u64,
) -> Ack {
    let sig = public_key(machine_sk)
        .and_then(|m| sign(machine_sk, &ack_digest(guardian, &m, wrap, result, ts)))
        .unwrap_or(Sig::from_bytes([0u8; 64]));
    Ack {
        wrap: *wrap,
        result,
        ts,
        sig,
    }
}

/// Whether `ack` is signed by `machine` for a session with `guardian`.
pub fn verify_ack(ack: &Ack, guardian: &PubKey, machine: &PubKey) -> bool {
    verify(
        machine,
        &ack_digest(guardian, machine, &ack.wrap, ack.result, ack.ts),
        &ack.sig,
    )
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// One local-link frame. `v` fields carry the protocol version as sent; the
/// session code closes on anything but [`PROTOCOL_VERSION`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// ward → guardian: opens the handshake with the ward's challenge `cw`.
    Hello {
        v: u64,
        challenge: [u8; 32],
    },
    /// guardian → ward: the guardian key, its challenge `cg` and its proof.
    Challenge {
        v: u64,
        guardian: PubKey,
        challenge: [u8; 32],
        sig: Sig,
    },
    /// ward → guardian: the machine key and its proof.
    Auth {
        machine: PubKey,
        sig: Sig,
    },
    /// guardian → ward: the session is authenticated.
    Ready {
        v: u64,
    },
    /// A gift wrap, either direction.
    Event(NostrEvent),
    /// Receipt of an EVENT.
    Ok {
        wrap: EventId,
        accepted: bool,
        reason: String,
    },
    /// ward → guardian: what the ward did with a wrap.
    Ack(Ack),
    Ping(u64),
    Pong(u64),
    /// Any other frame type (ignored, but counted towards rate limits).
    Unknown(String),
}

/// Why a frame body could not be decoded. Any of these closes the connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FrameError {
    #[error("empty frame")]
    Empty,
    #[error("frame of {0} bytes exceeds the maximum")]
    TooLarge(usize),
    #[error("frame is not valid JSON")]
    Json,
    #[error("frame is not an array headed by a type string")]
    NotFrame,
    #[error("malformed {0} frame")]
    Malformed(&'static str),
}

fn frame_value(frame: &Frame) -> Value {
    let v1 = |v: &u64| json!(v);
    match frame {
        Frame::Hello { v, challenge } => {
            json!(["HELLO", {"v": v1(v), "challenge": hex32(challenge)}])
        }
        Frame::Challenge {
            v,
            guardian,
            challenge,
            sig,
        } => json!(["CHALLENGE", {
            "v": v1(v),
            "guardian": guardian.to_hex(),
            "challenge": hex32(challenge),
            "sig": sig.to_hex(),
        }]),
        Frame::Auth { machine, sig } => {
            json!(["AUTH", {"machine": machine.to_hex(), "sig": sig.to_hex()}])
        }
        Frame::Ready { v } => json!(["READY", {"v": v1(v)}]),
        Frame::Event(ev) => json!(["EVENT", ev]),
        Frame::Ok {
            wrap,
            accepted,
            reason,
        } => json!(["OK", wrap.to_hex(), accepted, reason]),
        Frame::Ack(a) => json!(["ACK", {
            "wrap": a.wrap.to_hex(),
            "result": a.result.as_str(),
            "ts": a.ts,
            "sig": a.sig.to_hex(),
        }]),
        Frame::Ping(n) => json!(["PING", n]),
        Frame::Pong(n) => json!(["PONG", n]),
        Frame::Unknown(t) => json!([t]),
    }
}

/// The frame's JSON body, without the length prefix.
pub fn encode_frame_body(frame: &Frame) -> Vec<u8> {
    // Serialising a `Value` built from strings and integers cannot fail.
    serde_json::to_vec(&frame_value(frame)).unwrap_or_default()
}

/// The frame as sent: a 4-byte big-endian length followed by the JSON body.
/// The caller is responsible for not sending a body over [`MAX_FRAME`].
pub fn encode_frame(frame: &Frame) -> Vec<u8> {
    let body = encode_frame_body(frame);
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// Check a 4-byte length prefix: `1..=MAX_FRAME`.
pub fn frame_len(prefix: [u8; 4]) -> Result<usize, FrameError> {
    let n = u32::from_be_bytes(prefix) as usize;
    if n == 0 {
        Err(FrameError::Empty)
    } else if n > MAX_FRAME {
        Err(FrameError::TooLarge(n))
    } else {
        Ok(n)
    }
}

fn obj<'a>(
    arr: &'a [Value],
    name: &'static str,
) -> Result<&'a serde_json::Map<String, Value>, FrameError> {
    if arr.len() != 2 {
        return Err(FrameError::Malformed(name));
    }
    arr[1].as_object().ok_or(FrameError::Malformed(name))
}

fn field_u64(
    o: &serde_json::Map<String, Value>,
    k: &str,
    name: &'static str,
) -> Result<u64, FrameError> {
    o.get(k)
        .and_then(Value::as_u64)
        .ok_or(FrameError::Malformed(name))
}

fn field_str<'a>(
    o: &'a serde_json::Map<String, Value>,
    k: &str,
    name: &'static str,
) -> Result<&'a str, FrameError> {
    o.get(k)
        .and_then(Value::as_str)
        .ok_or(FrameError::Malformed(name))
}

fn field_32(
    o: &serde_json::Map<String, Value>,
    k: &str,
    name: &'static str,
) -> Result<[u8; 32], FrameError> {
    unhex32(field_str(o, k, name)?).ok_or(FrameError::Malformed(name))
}

fn field_sig(o: &serde_json::Map<String, Value>, name: &'static str) -> Result<Sig, FrameError> {
    Sig::from_hex(field_str(o, "sig", name)?).map_err(|_| FrameError::Malformed(name))
}

fn single_u64(arr: &[Value], name: &'static str) -> Result<u64, FrameError> {
    if arr.len() != 2 {
        return Err(FrameError::Malformed(name));
    }
    arr[1].as_u64().ok_or(FrameError::Malformed(name))
}

/// Decode a frame body (without its length prefix).
pub fn decode_frame(body: &[u8]) -> Result<Frame, FrameError> {
    if body.is_empty() {
        return Err(FrameError::Empty);
    }
    if body.len() > MAX_FRAME {
        return Err(FrameError::TooLarge(body.len()));
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| FrameError::Json)?;
    let arr = value.as_array().ok_or(FrameError::NotFrame)?;
    let ty = arr
        .first()
        .and_then(Value::as_str)
        .ok_or(FrameError::NotFrame)?;
    Ok(match ty {
        "HELLO" => {
            let o = obj(arr, "HELLO")?;
            Frame::Hello {
                v: field_u64(o, "v", "HELLO")?,
                challenge: field_32(o, "challenge", "HELLO")?,
            }
        }
        "CHALLENGE" => {
            let o = obj(arr, "CHALLENGE")?;
            Frame::Challenge {
                v: field_u64(o, "v", "CHALLENGE")?,
                guardian: PubKey::from_bytes(field_32(o, "guardian", "CHALLENGE")?),
                challenge: field_32(o, "challenge", "CHALLENGE")?,
                sig: field_sig(o, "CHALLENGE")?,
            }
        }
        "AUTH" => {
            let o = obj(arr, "AUTH")?;
            Frame::Auth {
                machine: PubKey::from_bytes(field_32(o, "machine", "AUTH")?),
                sig: field_sig(o, "AUTH")?,
            }
        }
        "READY" => {
            let o = obj(arr, "READY")?;
            Frame::Ready {
                v: field_u64(o, "v", "READY")?,
            }
        }
        "EVENT" => {
            if arr.len() != 2 {
                return Err(FrameError::Malformed("EVENT"));
            }
            let ev: NostrEvent = serde_json::from_value(arr[1].clone())
                .map_err(|_| FrameError::Malformed("EVENT"))?;
            Frame::Event(ev)
        }
        "OK" => {
            if arr.len() != 4 {
                return Err(FrameError::Malformed("OK"));
            }
            let wrap = arr[1]
                .as_str()
                .and_then(|s| EventId::from_hex(s).ok())
                .ok_or(FrameError::Malformed("OK"))?;
            let accepted = arr[2].as_bool().ok_or(FrameError::Malformed("OK"))?;
            let reason = arr[3].as_str().ok_or(FrameError::Malformed("OK"))?;
            Frame::Ok {
                wrap,
                accepted,
                reason: reason.to_owned(),
            }
        }
        "ACK" => {
            let o = obj(arr, "ACK")?;
            let result = AckResult::parse(field_str(o, "result", "ACK")?)
                .ok_or(FrameError::Malformed("ACK"))?;
            Frame::Ack(Ack {
                wrap: EventId::from_bytes(field_32(o, "wrap", "ACK")?),
                result,
                ts: field_u64(o, "ts", "ACK")?,
                sig: field_sig(o, "ACK")?,
            })
        }
        "PING" => Frame::Ping(single_u64(arr, "PING")?),
        "PONG" => Frame::Pong(single_u64(arr, "PONG")?),
        other => Frame::Unknown(other.to_owned()),
    })
}

// ---------------------------------------------------------------------------
// Wrap validation
// ---------------------------------------------------------------------------

/// Why a wrap may not travel over the link to (or from) a peer.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WrapError {
    #[error("not a gift wrap (kind {0})")]
    Kind(u16),
    #[error("wrap must carry exactly one p tag")]
    RecipientTags,
    #[error("wrap is addressed to someone else")]
    WrongRecipient,
    #[error("wrap id does not match its content")]
    Id,
    #[error("wrap signature is invalid")]
    Signature,
    #[error("wrap does not fit in one frame")]
    TooLarge,
}

/// Check that `wrap` is a NIP-59 gift wrap (kind 1059) whose single `p` tag
/// names `recipient`, whose id matches its content and whose outer signature
/// verifies, and whose EVENT frame body fits within [`MAX_FRAME`].
///
/// This is shape and integrity only; the inner seal and rumor are checked by
/// the same unwrap and verification as a relay copy.
pub fn validate_wrap_for(wrap: &NostrEvent, recipient: &PubKey) -> Result<(), WrapError> {
    if wrap.kind != kinds::GIFT_WRAP {
        return Err(WrapError::Kind(wrap.kind));
    }
    let mut p_tags = wrap
        .tags
        .iter()
        .filter(|t| t.first().map(String::as_str) == Some("p"));
    let p = match (p_tags.next(), p_tags.next()) {
        (Some(p), None) => p,
        _ => return Err(WrapError::RecipientTags),
    };
    let named = p.get(1).and_then(|s| PubKey::from_hex(s).ok());
    if named != Some(*recipient) {
        return Err(WrapError::WrongRecipient);
    }
    if !id_is_consistent(wrap) {
        return Err(WrapError::Id);
    }
    if !signature_is_valid(wrap) {
        return Err(WrapError::Signature);
    }
    if encode_frame_body(&Frame::Event(wrap.clone())).len() > MAX_FRAME {
        return Err(WrapError::TooLarge);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Peer addresses
// ---------------------------------------------------------------------------

/// Whether a peer address may take part in the local link: IPv4 private
/// (10/8, 172.16/12, 192.168/16) or link-local (169.254/16), or loopback when
/// `allow_loopback` is set. IPv4-mapped IPv6 is judged as its IPv4 address;
/// any other IPv6 address is refused (the link is IPv4 only).
pub fn peer_address_allowed(ip: IpAddr, allow_loopback: bool) -> bool {
    let v4 = match ip {
        IpAddr::V4(v4) => v4,
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => v4,
            None => return false,
        },
    };
    v4.is_private() || v4.is_link_local() || (allow_loopback && v4.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    fn sk(n: u8) -> [u8; 32] {
        let mut s = [0u8; 32];
        s[31] = n;
        s
    }

    fn wrap_for(recipient: &PubKey, tags: Option<Vec<Vec<String>>>, content: &str) -> NostrEvent {
        let author = sk(9);
        let mut ev = NostrEvent {
            id: EventId::from_bytes([0; 32]),
            pubkey: public_key(&author).unwrap(),
            created_at: 1_700_000_000,
            kind: kinds::GIFT_WRAP,
            tags: tags.unwrap_or_else(|| vec![vec!["p".into(), recipient.to_hex()]]),
            content: content.into(),
            sig: Sig::from_bytes([0; 64]),
        };
        ev.id = charter_verify::event::event_id(&ev);
        ev.sig = Sig::from_bytes(schnorr_sign(&author, ev.id.as_bytes()).unwrap());
        ev
    }

    #[test]
    fn every_frame_round_trips() {
        let g = public_key(&sk(1)).unwrap();
        let m = public_key(&sk(2)).unwrap();
        let frames = vec![
            Frame::Hello {
                v: 1,
                challenge: [7; 32],
            },
            Frame::Challenge {
                v: 1,
                guardian: g,
                challenge: [8; 32],
                sig: Sig::from_bytes([3; 64]),
            },
            Frame::Auth {
                machine: m,
                sig: Sig::from_bytes([4; 64]),
            },
            Frame::Ready { v: 1 },
            Frame::Event(wrap_for(&m, None, "x")),
            Frame::Ok {
                wrap: EventId::from_bytes([5; 32]),
                accepted: false,
                reason: "invalid".into(),
            },
            Frame::Ack(sign_ack(
                &sk(2),
                &g,
                &EventId::from_bytes([6; 32]),
                AckResult::Superseded,
                42,
            )),
            Frame::Ping(9),
            Frame::Pong(u64::MAX),
        ];
        for f in frames {
            let enc = encode_frame(&f);
            let n = frame_len([enc[0], enc[1], enc[2], enc[3]]).unwrap();
            assert_eq!(n, enc.len() - 4);
            assert_eq!(decode_frame(&enc[4..]).unwrap(), f);
        }
    }

    #[test]
    fn wire_shapes_match_the_contract() {
        let body = encode_frame_body(&Frame::Hello {
            v: 1,
            challenge: [0xab; 32],
        });
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, json!(["HELLO", {"v": 1, "challenge": "ab".repeat(32)}]));
        let body = encode_frame_body(&Frame::Ok {
            wrap: EventId::from_bytes([1; 32]),
            accepted: true,
            reason: String::new(),
        });
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v, json!(["OK", "01".repeat(32), true, ""]));
        let body = encode_frame_body(&Frame::Ping(3));
        assert_eq!(body, br#"["PING",3]"#);
    }

    #[test]
    fn unknown_types_decode_as_unknown() {
        assert_eq!(
            decode_frame(br#"["NOTICE","hi"]"#).unwrap(),
            Frame::Unknown("NOTICE".into())
        );
    }

    #[test]
    fn bad_bodies_are_errors_not_panics() {
        let cases: &[&[u8]] = &[
            b"",
            b"garbage",
            b"{}",
            b"[]",
            b"[1]",
            br#"["HELLO"]"#,
            br#"["HELLO",{"v":1}]"#,
            br#"["HELLO",{"v":1,"challenge":"AB"}]"#,
            br#"["HELLO",{"v":-1,"challenge":"00"}]"#,
            br#"["READY",{"v":"1"}]"#,
            br#"["EVENT",{}]"#,
            br#"["OK","00",true,""]"#,
            br#"["OK"]"#,
            br#"["ACK",{"wrap":"00","result":"applied","ts":1,"sig":"00"}]"#,
            br#"["PING",1.5]"#,
            br#"["PING"]"#,
            br#"["PONG",-3]"#,
            &[0xff, 0xfe, 0x00],
        ];
        for c in cases {
            assert!(decode_frame(c).is_err(), "{:?}", String::from_utf8_lossy(c));
        }
        let mut ack = encode_frame_body(&Frame::Ack(sign_ack(
            &sk(2),
            &public_key(&sk(1)).unwrap(),
            &EventId::from_bytes([6; 32]),
            AckResult::Applied,
            1,
        )));
        // An unknown result spelling is malformed, not a silent default.
        let s = String::from_utf8(ack.clone())
            .unwrap()
            .replace("applied", "Applied");
        ack = s.into_bytes();
        assert_eq!(decode_frame(&ack), Err(FrameError::Malformed("ACK")));
    }

    #[test]
    fn length_prefix_bounds() {
        assert_eq!(frame_len([0, 0, 0, 0]), Err(FrameError::Empty));
        assert_eq!(frame_len([0, 0, 0, 1]), Ok(1));
        assert_eq!(frame_len((MAX_FRAME as u32).to_be_bytes()), Ok(MAX_FRAME));
        assert_eq!(
            frame_len((MAX_FRAME as u32 + 1).to_be_bytes()),
            Err(FrameError::TooLarge(MAX_FRAME + 1))
        );
        assert!(decode_frame(&vec![b' '; MAX_FRAME + 1]).is_err());
    }

    #[test]
    fn proofs_sign_and_verify_and_bind_their_inputs() {
        let (gsk, msk) = (sk(1), sk(2));
        let g = public_key(&gsk).unwrap();
        let m = public_key(&msk).unwrap();
        let (cw, cg) = ([1u8; 32], [2u8; 32]);
        let sg = guardian_proof_sign(&gsk, &cw, &cg).unwrap();
        assert!(guardian_proof_verify(&cw, &cg, &g, &sg));
        assert!(!guardian_proof_verify(&cg, &cw, &g, &sg));
        assert!(!guardian_proof_verify(&cw, &cg, &m, &sg));
        let sw = ward_proof_sign(&msk, &cw, &cg, &g).unwrap();
        assert!(ward_proof_verify(&cw, &cg, &g, &m, &sw));
        assert!(!ward_proof_verify(&cw, &cg, &m, &m, &sw));
        // Domain separation: a guardian proof is never a ward proof.
        assert!(!ward_proof_verify(&cw, &cg, &g, &g, &sg));

        let wrap = EventId::from_bytes([3; 32]);
        let ack = sign_ack(&msk, &g, &wrap, AckResult::Applied, 7);
        assert!(verify_ack(&ack, &g, &m));
        assert!(!verify_ack(&ack, &m, &m));
        assert!(!verify_ack(&ack, &g, &g));
        let mut forged = ack.clone();
        forged.result = AckResult::Failed;
        assert!(!verify_ack(&forged, &g, &m));
        let mut forged = ack;
        forged.ts = 8;
        assert!(!verify_ack(&forged, &g, &m));
        // An invalid secret signs nothing that verifies.
        let bad = sign_ack(&[0u8; 32], &g, &wrap, AckResult::Applied, 7);
        assert!(!verify_ack(&bad, &g, &m));
    }

    #[test]
    fn wrap_validation() {
        let r = public_key(&sk(2)).unwrap();
        let other = public_key(&sk(3)).unwrap();
        assert_eq!(validate_wrap_for(&wrap_for(&r, None, "x"), &r), Ok(()));
        assert_eq!(
            validate_wrap_for(&wrap_for(&r, None, "x"), &other),
            Err(WrapError::WrongRecipient)
        );
        let two = vec![
            vec!["p".into(), r.to_hex()],
            vec!["p".into(), other.to_hex()],
        ];
        assert_eq!(
            validate_wrap_for(&wrap_for(&r, Some(two), "x"), &r),
            Err(WrapError::RecipientTags)
        );
        assert_eq!(
            validate_wrap_for(&wrap_for(&r, Some(vec![]), "x"), &r),
            Err(WrapError::RecipientTags)
        );
        assert_eq!(
            validate_wrap_for(&wrap_for(&r, Some(vec![vec!["p".into()]]), "x"), &r),
            Err(WrapError::WrongRecipient)
        );
        let mut kind = wrap_for(&r, None, "x");
        kind.kind = 1;
        assert_eq!(validate_wrap_for(&kind, &r), Err(WrapError::Kind(1)));
        let mut id = wrap_for(&r, None, "x");
        id.content = "y".into();
        assert_eq!(validate_wrap_for(&id, &r), Err(WrapError::Id));
        let mut sig = wrap_for(&r, None, "x");
        let mut b = *sig.sig.as_bytes();
        b[0] ^= 1;
        sig.sig = Sig::from_bytes(b);
        assert_eq!(validate_wrap_for(&sig, &r), Err(WrapError::Signature));
        let big = wrap_for(&r, None, &"a".repeat(MAX_FRAME));
        assert_eq!(validate_wrap_for(&big, &r), Err(WrapError::TooLarge));
    }

    #[test]
    fn address_classifier() {
        let allowed = [
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 255, 255, 255),
            Ipv4Addr::new(172, 16, 0, 1),
            Ipv4Addr::new(172, 31, 255, 254),
            Ipv4Addr::new(192, 168, 1, 5),
            Ipv4Addr::new(169, 254, 3, 4),
        ];
        for ip in allowed {
            assert!(peer_address_allowed(IpAddr::V4(ip), false), "{ip}");
        }
        let refused = [
            Ipv4Addr::new(8, 8, 8, 8),
            Ipv4Addr::new(172, 15, 0, 1),
            Ipv4Addr::new(172, 32, 0, 1),
            Ipv4Addr::new(192, 169, 0, 1),
            Ipv4Addr::new(100, 64, 0, 1),
            Ipv4Addr::new(0, 0, 0, 0),
            Ipv4Addr::new(255, 255, 255, 255),
            Ipv4Addr::new(224, 0, 0, 251),
        ];
        for ip in refused {
            assert!(!peer_address_allowed(IpAddr::V4(ip), true), "{ip}");
        }
        let lo = IpAddr::V4(Ipv4Addr::LOCALHOST);
        assert!(!peer_address_allowed(lo, false));
        assert!(peer_address_allowed(lo, true));
        let mapped = IpAddr::V6(Ipv4Addr::new(192, 168, 0, 2).to_ipv6_mapped());
        assert!(peer_address_allowed(mapped, false));
        assert!(!peer_address_allowed(IpAddr::V6(Ipv6Addr::LOCALHOST), true));
        assert!(!peer_address_allowed(
            IpAddr::V6("fe80::1".parse().unwrap()),
            true
        ));
    }
}
