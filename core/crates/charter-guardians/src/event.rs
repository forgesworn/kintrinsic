//! NIP-01 event shape and signature checks over plain JSON values.
//!
//! Inputs arrive as `serde_json::Value` because a statement's validity starts
//! with its shape (`bad_event` comes before `bad_signature`), and because an
//! approval also arrives nested inside other content. The id is recomputed
//! from the canonical NIP-01 array and the BIP-340 signature is checked with
//! the workspace's one crypto backend (`charter-crypto`).

use serde_json::Value;

/// The largest integer a JS reader holds exactly (2^53 − 1).
pub const MAX_SAFE_INTEGER: i64 = 9_007_199_254_740_991;

/// The `["t","charter-device"]` marker every Charter inner event carries.
pub const MARKER: [&str; 2] = ["t", "charter-device"];

/// A NIP-01 event that passed the shape check (not yet verified).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub id: String,
    pub pubkey: String,
    pub created_at: i64,
    pub kind: i64,
    pub tags: Vec<Vec<String>>,
    pub content: String,
    pub sig: String,
}

/// `s` is exactly `n` lowercase hex characters.
pub fn is_lower_hex(s: &str, n: usize) -> bool {
    s.len() == n && s.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

/// 64 lowercase hex (a key or event id).
pub fn is_hex64(s: &str) -> bool {
    is_lower_hex(s, 64)
}

/// 32 lowercase hex (a token).
pub fn is_hex32(s: &str) -> bool {
    is_lower_hex(s, 32)
}

/// A JSON number that is an integer a JS reader holds exactly (any sign).
pub fn safe_integer(v: &Value) -> Option<i64> {
    let Value::Number(n) = v else { return None };
    if let Some(i) = n.as_i64() {
        return (-MAX_SAFE_INTEGER..=MAX_SAFE_INTEGER)
            .contains(&i)
            .then_some(i);
    }
    if n.as_u64().is_some() {
        return None; // above i64::MAX
    }
    let f = n.as_f64()?;
    (f.fract() == 0.0 && f.abs() <= MAX_SAFE_INTEGER as f64).then_some(f as i64)
}

/// Unix seconds as the contract reads them: a non-negative safe integer.
pub fn unix_seconds(v: &Value) -> Option<i64> {
    safe_integer(v).filter(|i| *i >= 0)
}

/// The JSON value is the number 1 (`v`).
pub fn is_one(v: &Value) -> bool {
    v.is_number() && v.as_f64() == Some(1.0)
}

fn hex_str<'a>(o: &'a serde_json::Map<String, Value>, k: &str, n: usize) -> Option<&'a str> {
    o.get(k)?.as_str().filter(|s| is_lower_hex(s, n))
}

/// The NIP-01 shape check: an object with the seven fields of the right
/// types. Extra members are ignored.
pub fn event_shape(v: &Value) -> Option<Event> {
    let o = v.as_object()?;
    let id = hex_str(o, "id", 64)?;
    let pubkey = hex_str(o, "pubkey", 64)?;
    let sig = hex_str(o, "sig", 128)?;
    let kind = safe_integer(o.get("kind")?)?;
    let created_at = safe_integer(o.get("created_at")?)?;
    let content = o.get("content")?.as_str()?;
    let tags = o
        .get("tags")?
        .as_array()?
        .iter()
        .map(|t| {
            t.as_array()?
                .iter()
                .map(|x| x.as_str().map(str::to_owned))
                .collect::<Option<Vec<String>>>()
        })
        .collect::<Option<Vec<Vec<String>>>>()?;
    Some(Event {
        id: id.to_owned(),
        pubkey: pubkey.to_owned(),
        created_at,
        kind,
        tags,
        content: content.to_owned(),
        sig: sig.to_owned(),
    })
}

fn decode_hex<const N: usize>(s: &str) -> Option<[u8; N]> {
    if s.len() != N * 2 {
        return None;
    }
    let mut out = [0u8; N];
    for (i, chunk) in s.as_bytes().chunks(2).enumerate() {
        let hi = (chunk[0] as char).to_digit(16)?;
        let lo = (chunk[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

/// Decode lowercase or uppercase hex of any even length.
pub fn decode_hex_vec(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    s.as_bytes()
        .chunks(2)
        .map(|c| {
            let hi = (c[0] as char).to_digit(16)?;
            let lo = (c[1] as char).to_digit(16)?;
            Some((hi * 16 + lo) as u8)
        })
        .collect()
}

/// Lowercase hex.
pub fn encode_hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// The NIP-01 event hash: sha256 of `[0,pubkey,created_at,kind,tags,content]`.
pub fn event_hash(ev: &Event) -> [u8; 32] {
    let arr = serde_json::json!([0, ev.pubkey, ev.created_at, ev.kind, ev.tags, ev.content]);
    let s = serde_json::to_string(&arr).expect("canonical array serialises");
    charter_crypto::sha256(s.as_bytes())
}

/// The `id` is the NIP-01 hash and the BIP-340 `sig` verifies for `pubkey`.
pub fn verify_event(ev: &Event) -> bool {
    let hash = event_hash(ev);
    if encode_hex(&hash) != ev.id {
        return false;
    }
    match (decode_hex::<32>(&ev.pubkey), decode_hex::<64>(&ev.sig)) {
        (Some(pk), Some(sig)) => charter_crypto::schnorr_verify(&pk, &hash, &sig),
        _ => false,
    }
}

/// The event carries the `["t","charter-device"]` marker.
pub fn has_marker(ev: &Event) -> bool {
    ev.tags
        .iter()
        .any(|t| t.len() >= 2 && t[0] == MARKER[0] && t[1] == MARKER[1])
}

/// The values of every tag named `name` (a tag with no value counts as "").
pub fn tag_values<'a>(ev: &'a Event, name: &str) -> Vec<&'a str> {
    ev.tags
        .iter()
        .filter(|t| t.first().map(String::as_str) == Some(name))
        .map(|t| t.get(1).map(String::as_str).unwrap_or(""))
        .collect()
}

/// A shape-checked and verified event, or `None`.
pub fn verified_event(v: &Value) -> Option<Event> {
    event_shape(v).filter(verify_event)
}

/// Constant-time comparison of two strings (length is not secret).
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}
