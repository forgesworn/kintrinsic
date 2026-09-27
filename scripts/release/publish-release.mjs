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
// Env:
//   CHARTER_BLOSSOM_SERVERS  comma-separated (default below)
//   CHARTER_RELEASE_KEY_FILE key path (default ~/.charter-release/release-key.hex)
//   CHARTER_GITHUB_REPO      owner/name (default forgesworn/kintrinsic)
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
import { copyFileSync, mkdtempSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { join } from "node:path";
import {
  buildBlossomAuth,
  buildReleaseEvent,
  githubRelease,
  isCanonicalBlossomUrl,
  isGithubReleaseUrl,
  orderReleaseUrls,
  RELEASE_GITHUB_REPO,
  RELEASE_RELAYS,
} from "./release-helpers.mjs";

// Both live-verified 2026-08-12 with the 10 MB deb (upload + direct-200 GET).
// blossom.band was tried and REFUSES non-media uploads (HTTP 415);
// blossom.sovbit.host was unreachable. Re-verify before adding servers.
const DEFAULT_BLOSSOM = "https://blossom.primal.net,https://nostr.download";

function parseArgs(argv) {
  const args = { notes: "" };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--dry-run") args.dryRun = true;
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
  for (const req of ["channel", "artifact", "version", "versionCode"]) {
    if (args[req] === undefined || Number.isNaN(args[req]))
      throw new Error(`--${req.replace("versionCode", "version-code")} is required`);
  }
  return args;
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

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const sk = loadReleaseKey();
  console.log(`release key: ${getPublicKey(sk)}`);

  const bytes = readFileSync(args.artifact);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const sizeBytes = statSync(args.artifact).size;
  console.log(`${args.artifact}: sha256=${sha256} size=${sizeBytes}`);

  const gr = githubRelease(
    args.channel,
    args.version,
    process.env.CHARTER_GITHUB_REPO ?? RELEASE_GITHUB_REPO,
  );

  const servers = (process.env.CHARTER_BLOSSOM_SERVERS ?? DEFAULT_BLOSSOM)
    .split(",")
    .map((s) => s.trim().replace(/\/$/, ""))
    .filter(Boolean);

  // The LEGACY single device URL (manifests' `url`, and what fielded
  // redirect-refusing clients pick): extension-bearing, verified direct-200
  // with matching bytes — never reconstructed blindly.
  const ext = args.channel.endsWith("deb") ? "deb" : "apk";
  const deviceBase = (process.env.CHARTER_BLOSSOM_DL_BASE ?? "https://nostr.download").replace(
    /\/$/,
    "",
  );
  const deviceUrl = `${deviceBase}/${sha256}.${ext}`;

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

  // 2. Blossom as the mirror. Failure is a warning once GitHub verified.
  const blossom = [];
  if (args.dryRun) {
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
    process.exit(1);
  }
  const legacyUrl = blossom.includes(deviceUrl) ? deviceUrl : blossom[0];
  if (!args.dryRun && !blossom.includes(deviceUrl)) {
    console.error(`WARNING: preferred mirror ${deviceUrl} did not verify; legacy url is ${legacyUrl}`);
  }

  const urls = orderReleaseUrls([gr.url, legacyUrl, ...blossom]);
  // Belt and braces: buildReleaseEvent throws on a non-canonical URL too.
  const bad = urls.filter((u) => !isCanonicalBlossomUrl(u, sha256) && !isGithubReleaseUrl(u, gr.repo));
  if (bad.length) {
    console.error(`refusing to announce non-canonical url(s): ${bad.join(", ")}`);
    process.exit(1);
  }

  const event = finalizeEvent(
    buildReleaseEvent({
      channel: args.channel,
      versionName: args.version,
      versionCode: args.versionCode,
      sha256,
      sizeBytes,
      urls,
      certSha256: args.cert,
      notes: args.notes,
      createdAt: Math.floor(Date.now() / 1000),
    }),
    sk,
  );

  const emit = () => {
    if (args.emitUrlFile) writeFileSync(args.emitUrlFile, legacyUrl + "\n");
    if (args.emitUrlsFile) writeFileSync(args.emitUrlsFile, urls.join("\n") + "\n");
  };

  if (args.dryRun) {
    console.log(`--dry-run: would publish to ${RELEASE_RELAYS.join(", ")}`);
    console.log("--dry-run: signed event follows; nothing uploaded or published");
    console.log(JSON.stringify(event, null, 2));
    emit();
    return;
  }

  const results = await Promise.all(RELEASE_RELAYS.map((r) => publishToRelay(r, event)));
  for (const r of results) {
    console.log(`${r.ok ? "relay ok " : "relay FAIL"}: ${r.url} ${r.reason ?? ""}`);
  }
  const accepted = results.filter((r) => r.ok).length;
  if (accepted === 0) {
    console.error("every relay refused the release event");
    process.exit(1);
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
  console.log(`announced ${args.channel} ${args.version} (code ${args.versionCode})`);
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
