import { describe, expect, it } from "vitest";
import { decisionTiming } from "./decisionTiming";
import { buildAppOpenGrant } from "../wire/grant";
import type { ChildRequest } from "../domain/types";

const HEX64 = (b: string) => b.repeat(64);

const APP_OPEN_REQ: ChildRequest = {
  id: "req1",
  childId: "child1",
  requester: "device",
  deviceId: "dev1",
  kind: "app.open",
  createdAt: 0,
  title: "device asks to open Minecraft",
  appId: "com.mojang.minecraftpe",
  appLabel: "Minecraft",
  status: "pending",
  reqId: HEX64("a"),
  nonce: HEX64("b"),
  machine: HEX64("c"),
  subject: HEX64("d"),
};

const TIME_EXTEND_REQ: ChildRequest = {
  id: "req2",
  childId: "child1",
  requester: "device",
  deviceId: "dev1",
  kind: "time.extend",
  createdAt: 0,
  title: "device asks for 20 more minutes",
  minutesRequested: 20,
  limitHit: "budget",
  status: "pending",
  reqId: HEX64("a"),
  nonce: HEX64("b"),
  machine: HEX64("c"),
  subject: HEX64("d"),
};

const BUCKET_EXTEND_REQ: ChildRequest = {
  ...TIME_EXTEND_REQ,
  id: "req3",
  title: "device asks for 15 more minutes of Play",
  limitHit: "bucket",
  bucketId: "play",
};

/**
 * N1 (review round 2, 2026-08-03): `decisionContext`'s `minutesGranted`
 * ternary only ever covered `time.extend`, so every OTHER kind — including
 * `app.open` once it gained a real grant slot in review round 1 — silently
 * fell to `undefined`, and `realSigner.ts` reads that as `?? 0`: the
 * documented DENY value. An ALLOWED app.open shipped `minutesGranted: 0` —
 * indistinguishable from a deny, in the very field the round added.
 * `realSigner.test.ts` hand-builds `DecisionContext` directly (never calls
 * `decisionContext`/`decisionTiming` at all), so it could never have caught
 * this — these tests go THROUGH the real composition instead.
 */
describe("decisionTiming — app.open minutesGranted (N1 regression)", () => {
  it("an approved 30-minute window carries minutesGranted 30 all the way into the signed grant", () => {
    const timing = decisionTiming({ req: APP_OPEN_REQ, minutesGranted: 30 });
    expect(timing.minutesGranted).toBe(30);
    expect(timing.appOpen).toEqual({
      reqId: HEX64("a"),
      nonce: HEX64("b"),
      machine: HEX64("c"),
      subject: HEX64("d"),
      pkg: "com.mojang.minecraftpe",
    });

    const grant = buildAppOpenGrant({
      reqId: timing.appOpen!.reqId,
      nonce: timing.appOpen!.nonce,
      decision: "allow",
      pkg: timing.appOpen!.pkg,
      minutesGranted: timing.minutesGranted!,
      ts: 1_700_000_000,
    });
    expect(grant.decision).toBe("allow");
    expect(grant.params).toEqual({ pkg: "com.mojang.minecraftpe", minutesGranted: 30 });
  });

  it("a deny (minutesGranted 0, exactly what denyRequest always passes) carries 0 — never the allow value", () => {
    const timing = decisionTiming({ req: APP_OPEN_REQ, minutesGranted: 0 });
    expect(timing.minutesGranted).toBe(0);

    const grant = buildAppOpenGrant({
      reqId: timing.appOpen!.reqId,
      nonce: timing.appOpen!.nonce,
      decision: "deny",
      pkg: timing.appOpen!.pkg,
      minutesGranted: timing.minutesGranted!,
      ts: 1_700_000_000,
    });
    expect(grant.decision).toBe("deny");
    expect(grant.params).toEqual({ pkg: "com.mojang.minecraftpe", minutesGranted: 0 });
  });

  it("REGRESSION: omitting minutesGranted entirely still resolves to 0, never undefined", () => {
    // This is the exact shape of the original bug: nothing passed through
    // the ternary's `undefined` branch, which `realSigner.ts`'s `?? 0`
    // fallback silently turned into a deny-shaped grant even on an allow.
    const timing = decisionTiming({ req: APP_OPEN_REQ });
    expect(timing.minutesGranted).toBe(0);
  });

  it("does not disturb time.extend's own minutesGranted composition", () => {
    expect(decisionTiming({ req: TIME_EXTEND_REQ, minutesGranted: 15 }).minutesGranted).toBe(15);
    // Falls back to what was requested when the caller passes nothing.
    expect(decisionTiming({ req: TIME_EXTEND_REQ }).minutesGranted).toBe(20);
  });

  it("a kind with no grant slot (install.app) carries no minutesGranted at all", () => {
    const req: ChildRequest = { ...APP_OPEN_REQ, kind: "install.app", appId: "flatpak:org.videolan.VLC" };
    expect(decisionTiming({ req, minutesGranted: 99 }).minutesGranted).toBeUndefined();
  });
});

/**
 * C-1 (hardware round, 2026-08-03): a buckets-only ward — the feature's own
 * minimal configuration — carries neither `schedule.tz` nor `budget.tz`.
 * Before this, `decisionTiming` never received the buckets clause's own tz at
 * all, so a bucket ask's `tz` came out `undefined` and `realSigner.ts`
 * refused to sign ANY grant for it. These go through the real composition
 * (not a hand-built `DecisionContext`), the same discipline N1 established.
 */
describe("decisionTiming — bucket tz precedence (C-1 regression)", () => {
  it("a buckets-only ward (no schedule, no budget) still resolves a tz from bucketsTz", () => {
    const timing = decisionTiming({
      req: BUCKET_EXTEND_REQ,
      minutesGranted: 15,
      bucketsTz: "Europe/London",
    });
    expect(timing.tz).toBe("Europe/London");
  });

  // Review round 2 (2026-08-03): bucketsTz is a FALLBACK, never an override
  // — a first pass preferred it outright on a bucket hit, but `buckets.tz`
  // is seeded from the guardian's own local tz and unrelated to the
  // device's actual `time.extend` validity cap (schedule/budget only — see
  // `wire/grant.ts`'s `deviceExtendCapEod`), so overriding a working
  // schedule/budget ward's own tz could sign an `exp` the device silently
  // rejects. A working schedule ward must keep signing exactly what it
  // signs today.
  it("prefers schedule, then budget, over buckets for a bucket-hit ask — never an override", () => {
    const timing = decisionTiming({
      req: BUCKET_EXTEND_REQ,
      scheduleTz: "Asia/Tokyo",
      budgetTz: "America/Chicago",
      bucketsTz: "Europe/London",
    });
    expect(timing.tz).toBe("Asia/Tokyo");
  });

  it("falls back to schedule then budget when bucketsTz is absent", () => {
    expect(
      decisionTiming({ req: BUCKET_EXTEND_REQ, scheduleTz: "Asia/Tokyo" }).tz,
    ).toBe("Asia/Tokyo");
    expect(
      decisionTiming({ req: BUCKET_EXTEND_REQ, budgetTz: "America/Chicago" }).tz,
    ).toBe("America/Chicago");
  });

  it("stays undefined (a genuine refusal, never a silent phone-local grant) when all three are absent", () => {
    expect(decisionTiming({ req: BUCKET_EXTEND_REQ }).tz).toBeUndefined();
  });

  it("does not disturb a schedule/budget-hit ask's own precedence", () => {
    expect(
      decisionTiming({
        req: TIME_EXTEND_REQ, // limitHit: "budget"
        scheduleTz: "Asia/Tokyo",
        budgetTz: "America/Chicago",
        bucketsTz: "Europe/London",
      }).tz,
    ).toBe("America/Chicago");
  });
});
