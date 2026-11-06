// Generate the golden vectors for *Guardian roots and working keys (v1)*
// (spec/contract.md) from the TS reference in src/guardians. The Rust ward
// (core/, linux/) and the Android ward are tested against these.
//
// node cannot import the .ts source, so this imports from dist/ — run
// `npm run build` first.
//
// Usage (from repo root):
//   npm run build && node scripts/gen-guardian-vectors.mjs
//
// Output: core/crates/charter-testkit/vectors/guardians/
//   statements.json  individually signed statements, valid and invalid
//   scenarios.json   ordered ward steps with expected outcome and state
//   giftwrap.json    full NIP-59 wraps exercising the seal exceptions
//   roster.json      guardian-side family roster merge
//   pin.json         guardian PIN scrypt vectors and the backoff table
// The schema of each file is fixed in that directory's README.md.
//
// Byte-for-byte determinism, as gen-nostr-vectors.mjs: every key, BIP-340
// aux-rand, NIP-44 nonce, NIP-59 ephemeral key and timestamp is derived from
// a fixed, labelled seed via SHA-256. No wall clock, no CSPRNG. Each step's
// expected outcome is also asserted here against the intent written next to
// it, so a change in the reference that alters a vector fails loudly.
import { getPublicKey, getEventHash } from "nostr-tools/pure";
import * as nip44 from "nostr-tools/nip44";
import { schnorr } from "@noble/curves/secp256k1.js";
import { sha256 } from "@noble/hashes/sha2.js";
import { scrypt } from "@noble/hashes/scrypt.js";
import { bytesToHex, hexToBytes, concatBytes } from "@noble/hashes/utils.js";
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import * as G from "../dist/guardians/index.js";

const te = new TextEncoder();

function seedBytes(label) {
  return sha256(te.encode(`kintrinsic-testkit-vectors:${label}`));
}

function seedSecretKey(label) {
  const seed48 = concatBytes(seedBytes(`${label}:0`), seedBytes(`${label}:1`)).slice(0, 48);
  return schnorr.utils.randomSecretKey(seed48);
}

const __dirname = dirname(fileURLToPath(import.meta.url));
const OUT_DIR = join(__dirname, "..", "core", "crates", "charter-testkit", "vectors", "guardians");
mkdirSync(OUT_DIR, { recursive: true });

function write(name, obj) {
  const path = join(OUT_DIR, name);
  writeFileSync(path, JSON.stringify(obj, null, 2) + "\n");
  console.log("wrote", path);
}

// Deterministic signing: aux-rand is seeded from the event id, so every event
// gets distinct, reproducible aux data.
function signWith(sk, template) {
  const ev = {
    kind: template.kind,
    created_at: template.created_at,
    tags: template.tags.map((t) => [...t]),
    content: template.content,
    pubkey: getPublicKey(sk),
  };
  ev.id = getEventHash(ev);
  const sig = bytesToHex(schnorr.sign(hexToBytes(ev.id), sk, seedBytes(`guardians.aux:${ev.id}`)));
  return { id: ev.id, pubkey: ev.pubkey, created_at: ev.created_at, kind: ev.kind, tags: ev.tags, content: ev.content, sig };
}

// --- Keys ------------------------------------------------------------------
const KEYS = {};
function k(name) {
  if (!KEYS[name]) {
    const sk = seedSecretKey(`guardians.key.${name}`);
    KEYS[name] = { sk, pk: getPublicKey(sk), name };
  }
  return KEYS[name];
}
const R1 = k("R1"), R2 = k("R2"), R3 = k("R3"), R4 = k("R4"), R5 = k("R5");
const W1 = k("W1"), W1b = k("W1b"), W2 = k("W2"), W3 = k("W3"), W4 = k("W4");
const X = k("X");
const WARD = k("WARD");
const keysJson = () =>
  Object.fromEntries(
    Object.values(KEYS)
      .sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))
      .map((x) => [x.name, { secret: bytesToHex(x.sk), pubkey: x.pk }]),
  );

const T0 = 1_700_000_000;
const DAY = 86_400;
const H72 = 72 * 3600;
const RELAYS = ["wss://relay.one.example", "wss://relay.two.example"];
const tokenHex = (label) => bytesToHex(seedBytes(`guardians.token.${label}`).slice(0, 16));
// A CLAUSE's event id: the clause body is out of scope, so the id is derived
// from what identifies the clause in a step. The same arguments give the same
// id, so repeating a step is a re-delivery of that clause.
const clauseId = (author, subject, kind, issuedAt) =>
  bytesToHex(seedBytes(`guardians.clause:${author.name}:${subject}:${kind}:${issuedAt}`));
const MARKER = ["t", "charter-device"];

const st = {
  approval: (r, w, o) => signWith(r.sk, G.approvalTemplate(w.pk, o)),
  withdrawal: (r, w, o) => signWith(r.sk, G.withdrawalTemplate(w.pk, o)),
  coguardian: (r, r2, o) => signWith(r.sk, G.coGuardianTemplate(r2.pk, o)),
  removal: (by, r2, o) => signWith(by.sk, G.rootRemovalTemplate(r2.pk, o)),
  move: (r, r2, o) => signWith(r.sk, G.rootMoveTemplate(r2.pk, o)),
  cancel: (by, d, o) => signWith(by.sk, G.cancelTemplate(d, o)),
  objection: (by, d, remover, o) => signWith(by.sk, G.objectionTemplate(d, remover.pk, o)),
  raw: (by, tags, content, kind = G.CHARTER_GUARDIAN_STATEMENT, created_at = T0) =>
    signWith(by.sk, { kind, created_at, tags, content }),
};
const dG = (x) => `kintrinsic/guardian/${x.pk}`;
const dU = (x) => `kintrinsic/unguardian/${x.pk}`;
const dM = (x) => `kintrinsic/root/${x.pk}`;
const confirmEv = (w, statementId, token, now, approval) =>
  signWith(w.sk, G.presenceConfirmTemplate({ token, statement: statementId, approval, createdAt: now }));
const offerEv = (sealer, token, ts, approval) =>
  signWith(sealer.sk, G.pairOfferTemplate({ token, relays: RELAYS, ts, approval }));

// --- statements.json ---------------------------------------------------------
{
  const valid = [];
  const invalid = [];
  const addValid = (name, event) => {
    const r = G.parseStatement(event);
    if (!r.ok) throw new Error(`statement ${name} expected valid, got ${r.reason}`);
    const s = r.statement;
    valid.push({
      name,
      event,
      expected: {
        type: s.type, d: s.d, p: s.p, key: s.key, target: s.target, signer: s.signer,
        issuedAt: s.issuedAt, label: s.label, expiresAt: s.expiresAt,
      },
    });
  };
  const addInvalid = (name, event, reason) => {
    const r = G.parseStatement(event);
    if (r.ok || r.reason !== reason) throw new Error(`statement ${name}: want ${reason}, got ${r.ok ? "valid" : r.reason}`);
    invalid.push({ name, event, reason });
  };
  const o = { issuedAt: T0 };
  addValid("approval", st.approval(R1, W1, { ...o, label: "Sam's phone" }));
  addValid("approval_label_64_bytes_ascii", st.approval(R1, W1, { ...o, label: "a".repeat(64) }));
  addValid("approval_label_64_bytes_multibyte", st.approval(R1, W1, { ...o, label: "é".repeat(32) }));
  addValid("withdrawal", st.withdrawal(R1, W1, o));
  addValid("coguardian", st.coguardian(R1, R2, { ...o, label: "Alex", expiresAt: T0 + DAY }));
  addValid("coguardian_expires_beyond_cap", st.coguardian(R1, R2, { ...o, expiresAt: T0 + 30 * DAY }));
  addValid("removal_by_other_root", st.removal(R1, R2, o));
  addValid("removal_self", st.removal(R2, R2, o));
  addValid("move", st.move(R1, R3, o));
  addValid("cancel_coguardian", st.cancel(R1, dG(R2), o));
  addValid("cancel_removal", st.cancel(R1, dU(R2), o));
  addValid("cancel_move", st.cancel(R1, dM(R3), o));
  addValid("objection", st.objection(R2, dU(R2), R1, o));
  const dW = ["d", `kintrinsic/device/${W1.pk}`];
  const pW = ["p", W1.pk];
  const good = JSON.stringify({ v: 1, issuedAt: T0 });
  addValid("unknown_content_fields_ignored", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: T0, colour: "red", nested: { a: 1 } })));
  addValid("created_at_ignored", st.raw(R1, [dW, pW, MARKER], good, G.CHARTER_GUARDIAN_STATEMENT, 5));
  addValid("tag_order_free", st.raw(R1, [MARKER, pW, dW], good));
  addValid("label_surrogate_pair_and_escapes", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"label":"\\ud83d\\ude00 \\"x\\""}`));
  addValid("integer_beyond_2_53_in_unknown_field", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"big":123456789012345678901}`));
  addValid("nesting_depth_16_in_unknown_field", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"x":${"[".repeat(15)}${"]".repeat(15)}}`));

  const base = st.approval(R1, W1, o);
  addInvalid("not_an_object", "not an event", "bad_event");
  addInvalid("missing_sig_field", { ...base, sig: undefined }, "bad_event");
  addInvalid("tampered_content", { ...base, content: JSON.stringify({ v: 1, issuedAt: T0 + 1 }) }, "bad_signature");
  addInvalid("tampered_sig", { ...base, sig: "00".repeat(64) }, "bad_signature");
  addInvalid("wrong_id", { ...base, id: "11".repeat(32) }, "bad_signature");
  addInvalid("wrong_kind_30078", st.raw(R1, [dW, pW, MARKER], good, 30078), "wrong_kind");
  addInvalid("missing_marker", st.raw(R1, [dW, pW], good), "missing_marker");
  addInvalid("two_d_tags", st.raw(R1, [dW, dW, pW, MARKER], good), "d_tag_count");
  addInvalid("no_d_tag", st.raw(R1, [pW, MARKER], good), "d_tag_count");
  addInvalid("two_p_tags", st.raw(R1, [dW, pW, ["p", R2.pk], MARKER], good), "p_tag_count");
  addInvalid("no_p_tag", st.raw(R1, [dW, MARKER], good), "p_tag_count");
  addInvalid("unknown_prefix", st.raw(R1, [["d", `kindependence/device/${W1.pk}`], pW, MARKER], good), "unknown_d_prefix");
  addInvalid("uppercase_key_in_d", st.raw(R1, [["d", `kintrinsic/device/${W1.pk.toUpperCase()}`], pW, MARKER], good), "bad_key");
  addInvalid("short_p", st.raw(R1, [dW, ["p", W1.pk.slice(2)], MARKER], good), "bad_key");
  addInvalid("d_p_mismatch", st.raw(R1, [dW, ["p", W2.pk], MARKER], good), "d_p_mismatch");
  addInvalid("withdrawal_d_p_mismatch", st.raw(R1, [["d", `kintrinsic/revoke/${W1.pk}`], ["p", W2.pk], MARKER], good), "d_p_mismatch");
  addInvalid("cancel_of_approval", st.raw(R1, [["d", `kintrinsic/cancel/kintrinsic/device/${W1.pk}`], pW, MARKER], good), "bad_cancel_target");
  addInvalid("cancel_d_p_mismatch", st.raw(R1, [["d", `kintrinsic/cancel/${dG(R2)}`], ["p", R3.pk], MARKER], good), "d_p_mismatch");
  addInvalid("objection_of_coguardian", st.raw(R2, [["d", `kintrinsic/object/${dG(R2)}`], ["p", R1.pk], MARKER], good), "bad_objection_target");
  addInvalid("content_not_json", st.raw(R1, [dW, pW, MARKER], "not json"), "content_not_json");
  addInvalid("content_array", st.raw(R1, [dW, pW, MARKER], "[1]"), "content_not_json");
  addInvalid("v_2", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 2, issuedAt: T0 })), "bad_version");
  addInvalid("v_missing", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ issuedAt: T0 })), "bad_version");
  addInvalid("v_string", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: "1", issuedAt: T0 })), "bad_version");
  addInvalid("issued_at_missing", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1 })), "bad_issued_at");
  addInvalid("issued_at_fractional", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: T0 + 0.5 })), "content_not_json");
  // Strict JSON: duplicate names, non-integer number tokens, -0 and lone
  // surrogates are refused anywhere in the content.
  addInvalid("duplicate_issued_at", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"issuedAt":${T0 + 99}}`), "content_not_json");
  addInvalid("duplicate_in_nested_unknown", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"x":{"a":1,"a":2}}`), "content_not_json");
  addInvalid("v_1_0", st.raw(R1, [dW, pW, MARKER], `{"v":1.0,"issuedAt":${T0}}`), "content_not_json");
  addInvalid("issued_at_exponent", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":1.7e9}`), "content_not_json");
  addInvalid("issued_at_exponent_integer", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":17e8}`), "content_not_json");
  addInvalid("issued_at_negative_zero", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":-0}`), "content_not_json");
  addInvalid("float_in_unknown_field", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"x":0.5}`), "content_not_json");
  addInvalid("label_lone_surrogate", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"label":"\\ud800"}`), "content_not_json");
  addInvalid("label_lone_low_surrogate", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},"label":"a\\udc00"}`), "content_not_json");
  addInvalid("trailing_comma", st.raw(R1, [dW, pW, MARKER], `{"v":1,"issuedAt":${T0},}`), "content_not_json");
  // Nesting: the top-level object is depth 1; 16 is the most accepted.
  const nestedAt = (depth) => `{"v":1,"issuedAt":${T0},"x":${"[".repeat(depth - 1)}${"]".repeat(depth - 1)}}`;
  addInvalid("nesting_too_deep", st.raw(R1, [dW, pW, MARKER], nestedAt(17)), "content_not_json");
  addInvalid("issued_at_negative", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: -1 })), "bad_issued_at");
  addInvalid("issued_at_string", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: String(T0) })), "bad_issued_at");
  addInvalid("label_65_bytes", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: T0, label: "a".repeat(65) })), "bad_label");
  addInvalid("label_66_bytes_multibyte", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: T0, label: "é".repeat(33) })), "bad_label");
  addInvalid("label_not_string", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: T0, label: null })), "bad_label");
  addInvalid("expires_at_string", st.raw(R1, [dW, pW, MARKER], JSON.stringify({ v: 1, issuedAt: T0, expiresAt: "soon" })), "bad_expires_at");
  write("statements.json", { version: 1, keys: keysJson(), valid, invalid });
}

// --- scenarios.json ------------------------------------------------------------
const scenarios = [];
const finalStates = {};

function scenario(name, rules, description, build) {
  const steps = [];
  let state = null;
  const record = (input, outcome, now) => {
    steps.push({
      input,
      expect: {
        result: outcome.result,
        reason: outcome.reason,
        ticked: outcome.ticked ?? [],
        departed: outcome.departed ?? [],
        state: state ? G.projectState(state, now) : null,
      },
    });
  };
  const check = (label, outcome, want) => {
    const [result, reason] = want;
    if (outcome.result !== result || outcome.reason !== reason) {
      throw new Error(`${name} / ${label}: want ${result}/${reason}, got ${outcome.result}/${outcome.reason}`);
    }
  };
  const run = (label, input, fn, want) => {
    if (!state) throw new Error(`${name}: step before pairing`);
    const r = fn();
    check(label, r.outcome, want);
    state = r.state;
    record(input, r.outcome, input.now);
    return r;
  };
  const ctx = {
    get state() {
      return state;
    },
    pair(sealer, now, { approval, token = tokenHex("pair"), offerToken = token, ts = now, mintedAt = now - 30 } = {}, want = ["accepted", "paired"]) {
      const inner = offerEv(sealer, offerToken, ts, approval);
      const input = { type: "pair_offer", now, sealAuthor: sealer.pk, inner, token: { token, mintedAt } };
      const r = G.pairFromOffer({ sealAuthor: sealer.pk, inner }, { token, mintedAt }, now);
      const outcome = { result: r.ok ? "accepted" : "dropped", reason: r.reason };
      check("pair", outcome, want);
      if (r.ok) state = r.state;
      record(input, outcome, now);
    },
    inbound(seal, inner, now, want) {
      const input = { type: "inbound", now, sealAuthor: seal.pk, inner };
      return run("inbound", input, () => G.applyInbound(state, { sealAuthor: seal.pk, inner }, now), want);
    },
    tick(now, want = ["accepted", "ticked"]) {
      return run("tick", { type: "tick", now }, () => G.tick(state, now), want);
    },
    token(token, statementD, now, want = ["accepted", "token_registered"]) {
      const input = { type: "presence_token", now, token, statementD };
      return run("token", input, () => G.registerPresenceToken(state, token, statementD, now), want);
    },
    confirm(seal, inner, now, want) {
      const input = { type: "presence_confirm", now, sealAuthor: seal.pk, inner };
      return run("confirm", input, () => G.applyPresenceConfirm(state, { sealAuthor: seal.pk, inner }, now), want);
    },
    clause(author, issuedAt, now, want, { seal = author, subject = "subject-a", kind = "schedule", id = clauseId(author, subject, kind, issuedAt) } = {}) {
      const input = { type: "clause", now, id, author: author.pk, sealAuthor: seal.pk, subject, kind, issuedAt };
      return run("clause", input, () => G.acceptClause(state, { id, author: author.pk, sealAuthor: seal.pk, subject, kind, issuedAt }, now), want);
    },
    signed(author, ts, now, want, seal = author) {
      const input = { type: "signed", now, author: author.pk, sealAuthor: seal.pk, ts };
      return run("signed", input, () => G.acceptSignedEvent(state, { author: author.pk, sealAuthor: seal.pk, ts }, now), want);
    },
    // `relays` null: the body has no relay list (a PIN-only or battery-only RELAYS).
    relays(author, ts, relays, now, want, { channel = "wrapped", seal = author, guardianPin } = {}) {
      const ev = { author: author.pk, channel, sealAuthor: channel === "wrapped" ? seal.pk : null, ts };
      if (relays !== null) ev.relays = relays;
      if (guardianPin !== undefined) ev.guardianPin = guardianPin;
      const input = { type: "relays", now, ...ev };
      return run("relays", input, () => G.applyRelays(state, ev, now), want);
    },
    statusSent(now) {
      return run("status_sent", { type: "status_sent", now }, () => G.markStatusSent(state, now), ["accepted", "status_sent"]);
    },
  };
  build(ctx);
  scenarios.push({ name, rules, description, steps });
  finalStates[name] = state;
}

const A = (r) => ["accepted", r];
const D = (r) => ["dropped", r];

/** Pair by W1 (approved by R1) at T0. */
function pairW1(c) {
  c.pair(W1, T0, { approval: st.approval(R1, W1, { issuedAt: T0 - 1000, label: "Sam's phone" }) });
}

/** Pair by W1; add R2 with W2 through a confirmed co-guardian (by T0+30). */
function twoRoots(c) {
  pairW1(c);
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 5 });
  c.inbound(W1, cg, T0 + 10, A("coguardian_pending"));
  c.token(tokenHex("cg-R2"), dG(R2), T0 + 20);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("cg-R2"), T0 + 30, st.approval(R2, W2, { issuedAt: T0 + 15, label: "Alex's phone" })), T0 + 30, A("coguardian_applied"));
}

scenario("pair_root_direct", ["scan-to-pair"], "An offer sealed by a root with no approval pins that root; it receives directly.", (c) => {
  c.pair(R1, T0);
  c.clause(R1, T0 + 1, T0 + 10, A("clause_accepted"));
});

scenario("pair_working_key", ["scan-to-pair"], "An offer sealed by W carrying R's approval pins R, not W, and binds W.", (c) => {
  pairW1(c);
  c.clause(W1, T0 + 1, T0 + 10, A("clause_accepted"));
  c.clause(R1, T0 + 2, T0 + 10, A("clause_accepted"));
});

scenario("pair_refused_approval_names_other_key", ["scan-to-pair"], "The approval's p must be the seal author.", (c) => {
  c.pair(W1, T0, { approval: st.approval(R1, W2, { issuedAt: T0 }) }, D("seal_mismatch"));
});

scenario("pair_refused_invalid_approval", ["scan-to-pair"], "An offer sealed by W without a valid approval for W is refused.", (c) => {
  const ap = { ...st.approval(R1, W1, { issuedAt: T0 }), sig: "00".repeat(64) };
  c.pair(W1, T0, { approval: ap }, D("bad_approval"));
});

scenario("pair_refused_not_an_approval", ["scan-to-pair"], "The carried statement must be an approval.", (c) => {
  c.pair(W1, T0, { approval: st.withdrawal(R1, W1, { issuedAt: T0 }) }, D("bad_approval"));
});

scenario("pair_refused_wrong_token", ["scan-to-pair"], "Token rules are as for scan-to-pair.", (c) => {
  c.pair(R1, T0, { offerToken: tokenHex("other") }, D("token_invalid"));
});

scenario("pair_refused_token_expired", ["scan-to-pair"], "600-second token lifetime from the mint time.", (c) => {
  c.pair(R1, T0, { mintedAt: T0 - 600 }, D("token_invalid"));
});

scenario("pair_refused_future_ts", ["scan-to-pair"], "A future-dated offer ts is refused.", (c) => {
  c.pair(R1, T0, { ts: T0 + 1 }, D("token_invalid"));
});

scenario("seal_exception_1", ["seal exception 1", "signer set"], "An approval may be sealed only by the W it names.", (c) => {
  pairW1(c);
  c.inbound(R1, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, D("seal_mismatch"));
  c.inbound(W1, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, D("seal_mismatch"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0, label: "Sam's laptop" }), T0 + 10, A("approval_applied"));
  c.clause(W1b, T0 + 11, T0 + 12, A("clause_accepted"));
});

scenario("seal_exception_2", ["seal exception 2"], "A withdrawal, co-guardian, removal, move or cancel signed by R may be sealed by a current W bound to R; not by another root's W, a non-signer or a departed W.", (c) => {
  twoRoots(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 40 }), T0 + 50, A("approval_applied"));
  c.inbound(W2, st.withdrawal(R1, W1b, { issuedAt: T0 + 41 }), T0 + 50, D("seal_mismatch"));
  c.inbound(X, st.withdrawal(R1, W1b, { issuedAt: T0 + 41 }), T0 + 50, D("seal_mismatch"));
  c.inbound(W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 41 }), T0 + 50, A("withdrawal_applied"));
  c.inbound(W1b, st.coguardian(R1, R3, { issuedAt: T0 + 42 }), T0 + 50, D("seal_mismatch"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 42 }), T0 + 50, A("coguardian_pending"));
  c.inbound(W1, st.cancel(R1, dG(R3), { issuedAt: T0 + 43 }), T0 + 50, A("cancel_applied"));
});

scenario("approval_for_unpinned_root", ["delivery"], "An approval for a root that isn't pinned is dropped; its W's events are dropped, not buffered.", (c) => {
  pairW1(c);
  c.inbound(W2, st.approval(R2, W2, { issuedAt: T0 }), T0 + 10, D("signer_not_pinned"));
  c.clause(W2, T0 + 11, T0 + 12, D("not_signer"));
  c.inbound(X, st.coguardian(X, R3, { issuedAt: T0 }), T0 + 13, D("signer_not_pinned"));
});

scenario("rule1_binding", ["1"], "W binds to the root of the first valid approval; approvals for W from another root are ignored.", (c) => {
  twoRoots(c);
  c.inbound(W1, st.approval(R2, W1, { issuedAt: T0 + 40 }), T0 + 50, D("bound_elsewhere"));
  c.inbound(W3, st.approval(R2, W3, { issuedAt: T0 + 41 }), T0 + 50, A("approval_applied"));
  c.inbound(W3, st.approval(R1, W3, { issuedAt: T0 + 42 }), T0 + 50, D("bound_elsewhere"));
  c.inbound(R1, st.approval(R1, R2, { issuedAt: T0 + 43 }), T0 + 50, D("seal_mismatch"));
  c.inbound(R2, st.approval(R1, R2, { issuedAt: T0 + 43 }), T0 + 50, D("key_role_conflict"));
});

scenario("rule2_ordering", ["2"], "No pairing floor for statements; newest issuedAt per d wins; duplicate, stale and same-second different content are dropped.", (c) => {
  pairW1(c);
  const a1 = st.approval(R1, W1b, { issuedAt: T0 - 30 * DAY, label: "one" });
  c.inbound(W1b, a1, T0 + 10, A("approval_applied"));
  c.inbound(W1b, a1, T0 + 11, D("duplicate"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 - 30 * DAY - 1, label: "older" }), T0 + 12, D("stale"));
  // Same second, different event: the lower event id wins (every ward converges).
  const tie = st.approval(R1, W1b, { issuedAt: T0 - 30 * DAY, label: "same second" });
  c.inbound(W1b, tie, T0 + 13, tie.id < a1.id ? A("approval_updated") : D("tie_not_newer"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 1, label: "two" }), T0 + 14, A("approval_updated"));
});

scenario("rule3_withdrawal", ["3", "4"], "Withdrawal counts only from the binding root and is permanent.", (c) => {
  twoRoots(c);
  c.inbound(R2, st.withdrawal(R2, W1, { issuedAt: T0 + 40 }), T0 + 50, D("not_binding_root"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 40 }), T0 + 50, A("approval_applied"));
  c.inbound(W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 41 }), T0 + 50, A("withdrawal_applied"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 340 }), T0 + 60, D("key_departed"));
  c.inbound(R1, st.withdrawal(R1, W1b, { issuedAt: T0 + 42 }), T0 + 60, D("already_withdrawn"));
  c.clause(W1b, T0 + 61, T0 + 62, D("not_signer"));
  c.statusSent(T0 + 63);
});

scenario("rule3_early_withdrawal", ["3"], "A withdrawal before W is bound is held if a pinned root signed it, and pre-empts that root's approval; an unpinned signer's is dropped.", (c) => {
  twoRoots(c);
  c.inbound(X, st.withdrawal(X, W1b, { issuedAt: T0 }), T0 + 40, D("signer_not_pinned"));
  c.inbound(R1, st.withdrawal(R1, W1b, { issuedAt: T0 }), T0 + 40, A("withdrawal_held"));
  c.inbound(R1, st.withdrawal(R1, W1b, { issuedAt: T0 + 1 }), T0 + 40, D("duplicate"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 100 }), T0 + 41, A("approval_preempted"));
  c.inbound(R2, st.withdrawal(R2, W3, { issuedAt: T0 }), T0 + 42, A("withdrawal_held"));
  c.inbound(W3, st.approval(R1, W3, { issuedAt: T0 + 1 }), T0 + 43, A("approval_applied"));
});

scenario("rule2_tie_withdrawal_wins", ["2", "3"], "A withdrawal and an approval at the same issuedAt: the withdrawal wins in either order.", (c) => {
  pairW1(c);
  c.inbound(R1, st.withdrawal(R1, W1b, { issuedAt: T0 }), T0 + 10, A("withdrawal_held"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 11, A("approval_preempted"));
  c.inbound(W3, st.approval(R1, W3, { issuedAt: T0 }), T0 + 12, A("approval_applied"));
  c.inbound(R1, st.withdrawal(R1, W3, { issuedAt: T0 }), T0 + 13, A("withdrawal_applied"));
});

scenario("rule4_departure_clauses", ["4", "5", "who may sign what"], "Departed keys sign nothing new; their clauses stay until a current signer replaces them. A far-future clause is held at its effective time, so it pins nothing. No pairing floor for clauses.", (c) => {
  pairW1(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
  c.clause(W1b, T0 + DAY, T0 + 10, A("clause_accepted"));
  c.clause(W1, T0 + 10, T0 + 20, D("not_newer"));
  c.inbound(W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 30 }), T0 + 30, A("withdrawal_applied"));
  c.clause(W1b, T0 + 400, T0 + 400, D("not_signer"));
  c.clause(W1, T0 + 40, T0 + 40, A("clause_accepted"));
  c.clause(R1, T0 + 40, T0 + 41, D("not_newer"));
  c.clause(R1, T0 + 41, T0 + 41, A("clause_accepted"));
  c.clause(W1, T0 + 42, T0 + 42, D("seal_mismatch"), { seal: R1 });
  c.clause(W1, T0 - 1, T0 + 42, A("clause_accepted"), { subject: "subject-b" });
});

scenario("clause_clamp", ["who may sign what", "4", "6"], "A clause dated ahead of the ward clock is accepted at its effective time, min(issuedAt, now): the other parent's next rule replaces it. Each signer's floor for the slot is the highest issuedAt accepted from it, so a relay can't replay a superseded clause, and a signer whose issuedAt goes backwards is refused as a replay.", (c) => {
  twoRoots(c);
  const o = { kind: "apps" };
  c.clause(W1, T0 + DAY, T0 + 100, A("clause_accepted"), o);
  c.clause(W2, T0 + 200, T0 + 200, A("clause_accepted"), o);
  c.clause(W1, T0 + DAY, T0 + 300, D("replay"), o);
  c.clause(W1, T0 + DAY + 1, T0 + 400, A("clause_accepted"), o);
  c.clause(W1, T0 + DAY + 1, T0 + 500, D("replay"), o);
  c.clause(W2, T0 + 400, T0 + 450, D("not_newer"), o);
  c.clause(W2, T0 + DAY + 5, T0 + DAY + 10, A("clause_accepted"), o);
  c.clause(W1, T0 + DAY, T0 + DAY + 20, D("not_newer"), o);
  c.clause(W1, T0 + DAY + 1000, T0 + DAY + 20, A("clause_accepted"), o);
  c.clause(W2, T0 + DAY + 500, T0 + DAY + 20, A("clause_accepted"), o);
  c.clause(W1, T0 + DAY + 1000, T0 + DAY + 30, D("replay"), o);
  c.clause(W1, T0 + DAY + 21, T0 + DAY + 30, D("replay"), o);
  c.clause(W1, T0 + DAY + 1001, T0 + DAY + 30, A("clause_accepted"), o);
});

scenario("clause_flood_then_withdrawn", ["who may sign what", "4", "3"], "300 far-future clauses for one slot from a working key, each accepted (one floor per signer, no quota to fill); the key is withdrawn and its floors go. The other parent's clause, dated ahead of the ward clock, then replaces the departed key's rule.", (c) => {
  twoRoots(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 35 }), T0 + 35, A("approval_applied"));
  const o = { kind: "apps" };
  const FAR = T0 + 76 * 365 * DAY;
  for (let i = 0; i < 300; i++) c.clause(W1b, FAR + i, T0 + 100 + i, A("clause_accepted"), o);
  c.clause(W1b, FAR + 299, T0 + 400, D("replay"), o);
  c.inbound(W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 500 }), T0 + 500, A("withdrawal_applied"));
  c.clause(W1b, FAR + 300, T0 + 510, D("not_signer"), o);
  c.clause(W2, T0 + 660, T0 + 600, A("clause_accepted"), o);
  c.clause(W1, T0 + 700, T0 + 700, A("clause_accepted"), o);
});

scenario("clause_replay_superseded_far_future", ["who may sign what", "4"], "A relay re-delivers a superseded far-future clause. Its issuedAt beats the bar (the newer clause's effective time) but not its signer's floor, so it is refused, from whichever slot holder it would displace.", (c) => {
  twoRoots(c);
  const o = { kind: "content" };
  c.clause(W1, T0 + 2 * DAY, T0 + 100, A("clause_accepted"), o);
  c.clause(W1, T0 + 2 * DAY + 1, T0 + 200, A("clause_accepted"), o);
  c.clause(W2, T0 + 300, T0 + 300, A("clause_accepted"), o);
  c.clause(W1, T0 + 2 * DAY, T0 + 400, D("replay"), o);
  c.clause(W1, T0 + 2 * DAY + 1, T0 + 400, D("replay"), o);
  c.clause(W2, T0 + 300, T0 + 500, D("not_newer"), o);
  c.clause(W1, T0 + 2 * DAY + 2, T0 + 600, A("clause_accepted"), o);
});

scenario("clock_steps_back", ["who may sign what", "4", "2"], "The ward clock steps back (a dead RTC before NTP). A clause accepted then keeps the slot's bar where it was, max(bar, min(issuedAt, now)), so an older clause from another signer that a relay held back can't slip in under it. A statement slot's newest effective time never falls either: a statement whose effective time is below it is stale.", (c) => {
  twoRoots(c);
  const o = { kind: "apps" };
  c.clause(W1, T0 + 1000, T0 + 1000, A("clause_accepted"), o);
  c.clause(W2, T0 + 1001, T0 + 500, A("clause_accepted"), o);
  c.clause(R1, T0 + 800, T0 + 510, D("not_newer"), o);
  c.clause(W1, T0 + 1002, T0 + 520, A("clause_accepted"), o);
  c.clause(W2, T0 + 1003, T0 + 1100, A("clause_accepted"), o);
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 1100 }), T0 + 1100, A("coguardian_pending"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 1101 }), T0 + 600, D("stale"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 1102 }), T0 + 1102, A("coguardian_pending"));
});

scenario("statement_clamp", ["who may sign what", "pending", "7"], "A statement dated ahead of the ward clock is accepted; ordering, expiry, cancels, objections and departures use its effective time, min(issuedAt, now at receipt), so another device's correct clock is never refused.", (c) => {
  twoRoots(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + DAY }), T0 + 110, A("approval_applied"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + DAY }), T0 + 120, A("coguardian_pending"));
  c.inbound(W1b, st.cancel(R1, dG(R3), { issuedAt: T0 + 130 }), T0 + 130, A("cancel_applied"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 115 }), T0 + 130, D("stale"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 125 }), T0 + 130, D("replay"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + DAY + 1 }), T0 + 130, D("cancelled"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + DAY + 2 }), T0 + 140, A("coguardian_pending"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + DAY }), T0 + 140, A("removal_pending"));
  c.inbound(R2, st.objection(R2, dU(R2), R1, { issuedAt: T0 + 150 }), T0 + 150, A("objection_applied"));
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 2 * DAY }), T0 + 160, A("removal_applied"));
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 170 });
  c.inbound(W1, cg, T0 + 170, A("coguardian_pending"));
  c.token(tokenHex("clamp"), dG(R2), T0 + 180);
  c.confirm(W3, confirmEv(W3, cg.id, tokenHex("clamp"), T0 + 190, st.approval(R2, W3, { issuedAt: T0 + 3 * DAY })), T0 + 190, A("coguardian_applied"));
});

scenario("statement_replay_after_supersede", ["who may sign what", "2", "pending"], "Each signer's floor per statement slot is the highest issuedAt accepted from it: a relay can't replay a superseded far-future co-guardian over the newer one, though its effective time beats the slot's. Once the slot's effective time passes it, the replay is merely stale.", (c) => {
  twoRoots(c);
  const far = st.coguardian(R1, R3, { issuedAt: T0 + DAY });
  c.inbound(W1, far, T0 + 100, A("coguardian_pending"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + DAY + 1 }), T0 + 200, A("coguardian_pending"));
  c.inbound(W1, far, T0 + 300, D("replay"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + DAY + 5 }), T0 + DAY + 10, A("coguardian_pending"));
  c.inbound(W1, far, T0 + DAY + 20, D("stale"));
});

scenario("statement_replay_after_cancel", ["who may sign what", "2", "pending"], "A cancelled far-future move keeps its slot, so its replay is a duplicate; once a newer move with the same d displaces it and is cancelled in turn, its signer's floor still stops the replay from pending again.", (c) => {
  twoRoots(c);
  const far = st.move(R1, R3, { issuedAt: T0 + DAY });
  c.inbound(W1, far, T0 + 100, A("move_pending"));
  c.inbound(W1, st.cancel(R1, dM(R3), { issuedAt: T0 + 110 }), T0 + 110, A("cancel_applied"));
  c.inbound(W1, far, T0 + 120, D("duplicate"));
  c.inbound(W1, st.move(R1, R3, { issuedAt: T0 + DAY + 1 }), T0 + 130, A("move_pending"));
  c.inbound(W1, st.cancel(R1, dM(R3), { issuedAt: T0 + 140 }), T0 + 140, A("cancel_applied"));
  c.inbound(W1, far, T0 + 150, D("replay"));
});

scenario("statement_floor_per_install", ["who may sign what", "2", "seal exception 2"], "A statement's replay floor belongs to the install that sealed it. Two installs of one root each carry a root-signed co-guardian for the same slot, dated by their own clocks: the second, dated before the first's real issuedAt but after its effective time, is accepted. A relay re-delivers the seal too, so the first install's far-future statement is still a replay.", (c) => {
  twoRoots(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 40 }), T0 + 50, A("approval_applied"));
  const far = st.coguardian(R1, R3, { issuedAt: T0 + DAY });
  c.inbound(W1, far, T0 + 100, A("coguardian_pending"));
  const mine = st.coguardian(R1, R3, { issuedAt: T0 + 500 });
  c.inbound(W1b, mine, T0 + 200, A("coguardian_pending"));
  c.inbound(W1, far, T0 + 300, D("replay"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + DAY - 1 }), T0 + 300, D("replay"));
  c.inbound(W1b, mine, T0 + 300, D("duplicate"));
});

scenario("pair_approval_clamped",["scan-to-pair", "who may sign what"], "A pair offer's approval dated ahead of the ward clock still pairs; it counts as clamped.", (c) => {
  c.pair(W1, T0, { approval: st.approval(R1, W1, { issuedAt: T0 + DAY }) });
});

scenario("signer_gate_grant_usage_sync", ["who may sign what"], "GRANT and USAGE_SYNC authors must be in the signer set and seal = author. The gate judges no time: a GRANT keeps its own skew rule; USAGE_SYNC has none.", (c) => {
  pairW1(c);
  c.signed(W1, T0 + 5, T0 + 10, A("signed_accepted"));
  c.signed(R1, T0 + 5, T0 + 10, A("signed_accepted"));
  c.signed(X, T0 + 5, T0 + 10, D("not_signer"));
  c.signed(W1, T0 + 5, T0 + 10, D("seal_mismatch"), R1);
  c.signed(W1, T0 + DAY, T0 + 10, A("signed_accepted"));
});

scenario("rule6_coguardian", ["6", "presence confirmation"], "A co-guardian takes effect only after presence confirmation by an install bound to R2, which carries R2's approval; then both roots have equal authority.", (c) => {
  pairW1(c);
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 5, label: "Alex" });
  c.inbound(W1, cg, T0 + 10, A("coguardian_pending"));
  c.clause(W2, T0 + 11, T0 + 11, D("not_signer"));
  c.token(tokenHex("cg"), dG(R2), T0 + 20);
  c.token(tokenHex("cg"), null, T0 + 20, D("token_invalid"));
  c.token(tokenHex("nope"), dG(R3), T0 + 20, D("not_pending"));
  const ap = st.approval(R2, W2, { issuedAt: T0 + 15, label: "Alex's phone" });
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("cg"), T0 + 30), T0 + 30, D("approval_required"));
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("wrong"), T0 + 30, ap), T0 + 30, D("token_invalid"));
  c.confirm(W2, confirmEv(W2, "ee".repeat(32), tokenHex("cg"), T0 + 30, ap), T0 + 30, D("not_pending"));
  c.confirm(W3, confirmEv(W3, cg.id, tokenHex("cg"), T0 + 30, ap), T0 + 30, D("bad_approval"));
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("cg"), T0 + 30, st.approval(R3, W2, { issuedAt: T0 })), T0 + 30, D("bad_approval"));
  c.confirm(W1, confirmEv(W1, cg.id, tokenHex("cg"), T0 + 30, st.approval(R2, W1, { issuedAt: T0 })), T0 + 30, D("bound_elsewhere"));
  c.confirm(R2, confirmEv(R2, cg.id, tokenHex("cg"), T0 + 30, st.approval(R2, R2, { issuedAt: T0 })), T0 + 30, D("key_role_conflict"));
  c.confirm(W1, confirmEv(W2, cg.id, tokenHex("cg"), T0 + 30, ap), T0 + 30, D("seal_mismatch"));
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("cg"), T0 + 30, ap), T0 + 30, A("coguardian_applied"));
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("cg"), T0 + 31, ap), T0 + 31, D("not_pending"));
  c.clause(W1, T0 + 40, T0 + 50, A("clause_accepted"));
  c.clause(W2, T0 + 41, T0 + 50, A("clause_accepted"));
  c.clause(W1, T0 + 41, T0 + 50, D("not_newer"));
});

scenario("presence_token_lifetime", ["presence confirmation"], "A presence token lives 600 seconds and is single use.", (c) => {
  pairW1(c);
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 5 });
  c.inbound(W1, cg, T0 + 10, A("coguardian_pending"));
  c.token(tokenHex("life"), dG(R2), T0 + 20);
  const ap = st.approval(R2, W2, { issuedAt: T0 + 15 });
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("life"), T0 + 620, ap), T0 + 620, D("token_invalid"));
  c.token(tokenHex("life2"), null, T0 + 630);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("life2"), T0 + 640, ap), T0 + 640, A("coguardian_applied"));
});

scenario("root_quota", ["terms", "10"], "At most 4 roots: a fifth co-guardian is refused.", (c) => {
  twoRoots(c);
  for (const [r, w, n] of [[R3, W3, 3], [R4, W4, 4]]) {
    const cg = st.coguardian(R1, r, { issuedAt: T0 + 100 + n });
    c.inbound(W1, cg, T0 + 200, A("coguardian_pending"));
    c.token(tokenHex(`q${n}`), dG(r), T0 + 200);
    c.confirm(w, confirmEv(w, cg.id, tokenHex(`q${n}`), T0 + 200, st.approval(r, w, { issuedAt: T0 + 100 })), T0 + 200, A("coguardian_applied"));
  }
  c.inbound(W1, st.coguardian(R1, R5, { issuedAt: T0 + 150 }), T0 + 200, D("root_quota"));
});

scenario("key_quota", ["terms", "10"], "At most 16 active working keys per root; a withdrawal frees a slot.", (c) => {
  pairW1(c);
  const extra = [];
  for (let i = 0; i < 15; i++) extra.push(k(`Q${String(i).padStart(2, "0")}`));
  for (const w of extra) c.inbound(w, st.approval(R1, w, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
  const w17 = k("Q16");
  c.inbound(w17, st.approval(R1, w17, { issuedAt: T0 }), T0 + 10, D("root_key_quota"));
  c.inbound(W1, st.withdrawal(R1, extra[0], { issuedAt: T0 + 1 }), T0 + 11, A("withdrawal_applied"));
  c.inbound(w17, st.approval(R1, w17, { issuedAt: T0 }), T0 + 12, A("approval_applied"));
});

scenario("held_quota", ["10"], "64 held early statements per root; past it they are dropped, and a root at quota can't approve new working keys.", (c) => {
  pairW1(c);
  for (let i = 0; i < 64; i++) {
    const w = k(`H${String(i).padStart(2, "0")}`);
    c.inbound(R1, st.withdrawal(R1, w, { issuedAt: T0 }), T0 + 10, A("withdrawal_held"));
  }
  c.inbound(R1, st.withdrawal(R1, k("H64"), { issuedAt: T0 }), T0 + 10, D("held_quota"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, D("root_at_quota"));
  c.inbound(W1, st.approval(R1, W1, { issuedAt: T0 + 1, label: "renamed" }), T0 + 10, A("approval_updated"));
});

scenario("rule7_self_removal", ["7"], "Signed by R2 itself, a removal takes effect at once, unless R2 is the last root.", (c) => {
  twoRoots(c);
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60, A("removal_applied"));
  c.clause(W2, T0 + 61, T0 + 61, D("not_signer"));
  c.inbound(W1, st.removal(R1, R1, { issuedAt: T0 + 70 }), T0 + 80, D("last_root"));
  c.statusSent(T0 + 90);
});

scenario("rule7_removal_by_other_root", ["7", "9"], "Signed by another root: presence by an R1-bound install, then the later of confirmation and max(issuedAt, receipt) + 72 h; R2 and its keys depart and get one last STATUS.", (c) => {
  twoRoots(c);
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100 });
  c.inbound(W1, rm, T0 + 110, A("removal_pending"));
  c.token(tokenHex("rm"), dU(R2), T0 + 120);
  c.confirm(W2, confirmEv(W2, rm.id, tokenHex("rm"), T0 + 130), T0 + 130, D("confirmer_not_bound"));
  c.confirm(R1, confirmEv(R1, rm.id, tokenHex("rm"), T0 + 130), T0 + 130, D("confirmer_not_bound"));
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("rm"), T0 + 130), T0 + 130, A("removal_confirmed"));
  c.tick(T0 + 110 + H72 - 1);
  c.clause(W2, T0 + 200, T0 + 110 + H72 - 1, A("clause_accepted"));
  c.tick(T0 + 110 + H72);
  c.clause(W1, T0 + 199, T0 + 110 + H72 + 1, D("not_newer"));
  c.clause(W1, T0 + 201, T0 + 110 + H72 + 1, A("clause_accepted"));
  c.statusSent(T0 + 110 + H72 + 2);
});

scenario("rule7_removal_confirmed_after_notice", ["7"], "Confirmed after the 72 hours, a removal applies at once.", (c) => {
  twoRoots(c);
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100 });
  c.inbound(W1, rm, T0 + 110, A("removal_pending"));
  const late = T0 + 110 + H72 + 5;
  c.token(tokenHex("late"), null, late);
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("late"), late), late, A("removal_applied"));
});

scenario("rule7_objection", ["7", "seal exceptions"], "The root being removed, or an active W bound to it, objects; that drops the removal. Seal exception 2 covers the objection: a W bound to the objecting root may seal it.", (c) => {
  twoRoots(c);
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100 });
  c.inbound(W1, rm, T0 + 110, A("removal_pending"));
  const ob = st.objection(R2, dU(R2), R1, { issuedAt: T0 + 101 });
  c.inbound(W1, ob, T0 + 120, D("seal_mismatch"));
  c.inbound(R1, st.objection(R1, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120, D("objection_mismatch"));
  c.inbound(W1, st.objection(W1, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120, D("objection_mismatch"));
  c.inbound(X, st.objection(X, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 120, D("signer_not_pinned"));
  c.inbound(R2, st.objection(R2, dU(R2), R3, { issuedAt: T0 + 101 }), T0 + 120, D("objection_mismatch"));
  c.inbound(R2, st.objection(R2, dU(R2), R2, { issuedAt: T0 + 101 }), T0 + 120, D("objection_mismatch"));
  c.inbound(W2, ob, T0 + 120, A("objection_applied"));
  c.inbound(R2, ob, T0 + 121, D("duplicate"));
  c.tick(T0 + 100 + H72 + 1);
});

scenario("held_statements_cleared_on_departure", ["7", "4", "pending", "10"], "When a key departs, every held statement it signed or sealed goes and stops counting against its root's quota, and so do its objections, held or applied, with their block on later removals. A removed root's own objections go with it, so re-pinning it is not refused for its old held statements.", (c) => {
  twoRoots(c);
  c.inbound(W3, st.approval(R2, W3, { issuedAt: T0 + 40 }), T0 + 40, A("approval_applied"));
  c.inbound(W3, st.objection(W3, dU(R2), R1, { issuedAt: T0 + 50 }), T0 + 50, A("objection_held"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 55 }), T0 + 55, A("approval_applied"));
  c.inbound(W1b, st.cancel(R1, dG(R3), { issuedAt: T0 + 60 }), T0 + 60, A("cancel_held"));
  c.inbound(W2, st.withdrawal(R2, W3, { issuedAt: T0 + 70 }), T0 + 70, A("withdrawal_applied"));
  c.inbound(W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 80 }), T0 + 80, A("withdrawal_applied"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 90 }), T0 + 90, A("removal_pending"));
  c.inbound(W2, st.objection(W2, dU(R2), R1, { issuedAt: T0 + 100 }), T0 + 100, A("objection_applied"));
  c.inbound(R2, st.objection(R2, dU(R2), R1, { issuedAt: T0 + 105 }), T0 + 105, A("objection_held"));
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 110 }), T0 + 110, A("removal_applied"));
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 120 });
  c.inbound(W1, cg, T0 + 120, A("coguardian_pending"));
  c.token(tokenHex("held-cleared"), dG(R2), T0 + 130);
  c.confirm(W4, confirmEv(W4, cg.id, tokenHex("held-cleared"), T0 + 140, st.approval(R2, W4, { issuedAt: T0 + 135 })), T0 + 140, A("coguardian_applied"));
});

scenario("rule7_objection_by_working_key", ["7"], "A working key of the root being removed signs the objection itself (its app objects automatically); it drops the removal and blocks a re-issue for 7 days, as the root's own would, until that key departs: a withdrawn key's objections go with it.", (c) => {
  twoRoots(c);
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 110, A("removal_pending"));
  c.inbound(W2, st.objection(W2, dU(R2), R1, { issuedAt: T0 + 111 }), T0 + 120, A("objection_applied"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 200 }), T0 + 200, D("objected"));
  c.inbound(W2, st.withdrawal(R2, W2, { issuedAt: T0 + 300 }), T0 + 300, A("withdrawal_applied"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 310 }), T0 + 310, A("removal_pending"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 120 + 7 * DAY }), T0 + 120 + 7 * DAY, A("removal_pending"));
  c.inbound(W2, st.objection(W2, dU(R2), R1, { issuedAt: T0 + 121 + 7 * DAY }), T0 + 121 + 7 * DAY, D("signer_not_pinned"));
});

scenario("phone_root_mints_working_key", ["1", "7", "presence confirmation", "versioning"], "A phone-held root R1 (paired directly) mints a fresh W1 and approves it; W1 binds to R1 and then confirms presence for R1's own removal of R2.", (c) => {
  c.pair(R1, T0);
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 5 });
  c.inbound(R1, cg, T0 + 10, A("coguardian_pending"));
  c.token(tokenHex("phone-cg"), dG(R2), T0 + 20);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("phone-cg"), T0 + 30, st.approval(R2, W2, { issuedAt: T0 + 15 })), T0 + 30, A("coguardian_applied"));
  const rm = st.removal(R1, R2, { issuedAt: T0 + 40 });
  c.inbound(R1, rm, T0 + 40, A("removal_pending"));
  c.token(tokenHex("phone-rm"), dU(R2), T0 + 50);
  c.confirm(R1, confirmEv(R1, rm.id, tokenHex("phone-rm"), T0 + 60), T0 + 60, D("confirmer_not_bound"));
  c.inbound(W1, st.approval(R1, W1, { issuedAt: T0 + 61, label: "Sam's phone" }), T0 + 61, A("approval_applied"));
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("phone-rm"), T0 + 62), T0 + 62, A("removal_confirmed"));
});

scenario("rule7_objection_first", ["7"], "An objection that arrives before its removal is held and drops it on arrival; for 7 days after the objection any removal by that remover is refused.", (c) => {
  twoRoots(c);
  c.inbound(R2, st.objection(R2, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 110, A("objection_held"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 120, D("objected"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 101 }), T0 + 120, D("objected"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 102 }), T0 + 120, D("objected"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 110 + 7 * DAY }), T0 + 110 + 7 * DAY, A("removal_pending"));
});

scenario("rule7_pending_dropped_on_removal", ["7"], "Once a removal applies, every pending statement signed by R2 is dropped.", (c) => {
  twoRoots(c);
  c.inbound(W2, st.coguardian(R2, R3, { issuedAt: T0 + 90 }), T0 + 100, A("coguardian_pending"));
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100 });
  c.inbound(W1, rm, T0 + 110, A("removal_pending"));
  c.token(tokenHex("pd"), dU(R2), T0 + 120);
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("pd"), T0 + 130), T0 + 130, A("removal_confirmed"));
  c.tick(T0 + 110 + H72);
});

scenario("rule7_repin", ["7", "2"], "Re-pinning a removed root needs a co-guardian newer than the removal; a same-second one loses. Its old working keys stay departed.", (c) => {
  twoRoots(c);
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60, A("removal_applied"));
  c.inbound(W1, st.coguardian(R1, R2, { issuedAt: T0 + 50 }), T0 + 70, D("not_newer_than_departure"));
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 51 });
  c.inbound(W1, cg, T0 + 70, A("coguardian_pending"));
  c.token(tokenHex("repin"), dG(R2), T0 + 80);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("repin"), T0 + 90, st.approval(R2, W2, { issuedAt: T0 + 60 })), T0 + 90, D("key_departed"));
  c.confirm(W3, confirmEv(W3, cg.id, tokenHex("repin"), T0 + 90, st.approval(R2, W3, { issuedAt: T0 + 60 })), T0 + 90, A("coguardian_applied"));
});

scenario("rule7_never_no_roots", ["7"], "A removal that would leave no root is refused, when it would take effect too.", (c) => {
  twoRoots(c);
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100 });
  c.inbound(W1, rm, T0 + 110, A("removal_pending"));
  c.token(tokenHex("nr"), dU(R2), T0 + 120);
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("nr"), T0 + 130), T0 + 130, A("removal_confirmed"));
  c.inbound(W1, st.removal(R1, R1, { issuedAt: T0 + 140 }), T0 + 150, A("removal_applied"));
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 160 }), T0 + 170, D("last_root"));
});

scenario("rule8_move", ["8", "1", "4"], "A move takes effect after presence by an R2-bound install; R stays pinned until then. The confirming W travels; R's other keys depart and only R2 may re-bind them.", (c) => {
  pairW1(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
  const mv = st.move(R1, R3, { issuedAt: T0 + 100 });
  c.inbound(W1, mv, T0 + 110, A("move_pending"));
  c.inbound(W3, st.approval(R3, W3, { issuedAt: T0 + 101 }), T0 + 111, D("signer_not_pinned"));
  c.clause(R1, T0 + 112, T0 + 112, A("clause_accepted"));
  c.token(tokenHex("mv"), dM(R3), T0 + 120);
  c.confirm(W1, confirmEv(W1, mv.id, tokenHex("mv"), T0 + 130, st.approval(R3, W1, { issuedAt: T0 + 105 })), T0 + 130, A("move_applied"));
  c.clause(R1, T0 + 200, T0 + 210, D("not_signer"));
  c.clause(W1b, T0 + 200, T0 + 210, D("not_signer"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 200 }), T0 + 210, D("signer_not_pinned"));
  c.inbound(W1b, st.approval(R3, W1b, { issuedAt: T0 + 200 }), T0 + 210, A("approval_rebound"));
  c.clause(W1b, T0 + 211, T0 + 212, A("clause_accepted"));
  c.inbound(W1, st.coguardian(R3, R1, { issuedAt: T0 + 100 }), T0 + 213, D("not_newer_than_departure"));
});

scenario("rule8_move_cancelled", ["8", "pending"], "R can cancel its pending move; a move signed by a working key is not a root statement.", (c) => {
  pairW1(c);
  c.inbound(W1, st.move(W1, R3, { issuedAt: T0 + 101 }), T0 + 110, D("signer_not_pinned"));
  c.inbound(W1, st.move(R1, R3, { issuedAt: T0 + 100 }), T0 + 110, A("move_pending"));
  c.inbound(W1, st.cancel(R1, dM(R3), { issuedAt: T0 + 100 }), T0 + 120, A("cancel_applied"));
});

scenario("pending_expiry", ["pending"], "expiresAt is clamped to 7 days after issuedAt (7 days without one); a pending statement expires at it; one already expired is dropped.", (c) => {
  pairW1(c);
  const long = st.coguardian(R1, R2, { issuedAt: T0, expiresAt: T0 + 30 * DAY });
  c.inbound(W1, long, T0 + 10, A("coguardian_pending"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0, expiresAt: T0 + 100 }), T0 + 10, A("coguardian_pending"));
  c.inbound(W1, st.move(R1, R4, { issuedAt: T0 }), T0 + 10, A("move_pending"));
  c.tick(T0 + 99);
  c.tick(T0 + 100);
  c.tick(T0 + 7 * DAY);
  c.inbound(W1, st.coguardian(R1, R5, { issuedAt: T0 - 7 * DAY }), T0 + 7 * DAY, D("expired"));
  c.inbound(W1, long, T0 + 7 * DAY, D("duplicate"));
  const same = st.coguardian(R1, R2, { issuedAt: T0 });
  c.inbound(W1, same, T0 + 7 * DAY, D(same.id < long.id ? "expired" : "tie_not_newer"));
});

scenario("removal_expires_before_effect", ["7", "pending"], "A removal whose expiresAt comes before issuedAt + 72 h expires instead of taking effect.", (c) => {
  twoRoots(c);
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100, expiresAt: T0 + 100 + DAY });
  c.inbound(W1, rm, T0 + 110, A("removal_pending"));
  c.token(tokenHex("rx"), dU(R2), T0 + 120);
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("rx"), T0 + 130), T0 + 130, A("removal_confirmed"));
  c.tick(T0 + 100 + H72);
});

scenario("removal_effect_equals_expiry", ["7", "pending"], "A confirmed removal whose takesEffectAt equals its expiresAt expires: it takes effect only if strictly before.", (c) => {
  twoRoots(c);
  const rm = st.removal(R1, R2, { issuedAt: T0 + 100, expiresAt: T0 + 100 + H72 });
  c.inbound(W1, rm, T0 + 100, A("removal_pending"));
  c.token(tokenHex("eq"), dU(R2), T0 + 120);
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("eq"), T0 + 130), T0 + 130, A("removal_confirmed"));
  c.tick(T0 + 100 + H72 - 1);
  c.tick(T0 + 100 + H72);
});

scenario("cancel_lifecycle", ["pending", "2"], "A cancel drops pending with its d at or below its issuedAt (a same-second tie included); a later statement is unaffected; only the signer may cancel; a cancel that arrives first is held and binds only its own signer.", (c) => {
  twoRoots(c);
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 50 }), T0 + 60, A("coguardian_pending"));
  c.inbound(W2, st.cancel(R2, dG(R3), { issuedAt: T0 + 51 }), T0 + 60, D("cancel_signer_mismatch"));
  c.inbound(W1, st.cancel(R1, dG(R3), { issuedAt: T0 + 49 }), T0 + 60, A("cancel_held"));
  c.inbound(W1, st.cancel(R1, dG(R3), { issuedAt: T0 + 50 }), T0 + 60, A("cancel_applied"));
  c.inbound(W1, st.cancel(R1, dG(R4), { issuedAt: T0 + 70 }), T0 + 80, A("cancel_held"));
  c.inbound(W1, st.coguardian(R1, R4, { issuedAt: T0 + 70 }), T0 + 80, D("cancelled"));
  c.inbound(W2, st.coguardian(R2, R4, { issuedAt: T0 + 70 }), T0 + 80, A("coguardian_pending"));
  c.inbound(W1, st.coguardian(R1, R4, { issuedAt: T0 + 71 }), T0 + 80, A("coguardian_pending"));
});

scenario("rule9_recipients", ["9"], "A root with no W in the set receives directly; once it has one, its working keys do. A withdrawn key gets one last STATUS.", (c) => {
  c.pair(R1, T0);
  c.inbound(W1, st.approval(R1, W1, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 11, A("approval_applied"));
  c.inbound(R1, st.withdrawal(R1, W1b, { issuedAt: T0 + 1 }), T0 + 12, A("withdrawal_applied"));
  c.statusSent(T0 + 13);
  c.inbound(W1, st.withdrawal(R1, W1, { issuedAt: T0 + 2 }), T0 + 14, A("withdrawal_applied"));
  c.statusSent(T0 + 15);
});

scenario("rule11_relays", ["11"], "Only a root may change the relay list; a working key's RELAYS must leave it unchanged (it may carry the PIN or battery settings). A multi-root ward also sends STATUS on every list replaced within the last 7 days.", (c) => {
  twoRoots(c);
  const L34 = ["wss://relay.three.example", "wss://relay.four.example"];
  c.relays(R2, T0 + 100, L34, T0 + 100, A("relays_applied"));
  c.relays(W1, T0 + 100, ["wss://relay.five.example", "wss://relay.six.example"], T0 + 101, D("not_newer"));
  c.relays(X, T0 + 200, ["wss://relay.five.example", "wss://relay.six.example"], T0 + 201, D("not_signer"));
  c.relays(W1, T0 + 300, ["wss://relay.five.example", "wss://relay.one.example"], T0 + 300, D("relays_root_only"));
  c.relays(W2, T0 + 300, ["wss://relay.five.example", "wss://relay.one.example"], T0 + 300, D("relays_root_only"), { channel: "pasted" });
  c.relays(W1, T0 + 301, [...L34].reverse(), T0 + 301, A("relays_applied"));
  c.relays(W2, T0 + 302, null, T0 + 302, A("relays_applied"));
  c.relays(R1, T0 + 303, ["wss://relay.five.example", "wss://relay.one.example"], T0 + 303, A("relays_applied"));
  c.tick(T0 + 100 + 7 * DAY - 1);
  c.tick(T0 + 100 + 7 * DAY);
  c.tick(T0 + 303 + 7 * DAY);
});

scenario("relays_previous_cap", ["11"], "Only the 8 most recent previous relay lists are kept; a 9th change drops the oldest.", (c) => {
  twoRoots(c);
  // Ten changes: the list from pairing and the first change fall out of statusRelays.
  for (let n = 1; n <= 10; n++) {
    c.relays(R1, T0 + 100 + n, [`wss://relay.n${n}.example`, `wss://relay.m${n}.example`], T0 + 100 + n, A("relays_applied"));
  }
  c.tick(T0 + 110 + 7 * DAY);
});

scenario("relays_existing_checks", ["11", "offline relay recovery"], "The existing RELAYS checks apply first: no future created_at, at most 600 s old, not before pairing, newer than the last applied.", (c) => {
  pairW1(c);
  c.relays(W1, T0 + 101, RELAYS, T0 + 100, D("future"));
  c.relays(W1, T0 + 100, RELAYS, T0 + 701, D("stale"));
  c.relays(W1, T0 - 1, RELAYS, T0 + 10, D("pairing_floor"));
  c.relays(W1, T0 + 100, RELAYS, T0 + 100, D("seal_mismatch"), { seal: X });
  c.relays(R1, T0 + 100, ["wss://relay.three.example", "wss://relay.four.example"], T0 + 100, A("relays_applied"), { channel: "pasted" });
  c.relays(R1, T0 + 100, RELAYS, T0 + 110, D("not_newer"));
});

scenario("relays_guardian_pin", ["presence confirmation", "11"], "guardianPin is accepted only from a gift-wrapped RELAYS; a pasted relay code carrying one is refused whole. A PIN-only RELAYS (no relay list) from any signer is not refused for its age, nor for being ahead of the ward clock, and has its own replay floor per signer.", (c) => {
  pairW1(c);
  const pin = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.0").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash")) };
  const pin2 = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.1").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash2")) };
  c.relays(W1, T0 + 100, RELAYS, T0 + 100, D("pin_not_wrapped"), { channel: "pasted", guardianPin: pin });
  c.relays(W1, T0 + 100, RELAYS, T0 + 100, D("bad_pin_record"), { guardianPin: { ...pin, v: 2 } });
  c.relays(W1, T0 + 100, RELAYS, T0 + 100, A("relays_applied"), { guardianPin: pin });
  c.relays(W1, T0 + 200, null, T0 + 200 + DAY, A("relays_applied"), { guardianPin: pin2 });
  c.relays(W1, T0 + 200, null, T0 + 200 + DAY, D("not_newer"), { guardianPin: pin2 });
  c.relays(W1, T0 + DAY + 300, null, T0 + DAY + 300, D("pin_not_wrapped"), { channel: "pasted", guardianPin: pin });
  c.relays(W1, T0 + DAY + 401, null, T0 + DAY + 400, A("relays_applied"), { guardianPin: pin });
  c.relays(W1, T0 + DAY + 401, null, T0 + DAY + 402, D("not_newer"), { guardianPin: pin });
  c.relays(W1, T0 + DAY + 300, RELAYS, T0 + DAY + 901, D("stale"), { guardianPin: pin });
  c.relays(R1, T0 + DAY + 500, null, T0 + DAY + 500, A("relays_applied"), { guardianPin: pin });
});


scenario("pin_ahead_then_relay_code", ["11", "presence confirmation"], "A PIN-only RELAYS from a guardian phone ahead of the ward clock is accepted. Its floor is its signer's own, not the relay list's, so a relay code pasted afterwards, dated before the PIN, is not taken as already used; a resent PIN never blocks one either. Another signer's PIN has its own floor.", (c) => {
  pairW1(c);
  const pin = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.2").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash3")) };
  const RELAYS2 = ["wss://relay.three.example", "wss://relay.four.example"];
  c.relays(W1, T0 + 400, null, T0 + 100, A("relays_applied"), { guardianPin: pin });
  c.relays(W1, T0 + 400, null, T0 + 110, D("not_newer"), { guardianPin: pin });
  c.relays(R1, T0 + 150, RELAYS2, T0 + 160, A("relays_applied"), { channel: "pasted" });
  c.relays(W1, T0 + 401, null, T0 + 170, A("relays_applied"), { guardianPin: pin });
  c.relays(R1, T0 + 180, RELAYS, T0 + 190, A("relays_applied"), { channel: "pasted" });
  c.relays(R1, T0 + 180, RELAYS, T0 + 195, D("not_newer"), { channel: "pasted" });
  c.relays(R1, T0 + 200, null, T0 + 200, A("relays_applied"), { guardianPin: pin });
  c.relays(W1, T0 + 99, null, T0 + 210, D("not_newer"), { guardianPin: pin });
});

scenario("pin_older_after_newer", ["11", "presence confirmation"], "The PIN is ordered like a clause, across signers: a PIN is taken only above the PIN's bar, the effective time min(created_at, now) of the PIN in force, which never falls. An older PIN from another signer, processed after a newer one, is refused, and a RELAYS with a relay list can't carry it in either; a newer one from that signer is taken.", (c) => {
  pairW1(c);
  const older = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.3").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash4")) };
  const newer = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.4").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash5")) };
  const RELAYS2 = ["wss://relay.three.example", "wss://relay.four.example"];
  c.relays(W1, T0 + 200, null, T0 + 300, A("relays_applied"), { guardianPin: newer });
  c.relays(R1, T0 + 100, null, T0 + 310, D("not_newer"), { guardianPin: older });
  c.relays(R1, T0 + 150, RELAYS2, T0 + 320, D("not_newer"), { guardianPin: older });
  c.relays(R1, T0 + 301, null, T0 + 330, A("relays_applied"), { guardianPin: older });
});

scenario("pin_after_clock_corrected", ["11", "presence confirmation"], "A PIN from a guardian phone ahead of the ward clock sets the bar at the ward clock and its signer's floor at its created_at. Once the phone's clock is corrected, a PIN dated by the raw clock is at or below that floor and refused; the install's rising counter dates it above, so it is taken, and the bar never falls.", (c) => {
  pairW1(c);
  const first = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.5").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash6")) };
  const second = { v: 1, salt: bytesToHex(seedBytes("guardians.pin.salt.6").slice(0, 16)), hash: bytesToHex(seedBytes("guardians.pin.scenario.hash7")) };
  c.relays(W1, T0 + 1000, null, T0 + 20, A("relays_applied"), { guardianPin: first });
  c.relays(W1, T0 + 40, null, T0 + 40, D("not_newer"), { guardianPin: second });
  c.relays(W1, T0 + 1001, null, T0 + 40, A("relays_applied"), { guardianPin: second });
  c.relays(R1, T0 + 30, null, T0 + 50, D("not_newer"), { guardianPin: first });
});

scenario("coguardian_and_move_same_root", ["6", "8", "7"], "A co-guardian and a move naming the same R2: once one pins R2 the other is dropped, so R2 is never pinned twice and a later self-removal can't empty the ward.", (c) => {
  pairW1(c);
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 10 });
  const mv = st.move(R1, R2, { issuedAt: T0 + 11 });
  c.inbound(W1, cg, T0 + 20, A("coguardian_pending"));
  c.inbound(W1, mv, T0 + 20, A("move_pending"));
  c.token(tokenHex("dup1"), null, T0 + 30);
  c.token(tokenHex("dup2"), null, T0 + 30);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("dup1"), T0 + 40, st.approval(R2, W2, { issuedAt: T0 })), T0 + 40, A("coguardian_applied"));
  c.confirm(W3, confirmEv(W3, mv.id, tokenHex("dup2"), T0 + 41, st.approval(R2, W3, { issuedAt: T0 })), T0 + 41, D("not_pending"));
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60, A("removal_applied"));
  c.inbound(W1, st.removal(R1, R1, { issuedAt: T0 + 61 }), T0 + 70, D("last_root"));
});

scenario("confirm_rechecks_root_role", ["presence confirmation", "1"], "A confirmation re-checks R2: if R2 has become a working key meanwhile, the co-guardian can't pin it.", (c) => {
  pairW1(c);
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 10 });
  c.inbound(W1, cg, T0 + 20, A("coguardian_pending"));
  c.inbound(R2, st.approval(R1, R2, { issuedAt: T0 + 11 }), T0 + 21, A("approval_applied"));
  c.token(tokenHex("role"), null, T0 + 30);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("role"), T0 + 40, st.approval(R2, W2, { issuedAt: T0 })), T0 + 40, D("key_role_conflict"));
});

scenario("removal_notice_from_receipt", ["7"], "The 72-hour notice runs from the later of issuedAt and the ward's receipt, so a backdated removal still waits 72 hours.", (c) => {
  twoRoots(c);
  const now = T0 + 10 * DAY;
  const rm = st.removal(R1, R2, { issuedAt: now - H72 - 60 });
  c.inbound(W1, rm, now, A("removal_pending"));
  c.token(tokenHex("back"), dU(R2), now + 1);
  c.confirm(W1, confirmEv(W1, rm.id, tokenHex("back"), now + 2), now + 2, A("removal_confirmed"));
  c.tick(now + H72 - 1);
  c.tick(now + H72);
});

scenario("objection_below_pending_removal", ["7"], "An objection drops the pending removal by its p whatever the two issuedAt (here the objection's is older).", (c) => {
  twoRoots(c);
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 300 }), T0 + 100, A("removal_pending"));
  c.inbound(R2, st.objection(R2, dU(R2), R1, { issuedAt: T0 + 160 }), T0 + 110, A("objection_applied"));
  c.tick(T0 + 110 + H72 + 1);
});

scenario("removal_reissued_after_objection", ["7"], "After an objection, the same remover's new removals of the objector are refused for 7 days from the objection's receipt.", (c) => {
  twoRoots(c);
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 100, A("removal_pending"));
  c.inbound(R2, st.objection(R2, dU(R2), R1, { issuedAt: T0 + 101 }), T0 + 110, A("objection_applied"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 200 }), T0 + 200, D("objected"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 110 + 7 * DAY - 1 }), T0 + 110 + 7 * DAY - 1, D("objected"));
  c.inbound(W1, st.removal(R1, R2, { issuedAt: T0 + 110 + 7 * DAY }), T0 + 110 + 7 * DAY, A("removal_pending"));
});

scenario("clause_replay_after_departure", ["4"], "A relay can't restore an older rule from a current signer: the departed signer's clause keeps a floor of min(issuedAt, departure time).", (c) => {
  twoRoots(c);
  c.clause(W1, T0 + 100, T0 + 100, A("clause_accepted"), { kind: "apps" });
  c.clause(W2, T0 + 200, T0 + 200, A("clause_accepted"), { kind: "apps" });
  c.inbound(R2, st.withdrawal(R2, W2, { issuedAt: T0 + 210 }), T0 + 210, A("withdrawal_applied"));
  c.clause(W1, T0 + 100, T0 + 220, D("not_newer"), { kind: "apps" });
  c.clause(W1, T0 + 200, T0 + 220, D("not_newer"), { kind: "apps" });
  c.clause(W1, T0 + 201, T0 + 220, A("clause_accepted"), { kind: "apps" });
  c.clause(W2, T0 + 500, T0 + 230, D("not_signer"), { kind: "schedule" });
  c.clause(W1, T0 + 500, T0 + 230, A("clause_accepted"), { kind: "schedule" });
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 + 240 }), T0 + 240, A("approval_applied"));
  c.clause(W1b, T0 + 520, T0 + 240, A("clause_accepted"), { kind: "schedule" });
  c.inbound(W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 250 }), T0 + 250, A("withdrawal_applied"));
  c.clause(W1, T0 + 251, T0 + 260, D("replay"), { kind: "schedule" });
  c.clause(R1, T0 + 251, T0 + 260, A("clause_accepted"), { kind: "schedule" });
});

scenario("departed_root_as_working_key", ["4", "7"], "A removed root can't come back as another root's working key.", (c) => {
  twoRoots(c);
  c.inbound(W2, st.removal(R2, R2, { issuedAt: T0 + 50 }), T0 + 60, A("removal_applied"));
  c.inbound(R2, st.approval(R1, R2, { issuedAt: T0 + 70 }), T0 + 80, D("key_departed"));
  c.clause(R2, T0 + 81, T0 + 82, D("not_signer"));
});

scenario("withdrawal_of_move_departed_key", ["3", "8"], "A key that departed with a move can be withdrawn for good by the new root, or by its old root once that is pinned again.", (c) => {
  pairW1(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
  c.inbound(W3, st.approval(R1, W3, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
  const mv = st.move(R1, R3, { issuedAt: T0 + 100 });
  c.inbound(W1, mv, T0 + 110, A("move_pending"));
  c.token(tokenHex("mvw"), null, T0 + 120);
  c.confirm(W1, confirmEv(W1, mv.id, tokenHex("mvw"), T0 + 130, st.approval(R3, W1, { issuedAt: T0 + 105 })), T0 + 130, A("move_applied"));
  c.inbound(W1, st.withdrawal(R3, W1b, { issuedAt: T0 + 140 }), T0 + 150, A("withdrawal_applied"));
  c.inbound(W1b, st.approval(R3, W1b, { issuedAt: T0 + 160 }), T0 + 170, D("key_departed"));
  const cg = st.coguardian(R3, R1, { issuedAt: T0 + 180 });
  c.inbound(W1, cg, T0 + 190, A("coguardian_pending"));
  c.token(tokenHex("mvw2"), dG(R1), T0 + 200);
  c.confirm(W4, confirmEv(W4, cg.id, tokenHex("mvw2"), T0 + 210, st.approval(R1, W4, { issuedAt: T0 + 185 })), T0 + 210, A("coguardian_applied"));
  c.inbound(W2, st.withdrawal(R2, W3, { issuedAt: T0 + 220 }), T0 + 230, D("seal_mismatch"));
  c.inbound(W4, st.withdrawal(R1, W3, { issuedAt: T0 + 220 }), T0 + 230, A("withdrawal_applied"));
});

scenario("timer_tie_mutual_removals", ["7", "timers"], "Two confirmed removals due the same second (R1 removes R2, R2 removes R1): the lower d applies first, and the other, signed by the removed root, is dropped with it.", (c) => {
  twoRoots(c);
  const a = st.removal(R1, R2, { issuedAt: T0 + 100 });
  const b = st.removal(R2, R1, { issuedAt: T0 + 100 });
  c.inbound(W1, a, T0 + 100, A("removal_pending"));
  c.inbound(W2, b, T0 + 100, A("removal_pending"));
  c.token(tokenHex("mut1"), null, T0 + 110);
  c.token(tokenHex("mut2"), null, T0 + 110);
  c.confirm(W1, confirmEv(W1, a.id, tokenHex("mut1"), T0 + 120), T0 + 120, A("removal_confirmed"));
  c.confirm(W2, confirmEv(W2, b.id, tokenHex("mut2"), T0 + 120), T0 + 120, A("removal_confirmed"));
  c.tick(T0 + 100 + H72);
});

scenario("token_dropped_with_its_statement", ["presence confirmation", "pending"], "A token bound to a d goes when that pending statement is cancelled; a new pending statement with the same d needs a new token.", (c) => {
  pairW1(c);
  c.inbound(W1, st.coguardian(R1, R2, { issuedAt: T0 }), T0 + 10, A("coguardian_pending"));
  c.token(tokenHex("bound"), dG(R2), T0 + 20);
  c.inbound(W1, st.cancel(R1, dG(R2), { issuedAt: T0 }), T0 + 30, A("cancel_applied"));
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 1 });
  c.inbound(W1, cg, T0 + 40, A("coguardian_pending"));
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("bound"), T0 + 50, st.approval(R2, W2, { issuedAt: T0 })), T0 + 50, D("token_invalid"));
  c.token(tokenHex("bound"), dG(R2), T0 + 60);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("bound"), T0 + 70, st.approval(R2, W2, { issuedAt: T0 })), T0 + 70, A("coguardian_applied"));
});

scenario("removal_sealed_by_removed_roots_key", ["seal exception 2", "7"], "R1's removal of R2 sealed by R2's working key is refused: exception 2 needs a W bound to the signer.", (c) => {
  twoRoots(c);
  c.inbound(W2, st.removal(R1, R2, { issuedAt: T0 + 100 }), T0 + 110, D("seal_mismatch"));
});

scenario("cancel_per_signer", ["pending", "2"], "Cancels are kept per signer: another root's held cancel for the same d does not shadow the signer's own cancel.", (c) => {
  twoRoots(c);
  c.inbound(W2, st.cancel(R2, dG(R3), { issuedAt: T0 + 400 }), T0 + 150, A("cancel_held"));
  c.inbound(W1, st.coguardian(R1, R3, { issuedAt: T0 + 200 }), T0 + 160, A("coguardian_pending"));
  c.inbound(W1, st.cancel(R1, dG(R3), { issuedAt: T0 + 300 }), T0 + 170, A("cancel_applied"));
});

scenario("same_second_tie_lowest_id", ["2"], "Two statements with the same d and issuedAt: the lower event id wins whichever arrives first.", (c) => {
  twoRoots(c);
  const x = st.removal(R1, R2, { issuedAt: T0 + 100, label: "x" });
  const y = st.removal(R1, R2, { issuedAt: T0 + 100, label: "y" });
  const [lo, hi] = x.id < y.id ? [x, y] : [y, x];
  c.inbound(W1, hi, T0 + 110, A("removal_pending"));
  c.inbound(W1, lo, T0 + 111, A("removal_pending"));
  c.inbound(W1, hi, T0 + 112, D("tie_not_newer"));
});

scenario("confirm_clears_held_withdrawals", ["3", "10"], "Binding W at a confirmation discards other roots' held withdrawals for W, as an ordinary approval does.", (c) => {
  pairW1(c);
  c.inbound(R1, st.withdrawal(R1, W2, { issuedAt: T0 }), T0 + 5, A("withdrawal_held"));
  const cg = st.coguardian(R1, R2, { issuedAt: T0 + 5 });
  c.inbound(W1, cg, T0 + 10, A("coguardian_pending"));
  c.token(tokenHex("clr"), null, T0 + 20);
  c.confirm(W2, confirmEv(W2, cg.id, tokenHex("clr"), T0 + 30, st.approval(R2, W2, { issuedAt: T0 })), T0 + 30, A("coguardian_applied"));
});

scenario("giftwrap_setup", ["seal exceptions"], "Setup for giftwrap.json: paired by W1 (R1), with W1b approved.", (c) => {
  pairW1(c);
  c.inbound(W1b, st.approval(R1, W1b, { issuedAt: T0 }), T0 + 10, A("approval_applied"));
});

write("scenarios.json", { version: 1, keys: keysJson(), scenarios });

// --- giftwrap.json -----------------------------------------------------------
{
  function giftWrap(label, sealer, inner, recipientPk) {
    const seal = signWith(sealer.sk, {
      kind: 13,
      created_at: T0,
      tags: [],
      content: nip44.v2.encrypt(JSON.stringify(inner), nip44.v2.utils.getConversationKey(sealer.sk, recipientPk), seedBytes(`guardians.giftwrap.${label}.seal_nonce`)),
    });
    const eph = seedSecretKey(`guardians.giftwrap.${label}.wrap_ephemeral_key`);
    const wrap = signWith(eph, {
      kind: 1059,
      created_at: T0,
      tags: [["p", recipientPk]],
      content: nip44.v2.encrypt(JSON.stringify(seal), nip44.v2.utils.getConversationKey(eph, recipientPk), seedBytes(`guardians.giftwrap.${label}.wrap_nonce`)),
    });
    // Prove it opens, as NIP-59 prescribes.
    const s = JSON.parse(nip44.v2.decrypt(wrap.content, nip44.v2.utils.getConversationKey(WARD.sk, wrap.pubkey)));
    const i = JSON.parse(nip44.v2.decrypt(s.content, nip44.v2.utils.getConversationKey(WARD.sk, s.pubkey)));
    if (s.pubkey !== sealer.pk || i.id !== inner.id) throw new Error(`giftwrap ${label} does not round-trip`);
    return wrap;
  }
  const base = finalStates.giftwrap_setup;
  const now = T0 + 20;
  const entries = [];
  const add = (name, description, sealer, inner, want) => {
    const wrap = giftWrap(name, sealer, inner, WARD.pk);
    const r = G.applyInbound(base, { sealAuthor: sealer.pk, inner }, now);
    if (r.outcome.result !== want[0] || r.outcome.reason !== want[1]) {
      throw new Error(`giftwrap ${name}: want ${want}, got ${r.outcome.result}/${r.outcome.reason}`);
    }
    entries.push({
      name,
      description,
      wrap,
      expected_seal_author: sealer.pk,
      expected_inner: inner,
      setup: "giftwrap_setup",
      now,
      expect: { result: r.outcome.result, reason: r.outcome.reason, ticked: r.outcome.ticked, departed: r.outcome.departed, state: G.projectState(r.state, now) },
    });
  };
  add("withdrawal_by_root_sealed_by_working_key", "Exception 2: R1 signs the withdrawal of W1b; W1 (bound to R1, in the signer set) seals it.", W1, st.withdrawal(R1, W1b, { issuedAt: T0 + 1 }), A("withdrawal_applied"));
  add("approval_sealed_by_named_working_key", "Exception 1: R1 signs the approval of W3; W3 itself seals it (proof of possession).", W3, st.approval(R1, W3, { issuedAt: T0 + 1, label: "Sam's tablet" }), A("approval_applied"));
  add("approval_sealed_by_other_working_key", "Exception 1 negative: R1's approval of W3 sealed by W1 is ignored.", W1, st.approval(R1, W3, { issuedAt: T0 + 1 }), D("seal_mismatch"));
  add("coguardian_by_root_sealed_by_working_key", "Exception 2: R1 signs a co-guardian for R2; W1b seals it.", W1b, st.coguardian(R1, R2, { issuedAt: T0 + 1 }), A("coguardian_pending"));
  write("giftwrap.json", {
    version: 1,
    recipient_secret: bytesToHex(WARD.sk),
    recipient_pubkey: WARD.pk,
    keys: keysJson(),
    vectors: entries,
  });
}

// --- roster.json -------------------------------------------------------------
{
  const kid = bytesToHex(seedBytes("guardians.roster.subject.kid"));
  const kid2 = bytesToHex(seedBytes("guardians.roster.subject.kid2"));
  const m = (label) => bytesToHex(seedBytes(`guardians.roster.machine.${label}`));
  // A CLAUSE as a roster carries it: the signed inner event (kind 31113).
  const rule = (by, kind, issuedAt, subject = kid) =>
    signWith(by.sk, { kind: G.CHARTER_CLAUSE, created_at: issuedAt, tags: [MARKER], content: JSON.stringify({ kind, ...(subject ? { subject } : {}), issuedAt, body: {} }) });
  const localRule = rule(W1, "schedule", T0 + 600);
  const local = {
    children: [
      {
        subject: kid, issuedAt: T0, name: "Rob", colour: "#000000",
        devices: [
          { machine: m("laptop"), platform: "linux", name: "laptop", pairedHere: true, statusSeen: false },
          { machine: m("tablet"), platform: "android", name: "tablet", pairedHere: false, statusSeen: true },
          { machine: m("old"), platform: "android", name: "old phone", pairedHere: false, statusSeen: false },
        ],
        rules: [localRule],
        deviceRules: [{ machine: m("laptop"), rule: rule(W1, "update", T0 + 150) }],
      },
    ],
  };
  const now = T0 + 1000;
  const reported = [{ root: R1.pk, keys: [W1.pk] }, { root: R2.pk, keys: [W2.pk] }];
  const child = (subject, issuedAt, devices, name = "Robin") => ({ subject, issuedAt, name, colour: "#123456", devices });
  const dev = (label, platform = "android") => ({ machine: m(label), platform, name: label });
  const rosterEv = (from, root, children, extra = {}) =>
    signWith(from.sk, G.familyRosterTemplate({ children, approval: st.approval(root, from, { issuedAt: T0 }), createdAt: now, ...extra }));
  const vectors = [];
  const add = (name, description, sealer, inner, rep, want) => {
    const r = G.mergeRoster(local, { sealAuthor: sealer.pk, inner }, rep, now);
    const got = r.ok ? "ok" : r.reason;
    if (got !== want) throw new Error(`roster ${name}: want ${want}, got ${got}`);
    vectors.push({
      name, description, inbound: { sealAuthor: sealer.pk, inner }, reported: rep, now,
      expect: r.ok ? { ok: true, roster: r.roster, updated: r.updated, ignoredRules: r.ignoredRules } : { ok: false, reason: r.reason },
    });
  };
  add("newer_replaces_keeps_confirmed", "A newer record replaces name, colour and devices, but never deletes a device paired here or seen in STATUS; new devices arrive unconfirmed; a new child is added.", W2,
    rosterEv(W2, R2, [child(kid, T0 + 1, [dev("phone"), dev("tablet")]), child(kid2, T0 + 1, [dev("kid2-laptop", "linux")], "Sam")]), reported, "ok");
  add("same_second_keeps_local", "A record with the same issuedAt does not replace the local one.", W2, rosterEv(W2, R2, [child(kid, T0, [dev("phone")])]), reported, "ok");
  add("older_ignored", "An older record is ignored.", W2, rosterEv(W2, R2, [child(kid, T0 - 1, [dev("phone")])]), reported, "ok");
  add("issued_at_at_cap", "issuedAt exactly 300 s ahead is accepted.", W2, rosterEv(W2, R2, [child(kid, now + 300, [])]), reported, "ok");
  add("future", "A child record more than 300 s ahead refuses the whole roster.", W2, rosterEv(W2, R2, [child(kid, now + 301, [])]), reported, "future");
  add("root_not_reported", "The approving root is not reported by the family's wards.", W3, rosterEv(W3, R3, [child(kid, T0 + 1, [])]), reported, "root_not_reported");
  add("key_not_reported", "The sender is no longer listed under its root (withdrawn).", W2, rosterEv(W2, R2, [child(kid, T0 + 1, [])]), [{ root: R1.pk, keys: [W1.pk] }, { root: R2.pk, keys: [] }], "key_not_reported");
  add("seal_mismatch", "The seal author must be the roster's author.", X, rosterEv(W2, R2, [child(kid, T0 + 1, [])]), reported, "seal_mismatch");
  add("approval_names_other_key", "The carried approval must name the sender.", W2,
    signWith(W2.sk, G.familyRosterTemplate({ children: [], approval: st.approval(R1, W1, { issuedAt: T0 }), createdAt: now })), reported, "bad_approval");
  add("approval_missing", "A roster must carry the sender's approval.", W2,
    signWith(W2.sk, { kind: G.CHARTER_FAMILY_ROSTER, created_at: now, tags: [MARKER], content: JSON.stringify({ v: 1, children: [] }) }), reported, "bad_approval");
  const apW2 = st.approval(R2, W2, { issuedAt: T0 });
  add("duplicate_key", "Content is strict JSON.", W2,
    signWith(W2.sk, { kind: G.CHARTER_FAMILY_ROSTER, created_at: now, tags: [MARKER], content: `{"v":1,"v":1,"children":[],"approval":${JSON.stringify(apW2)}}` }), reported, "content_not_json");
  add("bad_version", "v must be 1.", W2,
    signWith(W2.sk, { kind: G.CHARTER_FAMILY_ROSTER, created_at: now, tags: [MARKER], content: JSON.stringify({ v: 2, children: [], approval: apW2 }) }), reported, "bad_version");
  add("bad_children", "A device machine must be 64 lowercase hex.", W2,
    rosterEv(W2, R2, [child(kid, T0 + 1, [{ machine: "ABC", platform: "android", name: "x" }])]), reported, "bad_children");
  add("wrong_kind", "Kind must be 31119.", W2, signWith(W2.sk, { kind: 31118, created_at: now, tags: [MARKER], content: JSON.stringify({ v: 1, children: [], approval: apW2 }) }), reported, "wrong_kind");
  add("bad_signature", "id and sig must verify.", W2, { ...rosterEv(W2, R2, [child(kid, T0 + 1, [])]), sig: "00".repeat(64) }, reported, "bad_signature");
  const withRules = (issuedAt, rules, subject = kid) => rosterEv(W2, R2, [{ ...child(subject, issuedAt, []), rules }]);
  add("rules_merge", "Rules merge per (subject, kind), newest issuedAt, independently of the child record (older here, so left alone). A rule from a key the wards don't report, one that doesn't verify, or one naming another child is ignored, not the roster.", W2,
    withRules(T0 - 1, [
      rule(W2, "schedule", T0 + 500),
      rule(R1, "apps", T0 + 400),
      rule(X, "budget", T0 + 700),
      { ...rule(W2, "times", T0 + 700), sig: "00".repeat(64) },
      rule(W2, "connections", T0 + 700, kid2),
      rule(W2, "learning", T0 + 700, null),
    ]), reported, "ok");
  add("rules_newer_replaces", "A newer rule of a kind replaces the local one; a new child arrives with its rules.", W2,
    rosterEv(W2, R2, [{ ...child(kid, T0, []), rules: [rule(W2, "schedule", T0 + 601)] }, { ...child(kid2, T0 + 1, [], "Sam"), rules: [rule(W1, "apps", T0 + 5, kid2)] }]), reported, "ok");
  add("rules_same_second_lower_id", "Two rules of one kind with the same issuedAt: the lower event id wins, so every install converges.", W2,
    withRules(T0, [rule(W2, "schedule", T0 + 600)]), reported, "ok");
  add("bad_rules", "A child's rules must be a list of objects.", W2,
    withRules(T0, "not a list"), reported, "bad_rules");
  const withDeviceRules = (rules, deviceRules) => rosterEv(W2, R2, [{ ...child(kid, T0, []), rules, deviceRules }]);
  add("device_rules_per_device", "Rules merge per (subject, kind, device): the same kind on two devices is kept for each, beside the family-wide rule of that kind, which is older. A device rule not newer than the family-wide rule is dropped; one without a 64-hex machine is ignored. The local rule the roster leaves out is kept: not carried is not cleared.", W2,
    withDeviceRules([rule(W1, "debugging", T0 + 100)], [
      { machine: m("tablet"), rule: rule(W2, "debugging", T0 + 200) },
      { machine: m("laptop"), rule: rule(W2, "debugging", T0 + 150) },
      { machine: m("phone"), rule: rule(W2, "debugging", T0 + 50) },
      { machine: "XY", rule: rule(W2, "debugging", T0 + 300) },
    ]), reported, "ok");
  add("device_rule_newer_same_device", "A newer device rule replaces the same device's rule of that kind only.", W2,
    withDeviceRules([], [
      { machine: m("tablet"), rule: rule(W2, "maintenance", T0 + 200) },
      { machine: m("tablet"), rule: rule(W1, "maintenance", T0 + 250) },
      { machine: m("laptop"), rule: rule(W2, "maintenance", T0 + 100) },
    ]), reported, "ok");
  add("family_rule_replaces_older_device_rules", "A ward holds one clause per kind, so a family-wide rule replaces the held device rules of its kind that it is newer than (the laptop's); a newer device rule (the tablet's) is taken beside it.", W2,
    withDeviceRules([rule(W1, "update", T0 + 175)], [
      { machine: m("tablet"), rule: rule(W2, "update", T0 + 200) },
    ]), reported, "ok");
  add("bad_device_rules", "A child's deviceRules must be a list of objects.", W2,
    withDeviceRules([], ["not an object"]), reported, "bad_rules");
  write("roster.json", { version: 1, keys: keysJson(), local, vectors });
}

// --- pin.json --------------------------------------------------------------
{
  const P = G.GUARDIAN_PIN_SCRYPT;
  const vectors = [];
  const pins = [["123456", 0], ["000000", 1], ["987654", 2], ["1234", 3]];
  for (const [pin, i] of pins) {
    const salt = bytesToHex(seedBytes(`guardians.pin.salt.${i}`).slice(0, 16));
    const hash = bytesToHex(scrypt(te.encode(pin), hexToBytes(salt), { N: P.N, r: P.r, p: P.p, dkLen: P.dkLen }));
    const record = { v: 1, salt, hash };
    if (!G.validateGuardianPinRecord(record).ok) throw new Error("pin record invalid");
    vectors.push({ pin, salt, hash, record });
  }
  const records = [
    { name: "valid", record: vectors[0].record, valid: true },
    { name: "salt_64_bytes", record: { v: 1, salt: "ab".repeat(64), hash: vectors[0].hash }, valid: true },
    { name: "v_2", record: { v: 2, salt: vectors[0].salt, hash: vectors[0].hash }, valid: false },
    { name: "salt_15_bytes", record: { v: 1, salt: "ab".repeat(15), hash: vectors[0].hash }, valid: false },
    { name: "salt_65_bytes", record: { v: 1, salt: "ab".repeat(65), hash: vectors[0].hash }, valid: false },
    { name: "hash_uppercase", record: { v: 1, salt: vectors[0].salt, hash: vectors[0].hash.toUpperCase() }, valid: false },
    { name: "hash_31_bytes", record: { v: 1, salt: vectors[0].salt, hash: vectors[0].hash.slice(2) }, valid: false },
    { name: "salt_odd_length", record: { v: 1, salt: "a".repeat(33), hash: vectors[0].hash }, valid: false },
  ];
  for (const r of records) if (G.validateGuardianPinRecord(r.record).ok !== r.valid) throw new Error(`pin record ${r.name}`);
  const backoff = [];
  for (let f = 0; f <= 13; f++) backoff.push({ failures: f, waitSeconds: G.pinBackoffSeconds(f) });
  write("pin.json", {
    version: 1,
    scrypt: { N: P.N, r: P.r, p: P.p, dkLen: P.dkLen, input: "utf8(pin)", salt: "hex-decoded record salt" },
    vectors,
    records,
    backoff,
  });
}

console.log("done.");
