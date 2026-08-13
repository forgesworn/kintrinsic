import { describe, expect, it } from "vitest";
import { myCharterVersionState, parseCarrierVersion, readCarrierVersion } from "./version";
import type { CarrierVersionReading } from "./version";
import type { UpdateManifest } from "../wire/types";

// The shell and the page update independently: deploying the PWA changes what
// is INSIDE the app, while the app's own Kotlin changes only on a new APK. On
// 2026-08-06 that cost a real test round — "I updated Kintrinsic" was true and
// the thing being waited for was still in an APK that had never been built.
// These pin the answers that make that legible.

const latest = (versionCode: number, versionName: string): UpdateManifest => ({
  versionName,
  versionCode,
  apkSha256: "a".repeat(64),
  certSha256: "b".repeat(64),
  sizeBytes: 1,
  builtAt: "2026-08-06T00:00:00Z",
});

const installed = { versionName: "0.1.5", versionCode: 6 };
const ok: CarrierVersionReading = { kind: "ok", version: installed };
const absent: CarrierVersionReading = { kind: "absent" };

describe("parseCarrierVersion", () => {
  it("reads a well-formed report", () => {
    expect(parseCarrierVersion('{"versionName":"0.1.5","versionCode":6}')).toEqual(installed);
  });

  /** A shell that cannot say what it is must never be reported as current. */
  it("treats a partial or malformed report as no answer at all", () => {
    expect(parseCarrierVersion('{"versionName":"0.1.5"}')).toBeNull();
    expect(parseCarrierVersion('{"versionCode":6}')).toBeNull();
    expect(parseCarrierVersion('{"versionName":"","versionCode":6}')).toBeNull();
    expect(parseCarrierVersion('{"versionName":"x","versionCode":0}')).toBeNull();
    expect(parseCarrierVersion('{"versionName":"x","versionCode":"6"}')).toBeNull();
    expect(parseCarrierVersion("not json")).toBeNull();
    expect(parseCarrierVersion(undefined)).toBeNull();
  });
});

describe("myCharterVersionState", () => {
  it("says nothing in an ordinary browser tab — there is no shell to check", () => {
    expect(myCharterVersionState(false, absent, latest(6, "0.1.5"))).toEqual({ kind: "not-carrier" });
  });

  /**
   * The load-bearing case. A shell with no `version()` necessarily predates
   * the release that added it, so it IS behind — reporting "unknown" here
   * would recreate the exact silence that cost the test round.
   */
  it("calls a shell that cannot report its version out of date", () => {
    expect(myCharterVersionState(true, absent, latest(6, "0.1.5"))).toEqual({
      kind: "too-old-to-say",
    });
  });

  it("still calls it out of date when the manifest is unreachable too", () => {
    expect(myCharterVersionState(true, absent, null)).toEqual({ kind: "too-old-to-say" });
  });

  it("offers the update when the shell is behind", () => {
    const m = latest(7, "0.1.6");
    expect(myCharterVersionState(true, ok, m)).toEqual({ kind: "behind", installed, latest: m });
  });

  it("is content when the shell matches the published build", () => {
    expect(myCharterVersionState(true, ok, latest(6, "0.1.5"))).toEqual({
      kind: "current",
      installed,
    });
  });

  /** A dev shell ahead of what is published is not "behind". */
  it("does not nag a shell newer than the manifest", () => {
    expect(myCharterVersionState(true, ok, latest(5, "0.1.4"))).toEqual({
      kind: "current",
      installed,
    });
  });

  /** Offline must not be dressed up as either reassurance or an alarm. */
  it("admits it could not check when the manifest is unreachable", () => {
    expect(myCharterVersionState(true, ok, null)).toEqual({ kind: "unknown", installed });
  });

  /**
   * REGRESSION (2026-08-06). decented installed a freshly-published 0.1.5 and the
   * footer told him it was out of date.
   *
   * Root cause was two bugs stacked. `readCarrierVersion` detached the bridge
   * method (`const v = carrier.version; v()`) — Android binds an injected
   * method to its object, so the call threw. That alone was survivable; what
   * made it LIE was collapsing "no method" and "call failed" into one `null`
   * and reading it as "old".
   *
   * A shell that HAS the method is necessarily at least the release that added
   * it, so "out of date" is the single answer we know to be wrong. Whatever
   * else a failed read is, it is never that.
   */
  it("never calls a shell out of date just because reading it failed", () => {
    const unreadable: CarrierVersionReading = { kind: "unreadable" };
    expect(myCharterVersionState(true, unreadable, latest(6, "0.1.5"))).toEqual({
      kind: "installed-unreadable",
    });
    // …and not even when the manifest is unreachable too.
    expect(myCharterVersionState(true, unreadable, null)).toEqual({
      kind: "installed-unreadable",
    });
  });
});

describe("readCarrierVersion", () => {
  const withCarrier = (carrier: unknown, run: () => void) => {
    const prev = window.CharterCarrier;
    (window as unknown as { CharterCarrier: unknown }).CharterCarrier = carrier;
    try {
      run();
    } finally {
      (window as unknown as { CharterCarrier: unknown }).CharterCarrier = prev;
    }
  };

  /**
   * The bug itself: the method must be invoked ON the bridge object. Android
   * rejects a detached call, so this asserts the receiver is preserved — a
   * `version()` that only works when bound would have failed the old code.
   */
  it("calls version() on the bridge object, not detached", () => {
    let boundToBridge = false;
    const carrier = {
      isCarrier: () => true,
      provision: () => {},
      version(this: unknown) {
        // Compared in place rather than captured: the old detached call would
        // arrive with `this` undefined, so this stays false and the assertion
        // below fails — which is precisely the regression being pinned.
        boundToBridge = this === carrier;
        return '{"versionName":"0.1.5","versionCode":6}';
      },
    };
    withCarrier(carrier, () => {
      expect(readCarrierVersion()).toEqual({ kind: "ok", version: installed });
      expect(boundToBridge).toBe(true);
    });
  });

  it("reports absent when the shell has no version method", () => {
    withCarrier({ isCarrier: () => true, provision: () => {} }, () => {
      expect(readCarrierVersion()).toEqual({ kind: "absent" });
    });
  });

  it("reports unreadable — not absent — when the call throws", () => {
    withCarrier(
      {
        isCarrier: () => true,
        provision: () => {},
        version: () => {
          throw new Error("Java bridge method invoked on non-injected object");
        },
      },
      () => expect(readCarrierVersion()).toEqual({ kind: "unreadable" }),
    );
  });

  it("reports unreadable when the payload is malformed", () => {
    withCarrier(
      { isCarrier: () => true, provision: () => {}, version: () => "not json" },
      () => expect(readCarrierVersion()).toEqual({ kind: "unreadable" }),
    );
  });
});
