// Generate REAL nostr-tools NIP-01 / BIP-340 golden vectors for the Rust
// charter-verify interop suite. Hand-written golden hex is forbidden — these
// are produced by the same library the guardian app (signet-app) uses.
//
// Usage (from repo root, after `npm ci`):
//   node scripts/gen-nostr-vectors.mjs
//
// Output: core/crates/charter-testkit/vectors/nostr/*.json
//
// NOTE: BIP-340 signing uses random aux data, so signatures differ each run.
// The Rust suite *verifies* the committed signature (and *reproduces* the
// deterministic event id) — it never reproduces the signature byte-for-byte.

import { finalizeEvent, getPublicKey, serializeEvent } from "nostr-tools/pure";
import * as nip44 from "nostr-tools/nip44";
import { wrapEvent, unwrapEvent } from "nostr-tools/nip59";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = join(
  __dirname,
  "..",
  "core",
  "crates",
  "charter-testkit",
  "vectors",
  "nostr",
);
mkdirSync(OUT_DIR, { recursive: true });

// Fixed test keys (deterministic, low-byte secrets — valid scalars).
function secret(low) {
  const s = new Uint8Array(32);
  s[31] = low;
  return s;
}
const GUARDIAN_SK = secret(0x11);
const MACHINE_SK = secret(0x22);
const GUARDIAN_PK = getPublicKey(GUARDIAN_SK);
const MACHINE_PK = getPublicKey(MACHINE_SK);

const REQ_ID = "1111111111111111111111111111111111111111111111111111111111111111";
const NONCE = "2222222222222222222222222222222222222222222222222222222222222222";
const TS = 1_700_000_000;
const EXP = 1_700_003_600;

const MARKER = ["t", "charter-device"];

function signWith(sk, template) {
  const ev = finalizeEvent(template, sk);
  return {
    id: ev.id,
    pubkey: ev.pubkey,
    created_at: ev.created_at,
    kind: ev.kind,
    tags: ev.tags,
    content: ev.content,
    sig: ev.sig,
  };
}

function write(name, obj) {
  const path = join(OUT_DIR, name);
  writeFileSync(path, JSON.stringify(obj, null, 2) + "\n");
  console.log("wrote", path);
}

// --- 1. Generic NIP-01 event (event-id KAT) -------------------------------
{
  const event = signWith(MACHINE_SK, {
    kind: 1,
    created_at: TS,
    tags: [["t", "charter-device"]],
    content: "charter nip01 event-id kat",
  });
  write("nip01_event.json", { event, canonical: serializeEvent(event) });
}

// --- 2. Golden GRANT (allow, install.flatpak), signed by the guardian -----
function grant(name, op, decision, params) {
  const payload = { v: 1, op, reqId: REQ_ID, nonce: NONCE, decision, ts: TS, exp: EXP, params };
  const content = JSON.stringify(payload);
  const event = signWith(GUARDIAN_SK, {
    kind: 31112,
    created_at: TS,
    tags: [MARKER, ["d", REQ_ID]],
    content,
  });
  write(name, { event, guardian_pubkey: GUARDIAN_PK, payload, canonical: serializeEvent(event) });
}

grant("grant_install_allow.json", "install.flatpak", "allow", {
  ref: "org.videolan.VLC",
  remote: "flathub",
});
grant("grant_install_deny.json", "install.flatpak", "deny", {
  ref: "org.videolan.VLC",
  remote: "flathub",
});
grant("grant_exec_allow.json", "exec.allow", "allow", {
  name: "SuperTuxKart",
  sha256: "ab".repeat(32), // a valid 64-char lowercase-hex sha256
  size: 123456,
  origin: "https://example.org/stk.AppImage",
});
grant("grant_time_extend_allow.json", "time.extend", "allow", {
  minutesGranted: 30,
  limitHit: "budget",
});

// --- 3. Golden CLAUSE (schedule), signed by the guardian ------------------
{
  const payload = {
    v: 1,
    kind: "schedule",
    issuedAt: TS,
    body: {
      v: 1,
      tz: "Europe/London",
      weekly: { mon: [{ start: "16:00", end: "20:00" }] },
      issuedAt: TS,
    },
  };
  const content = JSON.stringify(payload);
  const event = signWith(GUARDIAN_SK, {
    kind: 31113,
    created_at: TS,
    tags: [MARKER, ["d", "schedule"]],
    content,
  });
  write("clause_schedule.json", {
    event,
    guardian_pubkey: GUARDIAN_PK,
    payload,
    canonical: serializeEvent(event),
  });
}

// --- 3b. Golden SOFTWARE RELEASE (kind 30063), signed by the release key --
{
  const RELEASE_SK = secret(0x33); // throwaway TEST key, never the real one
  const RELEASE_PK = getPublicKey(RELEASE_SK);
  const sha = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c";
  const cert = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";
  const urls = [
    `https://blossom.example/${sha}`,
    `https://mirror.example/${sha}`,
  ];
  const event = signWith(RELEASE_SK, {
    kind: 30063,
    created_at: TS,
    tags: [
      ["d", "charter-apk"],
      ["version", "0.6.9"],
      ["version_code", "40"],
      ["x", sha],
      ["size", "27693181"],
      ["cert", cert],
      ...urls.map((u) => ["url", u]),
    ],
    content: "test release notes",
  });
  write("software_release.json", {
    event,
    release_pubkey: RELEASE_PK,
    expected: {
      channel: "charter-apk",
      versionName: "0.6.9",
      versionCode: 40,
      sha256: sha,
      sizeBytes: 27693181,
      certSha256: cert,
      urls,
    },
    canonical: serializeEvent(event),
  });
}

// --- 4. Keys (test-only, for deterministic cross-impl signing) ------------
write("keys.json", {
  guardian_secret: Buffer.from(GUARDIAN_SK).toString("hex"),
  guardian_pubkey: GUARDIAN_PK,
  machine_secret: Buffer.from(MACHINE_SK).toString("hex"),
  machine_pubkey: MACHINE_PK,
});

// --- 5. NIP-44 v2 KAT vectors (cross-impl parity) -------------------------
{
  const hexBytes = (h) => Uint8Array.from(Buffer.from(h, "hex"));
  const conversationKey = nip44.v2.utils.getConversationKey(MACHINE_SK, GUARDIAN_PK);
  // Plaintext lengths chosen to exercise the padding scheme boundaries.
  const cases = [
    { nonce: "00".repeat(32), plaintext: "a" },
    { nonce: "11".repeat(32), plaintext: "hello charter" },
    { nonce: "22".repeat(32), plaintext: "x".repeat(32) },
    { nonce: "33".repeat(32), plaintext: "y".repeat(33) },
    { nonce: "44".repeat(32), plaintext: "z".repeat(100) },
    { nonce: "55".repeat(32), plaintext: JSON.stringify({ v: 1, op: "time.extend" }) },
  ];
  const valid = cases.map((c) => ({
    nonce: c.nonce,
    plaintext: c.plaintext,
    payload: nip44.v2.encrypt(c.plaintext, conversationKey, hexBytes(c.nonce)),
  }));
  // Invalid payloads that MUST fail to decrypt.
  const goodPayload = valid[1].payload;
  const rawGood = Buffer.from(goodPayload, "base64");
  const flippedMac = Buffer.from(rawGood);
  flippedMac[flippedMac.length - 1] ^= 0x01;
  const wrongVersion = Buffer.from(rawGood);
  wrongVersion[0] = 0x01; // unsupported version
  const invalid = [
    { reason: "flipped_mac", payload: flippedMac.toString("base64") },
    { reason: "wrong_version", payload: wrongVersion.toString("base64") },
    { reason: "too_short", payload: Buffer.from([0x02, 0x00]).toString("base64") },
    { reason: "not_base64", payload: "!!!!not base64!!!!" },
  ];
  write("nip44_vectors.json", {
    sender_secret: Buffer.from(MACHINE_SK).toString("hex"),
    recipient_pubkey: GUARDIAN_PK,
    conversation_key: Buffer.from(conversationKey).toString("hex"),
    valid,
    invalid,
  });
}

// --- 6. NIP-59 gift-wrap fixture (machine -> guardian) --------------------
{
  const rumorTemplate = {
    kind: 31111,
    created_at: TS,
    tags: [MARKER, ["d", REQ_ID]],
    content: JSON.stringify({
      v: 1,
      op: "install.flatpak",
      reqId: REQ_ID,
      nonce: NONCE,
      subject: MACHINE_PK,
      machine: MACHINE_PK,
      ts: TS,
      params: { ref: "org.videolan.VLC", remote: "flathub" },
    }),
  };
  // wrapEvent: machine seals+wraps the rumor for the guardian.
  const wrap = wrapEvent(rumorTemplate, MACHINE_SK, GUARDIAN_PK);
  const rumor = unwrapEvent(wrap, GUARDIAN_SK);
  write("nip59_giftwrap.json", {
    recipient_secret: Buffer.from(GUARDIAN_SK).toString("hex"),
    recipient_pubkey: GUARDIAN_PK,
    sender_pubkey: MACHINE_PK,
    wrap,
    expected_rumor: {
      pubkey: rumor.pubkey,
      kind: rumor.kind,
      content: rumor.content,
    },
  });
}

console.log("done.");
