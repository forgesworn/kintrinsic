// node --test scripts/release/*.test.mjs
//
// Node-driven tests for scripts/release/lib.sh's shell functions —
// specifically `_properties_get` (the Java-Properties-shaped parser
// android_resolve_signing_env uses to read android/signing.properties) and
// the early RELEASE_CERT_SHA256 refusal in android_sign_rotated. lib.sh has
// no JS equivalent to test directly (it is bash, sourced by the
// android/scripts/build-*.sh scripts), so this drives real bash: source the
// file into a throwaway shell, run a snippet, and assert on its exit code
// and output.
import test from "node:test";
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = dirname(fileURLToPath(import.meta.url));
const LIB_SH = join(__dirname, "lib.sh");

/**
 * Run `script` in bash with lib.sh sourced, cwd set to `cwd` (the functions
 * under test read android/signing.properties relative to cwd), and any
 * extra env vars merged in. Returns { status, stdout, stderr } — never
 * throws, so a non-zero exit is just data to assert on.
 */
function runBash(cwd, script, env = {}) {
  try {
    const stdout = execFileSync("bash", ["-c", `source "${LIB_SH}"; ${script}`], {
      cwd,
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

function withTmpDir(fn) {
  const dir = mkdtempSync(join(tmpdir(), "charter-lib-test-"));
  try {
    return fn(dir);
  } finally {
    rmSync(dir, { recursive: true, force: true });
  }
}

// ---- _properties_get --------------------------------------------------

test("_properties_get reads key=value", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "storeFile=/a/b.keystore\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.status, 0);
    assert.equal(r.stdout.trim(), "/a/b.keystore");
  }));

test("_properties_get reads key = value (spaced =)", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "storeFile = /a/b.keystore\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.stdout.trim(), "/a/b.keystore");
  }));

test("_properties_get reads key: value", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "storeFile: /a/b.keystore\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.stdout.trim(), "/a/b.keystore");
  }));

test("_properties_get reads a whitespace-only separator", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "storeFile   /a/b.keystore\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.stdout.trim(), "/a/b.keystore");
  }));

test("_properties_get strips a trailing \\r (CRLF file)", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "storeFile=/a/b.keystore\r\nkeyAlias=release\r\n");
    const r1 = runBash(dir, "_properties_get p storeFile");
    assert.equal(r1.stdout, "/a/b.keystore\n");
    const r2 = runBash(dir, "_properties_get p keyAlias");
    assert.equal(r2.stdout, "release\n");
  }));

test("_properties_get ignores '#' and '!' comment lines", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "# a comment\n! also a comment\nstoreFile=/a/b.keystore\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.stdout.trim(), "/a/b.keystore");
  }));

test("_properties_get: a later duplicate key wins", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "storeFile=/first\nstoreFile=/second\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.stdout.trim(), "/second");
  }));

test("_properties_get exits 1 with no output when the key is absent", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "p"), "keyAlias=release\n");
    const r = runBash(dir, "_properties_get p storeFile");
    assert.equal(r.status, 1);
    assert.equal(r.stdout, "");
  }));

// ---- android_resolve_signing_env ---------------------------------------

test("android_resolve_signing_env: an already-exported env var wins over the file", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "signing.properties"), "storeFile=/from/file\n");
    const r = runBash(dir, 'android_resolve_signing_env; echo "F=$CHARTER_KEYSTORE_FILE"', {
      CHARTER_KEYSTORE_FILE: "/from/env",
    });
    assert.equal(r.status, 0);
    assert.match(r.stdout, /F=\/from\/env/);
  }));

test("android_resolve_signing_env: no file and no env -> no-op, exit 0", () =>
  withTmpDir((dir) => {
    const r = runBash(dir, 'android_resolve_signing_env; echo "F=${CHARTER_KEYSTORE_FILE:-unset}"');
    assert.equal(r.status, 0);
    assert.match(r.stdout, /F=unset/);
  }));

test("android_resolve_signing_env: a file with no storeFile key -> no-op, exit 0", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "signing.properties"), "someOtherKey=value\n");
    const r = runBash(dir, 'android_resolve_signing_env; echo "F=${CHARTER_KEYSTORE_FILE:-unset}"');
    assert.equal(r.status, 0);
    assert.match(r.stdout, /F=unset/);
  }));

test("android_resolve_signing_env: a complete key=value file resolves all four vars", () =>
  withTmpDir((dir) => {
    writeFileSync(
      join(dir, "signing.properties"),
      "storeFile=/a/b.keystore\nstorePassword=s3cr3t\nkeyAlias=release\nkeyPassword=k3y\n",
    );
    const r = runBash(
      dir,
      'android_resolve_signing_env; echo "F=$CHARTER_KEYSTORE_FILE P=$CHARTER_KEYSTORE_PASSWORD A=$CHARTER_KEY_ALIAS K=$CHARTER_KEY_PASSWORD"',
    );
    assert.equal(r.status, 0);
    assert.match(r.stdout, /F=\/a\/b\.keystore P=s3cr3t A=release K=k3y/);
  }));

test("android_resolve_signing_env: a complete key: value CRLF file resolves all four vars", () =>
  withTmpDir((dir) => {
    writeFileSync(
      join(dir, "signing.properties"),
      "storeFile: /a/b.keystore\r\nstorePassword: s3cr3t\r\nkeyAlias: release\r\nkeyPassword: k3y\r\n",
    );
    const r = runBash(
      dir,
      'android_resolve_signing_env; echo "F=$CHARTER_KEYSTORE_FILE P=$CHARTER_KEYSTORE_PASSWORD A=$CHARTER_KEY_ALIAS K=$CHARTER_KEY_PASSWORD"',
    );
    assert.equal(r.status, 0);
    assert.match(r.stdout, /F=\/a\/b\.keystore P=s3cr3t A=release K=k3y/);
  }));

test("android_resolve_signing_env: storeFile present but the rest missing FAILS loudly", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "signing.properties"), "storeFile=/a/b.keystore\n");
    const r = runBash(dir, "android_resolve_signing_env");
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /missing storePassword\/keyAlias\/keyPassword/);
  }));

test("android_resolve_signing_env: a malformed file (no '=' at all, no storeFile) is a no-op, not a crash", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "signing.properties"), "this is not a properties file\n\n\n");
    const r = runBash(dir, 'android_resolve_signing_env; echo "F=${CHARTER_KEYSTORE_FILE:-unset}"');
    assert.equal(r.status, 0);
    assert.match(r.stdout, /F=unset/);
  }));

// ---- android_sign_rotated: RELEASE_CERT_SHA256 must be pinned first ----

test("android_sign_rotated refuses real signing material while RELEASE_CERT_SHA256 is empty", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "apk"), "fake");
    const r = runBash(dir, "RELEASE_CERT_SHA256=; android_sign_rotated apk", {
      CHARTER_KEYSTORE_FILE: "/fake.keystore",
    });
    assert.notEqual(r.status, 0);
    assert.match(r.stderr, /release cert not pinned yet/);
  }));

test("android_sign_rotated: alpha bridge (no CHARTER_KEYSTORE_FILE) is unaffected by an empty pin", () =>
  withTmpDir((dir) => {
    writeFileSync(join(dir, "apk"), "fake");
    const r = runBash(dir, "RELEASE_CERT_SHA256=; android_sign_rotated apk");
    assert.equal(r.status, 0);
  }));
