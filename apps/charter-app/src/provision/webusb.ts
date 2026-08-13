// Typed adb-over-WebUSB session wrapper.
//
// Wraps the ya-webadb ("Tango") packages behind a small, stable interface so
// later provisioning steps (Task 5) never touch the raw library. See
// API_NOTES.md in this directory for the pinned, installed API surface this
// file was written against (adb 2.6.0, adb-daemon-webusb 2.3.2,
// adb-credential-web 2.1.0) — two deviations from the original plan are
// honored here: the credential store is a default export from a separate
// package, and exit codes are only available via `shellProtocol`.
import { Adb, AdbDaemonTransport } from "@yume-chan/adb";
import { AdbDaemonWebUsbDeviceManager } from "@yume-chan/adb-daemon-webusb";
// Credential store persists the adb RSA key (in IndexedDB) so the phone only
// prompts "Allow USB debugging?" once. Default export — not a named export.
import AdbWebCredentialStore from "@yume-chan/adb-credential-web";
// `@yume-chan/stream-extra`'s `ReadableStream` is, at runtime, the same
// native global constructor (see API_NOTES.md (f)) — it's re-exported here
// only so the TS type includes the async-iterable members `AdbSync.write()`
// expects, which the plain lib.dom.d.ts `ReadableStream` type lacks.
import { ReadableStream } from "@yume-chan/stream-extra";

export type ProvisionErrorCode =
  | "unsupported"
  | "no-device"
  | "auth-declined"
  | "adb"
  | "precondition";

export class ProvisionError extends Error {
  constructor(
    readonly code: ProvisionErrorCode,
    message: string,
  ) {
    super(message);
    this.name = "ProvisionError";
  }
}

export interface AdbSession {
  readonly serial: string;
  /**
   * Runs a one-shot shell command and returns its COMBINED stdout+stderr
   * text as `stdout`.
   *
   * `exitCode` is only reliable when the device supports the shell_v2
   * protocol (`adb.subprocess.shellProtocol`, present on essentially all
   * Android 5.0+ devices, but typed `| undefined` and must be checked). When
   * unavailable, this falls back to `noneProtocol`, which has no exit code
   * at all — `exitCode` is defaulted to `0` in that case and MUST NOT be
   * trusted. Both paths return the SAME shape: `stdout` is the combined
   * text, because `dpm set-device-owner` refusals and `am start` errors
   * print to STDERR on real (shell_v2) devices, and a caller that matched on
   * stdout alone would see an empty string and lose the remediation
   * message. Callers (provisioning logic) should detect success/failure by
   * matching on this combined `stdout` text (e.g. "Success", "Device owner
   * set"), not by checking `exitCode`.
   */
  shell(cmd: string): Promise<{ stdout: string; exitCode: number }>;
  pushFile(bytes: Uint8Array, remotePath: string): Promise<void>;
  close(): Promise<void>;
}

export function webUsbSupported(): boolean {
  return typeof navigator !== "undefined" && !!navigator.usb;
}

// One credential store per module instance, reused across connect attempts
// so the same RSA key (and thus "already authorized" state on the phone)
// persists across sessions, per API_NOTES.md (c).
let sharedCredentialStore: AdbWebCredentialStore | undefined;
function credentialStore(): AdbWebCredentialStore {
  if (!sharedCredentialStore) {
    sharedCredentialStore = new AdbWebCredentialStore("Charter");
  }
  return sharedCredentialStore;
}

export async function connectPhone(): Promise<AdbSession> {
  if (!webUsbSupported()) {
    throw new ProvisionError(
      "unsupported",
      "WebUSB needs a Chromium browser (Chrome, Edge, Brave).",
    );
  }

  const manager = AdbDaemonWebUsbDeviceManager.BROWSER;
  if (!manager) {
    throw new ProvisionError("unsupported", "This browser has no WebUSB.");
  }

  let device;
  try {
    device = await manager.requestDevice();
  } catch {
    // Only thrown for non-cancel USB errors; a user-cancelled picker
    // resolves to `undefined` instead (see API_NOTES.md (a)).
    throw new ProvisionError("no-device", "No device was selected.");
  }
  if (!device) {
    throw new ProvisionError("no-device", "No device was selected.");
  }

  const connection = await device.connect();

  let transport: AdbDaemonTransport;
  try {
    transport = await AdbDaemonTransport.authenticate({
      serial: device.serial,
      connection,
      credentialStore: credentialStore(),
    });
  } catch {
    throw new ProvisionError(
      "auth-declined",
      "The phone did not allow USB debugging. Tap 'Allow' on the phone and retry.",
    );
  }

  const adb = new Adb(transport);
  return wrapSession(adb);
}

function wrapSession(adb: Adb): AdbSession {
  return {
    serial: adb.serial,

    async shell(cmd: string) {
      const shellProtocol = adb.subprocess.shellProtocol;
      if (shellProtocol) {
        // Real exit code, but stdout/stderr come back separated — combine
        // them into the single `stdout` field the doc comment on
        // AdbSession.shell promises, so text-matching callers see refusals
        // that a real device printed to stderr (e.g. dpm/am errors).
        const { stdout, stderr, exitCode } = await shellProtocol.spawnWaitText(cmd);
        return { stdout: [stdout, stderr].filter(Boolean).join("\n"), exitCode };
      }
      // No shell_v2 support: already combined stdout+stderr, no exit code at
      // all. See the doc comment on AdbSession.shell — callers must match on
      // stdout text, not exitCode, in this path.
      const stdout = await adb.subprocess.noneProtocol.spawnWaitText(cmd);
      return { stdout, exitCode: 0 };
    },

    async pushFile(bytes: Uint8Array, remotePath: string) {
      const sync = await adb.sync();
      try {
        await sync.write({
          filename: remotePath,
          file: singleChunkReadable(bytes),
        });
      } finally {
        await sync.dispose();
      }
    },

    async close() {
      await adb.close();
    },
  };
}

function singleChunkReadable(bytes: Uint8Array): ReadableStream<Uint8Array> {
  return new ReadableStream<Uint8Array>({
    start(controller) {
      controller.enqueue(bytes);
      controller.close();
    },
  });
}
