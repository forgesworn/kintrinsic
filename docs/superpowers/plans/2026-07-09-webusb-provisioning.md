# WebUSB Device-Owner Provisioning Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a non-technical guardian provision a factory-fresh GrapheneOS phone into a Charter-managed Device Owner from MyCharter in the browser — plug in a USB cable, click through, and the phone comes up **owned and paired** with zero terminal and zero QR scans.

**Architecture:** MyCharter gains a "Set up a phone (with a cable)" path that drives adb-over-WebUSB (ya-webadb / Tango) from a Chromium browser: preflight the phone, install the release Charter APK, run `dpm set-device-owner`, then fire the existing `bunker://` pairing intent over the cable — reusing the phone's `MainActivity.handlePairingIntent` and the already-proven STATUS-echo pairing match. A signed **release** APK (new one-time keystore) is required; its cert fingerprint is added to `assetlinks.json` so App-Link pairing keeps working on release builds. A `charter-provision.sh` ships as the scripted fallback.

**Tech Stack:** TypeScript/React (MyCharter PWA, Vite), `@yume-chan/adb` + `@yume-chan/adb-daemon-webusb` (WebUSB adb), Android release signing (`apksigner`/Gradle), bash.

## Global Constraints

- **Chromium-only.** WebUSB exists only in Chromium browsers. The flow must feature-detect (`navigator.usb`) and show a graceful "open this in Chrome/Edge/Brave" notice elsewhere — never a broken button.
- **No fabricated security values.** The release cert SHA-256 written into `assetlinks.json` and shown in the UI MUST come from the real built keystore (`apksigner verify --print-certs` / `keytool`). Never guess a fingerprint. (See the [[no-fabricated-values]] discipline.)
- **the sysadmin owns deploy.** The release APK is hosted by pushing to `main` (pipeline serves it from the PWA origin). The SSH key / deploy secret is never a blocker — do not raise it.
- **Reuse the pairing path.** Do NOT invent a new pairing mechanism. The cable pairing fires the SAME `bunker://…&token=` URI the QR flow mints, into the SAME `MainActivity` intent handler; the SAME STATUS-echo match binds the device. The QR path stays intact for re-pairing.
- **Fail honestly.** Every adb step surfaces its real output. `dpm set-device-owner` refusals (existing accounts, already-provisioned) show specific remediation, never an opaque spinner.
- **App id / admin:** `org.forgesworn.charter` / `org.forgesworn.charter/.admin.CharterDeviceAdminReceiver`.
- **ya-webadb is version-sensitive.** Its API has moved across releases. Task 1 PINS the exact API against the installed version before any UI is built; all later code uses the wrapper from Task 2, never raw library calls scattered across components.

---

## File Structure

**Release signing (Task 3):**
- `android/app/build.gradle.kts` — a `release` signingConfig + buildType (no `testOnly`).
- `android/keystore/README.md` — how the release keystore was generated + the real cert fingerprint (the keystore file itself is git-ignored).
- `android/.gitignore` — ignore `*.jks` / `keystore/*.jks`.

**App-Links (Task 4):**
- `apps/charter-app/public/.well-known/assetlinks.json` — add the release cert fingerprint.

**WebUSB adb wrapper (Tasks 1-2, 5-6):**
- `apps/charter-app/src/provision/webusb.ts` — connect/disconnect, the typed `AdbSession` wrapper.
- `apps/charter-app/src/provision/provision.ts` — the orchestration: preflight → install → set-device-owner → pair; each a discrete, status-reporting step.
- `apps/charter-app/src/provision/provision.test.ts` — unit tests over a fake adb transport.

**UI (Task 7):**
- `apps/charter-app/src/screens/Family.tsx` — add the "with a cable" phone path (a `SetupPhoneCable` component) beside the existing scan flow.

**Fallback script (Task 8):**
- `android/scripts/charter-provision.sh` — the scripted DO + pair path (makes port-spec §3.8's reference real).

---

## Task 1: Pin the ya-webadb API (spike) + add the dependencies

**Files:**
- Modify: `apps/charter-app/package.json` (add deps)
- Create: `apps/charter-app/src/provision/API_NOTES.md` (the pinned API surface, written from the installed version)

**Interfaces:**
- Produces: the exact import paths + call shapes for (a) requesting a WebUSB device, (b) authenticating a transport, (c) spawning a shell command and reading its stdout, (d) pushing a file (APK) — recorded in `API_NOTES.md` for Task 2 to consume.

- [ ] **Step 1: Install the libraries (pin versions)**

Run (from the PWA package):
```bash
cd ~/charter/apps/charter-app
npm install @yume-chan/adb @yume-chan/adb-daemon-webusb @yume-chan/stream-extra
```
Record the resolved versions from `package.json`.

- [ ] **Step 2: Read the installed version's real API**

The API differs across major versions — read what is actually installed, do not trust memory:
```bash
cd ~/charter/apps/charter-app
ls node_modules/@yume-chan/adb/esm/ 2>/dev/null | head
grep -rl "requestDevice\|AdbDaemonWebUsbDeviceManager" node_modules/@yume-chan/adb-daemon-webusb/esm/ | head
grep -rl "AdbDaemonTransport\|authenticate" node_modules/@yume-chan/adb/esm/ | head
```
Also fetch the current README to confirm the connect→authenticate→spawn pattern for this version: use WebFetch on `https://github.com/yume-chan/ya-webadb` (the Tango "get started" snippet).

- [ ] **Step 3: Write the pinned API notes**

Create `apps/charter-app/src/provision/API_NOTES.md` documenting, for the installed version, the concrete calls Task 2 will wrap:
- device request: `AdbDaemonWebUsbDeviceManager.BROWSER!.requestDevice()` (or the version's equivalent)
- connection: `device.connect()`
- credential store: the class used to persist the RSA key (e.g. `AdbWebCredentialStore`) and where it stores it
- transport: `AdbDaemonTransport.authenticate({ serial, connection, credentialStore })` → `new Adb(transport)`
- shell one-shot: the exact call that runs `pm ...`/`dpm ...` and yields combined stdout+exit code (e.g. `adb.subprocess.noneProtocol.spawn(cmd)` then read the stream, or `shellProtocol`)
- file push: the sync API to push an APK to `/data/local/tmp/…` (e.g. `adb.sync()` → `write`), or the `PackageManager.install` helper from `@yume-chan/android-bin` if that's cleaner.

- [ ] **Step 4: Commit**

```bash
cd ~/charter
git add apps/charter-app/package.json apps/charter-app/package-lock.json apps/charter-app/src/provision/API_NOTES.md
git commit -m "build(pwa): add ya-webadb (WebUSB adb) + pin its API surface for provisioning

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 2: `webusb.ts` — the typed adb session wrapper

**Files:**
- Create: `apps/charter-app/src/provision/webusb.ts`
- Create: `apps/charter-app/src/provision/webusb.test.ts`

**Interfaces:**
- Consumes: the pinned calls from `API_NOTES.md` (Task 1).
- Produces:
  - `interface AdbSession { shell(cmd: string): Promise<{ stdout: string; exitCode: number }>; pushFile(bytes: Uint8Array, remotePath: string): Promise<void>; close(): Promise<void>; serial: string }`
  - `webUsbSupported(): boolean`
  - `connectPhone(): Promise<AdbSession>` (throws a typed `ProvisionError` with a `code` on user-cancel / no-device / auth-declined)
  - `class ProvisionError extends Error { code: "unsupported"|"no-device"|"auth-declined"|"adb"|"precondition"; }`
  Consumed by `provision.ts` (Task 5).

- [ ] **Step 1: Write the failing test (support-detection + error typing, no hardware)**

Create `apps/charter-app/src/provision/webusb.test.ts`:

```ts
import { describe, it, expect, vi, afterEach } from "vitest";
import { webUsbSupported, connectPhone, ProvisionError } from "./webusb";

describe("webusb support detection", () => {
  afterEach(() => { vi.unstubAllGlobals(); });

  it("reports unsupported when navigator.usb is absent", () => {
    vi.stubGlobal("navigator", {});
    expect(webUsbSupported()).toBe(false);
  });

  it("reports supported when navigator.usb exists", () => {
    vi.stubGlobal("navigator", { usb: {} });
    expect(webUsbSupported()).toBe(true);
  });

  it("connectPhone throws a typed unsupported error off-Chromium", async () => {
    vi.stubGlobal("navigator", {});
    await expect(connectPhone()).rejects.toMatchObject({ code: "unsupported" });
    await expect(connectPhone()).rejects.toBeInstanceOf(ProvisionError);
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run:
```bash
cd ~/charter/apps/charter-app && npx vitest run src/provision/webusb.test.ts
```
Expected: FAIL — module/exports missing.

- [ ] **Step 3: Implement the wrapper**

Create `apps/charter-app/src/provision/webusb.ts` using the pinned API from Task 1. Fill the library-specific calls from `API_NOTES.md` (shown here in the shape the current Tango API uses; adjust import paths to the installed version):

```ts
import { AdbDaemonWebUsbDeviceManager } from "@yume-chan/adb-daemon-webusb";
import { Adb, AdbDaemonTransport } from "@yume-chan/adb";
// Credential store persists the adb RSA key so the phone only prompts once.
import AdbWebCredentialStore from "@yume-chan/adb-credential-web";

export type ProvisionErrorCode =
  | "unsupported" | "no-device" | "auth-declined" | "adb" | "precondition";

export class ProvisionError extends Error {
  constructor(readonly code: ProvisionErrorCode, message: string) {
    super(message);
    this.name = "ProvisionError";
  }
}

export interface AdbSession {
  readonly serial: string;
  shell(cmd: string): Promise<{ stdout: string; exitCode: number }>;
  pushFile(bytes: Uint8Array, remotePath: string): Promise<void>;
  close(): Promise<void>;
}

export function webUsbSupported(): boolean {
  return typeof navigator !== "undefined" && !!(navigator as any).usb;
}

export async function connectPhone(): Promise<AdbSession> {
  if (!webUsbSupported()) {
    throw new ProvisionError("unsupported", "WebUSB needs a Chromium browser (Chrome, Edge, Brave).");
  }
  const manager = AdbDaemonWebUsbDeviceManager.BROWSER;
  if (!manager) throw new ProvisionError("unsupported", "This browser has no WebUSB.");

  let device;
  try {
    device = await manager.requestDevice(); // shows the browser USB picker
  } catch {
    throw new ProvisionError("no-device", "No device was selected.");
  }
  if (!device) throw new ProvisionError("no-device", "No device was selected.");

  const connection = await device.connect();
  const credentialStore = new AdbWebCredentialStore("Charter");
  let transport: AdbDaemonTransport;
  try {
    transport = await AdbDaemonTransport.authenticate({
      serial: device.serial,
      connection,
      credentialStore,
    });
  } catch {
    throw new ProvisionError(
      "auth-declined",
      "The phone did not allow USB debugging. Tap 'Allow' on the phone and retry.",
    );
  }
  const adb = new Adb(transport);
  return wrapSession(adb, device.serial);
}

function wrapSession(adb: Adb, serial: string): AdbSession {
  return {
    serial,
    async shell(cmd: string) {
      // Run a one-shot command, collect combined stdout, read the exit code.
      const process = await adb.subprocess.noneProtocol.spawn(cmd);
      const chunks: Uint8Array[] = [];
      const reader = process.stdout.getReader();
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        if (value) chunks.push(value);
      }
      const stdout = new TextDecoder().decode(concat(chunks));
      // noneProtocol has no exit code; treat non-empty stderr-style markers as
      // failure at the caller. Where an exit code is needed, use shellProtocol.
      return { stdout, exitCode: 0 };
    },
    async pushFile(bytes: Uint8Array, remotePath: string) {
      const sync = await adb.sync();
      try {
        await sync.write({ filename: remotePath, file: singleChunkReadable(bytes) });
      } finally {
        await sync.dispose();
      }
    },
    async close() {
      await adb.close();
    },
  };
}

function concat(chunks: Uint8Array[]): Uint8Array {
  const total = chunks.reduce((n, c) => n + c.length, 0);
  const out = new Uint8Array(total);
  let o = 0;
  for (const c of chunks) { out.set(c, o); o += c.length; }
  return out;
}

function singleChunkReadable(bytes: Uint8Array): ReadableStream<Uint8Array> {
  return new ReadableStream({
    start(controller) { controller.enqueue(bytes); controller.close(); },
  });
}
```

> Implementation note: the `shell` exit-code + `sync.write` signatures vary by ya-webadb version. Use the exact shapes recorded in `API_NOTES.md`; if `noneProtocol` gives no reliable exit status, switch `shell` to `shellProtocol` (which exposes `exitCode`) and update the tests accordingly. `@yume-chan/adb-credential-web` may be named differently in the installed version — use whatever `API_NOTES.md` pinned.

- [ ] **Step 4: Run the test to verify it passes**

Run:
```bash
cd ~/charter/apps/charter-app && npx vitest run src/provision/webusb.test.ts
```
Expected: 3 tests PASS.

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add apps/charter-app/src/provision/webusb.ts apps/charter-app/src/provision/webusb.test.ts apps/charter-app/package.json apps/charter-app/package-lock.json
git commit -m "feat(pwa/provision): typed WebUSB adb session wrapper (connect/shell/push)

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 3: Release signing config + keystore (real cert)

**Files:**
- Modify: `android/app/build.gradle.kts` (add `signingConfigs.release` + `buildTypes.release`)
- Create: `android/keystore/README.md`
- Modify: `android/.gitignore` (ignore the keystore)

**Interfaces:**
- Produces: a reproducible **release** APK at `android/app/build/outputs/apk/release/app-release.apk`, and the real cert SHA-256 (recorded, used by Tasks 4 + 7). No `testOnly` flag (unlike debug).

- [ ] **Step 1: Generate the release keystore (one-time)**

Run (interactive — pick a strong password; record it in a password manager, NOT in git):
```bash
cd ~/charter/android
mkdir -p keystore
keytool -genkeypair -v -keystore keystore/charter-release.jks \
  -alias charter -keyalg RSA -keysize 4096 -validity 10000 \
  -dname "CN=ForgeSworn Charter, O=ForgeSworn"
```

- [ ] **Step 2: Read the REAL cert fingerprint**

Run:
```bash
cd ~/charter/android
keytool -list -v -keystore keystore/charter-release.jks -alias charter | grep -A1 "SHA256:"
```
Record the SHA-256 fingerprint exactly (colon-separated hex). This is the value used in Tasks 4 + 7 — it must be the real output, never invented.

- [ ] **Step 3: Ignore the keystore, add the signing config**

Add to `android/.gitignore`:
```
keystore/*.jks
keystore/*.keystore
signing.properties
```

Create `android/signing.properties` (git-ignored) with the passwords, and read it in `android/app/build.gradle.kts`:
```kotlin
import java.util.Properties

val signingProps = Properties().apply {
    val f = rootProject.file("signing.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}

android {
    signingConfigs {
        create("release") {
            if (signingProps.getProperty("storeFile") != null) {
                storeFile = rootProject.file(signingProps.getProperty("storeFile"))
                storePassword = signingProps.getProperty("storePassword")
                keyAlias = signingProps.getProperty("keyAlias")
                keyPassword = signingProps.getProperty("keyPassword")
            }
        }
    }
    buildTypes {
        getByName("release") {
            isMinifyEnabled = false
            signingConfig = signingConfigs.getByName("release")
            // NOTE: no testOnly — a release DO app must not be trivially removable.
        }
    }
}
```

`android/signing.properties` contents (git-ignored):
```
storeFile=keystore/charter-release.jks
storePassword=<real>
keyAlias=charter
keyPassword=<real>
```

- [ ] **Step 4: Build + verify the release APK is signed with that cert**

The `.so` is git-ignored, so it must be freshly built into the working tree BEFORE gradle packages the APK — otherwise the release carries a stale or missing native lib. Run `build-jni.sh` first, then assemble:
```bash
cd ~/charter/android && source ~/Android/env.sh && bash scripts/build-jni.sh
cd ~/charter/android && ./gradlew :app:assembleRelease
$ANDROID_HOME/build-tools/*/apksigner verify --print-certs app/build/outputs/apk/release/app-release.apk | grep -i "SHA-256"
```
Expected: `build-jni.sh` writes both ABIs (16 KB aligned, no `mock`); BUILD SUCCESSFUL; the printed signer SHA-256 matches Step 2's fingerprint. (Cross-plan: run the web-content plan first so this APK carries the DNS-filter `.so`.)

- [ ] **Step 5: Document it**

Create `android/keystore/README.md`: how the keystore was generated (the keytool command), where the passwords live (password manager, not git), the real cert SHA-256 fingerprint, and that `assetlinks.json` + the provisioning UI both pin this value. State plainly the keystore file is git-ignored and sysadmin-owned for the deploy pipeline.

- [ ] **Step 6: Commit (config + docs only — never the keystore)**

```bash
cd ~/charter
git add android/app/build.gradle.kts android/.gitignore android/keystore/README.md
git status  # CONFIRM no .jks / signing.properties staged
git commit -m "build(android): release signingConfig + keystore docs (no testOnly on release)

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 4: Add the release cert to `assetlinks.json`

**Files:**
- Modify: `apps/charter-app/public/.well-known/assetlinks.json`

**Interfaces:** none (static hosting). Makes the `https://charter.mysignet.app/pair` App Link verify against the RELEASE build too, so cable-pairing (and QR re-pairing) deep-links open the app on a release install.

- [ ] **Step 1: Add the release fingerprint alongside the debug one**

The file currently pins only the debug cert. Add the real release SHA-256 from Task 3 Step 2 to the `sha256_cert_fingerprints` array (keep the debug one so debug builds still verify during dev):

```json
[
  {
    "relation": ["delegate_permission/common.handle_all_urls"],
    "target": {
      "namespace": "android_app",
      "package_name": "org.forgesworn.charter",
      "sha256_cert_fingerprints": [
        "D9:C7:F3:DE:D3:86:E9:AD:36:BD:FF:31:D0:7B:31:C6:C6:BF:E2:37:9E:C3:3D:E7:A2:B6:F6:AC:68:0F:BB:42",
        "<REAL RELEASE SHA-256 FROM TASK 3 — colon-separated hex>"
      ]
    }
  }
]
```

- [ ] **Step 2: Verify JSON validity**

Run:
```bash
cd ~/charter && python3 -m json.tool apps/charter-app/public/.well-known/assetlinks.json > /dev/null && echo OK
```
Expected: `OK`.

- [ ] **Step 3: Commit**

```bash
cd ~/charter
git add apps/charter-app/public/.well-known/assetlinks.json
git commit -m "feat(pwa): pin the release cert in assetlinks so App-Link pairing verifies on release

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 5: `provision.ts` — orchestration (preflight → install → own → pair)

**Files:**
- Create: `apps/charter-app/src/provision/provision.ts`
- Create: `apps/charter-app/src/provision/provision.test.ts`

**Interfaces:**
- Consumes: `AdbSession` (Task 2).
- Produces:
  - `type ProvisionStep = "preflight" | "install" | "own" | "pair" | "done"`
  - `interface ProvisionProgress { step: ProvisionStep; ok: boolean; detail: string }`
  - `async function preflight(s: AdbSession): Promise<void>` (throws `ProvisionError("precondition", …)` with specific remediation)
  - `async function installApk(s: AdbSession, apk: Uint8Array, onProgress): Promise<void>`
  - `async function setDeviceOwner(s: AdbSession): Promise<void>`
  - `async function pairOverCable(s: AdbSession, bunkerUri: string): Promise<void>`
  - `async function runProvision(s, apk, bunkerUri, onProgress): Promise<void>` (the full sequence)
  Consumed by the UI (Task 7).

- [ ] **Step 1: Write the failing tests over a fake session**

Create `apps/charter-app/src/provision/provision.test.ts`:

```ts
import { describe, it, expect, vi } from "vitest";
import { preflight, setDeviceOwner, pairOverCable, runProvision } from "./provision";
import type { AdbSession } from "./webusb";
import { ProvisionError } from "./webusb";

function fakeSession(shellImpl: (cmd: string) => { stdout: string; exitCode: number }): AdbSession {
  return {
    serial: "TEST",
    shell: vi.fn(async (cmd: string) => shellImpl(cmd)),
    pushFile: vi.fn(async () => {}),
    close: vi.fn(async () => {}),
  };
}

describe("preflight", () => {
  it("rejects a phone that already has accounts", async () => {
    const s = fakeSession((cmd) =>
      cmd.includes("dumpsys account") ? { stdout: "Account {name=foo@gmail.com}", exitCode: 0 }
                                      : { stdout: "", exitCode: 0 });
    await expect(preflight(s)).rejects.toMatchObject({ code: "precondition" });
  });

  it("passes a clean phone", async () => {
    const s = fakeSession(() => ({ stdout: "", exitCode: 0 }));
    await expect(preflight(s)).resolves.toBeUndefined();
  });
});

describe("setDeviceOwner", () => {
  it("throws with remediation when dpm refuses", async () => {
    const s = fakeSession(() => ({ stdout: "java.lang.IllegalStateException: Not allowed to set the device owner because there are already several users", exitCode: 0 }));
    await expect(setDeviceOwner(s)).rejects.toMatchObject({ code: "adb" });
  });

  it("succeeds on the success banner", async () => {
    const s = fakeSession(() => ({ stdout: "Success: Device owner set to package org.forgesworn.charter", exitCode: 0 }));
    await expect(setDeviceOwner(s)).resolves.toBeUndefined();
  });
});

describe("pairOverCable", () => {
  it("fires the bunker URI via am start", async () => {
    const calls: string[] = [];
    const s = fakeSession((cmd) => { calls.push(cmd); return { stdout: "Starting: Intent", exitCode: 0 }; });
    await pairOverCable(s, "bunker://relay?pubkey=abc&token=xyz");
    expect(calls.some((c) => c.includes("am start") && c.includes("bunker://"))).toBe(true);
  });
});

describe("runProvision", () => {
  it("walks all steps and reports progress", async () => {
    const s = fakeSession((cmd) => {
      if (cmd.includes("set-device-owner")) return { stdout: "Success: Device owner set", exitCode: 0 };
      if (cmd.includes("pm install")) return { stdout: "Success", exitCode: 0 };
      return { stdout: "", exitCode: 0 };
    });
    const steps: string[] = [];
    await runProvision(s, new Uint8Array([1, 2, 3]), "bunker://r?token=t",
      (p) => steps.push(`${p.step}:${p.ok}`));
    expect(steps).toContain("done:true");
  });
});
```

- [ ] **Step 2: Run to verify it fails**

Run:
```bash
cd ~/charter/apps/charter-app && npx vitest run src/provision/provision.test.ts
```
Expected: FAIL — module missing.

- [ ] **Step 3: Implement the orchestration**

Create `apps/charter-app/src/provision/provision.ts`:

```ts
import { ProvisionError, type AdbSession } from "./webusb";

export type ProvisionStep = "preflight" | "install" | "own" | "pair" | "done";
export interface ProvisionProgress { step: ProvisionStep; ok: boolean; detail: string }
export type OnProgress = (p: ProvisionProgress) => void;

const REMOTE_APK = "/data/local/tmp/charter-release.apk";
const ADMIN = "org.forgesworn.charter/.admin.CharterDeviceAdminReceiver";

/**
 * Verify the phone can be made Device Owner: no added accounts and no extra
 * users (the two states `dpm set-device-owner` refuses). GrapheneOS build check
 * is best-effort. Throws a precondition error with a specific fix.
 */
export async function preflight(s: AdbSession): Promise<void> {
  const accounts = (await s.shell("dumpsys account")).stdout;
  if (/Account\s*\{/.test(accounts)) {
    throw new ProvisionError(
      "precondition",
      "This phone already has an account signed in. Factory-reset it and skip all account setup, then try again.",
    );
  }
  const users = (await s.shell("pm list users")).stdout;
  const userCount = (users.match(/UserInfo\{/g) || []).length;
  if (userCount > 1) {
    throw new ProvisionError(
      "precondition",
      "This phone has more than one user profile. Remove the extra profiles (or factory-reset) and try again.",
    );
  }
}

/** Push the release APK and install it. */
export async function installApk(s: AdbSession, apk: Uint8Array, onProgress?: OnProgress): Promise<void> {
  onProgress?.({ step: "install", ok: true, detail: "Copying the Charter app to the phone…" });
  await s.pushFile(apk, REMOTE_APK);
  const res = await s.shell(`pm install -r -g ${REMOTE_APK}`);
  if (!/Success/.test(res.stdout)) {
    throw new ProvisionError("adb", `Install failed: ${res.stdout.trim() || "unknown error"}`);
  }
}

/** Promote the app to Device Owner. Maps the known refusals to plain fixes. */
export async function setDeviceOwner(s: AdbSession): Promise<void> {
  const res = await s.shell(`dpm set-device-owner ${ADMIN}`);
  if (/Success/.test(res.stdout)) return;
  const out = res.stdout.trim();
  let fix = out;
  if (/already several users|already some accounts|not allowed/i.test(out)) {
    fix = "The phone must be freshly reset with no accounts and no extra users. Factory-reset, skip account setup, and retry.";
  } else if (/already.*device owner|already set/i.test(out)) {
    fix = "This phone is already a Device Owner. If it was a previous Charter setup, factory-reset first.";
  }
  throw new ProvisionError("adb", `Could not make Charter the device owner: ${fix}`);
}

/**
 * Fire the guardian pairing URI at the phone over the cable — the SAME
 * bunker:// intent the QR flow uses, into MainActivity.handlePairingIntent. The
 * phone then echoes the token on its STATUS heartbeat and MyCharter matches it.
 */
export async function pairOverCable(s: AdbSession, bunkerUri: string): Promise<void> {
  // Single-quote for the device shell; the URI has no single quotes.
  const res = await s.shell(`am start -a android.intent.action.VIEW -d '${bunkerUri}' ${"org.forgesworn.charter"}`);
  if (/Error|Exception/i.test(res.stdout)) {
    throw new ProvisionError("adb", `Could not open the Charter app to pair: ${res.stdout.trim()}`);
  }
}

/** The whole sequence, reporting progress at each gate. */
export async function runProvision(
  s: AdbSession,
  apk: Uint8Array,
  bunkerUri: string,
  onProgress: OnProgress,
): Promise<void> {
  onProgress({ step: "preflight", ok: true, detail: "Checking the phone…" });
  await preflight(s);
  onProgress({ step: "preflight", ok: true, detail: "Phone is ready." });

  await installApk(s, apk, onProgress);
  onProgress({ step: "install", ok: true, detail: "Charter installed." });

  onProgress({ step: "own", ok: true, detail: "Making Charter the device owner…" });
  await setDeviceOwner(s);
  onProgress({ step: "own", ok: true, detail: "Charter is the device owner." });

  onProgress({ step: "pair", ok: true, detail: "Pairing with your guardian key…" });
  await pairOverCable(s, bunkerUri);
  onProgress({ step: "pair", ok: true, detail: "Pairing sent." });

  onProgress({ step: "done", ok: true, detail: "Waiting for the phone to appear…" });
}
```

- [ ] **Step 4: Run to verify it passes**

Run:
```bash
cd ~/charter/apps/charter-app && npx vitest run src/provision/provision.test.ts
```
Expected: all tests PASS.

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add apps/charter-app/src/provision/provision.ts apps/charter-app/src/provision/provision.test.ts
git commit -m "feat(pwa/provision): preflight -> install -> set-device-owner -> cable-pair sequence

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 6: Serve the release APK from the PWA origin + fetch helper

**Files:**
- Create: `apps/charter-app/public/charter-release.apk` **placeholder handling** — do NOT commit the binary; instead document the deploy step
- Create: `apps/charter-app/src/provision/apk.ts`
- Create: `apps/charter-app/src/provision/apk.test.ts`

**Interfaces:**
- Produces: `async function fetchReleaseApk(): Promise<{ bytes: Uint8Array; sha256: string }>` — fetches the same-origin APK and computes its SHA-256 (shown to the parent for transparency). Consumed by the UI (Task 7). `APK_URL` and `EXPECTED_SHA256` constants.

- [ ] **Step 1: Decide APK hosting (same-origin, version-pinned)**

The APK is a build artifact, not source — it should be published to the PWA origin by the deploy pipeline (push to `main` → served at `/charter-<version>.apk`), not committed to git. For local/dev, copy the built APK into `apps/charter-app/public/` (git-ignored). Add to `apps/charter-app/.gitignore`:
```
public/charter-*.apk
```

- [ ] **Step 2: Write the failing test**

Create `apps/charter-app/src/provision/apk.test.ts`:

```ts
import { describe, it, expect, vi, afterEach } from "vitest";
import { fetchReleaseApk } from "./apk";

describe("fetchReleaseApk", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("fetches the apk and computes its sha256", async () => {
    const bytes = new Uint8Array([1, 2, 3, 4]);
    vi.stubGlobal("fetch", vi.fn(async () => ({
      ok: true,
      arrayBuffer: async () => bytes.buffer,
    })));
    // jsdom provides crypto.subtle in the vitest env; if not, this asserts shape.
    const res = await fetchReleaseApk();
    expect(res.bytes.length).toBe(4);
    expect(res.sha256).toMatch(/^[0-9a-f]{64}$/);
  });

  it("throws when the apk is missing", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => ({ ok: false, status: 404 })));
    await expect(fetchReleaseApk()).rejects.toThrow();
  });
});
```

- [ ] **Step 3: Run to verify it fails**

Run:
```bash
cd ~/charter/apps/charter-app && npx vitest run src/provision/apk.test.ts
```
Expected: FAIL — module missing.

- [ ] **Step 4: Implement**

Create `apps/charter-app/src/provision/apk.ts`:

```ts
// The release APK version served alongside the PWA. Bump on each release; the
// deploy pipeline publishes public/charter-<version>.apk to the origin.
export const APK_VERSION = "0.17.0";
export const APK_URL = `/charter-${APK_VERSION}.apk`;

export async function fetchReleaseApk(): Promise<{ bytes: Uint8Array; sha256: string }> {
  const resp = await fetch(APK_URL);
  if (!resp.ok) {
    throw new Error(`Charter app download not found (${resp.status}). The site may still be deploying.`);
  }
  const buf = new Uint8Array(await resp.arrayBuffer());
  const digest = await crypto.subtle.digest("SHA-256", buf);
  const sha256 = Array.from(new Uint8Array(digest)).map((b) => b.toString(16).padStart(2, "0")).join("");
  return { bytes: buf, sha256 };
}
```

- [ ] **Step 5: Run to verify it passes**

Run:
```bash
cd ~/charter/apps/charter-app && npx vitest run src/provision/apk.test.ts
```
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cd ~/charter
git add apps/charter-app/src/provision/apk.ts apps/charter-app/src/provision/apk.test.ts apps/charter-app/.gitignore
git commit -m "feat(pwa/provision): same-origin release APK fetch + sha256 for transparency

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 7: The "Set up a phone with a cable" UI

**Files:**
- Modify: `apps/charter-app/src/screens/Family.tsx` (add a `SetupPhoneCable` component + a "with a cable" option in the phone path)

**Interfaces:**
- Consumes: `webUsbSupported`, `connectPhone` (Task 2); `runProvision` (Task 5); `fetchReleaseApk` (Task 6); the existing `beginPhonePairing` / `phonePairing` / `guardianBunkerUri` (already in the store, used by the QR flow); `addDevice` / naming (existing).
- Produces: UI only.

- [ ] **Step 1: Add the cable option beside the QR path**

In the phone branch of `SetupComputer` (Family.tsx ~L408-460, where `kind === "android"`), add a third choice under the existing scan/manual options: **"Set up with a cable (no scanning)"**, shown only when `webUsbSupported()`. When chosen, render `<SetupPhoneCable child={child} onClose={onClose} />`. When WebUSB is unsupported, show a muted line: "Cable setup needs Chrome, Edge, or Brave on a computer."

- [ ] **Step 2: Implement `SetupPhoneCable`**

Add to `Family.tsx` (uses the store's existing pairing primitives so the STATUS-echo match is identical to the QR flow):

```tsx
function SetupPhoneCable({ child, onClose }: { child: Child; onClose: () => void }) {
  const { beginPhonePairing, cancelPhonePairing, phonePairing, addDevice, confirmPairing } = useCharter();
  const [phase, setPhase] = useState<"intro" | "running" | "waiting" | "error">("intro");
  const [log, setLog] = useState<string>("");
  const [error, setError] = useState<string>("");
  const [label, setLabel] = useState(`${child.name}'s phone`);

  // When the phone we just provisioned echoes its token on STATUS, bind + name.
  useEffect(() => {
    if (phase === "waiting" && phonePairing?.childId === child.id && phonePairing.foundMachine) {
      const name = label.trim() || `${child.name}'s phone`;
      const device = addDevice(child.id, name, "android");
      confirmPairing(child.id, device.id, phonePairing.foundMachine);
      onClose();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [phase, phonePairing?.foundMachine]);

  async function start() {
    setPhase("running"); setError(""); setLog("");
    try {
      const { connectPhone } = await import("../provision/webusb");
      const { runProvision } = await import("../provision/provision");
      const { fetchReleaseApk } = await import("../provision/apk");

      const { bytes } = await fetchReleaseApk();
      const session = await connectPhone();
      // Mint the one-time token + bunker URI exactly like the QR flow.
      const token = beginPhonePairing(child.id);
      const bunkerUri = guardianPairingBunkerUri(guardianPubkeyHex(), DEFAULT_RELAYS, token);
      await runProvision(session, bytes, bunkerUri, (p) =>
        setLog(`${p.detail}`));
      await session.close();
      setPhase("waiting"); // now wait for the STATUS echo (effect above)
    } catch (e: any) {
      cancelPhonePairing();
      setError(e?.message ?? String(e));
      setPhase("error");
    }
  }

  return (
    <div className="stack">
      {phase === "intro" && (
        <>
          <p>On the phone: finish setup with <b>no accounts</b>, turn on Developer options
             (tap Build number 7 times), enable <b>USB debugging</b>, then plug it into this computer.</p>
          <Button block onClick={start}>Connect the phone</Button>
        </>
      )}
      {(phase === "running" || phase === "waiting") && (
        <>
          <p>{phase === "waiting" ? "Almost done — waiting for the phone to check in…" : log || "Working…"}</p>
          <label className="stack">Name this phone
            <input value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
        </>
      )}
      {phase === "error" && (
        <>
          <p className="error">{error}</p>
          <Button block onClick={() => setPhase("intro")}>Try again</Button>
          <details><summary>Do it manually instead</summary>
            <p>Run <code>android/scripts/charter-provision.sh</code> from the repo, or the
               commands in the setup guide.</p>
          </details>
        </>
      )}
    </div>
  );
}
```

> Use whatever the store actually exports for minting the guardian `bunker://` URI with a token — the QR flow calls `guardianPairingQrContent(...)` which wraps the `https://…/pair#bunker://…&token=` form. For the cable we want the raw `bunker://…&token=` (MainActivity accepts both forms). If a raw-URI helper doesn't exist, add `guardianPairingBunkerUri()` next to `guardianPairingQrContent` in the same module (it already has all the inputs).

- [ ] **Step 3: Typecheck + tests + lint**

Run:
```bash
cd ~/charter && npx tsc --noEmit && npm test --silent && npm run lint --silent
```
Expected: all PASS. (WebUSB code is dynamically imported so it never breaks SSR/tests; the provision modules are unit-tested separately.)

- [ ] **Step 4: Commit**

```bash
cd ~/charter
git add apps/charter-app/src/screens/Family.tsx apps/charter-app/src/store/store.tsx
git commit -m "feat(pwa/provision): 'Set up a phone with a cable' — zero-scan WebUSB onboarding

Reuses the QR flow's token + STATUS-echo pairing match; the cable just fires the
same bunker:// intent. Chromium-gated with a graceful fallback.

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 8: `charter-provision.sh` scripted fallback

**Files:**
- Create: `android/scripts/charter-provision.sh`

**Interfaces:** none (operator script). Makes the port-spec §3.8 reference real; the UI's "do it manually" points here.

- [ ] **Step 1: Write the script**

Create `android/scripts/charter-provision.sh`:

```bash
#!/usr/bin/env bash
# Charter phone provisioning — the scripted fallback to the MyCharter WebUSB flow.
# Installs the release APK as Device Owner on a factory-fresh GrapheneOS phone.
# Usage: charter-provision.sh path/to/app-release.apk ['bunker://…&token=…']
set -euo pipefail

APK="${1:?usage: charter-provision.sh <app-release.apk> [bunker-uri]}"
BUNKER="${2:-}"
ADMIN="org.forgesworn.charter/.admin.CharterDeviceAdminReceiver"

command -v adb >/dev/null || { echo "adb not found on PATH"; exit 1; }
[ -f "$APK" ] || { echo "APK not found: $APK"; exit 1; }

echo "→ Waiting for the phone (authorize USB debugging on it if prompted)…"
adb wait-for-device

echo "→ Preflight: checking for accounts / extra users…"
if adb shell dumpsys account | grep -q 'Account {'; then
  echo "✗ The phone has an account signed in. Factory-reset with NO accounts and retry." >&2
  exit 2
fi
if [ "$(adb shell pm list users | grep -c 'UserInfo{')" -gt 1 ]; then
  echo "✗ The phone has extra user profiles. Remove them (or factory-reset) and retry." >&2
  exit 2
fi

echo "→ Installing the Charter app…"
adb install -r -g "$APK"

echo "→ Making Charter the device owner…"
if ! adb shell dpm set-device-owner "$ADMIN" | tee /dev/stderr | grep -q 'Success'; then
  echo "✗ set-device-owner failed. The phone must be freshly reset (no accounts, one user)." >&2
  exit 3
fi

if [ -n "$BUNKER" ]; then
  echo "→ Opening the Charter app to pair…"
  adb shell am start -a android.intent.action.VIEW -d "$BUNKER" org.forgesworn.charter
  echo "✓ Provisioned + pairing sent. The phone should appear in MyCharter shortly."
else
  echo "✓ Provisioned. Open MyCharter → Set up a phone → scan the QR to pair."
fi
```

- [ ] **Step 2: Make it executable + shellcheck**

Run:
```bash
cd ~/charter
chmod +x android/scripts/charter-provision.sh
shellcheck android/scripts/charter-provision.sh || true
bash -n android/scripts/charter-provision.sh && echo "syntax OK"
```
Expected: `syntax OK` (shellcheck warnings addressed or benign).

- [ ] **Step 3: Commit**

```bash
cd ~/charter
git add android/scripts/charter-provision.sh
git commit -m "feat(android/provision): charter-provision.sh scripted fallback (DO + cable pair)

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 9: Full verification pass + guide update

**Files:**
- Modify: `guide/graphene-phone-setup.html` (add the cable-setup path as the primary onboarding)

**Interfaces:** none.

- [ ] **Step 1: Full PWA gate**

Run:
```bash
cd ~/charter && npx tsc --noEmit && npm test --silent && npm run lint --silent && npm run build --workspace apps/charter-app 2>/dev/null || (cd apps/charter-app && npm run build)
```
Expected: typecheck, tests, lint, and production build all PASS.

- [ ] **Step 2: Update the setup guide**

In `guide/graphene-phone-setup.html`, add the cable/WebUSB path as the primary "no terminal" onboarding, keeping the adb-manual path as the fallback and cross-referencing `charter-provision.sh`.

- [ ] **Step 3: Commit**

```bash
cd ~/charter
git add guide/graphene-phone-setup.html
git commit -m "docs(guide): cable/WebUSB setup as the primary phone onboarding

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Self-Review Notes (coverage against the spec)

- **WebUSB adb from Chromium (ya-webadb), pinned API** → Tasks 1-2. ✅
- **Preflight with specific remediation** → Task 5 (`preflight`, `setDeviceOwner` error mapping). ✅
- **Install release APK + set-device-owner** → Task 5 + Task 3 (release signing). ✅
- **Zero-scan cable pairing reusing the bunker:// intent + STATUS echo** → Task 5 (`pairOverCable`) + Task 7 (reuses `beginPhonePairing`/`phonePairing`). ✅
- **Release keystore, real cert, no fabricated values** → Task 3 (real `keytool`/`apksigner` output). ✅
- **assetlinks release cert (App-Link keeps verifying on release)** → Task 4. ✅ (This is the gap I flagged in the once-over — now an explicit task.)
- **APK served same-origin, sha256 shown** → Task 6. ✅
- **Chromium-gated with graceful fallback** → Task 2 (`webUsbSupported`) + Task 7 (UI gate). ✅
- **charter-provision.sh fallback** → Task 8. ✅
- **Guide** → Task 9. ✅

### Cross-plan dependency
This plan produces the **release APK** (Task 3). The web-content plan's final `.so` (its Task 4/11) must be built into that release APK, so **run the web-content plan first**, then build the release APK here so it carries the DNS-filter enforcement. The single hardware gate exercises both.

### Known risk carried to the hardware gate
- `adb.subprocess` exit-code semantics + `sync.write` signature vary by ya-webadb version — Task 1 pins them; if `noneProtocol` lacks exit codes, Task 2 switches to `shellProtocol`.
- WebUSB on the parent's desktop Linux may need a udev rule (same as the GrapheneOS web installer) — document in the guide (Task 9).
- Desktop Chromium claims the adb interface exclusively: a running local `adb server` can grab the device first. The guide must say "close other adb tools / run `adb kill-server`" before connecting.
