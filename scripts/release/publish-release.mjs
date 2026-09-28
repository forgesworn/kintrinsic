#!/usr/bin/env node
// Publish one release: upload the artifact to its GitHub Release (the PRIMARY
// host), mirror it to Blossom (secondary), then announce it as a signed
// kind-30063 event on the release relays. Invoked as the final step of
// publish-apk.sh / publish-carrier-apk.sh / publish-deb.sh.
//
// Transport is not the trust anchor: the release-event signature, the
// artifact sha256 and (APKs) the signing cert are. So the bytes come from the
// most reliable HTTPS host first — `https://github.com/<repo>/releases/
// download/<tag>/<asset>`, one tag per artifact (ward-v…, guardian-v…,
// linux-v…) — and Blossom second. The event carries both, GitHub first.
//
//   node scripts/release/publish-release.mjs \
//     --channel charter-apk --artifact path/to.apk \
//     --version 0.6.9 --version-code 40 [--cert <64hex>] [--notes "…"] [--dry-run] \
//     [--emit-url-file F] [--emit-urls-file F]
//
// --emit-url-file   the ONE legacy URL for the manifests' `url` field: the
//                   verified direct-200 Blossom address. Fielded clients
//                   (ward ≤ 0.6.12, charterd ≤ 0.7.9, guardian ≤ 0.1.14)
//                   refuse redirects, and GitHub download URLs 302.
// --emit-urls-file  the ordered list (GitHub first, then Blossom), one per
//                   line — the manifests' `urls` field.
//
// ---- --from-draft: the maintainer's half of the CI split -----------------
//
// .github/workflows/release-artifacts.yml builds + signs an artifact on a
// `ward-v*`/`guardian-v*`/`linux-v*` tag and lands it as a DRAFT GitHub
// Release (never published, never latest) with a SHA256SUMS file and a
// build-provenance attestation. This is the other half: verify that draft
// end to end, THEN publish it, THEN run the same Blossom + Nostr flow as
// above.
//
//   node scripts/release/publish-release.mjs --from-draft ward-v0.6.13 \
//     --version-code 44 [--cert <64hex>] [--notes "…"] [--dry-run] \
//     [--resume] [--emit-url-file F] [--emit-urls-file F]
//
// --version-code   required, same as the default mode: not on the GitHub
//                   Release itself, so the maintainer supplies it (from the
//                   CI run, or by reading the built artifact directly).
// --cert            optional; if given, MUST equal the APK's actual signing
//                   cert digest (read fresh with apksigner) or this refuses
//                   to publish. Ignored for the charter-deb channel.
// --resume          required to re-run against a release that a PREVIOUS
//                   --from-draft run already flipped to published (e.g. it
//                   died between publishing and announcing). Every
//                   verification step below still runs in full; only the
//                   publish-the-draft step is skipped. Without --resume, an
//                   already-published release is refused outright — this
//                   is never the default because it must be a deliberate
//                   choice, not something a stray re-run does by accident.
//
// Step 0, before anything else and before any `gh` call at all: the
// forgot-to-bump guard (assertVersionCodeBump, release-helpers.mjs) reads
// the channel's committed manifest under apps/charter-app/public/ and
// refuses outright if --version-code is not strictly greater than what is
// already published there. This runs the same in --dry-run.
//
// Verification (steps 1-6), then publish (skipped with --resume), then a
// device-facing download check, then the mirror + announce — any
// verification failure aborts before anything is announced or (without
// --resume) before the draft is touched:
//   1. the release named by the tag exists, and is still a draft unless
//      --resume was given;
//   2. `gh release download` the artifact + SHA256SUMS, and check the
//      downloaded bytes' sha256 against SHA256SUMS;
//   3. `gh attestation verify --signer-workflow …/release-artifacts.yml
//      --source-ref refs/tags/<tag> --deny-self-hosted-runners --format
//      json` against CHARTER_GITHUB_REPO (build provenance from exactly
//      this workflow and this tag, on a GitHub-hosted runner);
//   4. the attested source commit (read from that JSON) must be an
//      ancestor of origin/main (`git fetch origin main` first) — a CI run
//      against an unmerged or rewritten ref is refused;
//   5. for an APK channel, apksigner's v3 + rotation-lineage guard
//      (verifyRotatedApkCert, release-helpers.mjs) — CI's release
//      Environment makes the rotation lineage mandatory, so this always
//      requires v3 — then the RELEASE_CERT_SHA256 pin (assertPinnedCert:
//      refuses a debug-cert signer outright, refuses while the pin is
//      empty, requires an exact match once it's set);
//   6. if --cert was given, it must match the actual signing cert too.
// Only once all of that passes: publish the draft, verify the GitHub
// download URL serves it (200, following redirects), THEN mirror to
// Blossom and sign + announce the Nostr event, THEN — the only thing this
// mode does that publish-apk.sh/publish-carrier-apk.sh/publish-deb.sh don't
// need, since they write their own — write the channel's manifest under
// apps/charter-app/public/ (buildManifest/formatManifest,
// release-helpers.mjs: same fields, same field order, same formatting those
// scripts' write_manifest calls use) and run
// scripts/sync-front-door-downloads.sh so site/downloads.json and
// site/download.html follow. Nothing is committed or pushed; the run prints
// a "next: commit … and push" line instead.
// --dry-run makes NO network calls at all in this mode (no `gh`, no
// download): it only resolves the tag, runs the (local, no-network)
// forgot-to-bump guard, and prints the plan plus a shape-only preview of the
// manifest it would write (the real sha256/url/cert/builtAt are only known
// once the download above verifies).
//
// Env:
//   CHARTER_BLOSSOM_SERVERS  comma-separated (default below)
//   CHARTER_RELEASE_KEY_FILE key path (default ~/.charter-release/release-key.hex)
//   CHARTER_GITHUB_REPO      owner/name (default forgesworn/kintrinsic)
//   CHARTER_EXTRA_RELAYS     comma-separated wss:// relays added to RELEASE_RELAYS
//                            (Kintrinsic ships with no relay of its own as a
//                            default; this is how the founder's own relay, if
//                            any, is added back for a publish)
//
// Exit: non-zero, with nothing announced and no URL files written, when the
// GitHub Release upload or its end-to-end download check fails, or when NO
// Blossom mirror verifies as directly servable (200, no redirect): fielded
// clients refuse redirects and cannot fetch a GitHub-only release. Also
// non-zero when every relay refused the event. One Blossom mirror failing
// while another verifies is a warning.
// --dry-run makes no network writes and runs no `gh`: it prints the plan and
// the signed event.

import { finalizeEvent, getPublicKey } from "nostr-tools/pure";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  copyFileSync,
  existsSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  assertPinnedCert,
  assertVersionCodeBump,
  buildBlossomAuth,
  buildManifest,
  buildReleaseEvent,
  channelAndVersionFromTag,
  extractAttestedCommit,
  formatManifest,
  githubRelease,
  isCanonicalBlossomUrl,
  isGithubReleaseUrl,
  MANIFEST_FILES,
  orderReleaseUrls,
  parseSha256Sums,
  releaseRelaysWithExtra,
  RELEASE_GITHUB_REPO,
  verifyRotatedApkCert,
} from "./release-helpers.mjs";

// Both live-verified 2026-08-12 with the 10 MB deb (upload + direct-200 GET).
// blossom.band was tried and REFUSES non-media uploads (HTTP 415);
// blossom.sovbit.host was unreachable. Re-verify before adding servers.
const DEFAULT_BLOSSOM = "https://blossom.primal.net,https://nostr.download";

const __dirname = dirname(fileURLToPath(import.meta.url));
// scripts/release/ -> repo root.
const REPO_ROOT = join(__dirname, "..", "..");
const PUBLIC_DIR = join(REPO_ROOT, "apps", "charter-app", "public");

/**
 * The existing manifest's versionCode, or null when the channel has never
 * published one (no file) or the field is missing/malformed — the
 * forgot-to-bump guard's baseline. Reads the committed manifest straight off
 * disk, never `gh`, so it works the same in --dry-run as for real.
 */
function readManifestVersionCode(path) {
  if (!existsSync(path)) return null;
  let data;
  try {
    data = JSON.parse(readFileSync(path, "utf8"));
  } catch (err) {
    throw new Error(`cannot parse existing manifest ${path}: ${err.message}`);
  }
  return typeof data.versionCode === "number" ? data.versionCode : null;
}

function parseArgs(argv) {
  const args = { notes: "" };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--dry-run") args.dryRun = true;
    else if (a === "--resume") args.resume = true;
    else if (a === "--from-draft") args.fromDraft = argv[++i];
    else if (a === "--channel") args.channel = argv[++i];
    else if (a === "--artifact") args.artifact = argv[++i];
    else if (a === "--version") args.version = argv[++i];
    else if (a === "--version-code") args.versionCode = Number(argv[++i]);
    else if (a === "--cert") args.cert = argv[++i];
    else if (a === "--notes") args.notes = argv[++i];
    else if (a === "--emit-url-file") args.emitUrlFile = argv[++i];
    else if (a === "--emit-urls-file") args.emitUrlsFile = argv[++i];
    else throw new Error(`unknown argument: ${a}`);
  }
  if (args.fromDraft) {
    // --from-draft derives channel + version from the tag itself
    // (channelAndVersionFromTag); --channel/--artifact/--version make no
    // sense alongside it and are refused so the two modes can't be confused.
    if (args.channel || args.artifact || args.version)
      throw new Error("--from-draft is exclusive with --channel/--artifact/--version");
    if (args.versionCode === undefined || Number.isNaN(args.versionCode))
      throw new Error("--version-code is required");
    return args;
  }
  if (args.resume) throw new Error("--resume only makes sense with --from-draft");
  for (const req of ["channel", "artifact", "version", "versionCode"]) {
    if (args[req] === undefined || Number.isNaN(args[req]))
      throw new Error(`--${req.replace("versionCode", "version-code")} is required`);
  }
  return args;
}

/** Newest apksigner under $ANDROID_HOME/build-tools (mirrors lib.sh's
 * _android_apksigner). */
function findApksigner() {
  const home = process.env.ANDROID_HOME;
  if (!home) throw new Error("ANDROID_HOME is not set — needed to locate apksigner for --from-draft");
  const buildToolsDir = join(home, "build-tools");
  let versions;
  try {
    versions = readdirSync(buildToolsDir);
  } catch (err) {
    throw new Error(`cannot read ${buildToolsDir}: ${err.message}`);
  }
  const withApksigner = versions.filter((v) => existsSync(join(buildToolsDir, v, "apksigner")));
  if (withApksigner.length === 0) throw new Error(`no apksigner found under ${buildToolsDir}`);
  withApksigner.sort((a, b) => {
    const pa = a.split(".").map(Number);
    const pb = b.split(".").map(Number);
    for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
      const d = (pa[i] ?? 0) - (pb[i] ?? 0);
      if (d !== 0) return d;
    }
    return 0;
  });
  return join(buildToolsDir, withApksigner[withApksigner.length - 1], "apksigner");
}

function loadReleaseKey() {
  const file =
    process.env.CHARTER_RELEASE_KEY_FILE ??
    join(homedir(), ".charter-release", "release-key.hex");
  const hex = readFileSync(file, "utf8").trim();
  if (!/^[0-9a-f]{64}$/.test(hex)) throw new Error(`${file} is not a 64-hex secret`);
  return Uint8Array.from(Buffer.from(hex, "hex"));
}

async function uploadToBlossom(server, bytes, sha256, sk) {
  const auth = finalizeEvent(
    buildBlossomAuth({ sha256, createdAt: Math.floor(Date.now() / 1000) }),
    sk,
  );
  const header = `Nostr ${Buffer.from(JSON.stringify(auth)).toString("base64")}`;
  const res = await fetch(`${server}/upload`, {
    method: "PUT",
    headers: {
      Authorization: header,
      "Content-Type": "application/octet-stream",
      "X-SHA-256": sha256,
    },
    body: bytes,
  });
  if (!res.ok) throw new Error(`${server}: upload HTTP ${res.status}`);
}

async function fetchDirect(url) {
  // redirect:"manual": the on-device stagers refuse redirects, so this
  // simulates exactly what a device will experience.
  const res = await fetch(url, { redirect: "manual" });
  if (res.status === 200) return { bytes: Buffer.from(await res.arrayBuffer()) };
  if ([301, 302, 307, 308].includes(res.status)) {
    return { location: res.headers.get("location") };
  }
  return { error: `HTTP ${res.status}` };
}

/**
 * Verify a Blossom mirror the way a FIELDED device will use it: GET, no
 * redirects, full-body sha256. Ward ≤ 0.6.12, charterd ≤ 0.7.9 and guardian
 * ≤ 0.1.14 refuse redirects, so the legacy single `url` they are handed must
 * be a direct 200 — which is why this check stays strict even though newer
 * clients follow https redirects. The event may only ever carry the blob's
 * CANONICAL root address (`https://server/<sha>[.ext]`, see
 * isCanonicalBlossomUrl) — never a redirect target: primal fronts blobs with a
 * CDN whose `GET /<sha>` 302s to `media.primal.net/uploads2/…`; until
 * 2026-08-27 we announced that TARGET, and when primal purged it every ward on
 * 0.6.3 sat in a 404-retry loop. A redirecting server still counts as an
 * upload success (the blob is there), but it contributes no URL.
 */
async function verifyDeviceUrl(url, sha256) {
  const res = await fetch(url, { redirect: "manual" });
  if (res.status !== 200) return false;
  const bytes = Buffer.from(await res.arrayBuffer());
  return createHash("sha256").update(bytes).digest("hex") === sha256;
}

async function verifyBlossom(server, sha256, ext) {
  // Prefer the extension-bearing form (a browser download keeps a sensible
  // filename; blossom.primal.net serves it direct-200 where the bare form
  // 302s), then the bare BUD-01 form. Only a DIRECT 200 with the right bytes
  // yields a URL a device may be given.
  for (const url of [`${server}/${sha256}.${ext}`, `${server}/${sha256}`]) {
    const r = await fetchDirect(url);
    if (r.bytes) {
      const got = createHash("sha256").update(r.bytes).digest("hex");
      if (got !== sha256) {
        console.error(`mirror ${url}: serves WRONG bytes (${got})`);
        return null;
      }
      return url;
    }
    if (r.location) {
      console.error(`mirror ${url}: redirects (${r.location}) — blob present, but not device-servable; no URL from this mirror`);
      continue;
    }
    console.error(`mirror ${url}: ${r.error}`);
  }
  return null;
}

function publishToRelay(url, event, timeoutMs = 10_000) {
  return new Promise((resolve) => {
    let settled = false;
    const done = (ok, reason) => {
      if (settled) return;
      settled = true;
      try {
        ws.close();
      } catch {
        /* already closed */
      }
      resolve({ url, ok, reason });
    };
    const ws = new WebSocket(url);
    const timer = setTimeout(() => done(false, "timeout"), timeoutMs);
    ws.onopen = () => ws.send(JSON.stringify(["EVENT", event]));
    ws.onmessage = (m) => {
      try {
        const [type, id, ok, reason] = JSON.parse(m.data);
        if (type === "OK" && id === event.id) {
          clearTimeout(timer);
          done(Boolean(ok), reason ?? "");
        }
      } catch {
        /* ignore non-JSON frames */
      }
    };
    ws.onerror = () => {
      clearTimeout(timer);
      done(false, "socket error");
    };
  });
}

// ---- GitHub Releases (primary host) --------------------------------------

function gh(args) {
  return execFileSync("gh", args, { encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] });
}

/** Assets of the release at `tag`, or null when there is no such release. */
function ghReleaseAssets(repo, tag) {
  try {
    const out = gh(["release", "view", tag, "--repo", repo, "--json", "assets"]);
    return (JSON.parse(out).assets ?? []).map((a) => ({ name: a.name, size: a.size }));
  } catch (err) {
    const msg = `${err.stderr ?? ""}${err.message ?? ""}`;
    if (/release not found/i.test(msg)) return null;
    throw new Error(`gh release view ${tag}: ${msg.trim()}`);
  }
}

/**
 * The commit the new tag should name: HEAD when the remote already has it,
 * else none, and the release is refused — the trust anchors are the hash and
 * signatures, but a tag must never name a commit other than the release's.
 */
function tagTarget() {
  try {
    const head = execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" }).trim();
    const remote = execFileSync("git", ["branch", "-r", "--contains", head], {
      encoding: "utf8",
    }).trim();
    if (remote) return head;
  } catch {
    /* not a git checkout — fall through */
  }
  return null;
}

/**
 * Is `commit` an ancestor of origin/main? (Requires a prior `git fetch
 * origin main`.) `--from-draft` refuses to publish an artifact whose
 * attested build commit isn't reachable from the branch we actually ship
 * from — a CI run against an unmerged branch, or a forged/rewritten source
 * ref, must never be announced as a release.
 */
function isAncestorOfOriginMain(commit) {
  try {
    execFileSync("git", ["merge-base", "--is-ancestor", commit, "origin/main"], {
      stdio: ["ignore", "pipe", "pipe"],
    });
    return true;
  } catch {
    return false;
  }
}

/**
 * Download `url` the way a CURRENT client does — following redirects (all of
 * them https), full-body sha256. Resolves to the hex digest, or throws.
 */
async function fetchSha256Following(url) {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok) throw new Error(`HTTP ${res.status}`);
  if (!res.url.startsWith("https://")) throw new Error(`redirected off https: ${res.url}`);
  const bytes = Buffer.from(await res.arrayBuffer());
  return createHash("sha256").update(bytes).digest("hex");
}

async function verifyGithubDownload(url, sha256) {
  // The asset CDN can lag the upload by a moment: a few bounded retries.
  let last = "";
  for (let attempt = 1; attempt <= 5; attempt++) {
    try {
      const got = await fetchSha256Following(url);
      if (got === sha256) return;
      throw new Error(`serves WRONG bytes (${got})`);
    } catch (err) {
      last = err.message;
      if (/WRONG bytes/.test(last)) break;
      await new Promise((r) => setTimeout(r, 2_000 * attempt));
    }
  }
  throw new Error(`GitHub download ${url} did not verify: ${last}`);
}

/**
 * Put the artifact on its GitHub Release and prove the public download URL
 * serves exactly these bytes. Creates the release if absent (a normal,
 * non-draft release). NEVER overwrites: an existing asset of the same name
 * with the same bytes is accepted (an idempotent re-run); with different
 * bytes it is refused, and `--clobber` is never passed.
 */
async function publishToGithub({ gr, artifact, sha256, version, notes }) {
  let assets = ghReleaseAssets(gr.repo, gr.tag);
  if (assets === null) {
    const target = tagTarget();
    const body =
      `${gr.title}.\n\nsha256 \`${sha256}\`\n\n` +
      "Announced by a signed Nostr release event (kind 30063); the same bytes are " +
      "mirrored on Blossom. Devices verify the sha256 (and, for APKs, the signing " +
      "certificate) whichever host served them." +
      (notes ? `\n\n${notes}` : "");
    const args = ["release", "create", gr.tag, "--repo", gr.repo, "--title", gr.title, "--notes", body];
    if (!target)
      throw new Error(
        `HEAD is not on the remote, so ${gr.tag} would tag the wrong commit. ` +
          "Commit the version bump and push it first, then publish.",
      );
    args.push("--target", target);
    console.log(`github: creating release ${gr.tag} (${version})`);
    gh(args);
    assets = [];
  }
  if (assets.some((a) => a.name === gr.asset)) {
    const got = await fetchSha256Following(gr.url).catch((e) => `unreadable (${e.message})`);
    if (got !== sha256) {
      throw new Error(
        `${gr.tag} already has ${gr.asset} with DIFFERENT bytes (${got}); refusing to overwrite. ` +
          "Bump the version — a published asset is never replaced.",
      );
    }
    console.log(`github: ${gr.asset} already on ${gr.tag} with matching sha256 — not re-uploading`);
  } else {
    const dir = mkdtempSync(join(tmpdir(), "charter-release-"));
    try {
      const staged = join(dir, gr.asset);
      copyFileSync(artifact, staged);
      console.log(`github: uploading ${gr.asset} to ${gr.tag}`);
      gh(["release", "upload", gr.tag, staged, "--repo", gr.repo]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  }
  await verifyGithubDownload(gr.url, sha256);
  console.log(`github ok: ${gr.url}`);
}

function banner(lines) {
  const bar = "!".repeat(72);
  console.error([bar, ...lines.map((l) => `!! ${l}`), bar].join("\n"));
}

/**
 * Steps 2 onward, shared by the default (upload-it-yourself) mode and
 * `--from-draft` (the artifact is already the verified GitHub Release
 * asset): mirror to Blossom, build + sign the kind-30063 event, publish it
 * to the release relays, emit the manifest URL files. Throws (never calls
 * process.exit itself) on any of the FATAL conditions the original
 * single-function version did, so a caller's `finally` cleanup (the
 * `--from-draft` temp dir) always runs; `main()`'s top-level `.catch`
 * still turns an uncaught throw into a non-zero exit either way.
 */
async function announceRelease(
  sk,
  { channel, version, versionCode, cert, notes, sha256, sizeBytes, bytes, gr, dryRun, emitUrlFile, emitUrlsFile },
) {
  const servers = (process.env.CHARTER_BLOSSOM_SERVERS ?? DEFAULT_BLOSSOM)
    .split(",")
    .map((s) => s.trim().replace(/\/$/, ""))
    .filter(Boolean);

  // The LEGACY single device URL (manifests' `url`, and what fielded
  // redirect-refusing clients pick): extension-bearing, verified direct-200
  // with matching bytes — never reconstructed blindly.
  const ext = channel.endsWith("deb") ? "deb" : "apk";
  const deviceBase = (process.env.CHARTER_BLOSSOM_DL_BASE ?? "https://nostr.download").replace(
    /\/$/,
    "",
  );
  const deviceUrl = `${deviceBase}/${sha256}.${ext}`;

  // Blossom as the mirror. Failure is a warning once GitHub verified.
  const blossom = [];
  if (dryRun) {
    // A dry-run event must still be VALID: derive mirror URLs without uploading.
    blossom.push(deviceUrl);
    for (const s of servers) blossom.push(`${s}/${sha256}`);
  } else {
    for (const server of servers) {
      try {
        await uploadToBlossom(server, bytes, sha256, sk);
        const verified = await verifyBlossom(server, sha256, ext);
        if (verified) {
          blossom.push(verified);
          console.log(`mirror ok: ${verified}`);
        } else {
          console.error(`mirror ${server}: uploaded, but no device-servable URL`);
        }
      } catch (err) {
        console.error(`mirror FAILED: ${err.message}`);
      }
    }
    if (await verifyDeviceUrl(deviceUrl, sha256)) {
      if (!blossom.includes(deviceUrl)) blossom.unshift(deviceUrl);
    }
  }
  // Legacy URL: the preferred Blossom address if it verified, else any
  // verified (direct-200) Blossom address. None at all is fatal.
  if (blossom.length === 0) {
    banner([
      "NO BLOSSOM MIRROR VERIFIED — REFUSING TO ANNOUNCE. GitHub holds the release,",
      "but FIELDED clients (ward <= 0.6.12, charterd <= 0.7.9, guardian <= 0.1.14)",
      "refuse redirects and cannot fetch a GitHub download. Re-run once Blossom",
      "serves it (the GitHub step is idempotent).",
    ]);
    throw new Error("no Blossom mirror verified — refusing to announce");
  }
  const legacyUrl = blossom.includes(deviceUrl) ? deviceUrl : blossom[0];
  if (!dryRun && !blossom.includes(deviceUrl)) {
    console.error(`WARNING: preferred mirror ${deviceUrl} did not verify; legacy url is ${legacyUrl}`);
  }

  const urls = orderReleaseUrls([gr.url, legacyUrl, ...blossom]);
  // Belt and braces: buildReleaseEvent throws on a non-canonical URL too.
  const bad = urls.filter((u) => !isCanonicalBlossomUrl(u, sha256) && !isGithubReleaseUrl(u, gr.repo));
  if (bad.length) {
    console.error(`refusing to announce non-canonical url(s): ${bad.join(", ")}`);
    throw new Error(`refusing to announce non-canonical url(s): ${bad.join(", ")}`);
  }

  const event = finalizeEvent(
    buildReleaseEvent({
      channel,
      versionName: version,
      versionCode,
      sha256,
      sizeBytes,
      urls,
      certSha256: cert,
      notes,
      createdAt: Math.floor(Date.now() / 1000),
    }),
    sk,
  );

  const emit = () => {
    if (emitUrlFile) writeFileSync(emitUrlFile, legacyUrl + "\n");
    if (emitUrlsFile) writeFileSync(emitUrlsFile, urls.join("\n") + "\n");
  };

  const relays = releaseRelaysWithExtra();

  if (dryRun) {
    console.log(`--dry-run: would publish to ${relays.join(", ")}`);
    console.log("--dry-run: signed event follows; nothing uploaded or published");
    console.log(JSON.stringify(event, null, 2));
    emit();
    return { legacyUrl, urls };
  }

  const results = await Promise.all(relays.map((r) => publishToRelay(r, event)));
  for (const r of results) {
    console.log(`${r.ok ? "relay ok " : "relay FAIL"}: ${r.url} ${r.reason ?? ""}`);
  }
  const accepted = results.filter((r) => r.ok).length;
  if (accepted === 0) {
    console.error("every relay refused the release event");
    throw new Error("every relay refused the release event");
  }
  if (accepted < 2) {
    banner([
      `ONLY ${accepted} RELAY ACCEPTED THE RELEASE EVENT.`,
      "Devices that cannot reach that relay will not hear about this release.",
      "Re-run publish-release.mjs with the same arguments once they are up (the",
      "GitHub step is idempotent; the new event simply replaces this one).",
    ]);
  }
  console.log(`legacy url: ${legacyUrl}`);
  console.log(`urls: ${urls.join(" ")}`);
  emit();
  console.log(`announced ${channel} ${version} (code ${versionCode})`);
  return { legacyUrl, urls };
}

/** Wrap a `gh` call so a failure carries its stderr, not just an exit code. */
function ghOrThrow(args, context) {
  try {
    return gh(args);
  } catch (err) {
    const msg = `${err.stderr ?? ""}${err.message ?? ""}`;
    throw new Error(`${context}: ${msg.trim()}`);
  }
}

/**
 * `--from-draft <tag>`: verify a CI-drafted release end to end, publish the
 * draft, verify the GitHub download, then run the same Blossom + Nostr flow
 * as the default mode. See the module header for the full order.
 */
async function runFromDraft(args, sk) {
  const repo = process.env.CHARTER_GITHUB_REPO ?? RELEASE_GITHUB_REPO;
  const tag = args.fromDraft;
  const { channel, version } = channelAndVersionFromTag(tag);
  const gr = githubRelease(channel, version, repo);
  console.log(`--from-draft ${tag}: channel=${channel} version=${version} asset=${gr.asset}`);

  const manifestFile = MANIFEST_FILES[channel];
  const manifestPath = join(PUBLIC_DIR, manifestFile);
  const oldVersionCode = readManifestVersionCode(manifestPath);

  // The forgot-to-bump guard runs BEFORE anything else — even before the
  // draft is looked at — so a stray re-run refuses with nothing published
  // and no `gh` call made at all. It only needs the committed manifest and
  // --version-code, so it behaves the same in --dry-run as for real.
  if (args.dryRun) {
    console.log("--dry-run: would `gh release download` the asset + SHA256SUMS, check the sha256,");
    console.log("--dry-run: `gh attestation verify --format json` it (signer-workflow + source-ref +");
    console.log("--dry-run: deny-self-hosted-runners), check the attested commit is an ancestor of");
    console.log("--dry-run: origin/main, and (for an APK channel) verify the signing cert with");
    console.log("--dry-run: apksigner against the RELEASE_CERT_SHA256 pin — then publish the draft,");
    console.log("--dry-run: verify the GitHub download, and run the normal Blossom + Nostr flow.");
    console.log(`--dry-run: would then write apps/charter-app/public/${manifestFile} (exact values`);
    console.log("--dry-run: known only once the download above verifies; shown here as a shape):");
    const preview = {
      versionName: version,
      versionCode: args.versionCode,
      url: "<verified direct-200 Blossom URL, known only after `gh release download`>",
      urls: [gr.url, "<verified Blossom mirror>"],
      ...(channel === "charter-deb"
        ? { sha256: "<sha256 of the downloaded asset>" }
        : { apkSha256: "<sha256 of the downloaded asset>", certSha256: "<verified signing cert>" }),
      sizeBytes: "<downloaded asset size>",
      builtAt: "<the GitHub release asset's createdAt>",
    };
    console.log(JSON.stringify(preview, null, 2));
    console.log("--dry-run: No network calls made.");
    assertVersionCodeBump(oldVersionCode, args.versionCode, { manifestPath });
    return;
  }

  assertVersionCodeBump(oldVersionCode, args.versionCode, { manifestPath });

  const view = JSON.parse(
    ghOrThrow(["release", "view", tag, "--repo", repo, "--json", "isDraft,assets"], `gh release view ${tag}`),
  );
  // Already published: this is either a stray re-run (refuse, name --resume)
  // or an explicit --resume of a run that published the draft but didn't
  // finish mirroring/announcing (e.g. it died between the two). Either way,
  // re-verify everything from scratch below — a resume never skips
  // verification, only the publish step itself.
  const alreadyPublished = !view.isDraft;
  if (alreadyPublished && !args.resume) {
    throw new Error(
      `${tag} on ${repo} is already published (not a draft) — nothing to verify or publish. ` +
        "If you are resuming a previous --from-draft run that published the release but did not " +
        "finish mirroring to Blossom / announcing on Nostr, re-run with --resume.",
    );
  }
  if (!(view.assets ?? []).some((a) => a.name === gr.asset)) {
    throw new Error(
      `${alreadyPublished ? "published release" : "draft"} ${tag} on ${repo} has no asset named ${gr.asset}`,
    );
  }

  const dir = mkdtempSync(join(tmpdir(), "charter-from-draft-"));
  try {
    console.log(`downloading ${gr.asset} + SHA256SUMS from ${tag}…`);
    ghOrThrow(
      ["release", "download", tag, "--repo", repo, "--dir", dir, "--pattern", gr.asset, "--pattern", "SHA256SUMS"],
      `gh release download ${tag}`,
    );

    const artifactPath = join(dir, gr.asset);
    const bytes = readFileSync(artifactPath);
    const sha256 = createHash("sha256").update(bytes).digest("hex");
    const sizeBytes = statSync(artifactPath).size;

    const sums = parseSha256Sums(readFileSync(join(dir, "SHA256SUMS"), "utf8"));
    const expected = sums.get(gr.asset);
    if (!expected) throw new Error(`SHA256SUMS has no entry for ${gr.asset}`);
    if (expected !== sha256)
      throw new Error(`sha256 mismatch: SHA256SUMS says ${expected}, downloaded bytes hash to ${sha256}`);
    console.log(`sha256 ok: ${sha256}`);

    console.log("verifying build provenance (gh attestation verify)…");
    const attestOut = ghOrThrow(
      [
        "attestation",
        "verify",
        artifactPath,
        "--repo",
        repo,
        "--signer-workflow",
        `${repo}/.github/workflows/release-artifacts.yml`,
        "--source-ref",
        `refs/tags/${tag}`,
        "--deny-self-hosted-runners",
        "--format",
        "json",
      ],
      "gh attestation verify",
    );
    const attestedCommit = extractAttestedCommit(JSON.parse(attestOut));
    console.log(`attestation ok — attested source commit ${attestedCommit}`);

    try {
      execFileSync("git", ["fetch", "origin", "main"], { stdio: ["ignore", "pipe", "pipe"] });
    } catch (err) {
      throw new Error(`git fetch origin main: ${(err.stderr ?? err.message ?? "").toString().trim()}`);
    }
    if (!isAncestorOfOriginMain(attestedCommit)) {
      throw new Error(
        `attested source commit ${attestedCommit} is not an ancestor of origin/main — refusing to publish`,
      );
    }
    console.log(`source commit ${attestedCommit} verified as an ancestor of origin/main`);

    let cert = args.cert;
    if (channel !== "charter-deb") {
      const apksigner = findApksigner();
      const verifyOutput = execFileSync(apksigner, ["verify", "-v", "--print-certs", artifactPath], {
        encoding: "utf8",
      });
      let lineageOutput = null;
      try {
        lineageOutput = execFileSync(apksigner, ["lineage", "--in", artifactPath, "--print-certs"], {
          encoding: "utf8",
        });
      } catch {
        lineageOutput = null; // no lineage at all — verifyRotatedApkCert decides if that's fatal
      }
      const actualCert = verifyRotatedApkCert({ verifyOutput, lineageOutput });
      assertPinnedCert(actualCert);
      console.log(`apk signing cert ok (pinned): ${actualCert}`);
      if (args.cert && args.cert.toLowerCase() !== actualCert.toLowerCase()) {
        throw new Error(`--cert ${args.cert} does not match the APK's actual signing cert ${actualCert}`);
      }
      cert = actualCert;
    }

    // Everything above is verification only — nothing announced or touched
    // yet. From here on, in order (review fix #7): publish the draft FIRST,
    // then prove the GitHub download URL actually serves it, and only THEN
    // mirror to Blossom and sign + announce the Nostr event. A Blossom/relay
    // failure after this point leaves the GitHub Release public (by design:
    // it is the primary host) but not yet mirrored/announced — safe to
    // re-run with --resume, since the draft flip and the GitHub upload are
    // both done.
    if (alreadyPublished) {
      console.log(`--resume: ${tag} is already published — skipping the publish step, continuing`);
    } else {
      console.log(`publishing the draft release ${tag}…`);
      ghOrThrow(["release", "edit", tag, "--repo", repo, "--draft=false"], `gh release edit ${tag}`);
      console.log(`published: ${tag}`);
    }

    await verifyGithubDownload(gr.url, sha256);
    console.log(`github ok: ${gr.url}`);

    const { legacyUrl, urls } = await announceRelease(sk, {
      channel,
      version,
      versionCode: args.versionCode,
      cert,
      notes: args.notes,
      sha256,
      sizeBytes,
      bytes,
      gr,
      dryRun: false,
      emitUrlFile: args.emitUrlFile,
      emitUrlsFile: args.emitUrlsFile,
    });

    // The site's download manifest — same fields/format publish-apk.sh,
    // publish-carrier-apk.sh and publish-deb.sh write via lib.sh's
    // write_manifest, so the maintainer no longer hand-writes it after a
    // --from-draft publish.
    const assetInfo = (view.assets ?? []).find((a) => a.name === gr.asset);
    const builtAt = assetInfo?.createdAt;
    if (!builtAt) throw new Error(`gh release view ${tag}: asset ${gr.asset} has no createdAt`);
    const manifest = buildManifest({
      channel,
      versionName: version,
      versionCode: args.versionCode,
      url: legacyUrl,
      urls,
      sha256,
      certSha256: cert,
      sizeBytes,
      builtAt,
    });
    writeFileSync(manifestPath, formatManifest(manifest));
    console.log(`wrote apps/charter-app/public/${manifestFile}`);

    execFileSync("bash", [join(REPO_ROOT, "scripts", "sync-front-door-downloads.sh")], {
      cwd: REPO_ROOT,
      stdio: "inherit",
    });

    console.log(`next: commit apps/charter-app/public/${manifestFile} and site/, then push`);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const sk = loadReleaseKey();
  console.log(`release key: ${getPublicKey(sk)}`);

  if (args.fromDraft) {
    await runFromDraft(args, sk);
    return;
  }

  const bytes = readFileSync(args.artifact);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const sizeBytes = statSync(args.artifact).size;
  console.log(`${args.artifact}: sha256=${sha256} size=${sizeBytes}`);

  const gr = githubRelease(
    args.channel,
    args.version,
    process.env.CHARTER_GITHUB_REPO ?? RELEASE_GITHUB_REPO,
  );

  // 1. GitHub first. Any failure here aborts before Blossom or an announcement.
  if (args.dryRun) {
    console.log(`--dry-run: would ensure release ${gr.tag} on ${gr.repo} ("${gr.title}")`);
    console.log(`--dry-run: would upload ${gr.asset} (no --clobber) and verify ${gr.url}`);
  } else {
    try {
      await publishToGithub({ gr, artifact: args.artifact, sha256, version: args.version, notes: args.notes });
    } catch (err) {
      console.error(`GitHub Release FAILED: ${err.message}`);
      console.error("refusing to mirror or announce — fix GitHub and re-run (the upload is idempotent)");
      process.exit(1);
    }
  }

  await announceRelease(sk, {
    channel: args.channel,
    version: args.version,
    versionCode: args.versionCode,
    cert: args.cert,
    notes: args.notes,
    sha256,
    sizeBytes,
    bytes,
    gr,
    dryRun: args.dryRun,
    emitUrlFile: args.emitUrlFile,
    emitUrlsFile: args.emitUrlsFile,
  });
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
