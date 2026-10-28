import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SYNC = join(ROOT, "scripts", "sync-front-door-downloads.sh");
const STALE = "bf2fd45b51a728879f6d0f4e8c42109181d499bd15d024987795f9f08553cbbb";

function sync(site) {
  return execFileSync("bash", [SYNC], {
    cwd: ROOT,
    env: { ...process.env, CHARTER_YOU_DIR: site },
    encoding: "utf8",
  });
}

test("sync rewrites every printed checksum span, including a stale one", () => {
  const site = mkdtempSync(join(tmpdir(), "charter-site-"));
  try {
    copyFileSync(join(ROOT, "site", "download.html"), join(site, "download.html"));
    const page = join(site, "download.html");
    // Simulate a hand-typed checksum left over from an older release.
    writeFileSync(page, readFileSync(page, "utf8").replace(/(<span data-sha="linux">)[0-9a-f]{64}/, `$1${STALE}`));
    assert.ok(readFileSync(page, "utf8").includes(STALE));

    sync(site);

    const dl = JSON.parse(readFileSync(join(site, "downloads.json"), "utf8"));
    const html = readFileSync(page, "utf8");
    assert.ok(!html.includes(STALE));
    for (const key of ["android", "ward", "linux"]) {
      assert.match(dl[key].sha256, /^[0-9a-f]{64}$/);
      assert.ok(html.includes(`<span data-sha="${key}">${dl[key].sha256}</span>`), `${key} checksum`);
      assert.ok(html.includes(`href="${dl[key].url}"`), `${key} download link`);
    }
  } finally {
    rmSync(site, { recursive: true, force: true });
  }
});

test("sync fails loudly when a platform's checksum span is missing from the page", () => {
  const site = mkdtempSync(join(tmpdir(), "charter-site-"));
  try {
    const page = join(site, "download.html");
    writeFileSync(page, readFileSync(join(ROOT, "site", "download.html"), "utf8").replace(/<span data-sha="ward">/, '<span>'));
    assert.throws(() => execFileSync("bash", [SYNC], {
      cwd: ROOT, env: { ...process.env, CHARTER_YOU_DIR: site }, encoding: "utf8", stdio: "pipe",
    }), /data-sha="ward" checksum/);
  } finally {
    rmSync(site, { recursive: true, force: true });
  }
});

// ---- hostile feed values: refuse, never write them into the page ----------

import { cpSync, mkdirSync } from "node:fs";

function syncWithTamperedFeed(file, mutate) {
  const dir = mkdtempSync(join(tmpdir(), "charter-feed-"));
  const pub = join(dir, "public");
  const site = join(dir, "site");
  try {
    mkdirSync(pub);
    mkdirSync(site);
    for (const f of ["charter-apk.json", "mycharter-apk.json", "charter-deb.json"]) {
      cpSync(join(ROOT, "apps", "charter-app", "public", f), join(pub, f));
    }
    copyFileSync(join(ROOT, "site", "download.html"), join(site, "download.html"));
    const path = join(pub, file);
    writeFileSync(path, JSON.stringify(mutate(JSON.parse(readFileSync(path, "utf8")))));
    const before = readFileSync(join(site, "download.html"), "utf8");
    let err;
    try {
      execFileSync("bash", [SYNC], {
        cwd: ROOT,
        env: { ...process.env, CHARTER_YOU_DIR: site, CHARTER_PUBLIC_DIR: pub },
        encoding: "utf8",
        stdio: "pipe",
      });
    } catch (e) {
      err = e;
    }
    return { err, unchanged: readFileSync(join(site, "download.html"), "utf8") === before };
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

test("sync refuses a versionName with markup characters", () => {
  for (const bad of ['1.0"><script>', "1.0 0", "1.0<", "1.0'x", ""]) {
    const { err, unchanged } = syncWithTamperedFeed("charter-apk.json", (m) => ({ ...m, versionName: bad }));
    assert.ok(err, `accepted versionName ${JSON.stringify(bad)}`);
    assert.match(String(err.stderr), /versionName|missing version/);
    assert.ok(unchanged);
  }
});

test("sync refuses a download URL with a quote, angle bracket or space", () => {
  for (const bad of ['"', "<", ">", "'", " "]) {
    const url = `https://nostr.download/a${bad}b.apk`;
    const { err, unchanged } = syncWithTamperedFeed("charter-apk.json", (m) => ({ ...m, url, urls: [url] }));
    assert.ok(err, `accepted url containing ${JSON.stringify(bad)}`);
    assert.match(String(err.stderr), /not a plain https URL/);
    assert.ok(unchanged);
  }
});

test("sync refuses a non-https URL, including a bad mirror", () => {
  const github = "https://github.com/forgesworn/kintrinsic/releases/download/ward-v1/a.apk";
  let r = syncWithTamperedFeed("charter-apk.json", (m) => ({ ...m, url: "http://nostr.download/a.apk", urls: [] }));
  assert.ok(r.err);
  assert.ok(r.unchanged);
  r = syncWithTamperedFeed("charter-apk.json", (m) => ({
    ...m,
    url: "https://nostr.download/a b.apk",
    urls: [github],
  }));
  assert.ok(r.err);
  assert.match(String(r.err.stderr), /mirror url is not a plain https URL/);
  assert.ok(r.unchanged);
});
