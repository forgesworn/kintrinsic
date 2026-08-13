import { describe, it, expect } from "vitest";
import {
  APP_OPEN_GRANT_TTL_SECS,
  appOpenWindowUnix,
  buildAppOpenGrant,
  buildInstallApkGrant,
  buildTimeExtendGrant,
  clampGrantMinutes,
  DENY_CERT_SENTINEL,
  deviceExtendCapEod,
  endOfDayUnix,
  INSTALL_GRANT_TTL_SECS,
  pickExtendTz,
  startOfDayUnix,
  type TimeExtendDecision,
} from "./grant";

// 2023-11-14T22:13:20Z — a plain day (no DST transition anywhere relevant).
const TS = 1_700_000_000;

function decision(over: Partial<TimeExtendDecision> = {}): TimeExtendDecision {
  return {
    reqId: "ab".repeat(32),
    nonce: "cd".repeat(32),
    decision: "allow",
    minutesGranted: 30,
    limitHit: "budget",
    ts: TS,
    tz: "UTC",
    ...over,
  };
}

describe("pickExtendTz", () => {
  it("prefers the locked dimension's tz", () => {
    // A budget lock uses the budget tz even when a schedule tz is also present.
    expect(pickExtendTz("budget", "Asia/Tokyo", "Europe/London")).toBe("Europe/London");
    // A schedule (bedtime) lock uses the schedule tz.
    expect(pickExtendTz("schedule", "Asia/Tokyo", "Europe/London")).toBe("Asia/Tokyo");
  });
  it("falls back to the other dimension's tz when the locked one is absent", () => {
    expect(pickExtendTz("budget", "Asia/Tokyo", undefined)).toBe("Asia/Tokyo");
    expect(pickExtendTz("schedule", undefined, "Europe/London")).toBe("Europe/London");
  });
  it("is undefined only when neither tz is known (never the phone tz)", () => {
    expect(pickExtendTz("budget", undefined, undefined)).toBeUndefined();
    expect(pickExtendTz("schedule", undefined, undefined)).toBeUndefined();
  });

  // C-1 (hardware round, 2026-08-03): a `bucket` hit must fall all the way
  // through to the buckets clause's own tz when NEITHER schedule nor budget
  // exists — a named-times-only ward carries neither, so stopping at those
  // two left every bucket ask unsignable.
  //
  // Round 2 (review, 2026-08-03): bucketsTz is a FALLBACK, never an
  // override. A first pass preferred it OUTRIGHT on a bucket hit — but
  // `buckets.tz` is seeded from the guardian's own local tz at authoring
  // time, unrelated to schedule/budget, so a schedule+buckets ward with
  // divergent tz's could sign an `exp` the device's real cap (schedule/
  // budget only — see `deviceExtendCapEod`) would silently reject. A
  // working schedule ward must keep signing EXACTLY what it signs today.
  describe("bucket hit (named times, C-1 + round 2)", () => {
    it("prefers schedule, then budget, over buckets — never an override", () => {
      expect(pickExtendTz("bucket", "Asia/Tokyo", "America/Chicago", "Europe/London")).toBe(
        "Asia/Tokyo",
      );
      expect(pickExtendTz("bucket", undefined, "America/Chicago", "Europe/London")).toBe(
        "America/Chicago",
      );
    });
    it("falls all the way through to bucketsTz only when NEITHER schedule nor budget exists", () => {
      expect(pickExtendTz("bucket", undefined, undefined, "Europe/London")).toBe(
        "Europe/London",
      );
    });
    it("is undefined only when all three are absent", () => {
      expect(pickExtendTz("bucket", undefined, undefined, undefined)).toBeUndefined();
    });
  });
});

describe("deviceExtendCapEod", () => {
  it("is the schedule tz's end-of-day when only schedule exists", () => {
    expect(deviceExtendCapEod(TS, "Asia/Tokyo", undefined)).toBe(endOfDayUnix(TS, "Asia/Tokyo"));
  });
  it("is the budget tz's end-of-day when only budget exists", () => {
    expect(deviceExtendCapEod(TS, undefined, "America/Chicago")).toBe(
      endOfDayUnix(TS, "America/Chicago"),
    );
  });
  it("is the LATER of the two when both exist (mirrors charter-spine::time_extend_eod)", () => {
    // At TS, Tokyo (UTC+9) is already early the next morning (07:13) while
    // Chicago (UTC-6, CST) is mid-afternoon (16:13) — Tokyo has FARTHER to
    // go until its own local midnight, so its absolute eod lands later.
    const cap = deviceExtendCapEod(TS, "Asia/Tokyo", "America/Chicago");
    expect(cap).toBe(Math.max(endOfDayUnix(TS, "Asia/Tokyo"), endOfDayUnix(TS, "America/Chicago")));
    expect(cap).toBe(endOfDayUnix(TS, "Asia/Tokyo"));
  });
  it("is UTC end-of-day when neither exists — the device's own fallback", () => {
    expect(deviceExtendCapEod(TS, undefined, undefined)).toBe(endOfDayUnix(TS, "UTC"));
  });
});

describe("startOfDayUnix", () => {
  it("is the previous UTC midnight for tz UTC", () => {
    expect(startOfDayUnix(TS, "UTC")).toBe(TS - 80_000); // 22:13:20 into the day
  });

  it("agrees with endOfDayUnix (exactly one day apart) for a plain day", () => {
    expect(endOfDayUnix(TS, "Asia/Tokyo") - startOfDayUnix(TS, "Asia/Tokyo")).toBe(86_400);
  });

  it("an unknown tz falls back to the local start-of-day", () => {
    expect(startOfDayUnix(TS, "not/a-real-zone")).toBe(startOfDayUnix(TS, undefined));
    const local = new Date(TS * 1000);
    local.setHours(0, 0, 0, 0);
    expect(startOfDayUnix(TS, undefined)).toBe(local.getTime() / 1000);
  });
});

describe("clampGrantMinutes", () => {
  it("clamps onto the wire's u16 0..=1440 and floors fractions", () => {
    expect(clampGrantMinutes(30)).toBe(30);
    expect(clampGrantMinutes(0)).toBe(0);
    expect(clampGrantMinutes(1440)).toBe(1440);
    expect(clampGrantMinutes(2000)).toBe(1440);
    expect(clampGrantMinutes(-5)).toBe(0);
    expect(clampGrantMinutes(12.7)).toBe(12);
    expect(clampGrantMinutes(Number.NaN)).toBe(0);
  });
});

describe("endOfDayUnix", () => {
  it("is the next UTC midnight for tz UTC", () => {
    // 22:13:20 → 6400s left of the UTC day.
    expect(endOfDayUnix(TS, "UTC")).toBe(TS + 6_400);
  });

  it("is the next midnight in a fixed-offset tz (Asia/Tokyo, +9)", () => {
    // Tokyo is 2023-11-15T07:13:20+09:00 → next Tokyo midnight is
    // 2023-11-15T15:00:00Z.
    expect(endOfDayUnix(TS, "Asia/Tokyo")).toBe(1_700_060_400);
  });

  it("lands exactly on the boundary across a DST day (25h day in Chicago)", () => {
    // 2023-11-05 (US fall-back): Chicago's day is 25h. 06:00Z is 01:00 CDT;
    // the next Chicago midnight is 2023-11-06T00:00:00-06:00 = 06:00Z.
    const at = Date.UTC(2023, 10, 5, 6, 0, 0) / 1000;
    expect(endOfDayUnix(at, "America/Chicago")).toBe(Date.UTC(2023, 10, 6, 6, 0, 0) / 1000);
  });

  it("an unknown tz falls back to the local midnight", () => {
    expect(endOfDayUnix(TS, "not/a-real-zone")).toBe(endOfDayUnix(TS, undefined));
    const local = new Date(TS * 1000);
    local.setHours(24, 0, 0, 0); // the next LOCAL midnight
    expect(endOfDayUnix(TS, undefined)).toBe(local.getTime() / 1000);
  });
});

describe("buildTimeExtendGrant", () => {
  it("echoes reqId + nonce + limitHit verbatim and stamps ts/exp", () => {
    const g = buildTimeExtendGrant(decision({ limitHit: "schedule" }));
    expect(g).toEqual({
      v: 1,
      op: "time.extend",
      reqId: "ab".repeat(32),
      nonce: "cd".repeat(32),
      decision: "allow",
      ts: TS,
      exp: TS + 6_400, // end of the UTC day
      params: { minutesGranted: 30, limitHit: "schedule" },
    });
  });

  it("always satisfies exp > ts (the device rejects a degenerate expiry)", () => {
    const g = buildTimeExtendGrant(decision());
    expect(g.exp).toBeGreaterThan(g.ts);
    // Even asked at the stroke of tz-midnight, exp is the NEXT midnight.
    const midnight = buildTimeExtendGrant(decision({ ts: 1_699_920_000 })); // 00:00:00Z
    expect(midnight.exp).toBe(1_699_920_000 + 86_400);
  });

  it("clamps granted minutes onto the wire (0..=1440)", () => {
    expect(buildTimeExtendGrant(decision({ minutesGranted: 9_999 })).params.minutesGranted).toBe(1440);
    expect(buildTimeExtendGrant(decision({ minutesGranted: -1 })).params.minutesGranted).toBe(0);
  });

  it("a deny always carries 0 minutes and echoes limitHit", () => {
    const g = buildTimeExtendGrant(decision({ decision: "deny", minutesGranted: 30 }));
    expect(g.decision).toBe("deny");
    expect(g.params).toEqual({ minutesGranted: 0, limitHit: "budget" });
  });

  // Named times: a `bucket`-hit extension echoes `bucketId` verbatim, exactly
  // like `limitHit` — on both allow and deny — and stays absent for the
  // whole-device dimensions (never a stray empty string).
  describe("bucketId echo (named times)", () => {
    it("echoes bucketId verbatim on an allow", () => {
      const g = buildTimeExtendGrant(decision({ limitHit: "bucket", bucketId: "play", minutesGranted: 15 }));
      expect(g.params).toEqual({ minutesGranted: 15, limitHit: "bucket", bucketId: "play" });
    });

    it("echoes bucketId even on a deny (0 minutes)", () => {
      const g = buildTimeExtendGrant(
        decision({ limitHit: "bucket", bucketId: "play", decision: "deny", minutesGranted: 15 }),
      );
      expect(g.params).toEqual({ minutesGranted: 0, limitHit: "bucket", bucketId: "play" });
    });

    it("stays absent for the whole-device dimensions", () => {
      const g = buildTimeExtendGrant(decision({ limitHit: "budget" }));
      expect("bucketId" in g.params).toBe(false);
    });
  });

  // Review round 2 (2026-08-03): the device's own enactor rejects a
  // `time.extend` grant as Terminal — SILENTLY from this app's point of
  // view, it already published — whenever `exp` exceeds its OWN cap
  // (`charter-spine::enforcer_runtime::time_extend_eod`, mirrored here by
  // `deviceExtendCapEod`) by more than the 300s skew tolerance. `tz` alone
  // (the buckets-only fallback in particular) can land past that cap for
  // most tz/time-of-day combinations — `buildTimeExtendGrant` must clamp
  // `exp` DOWN to it regardless of which tz `pickExtendTz` resolved.
  describe("exp clamped to the device's own validity cap (review round 2)", () => {
    const ACCEPTS = (exp: number, cap: number) => expect(exp).toBeLessThanOrEqual(cap + 300);

    it("a buckets-only ward (no schedule, no budget) signs an exp the device rule accepts, in Asia/Tokyo", () => {
      const g = buildTimeExtendGrant(
        decision({ limitHit: "bucket", bucketId: "play", tz: "Asia/Tokyo" }),
      );
      const cap = deviceExtendCapEod(TS, undefined, undefined); // neither clause → UTC
      ACCEPTS(g.exp, cap);
      // Concretely: clamped down to the UTC eod, not left at Tokyo's own
      // (later) local midnight — this TS is exactly the case that overshoots.
      expect(g.exp).toBe(cap);
      expect(endOfDayUnix(TS, "Asia/Tokyo")).toBeGreaterThan(cap); // the overshoot, unclamped
    });

    it("a schedule+buckets ward with divergent tz's clamps to the SCHEDULE eod, never the buckets one", () => {
      // Simulates `tz` having resolved to the buckets fallback despite a
      // schedule existing (exactly round 1's bug, or any future divergence)
      // — the clamp is the safety net regardless of how `tz` got picked.
      const g = buildTimeExtendGrant(
        decision({
          limitHit: "bucket",
          bucketId: "play",
          tz: "Asia/Tokyo", // bucketsTz — far ahead, would overshoot
          scheduleTz: "America/Chicago",
        }),
      );
      const cap = deviceExtendCapEod(TS, "America/Chicago", undefined);
      ACCEPTS(g.exp, cap);
      expect(g.exp).toBe(cap);
      expect(g.exp).toBe(endOfDayUnix(TS, "America/Chicago"));
    });

    it("the London 23:10 BST case (reviewer-measured overshoot window) now passes", () => {
      // 2024-07-15T22:10:00Z = 2024-07-15T23:10:00+01:00 (BST).
      const londonTs = Date.UTC(2024, 6, 15, 22, 10, 0) / 1000;
      const g = buildTimeExtendGrant(
        decision({
          limitHit: "bucket",
          bucketId: "play",
          ts: londonTs,
          tz: "Europe/London",
        }),
      );
      const cap = deviceExtendCapEod(londonTs, undefined, undefined);
      ACCEPTS(g.exp, cap);
    });

    it("never clamps UP — a natural exp already inside the cap is left alone", () => {
      // schedule and buckets AGREE here, so the clamp must be a no-op.
      const g = buildTimeExtendGrant(
        decision({ limitHit: "bucket", bucketId: "play", tz: "UTC", scheduleTz: "UTC" }),
      );
      expect(g.exp).toBe(endOfDayUnix(TS, "UTC"));
    });

    it("still satisfies exp > ts even when the clamp bites hard (skew-of-a-second ts)", () => {
      const g = buildTimeExtendGrant(
        decision({ limitHit: "bucket", bucketId: "play", ts: 1_699_920_000, tz: "Asia/Tokyo" }), // exactly UTC midnight
      );
      expect(g.exp).toBeGreaterThan(g.ts);
    });
  });
});

describe("buildInstallApkGrant", () => {
  const REQ = "ab".repeat(32);
  const NONCE = "cd".repeat(32);
  const CERT = "3c".repeat(32);

  it("echoes reqId/nonce and pins the cert, exp = ts + TTL", () => {
    const g = buildInstallApkGrant({
      reqId: REQ,
      nonce: NONCE,
      decision: "allow",
      packageName: "app.example.thing",
      signerCertSha256: CERT,
      source: "staged",
      ts: TS,
    });
    expect(g.op).toBe("install.apk");
    expect(g.reqId).toBe(REQ);
    expect(g.nonce).toBe(NONCE);
    expect(g.decision).toBe("allow");
    expect(g.exp).toBe(TS + INSTALL_GRANT_TTL_SECS);
    expect(g.params).toEqual({
      packageName: "app.example.thing",
      signerCertSha256: CERT,
      source: "staged",
    });
  });

  it("includes versionCode only when provided", () => {
    const withV = buildInstallApkGrant({
      reqId: REQ, nonce: NONCE, decision: "allow",
      packageName: "app.example.thing", signerCertSha256: CERT, source: "staged",
      ts: TS, versionCode: 42,
    });
    expect(withV.params.versionCode).toBe(42);
    const withoutV = buildInstallApkGrant({
      reqId: REQ, nonce: NONCE, decision: "allow",
      packageName: "app.example.thing", signerCertSha256: CERT, source: "staged", ts: TS,
    });
    expect("versionCode" in withoutV.params).toBe(false);
  });

  it("a deny needs no cert — it carries the zero sentinel", () => {
    const g = buildInstallApkGrant({
      reqId: REQ, nonce: NONCE, decision: "deny",
      packageName: "app.example.thing", source: "staged", ts: TS,
    });
    expect(g.decision).toBe("deny");
    expect(g.params.signerCertSha256).toBe(DENY_CERT_SENTINEL);
    expect(g.params.packageName).toBe("app.example.thing");
  });

  it("an ALLOW without a pinned cert is a programming error and throws", () => {
    expect(() =>
      buildInstallApkGrant({
        reqId: REQ, nonce: NONCE, decision: "allow",
        packageName: "app.example.thing", source: "staged", ts: TS,
      }),
    ).toThrow(/signerCertSha256/);
  });
});

describe("buildAppOpenGrant", () => {
  const REQ = "ab".repeat(32);
  const NONCE = "cd".repeat(32);
  const PKG = "com.mojang.minecraftpe";

  it("echoes reqId/nonce/pkg, exp = ts + TTL", () => {
    const g = buildAppOpenGrant({
      reqId: REQ,
      nonce: NONCE,
      decision: "allow",
      pkg: PKG,
      minutesGranted: 30,
      ts: TS,
    });
    expect(g.op).toBe("app.open");
    expect(g.reqId).toBe(REQ);
    expect(g.nonce).toBe(NONCE);
    expect(g.decision).toBe("allow");
    expect(g.exp).toBe(TS + APP_OPEN_GRANT_TTL_SECS);
    expect(g.params).toEqual({ pkg: PKG, minutesGranted: 30 });
  });

  it("a deny always carries 0 minutes, regardless of what's passed", () => {
    const g = buildAppOpenGrant({
      reqId: REQ,
      nonce: NONCE,
      decision: "deny",
      pkg: PKG,
      minutesGranted: 30,
      ts: TS,
    });
    expect(g.decision).toBe("deny");
    expect(g.params).toEqual({ pkg: PKG, minutesGranted: 0 });
  });

  it("clamps granted minutes onto the wire (0..=1440), same as time.extend", () => {
    const g = buildAppOpenGrant({
      reqId: REQ,
      nonce: NONCE,
      decision: "allow",
      pkg: PKG,
      minutesGranted: 9_999,
      ts: TS,
    });
    expect(g.params.minutesGranted).toBe(1440);
  });
});

describe("appOpenWindowUnix", () => {
  it("30m and 1h are plain durations off nowUnix, tz irrelevant", () => {
    expect(appOpenWindowUnix("30m", TS)).toBe(TS + 30 * 60);
    expect(appOpenWindowUnix("1h", TS)).toBe(TS + 60 * 60);
    expect(appOpenWindowUnix("30m", TS, undefined)).toBe(TS + 30 * 60);
  });

  it("restOfDay is end-of-day in the CHILD's tz", () => {
    expect(appOpenWindowUnix("restOfDay", TS, "UTC")).toBe(endOfDayUnix(TS, "UTC"));
    expect(appOpenWindowUnix("restOfDay", TS, "Asia/Tokyo")).toBe(endOfDayUnix(TS, "Asia/Tokyo"));
  });

  it("refuses restOfDay with an unknown child tz — never a silent guardian-local fallback", () => {
    expect(appOpenWindowUnix("restOfDay", TS, undefined)).toBeNull();
  });
});
