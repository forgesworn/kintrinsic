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
