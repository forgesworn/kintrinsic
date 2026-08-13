# ya-webadb (Tango) API — pinned against installed versions

This documents the **concrete, installed** API surface for WebUSB-based adb
provisioning. It was read directly from the packages' `.d.ts` and `.js` in
`node_modules/` (not from memory, not from stale blog posts — see "Traps"
below). Cross-checked against the versioned docs at
`https://tangoadb.dev/1.0.0/...` where fetchable.

## Installed versions (pinned in `package.json`)

| Package | Version | Role |
|---|---|---|
| `@yume-chan/adb` | `2.6.0` | Core `Adb` client, transport interface, subprocess/sync services |
| `@yume-chan/adb-daemon-webusb` | `2.3.2` | WebUSB `AdbDaemonConnection` (device discovery + connect) |
| `@yume-chan/stream-extra` | `2.6.1` | Stream types (`ReadableStream`/`WritableStream`/`Consumable`/`MaybeConsumable`) shared across the above |
| `@yume-chan/adb-credential-web` | `2.1.0` | **Not in the original plan list — required.** Browser `AdbCredentialStore` implementation (RSA keypair persisted in IndexedDB) |

All four are plain ESM packages (`"type": "module"`, `main`/`types` both point
at `esm/index.js` / `esm/index.d.ts`, **no `exports` map**), so ordinary bare
imports work: `import { X } from "@yume-chan/adb"` pulls from the single
barrel `esm/index.js`, which re-exports everything below.

`@yume-chan/android-bin` (`2.1.0` on npm, peer-compatible with `@yume-chan/adb ^2.1.0`)
was inspected but **not installed** — it's an optional, cleaner alternative
for (f) below. Install it in Task 2 if the `PackageManager.pushAndInstallStream`
helper is used instead of raw `sync().write()`.

## (a) Requesting a WebUSB device

```ts
import { AdbDaemonWebUsbDeviceManager } from "@yume-chan/adb-daemon-webusb";

const manager = AdbDaemonWebUsbDeviceManager.BROWSER; // undefined if WebUSB unsupported
const device = await manager?.requestDevice(); // AdbDaemonWebUsbDevice | undefined
```

- `AdbDaemonWebUsbDeviceManager.BROWSER` is a **static readonly property**
  (not a getter/method), evaluated once at module load as
  `navigator.usb ? new AdbDaemonWebUsbDeviceManager(navigator.usb) : undefined`.
  Use `AdbDaemonWebUsbDeviceManager.BROWSER?.requestDevice()` (plan's `!` also
  works but `?.` is safer for the "WebUSB unsupported" case).
- `requestDevice(options?: { filters?, exclusionFilters?: readonly USBDeviceFilter[] })`
  returns `Promise<AdbDaemonWebUsbDevice | undefined>`.
  **Important:** if the user cancels the browser's device picker, this
  resolves to `undefined` — it does **not** throw. (Confirmed in
  `manager.js`: a `NotFoundError` from the underlying `USB.requestDevice()`
  is caught and turned into `undefined`; other errors are rethrown.) Task 2's
  UX should treat `undefined` as "user cancelled," distinct from a thrown
  error.
- `device.serial: string` and `device.name: string` getters are available
  after a device is returned, for display before connecting.

## (b) Connecting

```ts
const connection = await device.connect(); // AdbDaemonWebUsbConnection
```

- `AdbDaemonWebUsbDevice.connect(): Promise<AdbDaemonWebUsbConnection>`.
- `AdbDaemonWebUsbConnection` implements
  `ReadableWritablePair<AdbPacketData, Consumable<AdbPacketInit>>`, which is
  exactly the `AdbDaemonConnection` type expected by
  `AdbDaemonTransport.authenticate()` below — pass it straight through, no
  adapting needed.

## (c) Credential store

**Deviation from plan:** `@yume-chan/adb-credential-web` is a real, separate
npm package (not bundled into `@yume-chan/adb`) and was **not** in the
original three-package install list. It had to be installed explicitly. Its
default export is the concrete class:

```ts
import AdbWebCredentialStore from "@yume-chan/adb-credential-web";

const credentialStore = new AdbWebCredentialStore(); // or new AdbWebCredentialStore("MyCharter")
```

- Default export (not named) — `import AdbWebCredentialStore from "..."`.
- Constructor takes an optional `appName` (default `"Tango"`), used **only**
  as a display label baked into the generated key's `name` field
  (`"${appName}@${location.hostname}"`) — it does **not** change where the
  key is stored.
- Storage: an IndexedDB database literally named `"Tango"` (hardcoded, one
  object store `"Authentication"`, `autoIncrement`). This is a fixed,
  library-internal detail — not configurable, not namespaced per-app. If
  MyCharter ever needs to isolate this from other Tango-based tools running
  on the same origin, a custom `AdbCredentialStore` implementation (just
  `generateKey()` + `iterateKeys()`, see `daemon/auth.d.ts` in `@yume-chan/adb`)
  would be needed — not required for v1.
- `generateKey(): Promise<AdbPrivateKey>` generates a new RSA-2048 key via
  WebCrypto and appends it to IndexedDB (does **not** overwrite prior keys —
  each call adds a new key).
- `iterateKeys(): AsyncGenerator<AdbPrivateKey>` yields all stored keys.
- Task 2 should construct **one** `AdbWebCredentialStore` and reuse it across
  connect attempts (so the same RSA key — and thus the same "already
  authorized" state on the phone — persists across sessions).

## (d) Authenticating the transport

```ts
import { Adb } from "@yume-chan/adb";
import { AdbDaemonTransport } from "@yume-chan/adb";

const transport = await AdbDaemonTransport.authenticate({
  serial: device.serial,
  connection,
  credentialStore,
  // optional: authenticators, features, initialDelayedAckBytes,
  // preserveConnection, readTimeLimit
});
const adb = new Adb(transport);
```

- `AdbDaemonTransport.authenticate(options: AdbDaemonAuthenticationOptions): Promise<AdbDaemonTransport>`
  is a **static** method — matches the plan's assumed shape exactly.
- `new Adb(transport)` — plain constructor, matches plan exactly.
- On the phone, this is the step that triggers the "Allow USB debugging?"
  RSA-fingerprint prompt (first connection only, until the user accepts and
  optionally checks "always allow from this computer").
- `adb.close()` closes the transport (and the underlying WebUSB connection
  unless `preserveConnection: true` was passed).

## (e) One-shot shell command + exit code — **key deviation**

`adb.subprocess` exposes **two** separate protocols with **different
capabilities**. This is the plan's most important open question, now
resolved:

```ts
adb.subprocess.noneProtocol   // AdbNoneProtocolSubprocessService — ALWAYS present
adb.subprocess.shellProtocol  // AdbShellProtocolSubprocessService | undefined
```

- `shellProtocol` is only constructed when
  `adb.canUseFeature(AdbFeature.ShellV2)` is true (checked once, in
  `AdbSubprocessService`'s constructor — see `commands/subprocess/service.js`).
  `ShellV2` (`"shell_v2"`) has been present on essentially all Android 5.0+
  devices for years, so on a target GrapheneOS phone this will be defined,
  but **must still be null-checked** — it's typed `| undefined`.

**`noneProtocol` — NO exit code:**

```ts
export interface AdbNoneProtocolProcess {
  stdin: WritableStream<MaybeConsumable<Uint8Array>>;
  output: ReadableStream<Uint8Array>; // mixed stdout+stderr, no separation
  exited: Promise<void>;              // resolves when process exits — NO CODE
  kill(): void | Promise<void>;
}
adb.subprocess.noneProtocol.spawn(cmd): Promise<AdbNoneProtocolProcess>
adb.subprocess.noneProtocol.spawnWait(cmd): Promise<Uint8Array>       // combined output only
adb.subprocess.noneProtocol.spawnWaitText(cmd): Promise<string>       // combined output only
```

**`shellProtocol` — HAS exit code + separated stdout/stderr:**

```ts
export interface AdbShellProtocolProcess {
  stdin: WritableStream<MaybeConsumable<Uint8Array>>;
  stdout: ReadableStream<Uint8Array>;
  stderr: ReadableStream<Uint8Array>;
  exited: Promise<number>;   // <-- exit code
  kill(): void | Promise<void>;
}
adb.subprocess.shellProtocol.spawn(cmd): Promise<AdbShellProtocolProcess>
adb.subprocess.shellProtocol.spawnWait(cmd): Promise<{ stdout: Uint8Array; stderr: Uint8Array; exitCode: number }>
adb.subprocess.shellProtocol.spawnWaitText(cmd): Promise<{ stdout: string; stderr: string; exitCode: number }>
```

**Consequence for Task 2's wrapper:** to run `pm ...` / `dpm ...` and get a
reliable exit code (needed to know if provisioning actually succeeded, not
just that the command "ran"), the wrapper **must** use
`adb.subprocess.shellProtocol` (feature-detected, with a clear error if
`undefined`), calling `spawnWaitText(cmd)` for the common case
(`{ stdout, stderr, exitCode }`, all as `string`/`number`). Falling back to
`noneProtocol` should only happen if `shellProtocol` is genuinely
unavailable, and in that case the wrapper **cannot** report a real exit
code — only combined text output — and callers must not treat "no error
thrown" as "command succeeded."

`cmd` for all of the above is `string | readonly string[]` (an argv array is
safer than a hand-joined shell string — avoids quoting bugs for package
names / paths with special characters).

## (f) Pushing a file (APK)

Two options, both confirmed against installed/inspectable sources:

**Option 1 — raw sync push (`@yume-chan/adb`, already installed):**

```ts
const sync = await adb.sync();               // Promise<AdbSync>
await sync.write({
  filename: "/data/local/tmp/app.apk",
  file: readableStreamOfBytes,                // ReadableStream<MaybeConsumable<Uint8Array>>
  permission: 0o644,                          // optional
  mtime: Date.now() / 1000 | 0,               // optional, seconds
});
await sync.dispose();                          // release the sync socket
```
Then run the install separately via (e), e.g.
`adb.subprocess.shellProtocol.spawnWaitText(["pm", "install", "/data/local/tmp/app.apk"])`.

- `sync.write(options: AdbSyncWriteOptions): Promise<void>` — no built-in
  progress callback; the `file` stream itself is what's consumed, so a
  progress UI would need to wrap it in a counting `TransformStream`.
- **Browser stream compatibility:** in a browser, `@yume-chan/stream-extra`'s
  `ReadableStream`/`WritableStream` types are just re-declarations of the
  native DOM globals (`esm/types.js` is an empty module — no runtime
  polyfill shipped for browser targets), so a plain
  `someFile.stream()` (`File`/`Blob` → native `ReadableStream<Uint8Array>`)
  is structurally assignable to `ReadableStream<MaybeConsumable<Uint8Array>>`
  directly (`MaybeConsumable<Uint8Array> = Uint8Array | Consumable<Uint8Array>`).
  No manual wrapping should be needed; if TS variance complains, use
  `new WrapReadableStream(file.stream())` from `@yume-chan/stream-extra`.
- Must call `sync().dispose()` (or reuse across multiple `write`/`stat`
  calls, then dispose once) — the sync connection holds a socket open.

**Option 2 — `PackageManager.pushAndInstallStream` (`@yume-chan/android-bin`, NOT yet installed):**

```ts
import { PackageManager } from "@yume-chan/android-bin";

const pm = new PackageManager(adb);
const output = await pm.pushAndInstallStream(readableStreamOfBytes, {
  grantRuntimePermissions: true,   // -g, etc. — see PackageManagerInstallOptions
});
```
- `pushAndInstallStream(stream: ReadableStream<MaybeConsumable<Uint8Array>>, options?: Partial<PackageManagerInstallOptions>): Promise<string>`
  pushes + installs in one call (no manual `/data/local/tmp` path, no
  separate `pm install` shell-out). `PackageManagerInstallOptions` maps
  1:1 to `pm install` flags (`skipExisting` → `-R`, `grantRuntimePermissions`
  → `-g`, `installReason` → `--install-reason`, etc. — full list in
  `pm.d.ts`).
- Also exposes `install(apks: string[], options?)` (installs from paths
  already on-device), `uninstall()`, `listPackages()`, and lower-level
  `sessionCreate`/`sessionAddSplitStream`/`sessionCommit` for multi-APK
  (split) installs — useful if provisioning ever needs to push a bundle
  instead of a single APK.
- **This is the cleaner option for Task 2** (fewer manual steps, no
  leftover file in `/data/local/tmp`, gets a result string directly). It
  needs `npm install @yume-chan/android-bin` (peer-compatible with the
  installed `@yume-chan/adb@2.6.0` — its `package.json` declares
  `"@yume-chan/adb": "^2.1.0"`) before Task 2 can use it. Verified its
  `.d.ts` shape by installing it into a throwaway scratch dir (not added to
  this project) — not committed here since Task 1 only pins what's actually
  wired into `charter-app`.

## Traps confirmed while researching (for anyone re-deriving this later)

- **Do not trust blog posts / old examples.** A 2025-dated blog post found
  during research showed `AdbWebUsbBackend.requestDevice()` +
  `Adb.authenticate(backend)` + `adb.install(...)` — none of which exist in
  the installed `2.6.0`/`2.3.2` API (`AdbWebUsbBackend` doesn't exist;
  `Adb.authenticate` doesn't exist; `Adb` has no `.install()` method). This
  is exactly the "API moved across major versions" trap the task brief
  warned about.
- The current versioned docs (`https://tangoadb.dev/1.0.0/...`) **do**
  cross-check cleanly with the installed `.d.ts`/`.js`: confirmed
  `AdbDaemonWebUsbDeviceManager.BROWSER` usage and the
  `AdbWebCredentialStore` import/constructor independently via
  `tangoadb.dev/1.0.0/tango/daemon/usb/device-manager/` and
  `tangoadb.dev/1.0.0/tango/daemon/credential-store/`. The root
  `github.com/yume-chan/ya-webadb` README and the unversioned
  `tangoadb.dev/` quick-start have no inline code (docs site renders code
  blocks client-side; WebFetch only sees the server HTML shell for those
  pages) — the installed `.d.ts` remains the primary source for everything
  in this document.

## Quick reference — full import list for Task 2

```ts
import { Adb, AdbDaemonTransport, AdbFeature } from "@yume-chan/adb";
import { AdbDaemonWebUsbDeviceManager } from "@yume-chan/adb-daemon-webusb";
import AdbWebCredentialStore from "@yume-chan/adb-credential-web";
// import { PackageManager } from "@yume-chan/android-bin"; // if Option 2 is chosen for (f); requires `npm install @yume-chan/android-bin`
```
