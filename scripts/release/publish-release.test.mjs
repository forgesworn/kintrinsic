// node --test scripts/release/
//
// publish-release.mjs is a CLI script (no exports of its own argument
// parsing), so these drive it as a subprocess rather than importing
// anything — just enough coverage for the --resume flag's own validation,
// which release-helpers.test.mjs can't reach.
import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { randomBytes } from "node:crypto";

const __dirname = dirname(fileURLToPath(import.meta.url));
const SCRIPT = join(__dirname, "publish-release.mjs");

function run(args, env = {}) {
  try {
    const stdout = execFileSync("node", [SCRIPT, ...args], {
      env: { ...process.env, ...env },
      encoding: "utf8",
    });
    return { status: 0, stdout, stderr: "" };
  } catch (err) {
    return {
      status: err.status ?? 1,
      stdout: (err.stdout ?? "").toString(),
      stderr: (err.stderr ?? "").toString(),
    };
  }
}

test("--resume without --from-draft is refused before touching the release key", () => {
  const r = run(["--resume", "--channel", "charter-apk", "--artifact", "x", "--version", "1.0.0", "--version-code", "1"]);
  assert.notEqual(r.status, 0);
  assert.match(r.stderr, /--resume only makes sense with --from-draft/);
});

test("--from-draft --resume --dry-run prints the plan and makes no network calls", () => {
  const dir = mkdtempSync(join(tmpdir(), "charter-release-key-"));
  try {
    const keyFile = join(dir, "release-key.hex");
    writeFileSync(keyFile, randomBytes(32).toString("hex"));
    // --version-code must clear the (local, no-network) forgot-to-bump guard
    // against the real committed apps/charter-app/public/charter-apk.json —
    // 999 is comfortably above any versionCode that file will ever hold.
    const r = run(["--from-draft", "ward-v9.9.9", "--version-code", "999", "--resume", "--dry-run"], {
      CHARTER_RELEASE_KEY_FILE: keyFile,
    });
    assert.equal(r.status, 0);
    assert.match(r.stdout, /--from-draft ward-v9\.9\.9/);
    assert.match(r.stdout, /No network calls made/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});

test("--from-draft --dry-run prints the manifest shape it would write, then refuses on the forgot-to-bump guard, before any `gh` call", () => {
  const dir = mkdtempSync(join(tmpdir(), "charter-release-key-"));
  try {
    const keyFile = join(dir, "release-key.hex");
    writeFileSync(keyFile, randomBytes(32).toString("hex"));
    // The real committed apps/charter-app/public/mycharter-apk.json is at
    // versionCode 17 (guardian-v0.1.16) — passing the same code must refuse.
    const r = run(
      ["--from-draft", "guardian-v0.1.16", "--version-code", "17", "--dry-run", "--resume"],
      { CHARTER_RELEASE_KEY_FILE: keyFile },
    );
    assert.notEqual(r.status, 0);
    // The manifest-shape preview printed to stdout before the guard fired.
    assert.match(r.stdout, /would then write apps\/charter-app\/public\/mycharter-apk\.json/);
    assert.match(r.stdout, /"apkSha256": "<sha256 of the downloaded asset>"/);
    assert.match(r.stdout, /"certSha256": "<verified signing cert>"/);
    // The guard's refusal — nothing published, no `gh` call ever made.
    assert.match(r.stderr, /versionCode 17 <= published 17/);
    assert.match(r.stderr, /mycharter-apk\.json/);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
});
