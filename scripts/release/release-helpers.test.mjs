// node --test scripts/release/
import test from "node:test";
import assert from "node:assert/strict";
import {
  assertPinnedCert,
  assertVersionCodeBump,
  buildBlossomAuth,
  buildManifest,
  buildReleaseEvent,
  channelAndVersionFromTag,
  CI_RELEASE_TAG_RE,
  DEBUG_CERT_SHA256,
  extractAttestedCommit,
  formatManifest,
  githubRelease,
  isCanonicalBlossomUrl,
  isGithubReleaseUrl,
  MANIFEST_FILES,
  orderReleaseUrls,
  parseApksignerLineage,
  parseApksignerVerify,
  parseSha256Sums,
  releaseRelaysWithExtra,
  RELEASE_CERT_SHA256,
  RELEASE_RELAYS,
  verifyRotatedApkCert,
} from "./release-helpers.mjs";

const SHA = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c";
const CERT = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";
const TS = 1_754_900_000;

const base = {
  channel: "charter-apk",
  versionName: "0.6.9",
  versionCode: 40,
  sha256: SHA,
  sizeBytes: 27_693_181,
  urls: [`https://blossom.example/${SHA}`, `https://mirror.example/${SHA}`],
  certSha256: CERT,
  notes: "notes",
  createdAt: TS,
};

test("buildReleaseEvent emits the exact tag shape", () => {
  const ev = buildReleaseEvent(base);
  assert.equal(ev.kind, 30063);
  assert.equal(ev.created_at, TS);
  assert.equal(ev.content, "notes");
  assert.deepEqual(ev.tags, [
    ["d", "charter-apk"],
    ["version", "0.6.9"],
    ["version_code", "40"],
    ["x", SHA],
    ["size", "27693181"],
    ["cert", CERT],
    ["url", `https://blossom.example/${SHA}`],
    ["url", `https://mirror.example/${SHA}`],
  ]);
});

test("charter-deb needs no cert and emits none", () => {
  const ev = buildReleaseEvent({ ...base, channel: "charter-deb", certSha256: undefined });
  assert.ok(!ev.tags.some((t) => t[0] === "cert"));
});

test("invalid fields throw", () => {
  assert.throws(() => buildReleaseEvent({ ...base, channel: "nope" }), /bad channel/);
  assert.throws(() => buildReleaseEvent({ ...base, versionCode: 0 }), /versionCode/);
  assert.throws(() => buildReleaseEvent({ ...base, sha256: SHA.toUpperCase() }), /sha256/);
  assert.throws(() => buildReleaseEvent({ ...base, urls: [] }), /url/);
  assert.throws(
    () => buildReleaseEvent({ ...base, urls: [`http://x.example/${SHA}`] }),
    /https/,
  );
  assert.throws(() => buildReleaseEvent({ ...base, certSha256: undefined }), /cert/);
  assert.throws(() => buildReleaseEvent({ ...base, sizeBytes: -1 }), /sizeBytes/);
});

test("buildBlossomAuth emits BUD-02 shape with a 10-minute expiry", () => {
  const ev = buildBlossomAuth({ sha256: SHA, createdAt: TS });
  assert.equal(ev.kind, 24242);
  assert.deepEqual(ev.tags, [
    ["t", "upload"],
    ["x", SHA],
    ["expiration", String(TS + 600)],
  ]);
  assert.throws(() => buildBlossomAuth({ sha256: "zz", createdAt: TS }), /sha256/);
});

test("isCanonicalBlossomUrl accepts only the blob's root address", () => {
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}`, SHA), true);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}.apk`, SHA), true);
  assert.equal(isCanonicalBlossomUrl(`https://blossom.primal.net/${SHA}.deb`, SHA), true);
  // The 2026-08-27 outage: primal's CDN redirect target is not an address.
  assert.equal(
    isCanonicalBlossomUrl(`https://media.primal.net/uploads2/a/1e/a8/${SHA}`, SHA),
    false,
  );
  assert.equal(isCanonicalBlossomUrl(`http://nostr.download/${SHA}`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${"b".repeat(64)}`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}?x=1`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(`https://nostr.download/${SHA}.tar.gz`, SHA), false);
  assert.equal(isCanonicalBlossomUrl(42, SHA), false);
});

test("buildReleaseEvent refuses a non-canonical mirror url", () => {
  assert.throws(
    () =>
      buildReleaseEvent({
        ...base,
        urls: [`https://media.primal.net/uploads2/a/1e/a8/${SHA}`],
      }),
    /canonical root address/,
  );
});

// ---- GitHub Releases as the primary host ----------------------------------

const GH = "https://github.com/forgesworn/kintrinsic/releases/download";

test("githubRelease names one tag per artifact", () => {
  assert.deepEqual(githubRelease("charter-apk", "0.6.13"), {
    repo: "forgesworn/kintrinsic",
    tag: "ward-v0.6.13",
    asset: "kintrinsic-ward-0.6.13.apk",
    title: "Ward 0.6.13",
    url: `${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`,
  });
  assert.equal(
    githubRelease("mycharter-apk", "0.1.15").url,
    `${GH}/guardian-v0.1.15/kintrinsic-0.1.15.apk`,
  );
  assert.equal(
    githubRelease("charter-deb", "0.7.10").url,
    `${GH}/linux-v0.7.10/kintrinsic_0.7.10_amd64.deb`,
  );
  assert.throws(() => githubRelease("nope", "1.0.0"), /bad channel/);
  assert.throws(() => githubRelease("charter-apk", "../x"), /bad version/);
  assert.throws(() => githubRelease("charter-apk", ""), /bad version/);
});

test("isGithubReleaseUrl accepts only our repo's per-artifact release downloads", () => {
  assert.equal(isGithubReleaseUrl(`${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`), true);
  assert.equal(isGithubReleaseUrl(`${GH}/linux-v0.7.10/kintrinsic_0.7.10_amd64.deb`), true);
  // Another repo, another tag scheme, a query, plain http, a CDN target: no.
  assert.equal(
    isGithubReleaseUrl("https://github.com/evil/kintrinsic/releases/download/ward-v1/x.apk"),
    false,
  );
  assert.equal(isGithubReleaseUrl(`${GH}/v0.6.13/kintrinsic-ward-0.6.13.apk`), false);
  assert.equal(isGithubReleaseUrl(`${GH}/ward-v0.6.13/x.apk?raw=1`), false);
  assert.equal(isGithubReleaseUrl(`http://github.com/forgesworn/kintrinsic/releases/download/ward-v1/x.apk`), false);
  assert.equal(
    isGithubReleaseUrl("https://release-assets.githubusercontent.com/github-production-release-asset/1/x"),
    false,
  );
  assert.equal(isGithubReleaseUrl(`${GH}/ward-v0.6.13/a/b.apk`), false);
  assert.equal(isGithubReleaseUrl(42), false);
});

test("orderReleaseUrls puts GitHub first and keeps Blossom order", () => {
  const gh = `${GH}/ward-v0.6.13/kintrinsic-ward-0.6.13.apk`;
  const a = `https://nostr.download/${SHA}.apk`;
  const b = `https://blossom.primal.net/${SHA}`;
  assert.deepEqual(orderReleaseUrls([a, b, gh, a]), [gh, a, b]);
});

test("buildReleaseEvent carries the GitHub download first, then Blossom", () => {
  const gh = `${GH}/ward-v0.6.9/kintrinsic-ward-0.6.9.apk`;
  const ev = buildReleaseEvent({ ...base, urls: [gh, `https://nostr.download/${SHA}.apk`] });
  assert.deepEqual(
    ev.tags.filter((t) => t[0] === "url"),
    [
      ["url", gh],
      ["url", `https://nostr.download/${SHA}.apk`],
    ],
  );
  assert.throws(
    () =>
      buildReleaseEvent({
        ...base,
        urls: ["https://github.com/evil/fork/releases/download/ward-v0.6.9/x.apk"],
      }),
    /GitHub Release download or/,
  );
});

test("release relays are all public — no relay run by the project is a default", () => {
  assert.ok(RELEASE_RELAYS.length >= 3);
  assert.ok(!RELEASE_RELAYS.includes("wss://relay.trotters.cc"));
});

test("releaseRelaysWithExtra appends CHARTER_EXTRA_RELAYS without duplicating", () => {
  assert.deepEqual(releaseRelaysWithExtra({}), RELEASE_RELAYS);
  assert.deepEqual(
    releaseRelaysWithExtra({ CHARTER_EXTRA_RELAYS: "wss://relay.trotters.cc, not-a-relay" }),
    [...RELEASE_RELAYS, "wss://relay.trotters.cc"],
  );
  assert.deepEqual(
    releaseRelaysWithExtra({ CHARTER_EXTRA_RELAYS: RELEASE_RELAYS[0] }),
    RELEASE_RELAYS,
  );
});

// ---- `--from-draft` verification helpers -----------------------------------

test("channelAndVersionFromTag is the reverse of githubRelease's naming", () => {
  assert.deepEqual(channelAndVersionFromTag("ward-v0.6.13"), {
    channel: "charter-apk",
    version: "0.6.13",
  });
  assert.deepEqual(channelAndVersionFromTag("guardian-v0.1.15"), {
    channel: "mycharter-apk",
    version: "0.1.15",
  });
  assert.deepEqual(channelAndVersionFromTag("linux-v0.7.10"), {
    channel: "charter-deb",
    version: "0.7.10",
  });
  assert.throws(() => channelAndVersionFromTag("v1.0.0"), /matches none of/);
  assert.throws(() => channelAndVersionFromTag("ward-v../x"), /malformed version/);
});

test("DEBUG_CERT_SHA256 is the fielded debug cert (matches lib.sh, assetlinks.json)", () => {
  assert.equal(DEBUG_CERT_SHA256, CERT);
  assert.match(DEBUG_CERT_SHA256, /^[0-9a-f]{64}$/);
});

test("parseSha256Sums parses sha256sum's two-space format", () => {
  const text = `${SHA}  kintrinsic-ward-0.6.13.apk\n`;
  assert.deepEqual(parseSha256Sums(text), new Map([["kintrinsic-ward-0.6.13.apk", SHA]]));
});

test("parseSha256Sums accepts the binary-mode '*' marker and blank lines", () => {
  const text = `\n${SHA} *kintrinsic_0.7.10_amd64.deb\n\n`;
  assert.deepEqual(
    parseSha256Sums(text),
    new Map([["kintrinsic_0.7.10_amd64.deb", SHA]]),
  );
});

test("parseSha256Sums throws on a malformed line", () => {
  assert.throws(() => parseSha256Sums("not-a-hash-line\n"), /malformed SHA256SUMS line/);
});

// Real `apksigner verify -v --print-certs` output, captured 2026-09-27
// against a debug-signed app-release.apk (v2, unrotated) and against the
// same APK re-signed with a throwaway key + a debug -> new-key lineage
// (v3, rotated) — see android/scripts/build-apk.sh / lib.sh.
const NEW_CERT = "28649d948fe1ab894a135785a196ac58b26510c0719084c583d077e315a57cb5";

const DEBUG_VERIFY_OUTPUT = `Verifies
Verified using v1 scheme (JAR signing): false
Verified using v2 scheme (APK Signature Scheme v2): true
Verified using v3 scheme (APK Signature Scheme v3): false
Verified using v3.1 scheme (APK Signature Scheme v3.1): false
Verified using v4 scheme (APK Signature Scheme v4): false
Verified for SourceStamp: false
Number of signers: 1
Signer #1 certificate DN: C=US, O=Android, CN=Android Debug
Signer #1 certificate SHA-256 digest: ${CERT}
Signer #1 key algorithm: RSA
`;

const ROTATED_VERIFY_OUTPUT = `Verifies
Verified using v1 scheme (JAR signing): false
Verified using v2 scheme (APK Signature Scheme v2): false
Verified using v3 scheme (APK Signature Scheme v3): true
Verified using v3.1 scheme (APK Signature Scheme v3.1): false
Verified using v4 scheme (APK Signature Scheme v4): false
Verified for SourceStamp: false
Number of signers: 1
Signer #1 certificate DN: CN=New Test
Signer #1 certificate SHA-256 digest: ${NEW_CERT}
Signer #1 key algorithm: RSA
`;

const ROTATED_LINEAGE_OUTPUT = `Signer #1 in lineage certificate DN: C=US, O=Android, CN=Android Debug
Signer #1 in lineage certificate SHA-256 digest: ${CERT}
Has installed data capability: true
Has rollback capability      : false
Signer #2 in lineage certificate DN: CN=New Test
Signer #2 in lineage certificate SHA-256 digest: ${NEW_CERT}
Has installed data capability: true
Has rollback capability      : false
`;

test("parseApksignerVerify reads v3/signers/cert from real apksigner output", () => {
  assert.deepEqual(parseApksignerVerify(DEBUG_VERIFY_OUTPUT), {
    v3: false,
    signers: 1,
    cert: CERT,
  });
  assert.deepEqual(parseApksignerVerify(ROTATED_VERIFY_OUTPUT), {
    v3: true,
    signers: 1,
    cert: NEW_CERT,
  });
});

test("parseApksignerLineage reads the ordered signer list from real apksigner output", () => {
  assert.deepEqual(parseApksignerLineage(ROTATED_LINEAGE_OUTPUT), [
    { index: 1, sha256: CERT },
    { index: 2, sha256: NEW_CERT },
  ]);
});

test("verifyRotatedApkCert refuses an unrotated (v2, debug-signed) build", () => {
  // --from-draft only ever verifies CI-drafted artifacts, and CI's release
  // environment makes the rotation lineage mandatory (C1/C5) — an
  // alpha-bridge debug build can never reach a draft release, so this
  // function requires v3 unconditionally (unlike lib.sh's
  // android_verify_signing, which stays lenient for the LOCAL alpha-bridge
  // build path).
  assert.throws(
    () => verifyRotatedApkCert({ verifyOutput: DEBUG_VERIFY_OUTPUT, lineageOutput: null }),
    /not v3-verified/,
  );
});

test("verifyRotatedApkCert accepts a properly rotated build", () => {
  assert.equal(
    verifyRotatedApkCert({
      verifyOutput: ROTATED_VERIFY_OUTPUT,
      lineageOutput: ROTATED_LINEAGE_OUTPUT,
    }),
    NEW_CERT,
  );
});

test("verifyRotatedApkCert refuses v3-unverified output for a non-debug signer", () => {
  const notV3 = ROTATED_VERIFY_OUTPUT.replace(
    "Verified using v3 scheme (APK Signature Scheme v3): true",
    "Verified using v3 scheme (APK Signature Scheme v3): false",
  );
  assert.throws(
    () => verifyRotatedApkCert({ verifyOutput: notV3, lineageOutput: ROTATED_LINEAGE_OUTPUT }),
    /not v3-verified/,
  );
});

test("verifyRotatedApkCert refuses a non-debug signer with no lineage at all", () => {
  assert.throws(
    () => verifyRotatedApkCert({ verifyOutput: ROTATED_VERIFY_OUTPUT, lineageOutput: null }),
    /carries no lineage/,
  );
});

test("verifyRotatedApkCert refuses a lineage that does not start at the debug cert", () => {
  const badLineage = ROTATED_LINEAGE_OUTPUT.replace(CERT, "ab".repeat(32));
  assert.throws(
    () => verifyRotatedApkCert({ verifyOutput: ROTATED_VERIFY_OUTPUT, lineageOutput: badLineage }),
    /does not start at the fielded debug cert/,
  );
});

test("verifyRotatedApkCert refuses more than one current signer", () => {
  const twoSigners = ROTATED_VERIFY_OUTPUT.replace("Number of signers: 1", "Number of signers: 2");
  assert.throws(
    () => verifyRotatedApkCert({ verifyOutput: twoSigners, lineageOutput: ROTATED_LINEAGE_OUTPUT }),
    /expected exactly 1/,
  );
});

// ---- RELEASE_CERT_SHA256 pinning (assertPinnedCert) ------------------------

test("RELEASE_CERT_SHA256 is pinned to the sysadmin's release cert", () => {
  assert.equal(
    RELEASE_CERT_SHA256,
    "4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc",
  );
});

test("assertPinnedCert refuses an APK while the pin is empty", () => {
  assert.throws(
    () => assertPinnedCert(NEW_CERT, { releaseCertSha256: "" }),
    /release cert not pinned yet/,
  );
});

test("assertPinnedCert refuses a signer equal to the debug cert, pinned or not", () => {
  assert.throws(
    () => assertPinnedCert(CERT, { releaseCertSha256: "" }),
    /is the DEBUG cert/,
  );
  assert.throws(
    () => assertPinnedCert(CERT, { releaseCertSha256: CERT }),
    /is the DEBUG cert/,
  );
});

test("assertPinnedCert passes when the signer matches the pin exactly", () => {
  assert.doesNotThrow(() => assertPinnedCert(NEW_CERT, { releaseCertSha256: NEW_CERT }));
  // Case-insensitive on both sides.
  assert.doesNotThrow(() =>
    assertPinnedCert(NEW_CERT.toUpperCase(), { releaseCertSha256: NEW_CERT }),
  );
});

test("assertPinnedCert refuses a signer that does not match a non-empty pin", () => {
  assert.throws(
    () => assertPinnedCert(NEW_CERT, { releaseCertSha256: "ab".repeat(32) }),
    /does not match the pinned RELEASE_CERT_SHA256/,
  );
});

test("assertPinnedCert refuses a malformed cert digest", () => {
  assert.throws(() => assertPinnedCert("not-hex", { releaseCertSha256: NEW_CERT }), /not a valid sha256/);
});

// ---- CI_RELEASE_TAG_RE (the workflow's shell-injection guard) --------------

test("CI_RELEASE_TAG_RE accepts exactly ward-v/guardian-v/linux-v + a plain three-part version", () => {
  assert.match("ward-v0.6.13", CI_RELEASE_TAG_RE);
  assert.match("guardian-v0.1.15", CI_RELEASE_TAG_RE);
  assert.match("linux-v0.7.10", CI_RELEASE_TAG_RE);
});

test("CI_RELEASE_TAG_RE rejects anything else, including pre-release suffixes and injection attempts", () => {
  assert.doesNotMatch("ward-v1.0", CI_RELEASE_TAG_RE);
  assert.doesNotMatch("ward-v1.0.0-rc1", CI_RELEASE_TAG_RE);
  assert.doesNotMatch("v1.0.0", CI_RELEASE_TAG_RE);
  assert.doesNotMatch("ward-v1.0.0; rm -rf /", CI_RELEASE_TAG_RE);
  assert.doesNotMatch("ward-v1.0.0`touch /tmp/pwned`", CI_RELEASE_TAG_RE);
  assert.doesNotMatch("ward-v1.0.0$(id)", CI_RELEASE_TAG_RE);
  assert.doesNotMatch("ward-v1.0.0 && echo hi", CI_RELEASE_TAG_RE);
});

// ---- extractAttestedCommit (gh attestation verify --format json) ----------

const ATTESTED_COMMIT = "a".repeat(40);

test("extractAttestedCommit reads the certificate's sourceRepositoryDigest", () => {
  const json = [
    {
      verificationResult: {
        signature: { certificate: { sourceRepositoryDigest: ATTESTED_COMMIT } },
      },
    },
  ];
  assert.equal(extractAttestedCommit(json), ATTESTED_COMMIT);
});

test("extractAttestedCommit falls back to the SLSA resolvedDependencies gitCommit digest", () => {
  const json = [
    {
      verificationResult: {
        statement: {
          predicate: {
            buildDefinition: {
              resolvedDependencies: [
                { uri: "git+https://github.com/forgesworn/kintrinsic", digest: { gitCommit: ATTESTED_COMMIT } },
              ],
            },
          },
        },
      },
    },
  ];
  assert.equal(extractAttestedCommit(json), ATTESTED_COMMIT);
});

test("extractAttestedCommit is case-insensitive and lowercases its result", () => {
  const json = [
    { verificationResult: { signature: { certificate: { sourceRepositoryDigest: ATTESTED_COMMIT.toUpperCase() } } } },
  ];
  assert.equal(extractAttestedCommit(json), ATTESTED_COMMIT);
});

test("extractAttestedCommit accepts a single object as well as an array", () => {
  const json = { verificationResult: { signature: { certificate: { sourceRepositoryDigest: ATTESTED_COMMIT } } } };
  assert.equal(extractAttestedCommit(json), ATTESTED_COMMIT);
});

test("extractAttestedCommit fails closed when no commit is found anywhere", () => {
  assert.throws(() => extractAttestedCommit([{ verificationResult: {} }]), /could not find an attested source commit/);
  assert.throws(() => extractAttestedCommit([]), /could not find an attested source commit/);
});

// ---- buildManifest / formatManifest ----------------------------------------

const GITHUB_URL = `https://github.com/forgesworn/kintrinsic/releases/download/guardian-v0.1.16/kintrinsic-0.1.16.apk`;
const BLOSSOM_URL = `https://nostr.download/${SHA}.apk`;

test("MANIFEST_FILES names one file per channel", () => {
  assert.deepEqual(MANIFEST_FILES, {
    "charter-apk": "charter-apk.json",
    "mycharter-apk": "mycharter-apk.json",
    "charter-deb": "charter-deb.json",
  });
});

test("buildManifest: APK shape and field order matches the hand-written example", () => {
  const m = buildManifest({
    channel: "mycharter-apk",
    versionName: "0.1.16",
    versionCode: 17,
    url: BLOSSOM_URL,
    urls: [GITHUB_URL, BLOSSOM_URL],
    sha256: SHA,
    certSha256: CERT,
    sizeBytes: 10_363_951,
    builtAt: "2026-09-28T15:30:55Z",
  });
  assert.deepEqual(Object.keys(m), [
    "versionName",
    "versionCode",
    "url",
    "urls",
    "apkSha256",
    "certSha256",
    "sizeBytes",
    "builtAt",
  ]);
  assert.equal(m.apkSha256, SHA);
  assert.equal(m.certSha256, CERT);
  assert.deepEqual(m.urls, [GITHUB_URL, BLOSSOM_URL]);
});

test("buildManifest: charter-deb shape has no certSha256 and uses `sha256`", () => {
  const m = buildManifest({
    channel: "charter-deb",
    versionName: "0.7.9",
    versionCode: 709,
    url: `https://nostr.download/${SHA}.deb`,
    urls: [`https://github.com/forgesworn/kintrinsic/releases/download/linux-v0.7.9/kintrinsic_0.7.9_amd64.deb`, `https://nostr.download/${SHA}.deb`],
    sha256: SHA,
    sizeBytes: 11_162_984,
    builtAt: "2026-09-27T11:44:46Z",
  });
  assert.deepEqual(Object.keys(m), ["versionName", "versionCode", "url", "urls", "sha256", "sizeBytes", "builtAt"]);
  assert.equal(m.sha256, SHA);
  assert.equal("certSha256" in m, false);
});

test("buildManifest refuses an APK channel with no certSha256", () => {
  assert.throws(
    () =>
      buildManifest({
        channel: "charter-apk",
        versionName: "0.6.13",
        versionCode: 44,
        url: BLOSSOM_URL,
        urls: [GITHUB_URL, BLOSSOM_URL],
        sha256: SHA,
        sizeBytes: 100,
        builtAt: "2026-09-28T00:00:00Z",
      }),
    /certSha256/,
  );
});

test("buildManifest refuses a bad channel, non-https url, empty urls, or malformed sha256", () => {
  const good = {
    channel: "charter-deb",
    versionName: "0.7.9",
    versionCode: 709,
    url: `https://nostr.download/${SHA}.deb`,
    urls: [`https://nostr.download/${SHA}.deb`],
    sha256: SHA,
    sizeBytes: 100,
    builtAt: "2026-09-27T11:44:46Z",
  };
  assert.throws(() => buildManifest({ ...good, channel: "not-a-channel" }), /bad channel/);
  assert.throws(() => buildManifest({ ...good, url: "http://insecure" }), /https/);
  assert.throws(() => buildManifest({ ...good, urls: [] }), /urls/);
  assert.throws(() => buildManifest({ ...good, sha256: "not-hex" }), /sha256/);
  assert.throws(() => buildManifest({ ...good, versionCode: 0 }), /versionCode/);
  assert.throws(() => buildManifest({ ...good, sizeBytes: 0 }), /sizeBytes/);
});

test("formatManifest renders the exact on-disk text of the hand-written mycharter-apk.json example", () => {
  const m = buildManifest({
    channel: "mycharter-apk",
    versionName: "0.1.16",
    versionCode: 17,
    url: BLOSSOM_URL,
    urls: [GITHUB_URL, BLOSSOM_URL],
    sha256: SHA,
    certSha256: CERT,
    sizeBytes: 10_363_951,
    builtAt: "2026-09-28T15:30:55Z",
  });
  const text = formatManifest(m);
  assert.equal(
    text,
    `{
  "versionName": "0.1.16",
  "versionCode": 17,
  "url": "${BLOSSOM_URL}",
  "urls": ["${GITHUB_URL}", "${BLOSSOM_URL}"],
  "apkSha256": "${SHA}",
  "certSha256": "${CERT}",
  "sizeBytes": 10363951,
  "builtAt": "2026-09-28T15:30:55Z"
}
`,
  );
});

test("formatManifest renders the charter-deb shape (no certSha256 line)", () => {
  const m = buildManifest({
    channel: "charter-deb",
    versionName: "0.7.9",
    versionCode: 709,
    url: `https://nostr.download/${SHA}.deb`,
    urls: [`https://nostr.download/${SHA}.deb`],
    sha256: SHA,
    sizeBytes: 11_162_984,
    builtAt: "2026-09-27T11:44:46Z",
  });
  const text = formatManifest(m);
  assert.doesNotMatch(text, /certSha256/);
  assert.match(text, /^\{\n  "versionName": "0\.7\.9",\n/);
  assert.match(text, /\n\}\n$/);
});

// ---- assertVersionCodeBump --------------------------------------------------

test("assertVersionCodeBump passes when there is no existing manifest (null/undefined old)", () => {
  assert.doesNotThrow(() => assertVersionCodeBump(null, 1));
  assert.doesNotThrow(() => assertVersionCodeBump(undefined, 1));
});

test("assertVersionCodeBump passes when the new versionCode is strictly greater", () => {
  assert.doesNotThrow(() => assertVersionCodeBump(17, 18));
});

test("assertVersionCodeBump refuses an equal versionCode", () => {
  assert.throws(() => assertVersionCodeBump(17, 17), /versionCode 17 <= published 17/);
});

test("assertVersionCodeBump refuses a lower versionCode", () => {
  assert.throws(() => assertVersionCodeBump(17, 5), /versionCode 5 <= published 17/);
});

test("assertVersionCodeBump includes the manifest path in its message when given one", () => {
  assert.throws(() => assertVersionCodeBump(17, 17, { manifestPath: "apps/charter-app/public/mycharter-apk.json" }), /mycharter-apk\.json/);
});

test("assertVersionCodeBump refuses a non-positive-integer new versionCode even with no old one", () => {
  assert.throws(() => assertVersionCodeBump(null, 0), /versionCode must be a positive integer/);
  assert.throws(() => assertVersionCodeBump(null, -1), /versionCode must be a positive integer/);
  assert.throws(() => assertVersionCodeBump(null, 1.5), /versionCode must be a positive integer/);
});
