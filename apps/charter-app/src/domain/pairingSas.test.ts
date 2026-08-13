import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { nip19 } from "nostr-tools";
import { pairingSas } from "./pairingSas";

const PK = "a".repeat(63) + "b";
const PK2 = "c".repeat(64);
const TOKEN = "0123456789abcdef0123456789abcdef";

/*
 * The frozen cross-language vectors (S6). THREE runtimes derive this code —
 * this module, the hand-written copy in `public/pair/index.html`, and the ward
 * app's Kotlin `PairingSas` — because they cannot share code. The failure to
 * guard against is not "the code is wrong", it is "the screens disagree",
 * which teaches a family the check is broken and to tap through it. All three
 * are pinned to this one file; the Kotlin side reads it off the shared test
 * classpath (`android/app/build.gradle.kts` sourceSets).
 */
const VECTORS = JSON.parse(
  readFileSync(
    resolve(__dirname, "../../../../core/crates/charter-testkit/vectors/pairing/sas_vectors.json"),
    "utf8",
  ),
) as {
  vectors: { name: string; pubkeyHex: string; token: string; code: string }[];
  npubVectors: { pubkeyHex: string; npub: string }[];
};

describe("the frozen cross-language vectors", () => {
  it("has the full set — a truncated file must fail, not assert nothing", () => {
    expect(VECTORS.vectors.length).toBeGreaterThanOrEqual(6);
    expect(VECTORS.npubVectors.length).toBeGreaterThanOrEqual(4);
  });

  it("pairingSas agrees with every one", async () => {
    for (const v of VECTORS.vectors) {
      expect(await pairingSas(v.pubkeyHex, v.token), v.name).toBe(v.code);
    }
  });

  it("nip19.npubEncode agrees with every one", () => {
    for (const v of VECTORS.npubVectors) {
      expect(nip19.npubEncode(v.pubkeyHex)).toBe(v.npub);
    }
  });
});

describe("pairingSas", () => {
  it("is six digits in two groups", async () => {
    expect(await pairingSas(PK, TOKEN)).toMatch(/^\d{3} \d{3}$/);
  });

  it("is stable for the same guardian and token", async () => {
    expect(await pairingSas(PK, TOKEN)).toBe(await pairingSas(PK, TOKEN));
  });

  it("does not care how the pubkey was cased", async () => {
    expect(await pairingSas(PK.toUpperCase(), TOKEN)).toBe(await pairingSas(PK, TOKEN));
  });

  it("changes when the guardian changes — the attack it exists for", async () => {
    expect(await pairingSas(PK2, TOKEN)).not.toBe(await pairingSas(PK, TOKEN));
  });

  // A code tied only to the pubkey would repeat for every pairing this
  // guardian ever does, so one glimpse of it would be replayable forever.
  it("changes when the token changes, so a code seen once cannot be reused", async () => {
    expect(await pairingSas(PK, "f".repeat(32))).not.toBe(await pairingSas(PK, TOKEN));
  });
});

/*
 * The anti-drift check (S6). `public/pair/index.html` is static HTML with no
 * bundler, so it carries its own copy of the derivation. Two screens showing
 * DIFFERENT codes for the same link would be worse than showing none — it
 * would teach a family that the check is broken and to tap through it. So the
 * page's own function is pulled out of the file and run head-to-head here.
 */
describe("the /pair page's copy of the derivation", () => {
  const html = readFileSync(
    resolve(__dirname, "../../public/pair/index.html"),
    "utf8",
  );

  /** Lift `function sasFrom(…) { … }` out of the page and make it callable. */
  function pageSasFrom(): (pk: string, token: string) => Promise<string> {
    const start = html.indexOf("function sasFrom(");
    expect(start, "the /pair page must still define sasFrom").toBeGreaterThan(-1);
    // Balance braces from the function's opening `{` to its close.
    let depth = 0;
    let end = -1;
    for (let i = html.indexOf("{", start); i < html.length; i++) {
      if (html[i] === "{") depth++;
      else if (html[i] === "}" && --depth === 0) {
        end = i + 1;
        break;
      }
    }
    expect(end, "sasFrom must be a balanced function body").toBeGreaterThan(-1);
    const src = html.slice(start, end);
    return new Function(`${src}; return sasFrom;`)() as (
      pk: string,
      token: string,
    ) => Promise<string>;
  }

  it("agrees with src/domain/pairingSas.ts", async () => {
    const fromPage = pageSasFrom();
    for (const [pk, token] of [
      [PK, TOKEN],
      [PK2, TOKEN],
      [PK, "f".repeat(32)],
      [PK, ""],
    ] as const) {
      expect(await fromPage(pk, token)).toBe(await pairingSas(pk, token));
    }
  });

  it("agrees with the frozen vectors the Kotlin side is pinned to", async () => {
    const fromPage = pageSasFrom();
    for (const v of VECTORS.vectors) {
      expect(await fromPage(v.pubkeyHex, v.token), v.name).toBe(v.code);
    }
  });
});

/*
 * And the npub the page renders must be the SAME string Kintrinsic shows the
 * parent — the page hand-rolls bech32 because it cannot import nostr-tools.
 */
describe("the /pair page's npub encoder", () => {
  const html = readFileSync(
    resolve(__dirname, "../../public/pair/index.html"),
    "utf8",
  );

  function pageNpubEncode(): (hex: string) => string {
    const start = html.indexOf("function npubEncode(");
    expect(start, "the /pair page must still define npubEncode").toBeGreaterThan(-1);
    let depth = 0;
    let end = -1;
    for (let i = html.indexOf("{", start); i < html.length; i++) {
      if (html[i] === "{") depth++;
      else if (html[i] === "}" && --depth === 0) {
        end = i + 1;
        break;
      }
    }
    const src = html.slice(start, end);
    return new Function(`${src}; return npubEncode;`)() as (hex: string) => string;
  }

  it("matches nip19.npubEncode", () => {
    const encode = pageNpubEncode();
    for (const pk of [PK, PK2, "0".repeat(64), "f".repeat(64), "0123456789abcdef".repeat(4)]) {
      expect(encode(pk)).toBe(nip19.npubEncode(pk));
    }
  });

  it("matches the frozen npub vectors the Kotlin side is pinned to", () => {
    const encode = pageNpubEncode();
    for (const v of VECTORS.npubVectors) {
      expect(encode(v.pubkeyHex)).toBe(v.npub);
    }
  });
});
