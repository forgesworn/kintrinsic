#!/usr/bin/env node
// Publish one release: upload the artifact to Blossom mirrors, then announce
// it as a signed kind-30063 event on the release relays. Invoked as the final
// step of publish-apk.sh / publish-carrier-apk.sh / publish-deb.sh; the origin
// JSON feeds those scripts write remain the fallback until D3.
//
//   node scripts/release/publish-release.mjs \
//     --channel charter-apk --artifact path/to.apk \
//     --version 0.6.9 --version-code 40 [--cert <64hex>] [--notes "…"] [--dry-run]
//
// Env:
//   CHARTER_BLOSSOM_SERVERS  comma-separated (default below)
//   CHARTER_RELEASE_KEY_FILE key path (default ~/.charter-release/release-key.hex)
//
// Exit: non-zero when no Blossom mirror verified (unless --dry-run), or when
// every relay refused the event.

import { finalizeEvent, getPublicKey } from "nostr-tools/pure";
import { createHash } from "node:crypto";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import {
  buildBlossomAuth,
  buildReleaseEvent,
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
 * Verify a mirror the way a DEVICE will use it: GET, no redirects, full-body
 * sha256. If the server answers with ONE redirect (primal fronts blobs with
 * a CDN: HEAD lies 200, GET 302s — found live 2026-08-12), resolve it and
 * verify the target instead — the event then carries the direct URL.
 * Returns the verified URL or null.
 */
/**
 * Verify the EXACT device download URL the manifests + front door will use:
 * direct 200 (the on-device stagers refuse redirects), serving exactly these
 * bytes. This is what stops a partial-mirror success (e.g. primal up,
 * nostr.download down) from leaving a dead URL in a committed manifest.
 */
async function verifyDeviceUrl(url, sha256) {
  const res = await fetch(url, { redirect: "manual" });
  if (res.status !== 200) return false;
  const bytes = Buffer.from(await res.arrayBuffer());
  return createHash("sha256").update(bytes).digest("hex") === sha256;
}

async function verifyBlossom(server, sha256) {
  let url = `${server}/${sha256}`;
  for (let hop = 0; hop < 2; hop++) {
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
      url = new URL(r.location, url).toString();
      if (!url.startsWith("https://")) return null;
      continue;
    }
    console.error(`mirror ${url}: ${r.error}`);
    return null;
  }
  console.error(`mirror ${server}: too many redirects`);
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

async function main() {
  const args = parseArgs(process.argv.slice(2));
  const sk = loadReleaseKey();
  console.log(`release key: ${getPublicKey(sk)}`);

  const bytes = readFileSync(args.artifact);
  const sha256 = createHash("sha256").update(bytes).digest("hex");
  const sizeBytes = statSync(args.artifact).size;
  console.log(`${args.artifact}: sha256=${sha256} size=${sizeBytes}`);

  const servers = (process.env.CHARTER_BLOSSOM_SERVERS ?? DEFAULT_BLOSSOM)
    .split(",")
    .map((s) => s.trim().replace(/\/$/, ""))
    .filter(Boolean);

  const urls = [];
  if (args.dryRun) {
    // A dry-run event must still be VALID: derive mirror URLs without uploading.
    for (const s of servers) urls.push(`${s}/${sha256}`);
  } else {
    for (const server of servers) {
      try {
        await uploadToBlossom(server, bytes, sha256, sk);
        const verified = await verifyBlossom(server, sha256);
        if (verified) {
          urls.push(verified);
          console.log(`mirror ok: ${verified}`);
        } else {
          console.error(`mirror FAILED verification: ${server}`);
        }
      } catch (err) {
        console.error(`mirror FAILED: ${err.message}`);
      }
    }
    if (urls.length === 0) {
      console.error("no Blossom mirror verified — refusing to announce");
      process.exit(1);
    }
  }

  // The single DEVICE download URL the manifests + downloads.json will name.
  // Extension-bearing (so a browser download keeps a sensible filename) and
  // verified direct-200 with matching bytes — never reconstructed blindly.
  const ext = args.channel.endsWith("deb") ? "deb" : "apk";
  const deviceBase = (process.env.CHARTER_BLOSSOM_DL_BASE ?? "https://nostr.download").replace(
    /\/$/,
    "",
  );
  const deviceUrl = `${deviceBase}/${sha256}.${ext}`;
  if (!args.dryRun && !(await verifyDeviceUrl(deviceUrl, sha256))) {
    console.error(`device URL not servable (direct-200 + matching bytes): ${deviceUrl}`);
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

  if (args.dryRun) {
    console.log("--dry-run: signed event follows; nothing uploaded or published");
    console.log(JSON.stringify(event, null, 2));
    if (args.emitUrlFile) writeFileSync(args.emitUrlFile, deviceUrl + "\n");
    return;
  }

  const results = await Promise.all(RELEASE_RELAYS.map((r) => publishToRelay(r, event)));
  for (const r of results) {
    console.log(`${r.ok ? "relay ok " : "relay FAIL"}: ${r.url} ${r.reason ?? ""}`);
  }
  if (!results.some((r) => r.ok)) {
    console.error("every relay refused the release event");
    process.exit(1);
  }
  console.log(`device url verified: ${deviceUrl}`);
  if (args.emitUrlFile) writeFileSync(args.emitUrlFile, deviceUrl + "\n");
  console.log(`announced ${args.channel} ${args.version} (code ${args.versionCode})`);
}

main().catch((err) => {
  console.error(err.message);
  process.exit(1);
});
