// Pure builders for the D2 release pipeline — no network, no filesystem, so
// `node --test` covers them exactly. The event shape mirrors
// core/crates/charter-verify/src/software_release.rs and
// apps/charter-app/src/release/releaseEvent.ts (golden vector:
// core/crates/charter-testkit/vectors/nostr/software_release.json).

/** Keep in sync with releaseTrust.ts / release_check.rs. No relay run by the
 * project is a default (Kintrinsic is decentralised by design) — these are
 * public relays, the same ones clients poll by default. */
export const SOFTWARE_RELEASE_KIND = 30063;
export const RELEASE_RELAYS = [
  "wss://relay.damus.io",
  "wss://nos.lol",
  "wss://relay.primal.net",
];
export const RELEASE_CHANNELS = ["charter-apk", "mycharter-apk", "charter-deb"];

/**
 * RELEASE_RELAYS plus any operator-added relays from `CHARTER_EXTRA_RELAYS`
 * (comma-separated `wss://` URLs, e.g. a relay the founder runs themselves).
 * Kintrinsic ships with none of its own as a default; this is how one is
 * added back for a publish, without making it load-bearing for every client.
 */
export function releaseRelaysWithExtra(env = process.env) {
  const extra = (env.CHARTER_EXTRA_RELAYS ?? "")
    .split(",")
    .map((r) => r.trim())
    .filter((r) => r.startsWith("wss://"));
  return [...new Set([...RELEASE_RELAYS, ...extra])];
}

/**
 * The public repository whose GitHub Releases are the PRIMARY artifact host.
 * Transport is not the trust anchor (the release-event signature, the
 * artifact sha256 and, for APKs, the signing cert are), so bytes come from
 * the most reliable HTTPS host first and Blossom second.
 */
export const RELEASE_GITHUB_REPO = "forgesworn/kintrinsic";

/** Per-channel GitHub Release naming: one tag per artifact. */
const GITHUB_NAMING = {
  "charter-apk": {
    tagPrefix: "ward-v",
    title: "Ward",
    asset: (v) => `kintrinsic-ward-${v}.apk`,
  },
  "mycharter-apk": {
    tagPrefix: "guardian-v",
    title: "Kintrinsic (guardian)",
    asset: (v) => `kintrinsic-${v}.apk`,
  },
  "charter-deb": {
    tagPrefix: "linux-v",
    title: "Kintrinsic for Linux",
    asset: (v) => `kintrinsic_${v}_amd64.deb`,
  },
};

const VERSION_RE = /^[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/;
const TAG_RE = /^(ward|guardian|linux)-v[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$/;
const ASSET_RE = /^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$/;
const REPO_RE = /^[A-Za-z0-9-]+\/[A-Za-z0-9._-]+$/;

/**
 * The GitHub Release coordinates for one artifact: tag (`ward-v0.6.13`),
 * asset file name, human title, and the public download URL. The URL 302s to
 * GitHub's asset CDN; clients that follow https redirects fetch it directly,
 * older ones fall through to the Blossom mirror.
 */
export function githubRelease(channel, version, repo = RELEASE_GITHUB_REPO) {
  const n = GITHUB_NAMING[channel];
  if (!n) throw new Error(`bad channel: ${channel}`);
  if (typeof version !== "string" || !VERSION_RE.test(version))
    throw new Error(`bad version for a release tag: ${version}`);
  if (!REPO_RE.test(repo)) throw new Error(`bad GitHub repo: ${repo}`);
  const tag = `${n.tagPrefix}${version}`;
  const asset = n.asset(version);
  return {
    repo,
    tag,
    asset,
    title: `${n.title} ${version}`,
    url: `https://github.com/${repo}/releases/download/${tag}/${asset}`,
  };
}

/**
 * Is `url` a GitHub Release download address of `repo`
 * (`https://github.com/<repo>/releases/download/<tag>/<asset>`) with one of
 * our per-artifact tags? The only non-Blossom address a release event may
 * name: stable for as long as the release exists, and every client pins the
 * bytes to the event's sha256 whatever the host serves.
 */
export function isGithubReleaseUrl(url, repo = RELEASE_GITHUB_REPO) {
  if (typeof url !== "string") return false;
  let u;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.host !== "github.com") return false;
  if (u.search || u.hash || u.username || u.password) return false;
  const prefix = `/${repo}/releases/download/`;
  if (!u.pathname.startsWith(prefix)) return false;
  const rest = u.pathname.slice(prefix.length).split("/");
  return rest.length === 2 && TAG_RE.test(rest[0]) && ASSET_RE.test(rest[1]);
}

/** Order announced download URLs: GitHub Release first, then Blossom, dedup'd. */
export function orderReleaseUrls(urls, repo = RELEASE_GITHUB_REPO) {
  const uniq = [...new Set(urls)];
  return [
    ...uniq.filter((u) => isGithubReleaseUrl(u, repo)),
    ...uniq.filter((u) => !isGithubReleaseUrl(u, repo)),
  ];
}

/**
 * Tag -> {channel, version}, the reverse of githubRelease's naming. Used by
 * `--from-draft <tag>` (publish-release.mjs) to work out what a draft
 * release's tag names without anything else to go on. Throws on a tag that
 * matches none of the three artifact prefixes.
 */
export function channelAndVersionFromTag(tag) {
  if (typeof tag !== "string") throw new Error("tag must be a string");
  for (const [channel, n] of Object.entries(GITHUB_NAMING)) {
    if (tag.startsWith(n.tagPrefix)) {
      const version = tag.slice(n.tagPrefix.length);
      if (!VERSION_RE.test(version)) throw new Error(`tag '${tag}' has a malformed version`);
      return { channel, version };
    }
  }
  throw new Error(`tag '${tag}' matches none of ward-v*, guardian-v*, linux-v*`);
}

/** BUD-02 Blossom upload authorization kind. */
export const BLOSSOM_AUTH_KIND = 24242;

const HEX64 = /^[0-9a-f]{64}$/;

/**
 * A URL a device may be told to fetch `sha256` from: https, and the blob at
 * the server ROOT, addressed by its own hash (`https://host/<sha>` or
 * `https://host/<sha>.<ext>`, BUD-01). That path is the one address a
 * Blossom server promises to keep serving. Anything else — in particular a
 * CDN redirect *target* like `media.primal.net/uploads2/a/1e/a8/<sha>` — is
 * an implementation detail that can vanish (primal's did, 2026-08-27: every
 * ward on 0.6.3 sat in a 404-retry loop while the second mirror was fine).
 */
export function isCanonicalBlossomUrl(url, sha256) {
  if (typeof url !== "string" || !HEX64.test(sha256 ?? "")) return false;
  let u;
  try {
    u = new URL(url);
  } catch {
    return false;
  }
  if (u.protocol !== "https:" || u.search || u.hash || u.username || u.password) return false;
  const m = /^\/([0-9a-f]{64})(\.[A-Za-z0-9]{1,8})?$/.exec(u.pathname);
  return m !== null && m[1] === sha256;
}

/**
 * The unsigned kind-30063 template announcing one artifact. Throws on any
 * invalid field — a malformed release event must never reach finalizeEvent.
 */
export function buildReleaseEvent({
  channel,
  versionName,
  versionCode,
  sha256,
  sizeBytes,
  urls,
  certSha256,
  notes,
  createdAt,
}) {
  if (!RELEASE_CHANNELS.includes(channel)) throw new Error(`bad channel: ${channel}`);
  if (typeof versionName !== "string" || versionName.length === 0)
    throw new Error("versionName required");
  if (!Number.isInteger(versionCode) || versionCode <= 0)
    throw new Error("versionCode must be a positive integer");
  if (!HEX64.test(sha256)) throw new Error("sha256 must be 64 lowercase hex chars");
  if (!Number.isInteger(sizeBytes) || sizeBytes <= 0)
    throw new Error("sizeBytes must be a positive integer");
  if (!Array.isArray(urls) || urls.length === 0) throw new Error("at least one url required");
  for (const u of urls) {
    if (typeof u !== "string" || !u.startsWith("https://"))
      throw new Error(`mirror url must be https: ${u}`);
    if (!isCanonicalBlossomUrl(u, sha256) && !isGithubReleaseUrl(u))
      throw new Error(
        `mirror url must be a GitHub Release download or the blob's canonical root address (https://host/<sha>[.ext]): ${u}`,
      );
  }
  if (channel !== "charter-deb" && !HEX64.test(certSha256 ?? ""))
    throw new Error("APK channels require certSha256 (64 lowercase hex chars)");
  if (!Number.isInteger(createdAt) || createdAt <= 0) throw new Error("createdAt required");

  const tags = [
    ["d", channel],
    ["version", versionName],
    ["version_code", String(versionCode)],
    ["x", sha256],
    ["size", String(sizeBytes)],
  ];
  if (certSha256) tags.push(["cert", certSha256]);
  for (const u of urls) tags.push(["url", u]);
  return {
    kind: SOFTWARE_RELEASE_KIND,
    created_at: createdAt,
    tags,
    content: notes ?? "",
  };
}

/**
 * The unsigned BUD-02 upload-authorization template. Signed with the release
 * key and sent as `Authorization: Nostr <base64(signed json)>` on the PUT.
 */
export function buildBlossomAuth({ sha256, createdAt }) {
  if (!HEX64.test(sha256)) throw new Error("sha256 must be 64 lowercase hex chars");
  if (!Number.isInteger(createdAt) || createdAt <= 0) throw new Error("createdAt required");
  return {
    kind: BLOSSOM_AUTH_KIND,
    created_at: createdAt,
    tags: [
      ["t", "upload"],
      ["x", sha256],
      ["expiration", String(createdAt + 600)],
    ],
    content: "Charter release artifact upload",
  };
}

// ---- `--from-draft` verification (publish-release.mjs) --------------------
//
// The fielded debug certificate — MUST match scripts/release/lib.sh's own
// DEBUG_CERT_SHA256 (that copy is bash, this one is JS; both are the same
// well-known public value, verified against a real debug-signed build and
// against apps/charter-app/public/.well-known/assetlinks.json on
// 2026-09-27). A v3-rotated release APK's lineage must start here.
export const DEBUG_CERT_SHA256 = "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42";

// The pinned release-key certificate fingerprint (lowercase hex, 64 chars).
// Pinned to the sysadmin's release cert (android-signing-rotation plan,
// 2026-09-27) — kept as two copies for the same reason DEBUG_CERT_SHA256 is
// (this one is JS, scripts/release/lib.sh's own RELEASE_CERT_SHA256 is
// bash). See docs/releasing.md.
export const RELEASE_CERT_SHA256 = "4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc";

/**
 * The cert-pin guard `--from-draft` runs for every APK channel, on top of
 * verifyRotatedApkCert's internal-consistency checks. verifyRotatedApkCert
 * happily accepts a v3-verified APK whose CURRENT signer IS the debug cert
 * (its lineage checks only run when the signer differs from the debug
 * cert) — this is the explicit belt for that: a signer equal to the debug
 * cert is refused outright, regardless of the pin. Once RELEASE_CERT_SHA256
 * is set, the signer must equal it exactly; while it is still empty, this
 * refuses rather than silently accepting anything that merely isn't the
 * debug cert. Throws on any failure.
 */
export function assertPinnedCert(
  cert,
  { releaseCertSha256 = RELEASE_CERT_SHA256, debugCertSha256 = DEBUG_CERT_SHA256 } = {},
) {
  const c = (cert ?? "").toLowerCase();
  if (!HEX64.test(c)) throw new Error(`not a valid sha256 cert digest: ${cert}`);
  if (c === debugCertSha256.toLowerCase())
    throw new Error(
      `signing cert ${c} is the DEBUG cert — refusing to publish an APK release signed with it`,
    );
  if (!releaseCertSha256)
    throw new Error("release cert not pinned yet — see docs/releasing.md");
  if (c !== releaseCertSha256.toLowerCase())
    throw new Error(`signing cert ${c} does not match the pinned RELEASE_CERT_SHA256 (${releaseCertSha256})`);
}

/**
 * Strict tag-name validator for the CI release workflow's shell-injection
 * guard (release-artifacts.yml's "Validate the tag" step): before ANY value
 * derived from a workflow_dispatch input or a ref name is used inside a
 * `run:` shell block, it must match this. Deliberately stricter than TAG_RE
 * above (which accepts full semver — used only for the JS-side URL/tag
 * naming that never touches a shell): a plain three-part numeric version,
 * no pre-release/build suffix, so nothing resembling a shell metacharacter
 * can ever reach `run:`. The workflow's own regex must stay textually
 * identical to this one; there is no import across YAML and JS, so the test
 * for this constant is what keeps the two from drifting apart unnoticed.
 */
export const CI_RELEASE_TAG_RE = /^(ward|guardian|linux)-v[0-9]+\.[0-9]+\.[0-9]+$/;

/**
 * Parse a `sha256sum`-style SHA256SUMS file ("<64 lowercase hex>  <name>"
 * per line, sha256sum's own two-space — or one-space, or a leading "*" for
 * binary mode — format) into a Map<filename, hexDigest>. Throws on any
 * non-blank line that doesn't match.
 */
export function parseSha256Sums(text) {
  const map = new Map();
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (!line) continue;
    const m = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line);
    if (!m) throw new Error(`malformed SHA256SUMS line: ${raw}`);
    map.set(m[2], m[1]);
  }
  return map;
}

/**
 * Parse `apksigner verify -v --print-certs` output into the facts
 * android_verify_signing (scripts/release/lib.sh) checks: whether v3
 * verified, how many CURRENT signers, and the first (current) signer's
 * SHA-256 digest. Pure — the impure caller runs apksigner and passes its
 * stdout in.
 */
export function parseApksignerVerify(output) {
  const v3 = /^Verified using v3 scheme \(APK Signature Scheme v3\): true$/m.test(output);
  const signersMatch = /^Number of signers: (\d+)$/m.exec(output);
  const certMatch = /^Signer #1 certificate SHA-256 digest: ([0-9a-fA-F]+)$/m.exec(output);
  return {
    v3,
    signers: signersMatch ? Number(signersMatch[1]) : null,
    cert: certMatch ? certMatch[1].toLowerCase() : null,
  };
}

/**
 * Parse `apksigner lineage --in <apk> --print-certs` output into an ordered
 * list of `{ index, sha256 }` (lowercase hex), oldest signer first. Pure.
 */
export function parseApksignerLineage(output) {
  const entries = [];
  const re = /^Signer #(\d+) in lineage certificate SHA-256 digest: ([0-9a-fA-F]+)$/gm;
  let m;
  while ((m = re.exec(output)) !== null) {
    entries.push({ index: Number(m[1]), sha256: m[2].toLowerCase() });
  }
  return entries;
}

/**
 * The cert guard `--from-draft` runs over an already-captured apksigner
 * output — pure and testable without a real APK or the apksigner binary.
 * Unlike lib.sh's `android_verify_signing` (which stays lenient for the
 * LOCAL alpha-bridge debug build), this ALWAYS requires v3: `--from-draft`
 * only ever verifies a CI-drafted artifact, and CI's release Environment
 * makes the rotation lineage mandatory (rotation plan C1/C5) — an
 * alpha-bridge build can never reach a draft release. Guards, in order:
 *   1. v3 verified true.
 *   2. exactly one current signer.
 *   3. if that signer is not `debugCertSha256`, `lineageOutput` must exist,
 *      its first signer must be the debug cert, and it must include the
 *      current signer somewhere in it (a rotation FROM the fielded cert,
 *      not an unrelated key with a lineage of its own).
 * Throws on any failure; returns the current signer's sha256 (lowercase
 * hex) on success.
 */
export function verifyRotatedApkCert({ verifyOutput, lineageOutput, debugCertSha256 = DEBUG_CERT_SHA256 }) {
  const { v3, signers, cert } = parseApksignerVerify(verifyOutput);
  if (!v3) throw new Error("apk is not v3-verified");
  if (signers !== 1) throw new Error(`apk reports ${signers} current signers, expected exactly 1`);
  if (!cert) throw new Error("could not read the signing cert digest");
  if (cert !== debugCertSha256) {
    if (!lineageOutput) throw new Error(`apk's signer (${cert}) is not the debug cert and it carries no lineage`);
    const lineage = parseApksignerLineage(lineageOutput);
    if (lineage.length === 0 || lineage[0].sha256 !== debugCertSha256)
      throw new Error(`apk's lineage does not start at the fielded debug cert (${debugCertSha256})`);
    if (!lineage.some((e) => e.sha256 === cert))
      throw new Error(`apk's lineage does not include its own current signer (${cert})`);
  }
  return cert;
}

const COMMIT_RE = /^[0-9a-f]{40}$/i;

/**
 * Extract the attested source commit from `gh attestation verify --format
 * json` output — an array of verification results, one per matching
 * attestation. `--from-draft` must refuse to publish unless this commit is
 * an ancestor of origin/main, so this has to find it reliably and FAIL
 * CLOSED (throw) rather than silently skip that check when the shape isn't
 * what it expected.
 *
 * Checked, in order, against the two places a GitHub build-provenance
 * attestation carries it:
 *   1. the Fulcio signing certificate's `sourceRepositoryDigest` extension,
 *      as `gh attestation verify --format json` surfaces it under
 *      `verificationResult.signature.certificate`;
 *   2. the SLSA provenance statement's resolved build dependency —
 *      `verificationResult.statement.predicate.buildDefinition
 *      .resolvedDependencies[].digest.gitCommit` — for the workflow's own
 *      repository entry (a build can resolve more than one dependency;
 *      only a full 40-hex commit counts).
 */
export function extractAttestedCommit(json) {
  const results = Array.isArray(json) ? json : [json];
  for (const r of results) {
    const cert = r?.verificationResult?.signature?.certificate;
    const fromCert = cert?.sourceRepositoryDigest;
    if (typeof fromCert === "string" && COMMIT_RE.test(fromCert)) return fromCert.toLowerCase();

    const deps = r?.verificationResult?.statement?.predicate?.buildDefinition?.resolvedDependencies;
    if (Array.isArray(deps)) {
      for (const d of deps) {
        const c = d?.digest?.gitCommit;
        if (typeof c === "string" && COMMIT_RE.test(c)) return c.toLowerCase();
      }
    }
  }
  throw new Error(
    "could not find an attested source commit in `gh attestation verify --format json` output — refusing to publish",
  );
}
