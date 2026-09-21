import { describe, expect, it } from "vitest";
import type { Device } from "./types";
import type { DeviceStatus } from "../wire/status";
import { deliverableDevices, supportNote, tooOldNote, unsupportedNote, wardenSupport } from "./wardenSupport";

function device(over: Partial<Device>): Device {
  return {
    id: "d1",
    label: "Phone",
    platform: "android",
    pairing: "paired",
    devicePubkey: "aa".repeat(32),
    ...over,
  } as Device;
}

function status(over: Partial<DeviceStatus>): DeviceStatus {
  return {
    v: 1,
    subject: "cd".repeat(32),
    machine: "aa".repeat(32),
    ts: 1000,
    dayKey: "2026-07-28",
    usedTodaySecs: 0,
    windowLeftSecs: 0,
    quotaLeftSecs: 0,
    effectiveSecs: 1800,
    locked: false,
    source: "guardian",
    ...over,
  };
}

const phone = device({ id: "p1", platform: "android", devicePubkey: "aa".repeat(32) });
const laptop = device({ id: "l1", platform: "linux", devicePubkey: "bb".repeat(32) });

describe("deliverableDevices", () => {
  it("is every paired device with a key, whatever the platform", () => {
    const half = device({ id: "u1", pairing: "unpaired", devicePubkey: undefined });
    expect(deliverableDevices([phone, laptop, half]).map((d) => d.id)).toEqual(["p1", "l1"]);
  });
});

describe("wardenSupport", () => {
  // The rule decented set (2026-07-28), after a switched-off laptop greyed out
  // Finish now: a guardian must be able to call time even when nobody is at
  // their device. The clause is durable — the relay holds it and the device
  // applies it when it comes back — so SILENCE IS NOT INCAPACITY. Only a
  // device that actually reports an old Kintrinsic is known to be unable.
  it("lets the guardian act on a device that has not reported", () => {
    const g = wardenSupport([laptop], {}, "standdown");
    expect(g.canSend).toBe(true);
    expect(g.tooOld).toEqual([]);
  });

  it("lets the guardian act when every device reports a version that knows it", () => {
    const g = wardenSupport([phone, laptop], {
      [phone.devicePubkey as string]: status({ appVersionCode: 25 }),
      [laptop.devicePubkey as string]: status({ appVersionCode: 403 }),
    }, "standdown");
    expect(g.canSend).toBe(true);
  });

  // Naming the laggard is the point: the guardian can still act on the rest,
  // and knows exactly which device to update.
  it("still sends when only SOME devices are too old, and names them", () => {
    const g = wardenSupport([phone, laptop], {
      [phone.devicePubkey as string]: status({ appVersionCode: 25 }),
      [laptop.devicePubkey as string]: status({ appVersionCode: 305 }),
    }, "standdown");
    expect(g.canSend).toBe(true);
    expect(g.tooOld.map((d) => d.id)).toEqual(["l1"]);
  });

  // Only when NOTHING could honour it is the button a lie.
  it("refuses only when every device is known to be too old", () => {
    const g = wardenSupport([phone], {
      [phone.devicePubkey as string]: status({ appVersionCode: 23 }),
    }, "standdown");
    expect(g.canSend).toBe(false);
    expect(g.tooOld.map((d) => d.id)).toEqual(["p1"]);
  });

  it("counts each platform on its own version scheme", () => {
    // 401 is a current charterd but would be a far-future APK code; 25 is a
    // current APK but would be a pre-release charterd.
    const g = wardenSupport([phone, laptop], {
      [phone.devicePubkey as string]: status({ appVersionCode: 25 }),
      [laptop.devicePubkey as string]: status({ appVersionCode: 401 }),
    }, "gift");
    expect(g.canSend).toBe(true);
    expect(g.tooOld).toEqual([]);
  });

  // With nothing paired the ask is refused downstream with a clearer message
  // ("pair their phone first"), so the button stays live to deliver it.
  it("leaves an unpaired ward's button alive to explain itself", () => {
    expect(wardenSupport([], {}, "standdown").canSend).toBe(true);
  });
});

// A platform that never enforces a clause is NOT the same as an old warden,
// and conflating them was the root of the audit's whole silent-gap class
// (2026-08-02). An old warden honours the rule once it updates, so silence
// means "wait"; a laptop will never read `listening` however current it is, so
// a guardian who isn't told is authoring a rule that does not exist. Version
// thresholds cannot express this — the device reports a perfectly current
// Kintrinsic and still ignores the clause.
describe("wardenSupport — platforms that never enforce a clause", () => {
  it("names a laptop for phone-only clauses even when it reports the newest Kintrinsic", () => {
    const fresh = { [laptop.devicePubkey as string]: status({ appVersionCode: 99999 }) };
    for (const feature of ["listening", "tethering", "lifeline"] as const) {
      const s = wardenSupport([laptop], fresh, feature);
      expect(s.unsupported.map((d) => d.id), feature).toEqual(["l1"]);
      expect(s.tooOld, feature).toEqual([]);
      // Nothing paired could honour it, so the control is a lie as offered.
      expect(s.canSend, feature).toBe(false);
    }
  });

  it("names a phone for laptop-only clauses", () => {
    for (const feature of ["learning"] as const) {
      const s = wardenSupport([phone], {}, feature);
      expect(s.unsupported.map((d) => d.id), feature).toEqual(["p1"]);
      expect(s.canSend, feature).toBe(false);
    }
  });

  it("still offers the control when one device CAN honour it", () => {
    const s = wardenSupport([phone, laptop], {}, "learning");
    expect(s.unsupported.map((d) => d.id)).toEqual(["p1"]);
    expect(s.canSend).toBe(true);
  });

  it("leaves clauses both wardens enforce alone", () => {
    for (const feature of ["standdown", "gift", "appHold"] as const) {
      expect(wardenSupport([phone, laptop], {}, feature).unsupported, feature).toEqual([]);
    }
  });
});

// Named times (2026-08): buckets went from Android-NEVER to Android-gated —
// the branch that finally ships Android enforcement. Distinct from the
// never-enforce cases above: an old Android build honours buckets once it
// updates (silence means "wait"), it isn't a permanent platform gap.
describe("wardenSupport — buckets (android version-gated, no longer NEVER)", () => {
  it("lets the guardian act on a phone that has not reported", () => {
    const s = wardenSupport([phone], {}, "buckets");
    expect(s.canSend).toBe(true);
    expect(s.unsupported).toEqual([]);
    expect(s.tooOld).toEqual([]);
  });

  it("names a phone reporting older than the shipping versionCode", () => {
    const s = wardenSupport([phone, laptop], {
      [phone.devicePubkey as string]: status({ appVersionCode: 37 }),
    }, "buckets");
    expect(s.tooOld.map((d) => d.id)).toEqual(["p1"]);
    expect(s.unsupported).toEqual([]);
    expect(s.canSend).toBe(true); // the laptop still honours it
  });

  it("sends when a phone reports the shipping versionCode or newer", () => {
    const s = wardenSupport([phone], {
      [phone.devicePubkey as string]: status({ appVersionCode: 38 }),
    }, "buckets");
    expect(s.tooOld).toEqual([]);
    expect(s.canSend).toBe(true);
  });

  it("a laptop is never gated on buckets (already shipped there)", () => {
    const s = wardenSupport([laptop], { [laptop.devicePubkey as string]: status({ appVersionCode: 1 }) }, "buckets");
    expect(s.tooOld).toEqual([]);
    expect(s.unsupported).toEqual([]);
  });
});

describe("wardenSupport — bucketsWeekly / appOpenAsk (new on both platforms)", () => {
  it("gates both platforms on this branch's shipping versions", () => {
    for (const feature of ["bucketsWeekly", "appOpenAsk"] as const) {
      const stale = wardenSupport([phone, laptop], {
        [phone.devicePubkey as string]: status({ appVersionCode: 37 }),
        [laptop.devicePubkey as string]: status({ appVersionCode: 702 }),
      }, feature);
      expect(stale.tooOld.map((d) => d.id), feature).toEqual(["p1", "l1"]);
      expect(stale.unsupported, feature).toEqual([]);

      const current = wardenSupport([phone, laptop], {
        [phone.devicePubkey as string]: status({ appVersionCode: 38 }),
        [laptop.devicePubkey as string]: status({ appVersionCode: 703 }),
      }, feature);
      expect(current.tooOld, feature).toEqual([]);
      expect(current.canSend, feature).toBe(true);
    }
  });

  it("silence is not incapacity — an unreported device is assumed capable", () => {
    for (const feature of ["bucketsWeekly", "appOpenAsk"] as const) {
      const s = wardenSupport([phone, laptop], {}, feature);
      expect(s.canSend, feature).toBe(true);
      expect(s.tooOld, feature).toEqual([]);
    }
  });
});

// Honest attribution (2026-08-03): both the `cmdline:` identity form and the
// unrecognised-time counter are Linux-only concepts, at every version — not
// merely new-and-gated like buckets/appHold. An Android build never grows
// either, so this is NEVER, exactly like listening/tethering/lifeline above.
describe("wardenSupport — cmdlineIdentity / unrecognisedTime (Linux-only, NEVER on Android)", () => {
  it("names a phone as unsupported at any reported version", () => {
    for (const feature of ["cmdlineIdentity", "unrecognisedTime"] as const) {
      const s = wardenSupport([phone], { [phone.devicePubkey as string]: status({ appVersionCode: 99999 }) }, feature);
      expect(s.unsupported.map((d) => d.id), feature).toEqual(["p1"]);
      expect(s.canSend, feature).toBe(false);
    }
  });

  it("gates a laptop on charterd 705, not merely on being Linux", () => {
    for (const feature of ["cmdlineIdentity", "unrecognisedTime"] as const) {
      const stale = wardenSupport([laptop], { [laptop.devicePubkey as string]: status({ appVersionCode: 704 }) }, feature);
      expect(stale.tooOld.map((d) => d.id), feature).toEqual(["l1"]);
      expect(stale.unsupported, feature).toEqual([]);

      const current = wardenSupport([laptop], { [laptop.devicePubkey as string]: status({ appVersionCode: 705 }) }, feature);
      expect(current.tooOld, feature).toEqual([]);
      expect(current.canSend, feature).toBe(true);
    }
  });

  it("silence is not incapacity — a laptop that hasn't reported is assumed capable", () => {
    for (const feature of ["cmdlineIdentity", "unrecognisedTime"] as const) {
      const s = wardenSupport([laptop], {}, feature);
      expect(s.tooOld, feature).toEqual([]);
      expect(s.canSend, feature).toBe(true);
    }
  });

  it("a mixed ward can still send while the phone is named as never-supporting", () => {
    for (const feature of ["cmdlineIdentity", "unrecognisedTime"] as const) {
      const s = wardenSupport([phone, laptop], {
        [laptop.devicePubkey as string]: status({ appVersionCode: 705 }),
      }, feature);
      expect(s.unsupported.map((d) => d.id), feature).toEqual(["p1"]);
      expect(s.canSend, feature).toBe(true);
    }
  });
});

// The `named` time model (2026-08-06 "named costs" design): Android support
// is a deliberate later pass, not yet shipped on any build — gated NEVER,
// same as `cmdlineIdentity`/`unrecognisedTime` above, not a version threshold
// this repo hasn't reached yet.
describe("wardenSupport — timeModel (Linux 706+, NEVER on Android)", () => {
  it("names a phone as unsupported at any reported version", () => {
    const s = wardenSupport([phone], { [phone.devicePubkey as string]: status({ appVersionCode: 99999 }) }, "timeModel");
    expect(s.unsupported.map((d) => d.id)).toEqual(["p1"]);
    expect(s.canSend).toBe(false);
  });

  it("gates a laptop on charterd 706, not merely on being Linux", () => {
    const stale = wardenSupport([laptop], { [laptop.devicePubkey as string]: status({ appVersionCode: 705 }) }, "timeModel");
    expect(stale.tooOld.map((d) => d.id)).toEqual(["l1"]);
    expect(stale.unsupported).toEqual([]);

    const current = wardenSupport([laptop], { [laptop.devicePubkey as string]: status({ appVersionCode: 706 }) }, "timeModel");
    expect(current.tooOld).toEqual([]);
    expect(current.canSend).toBe(true);
  });

  it("silence is not incapacity — a laptop that hasn't reported is assumed capable", () => {
    const s = wardenSupport([laptop], {}, "timeModel");
    expect(s.tooOld).toEqual([]);
    expect(s.canSend).toBe(true);
  });

  it("a mixed ward can still send while the phone is named as never-supporting", () => {
    const s = wardenSupport([phone, laptop], {
      [laptop.devicePubkey as string]: status({ appVersionCode: 706 }),
    }, "timeModel");
    expect(s.unsupported.map((d) => d.id)).toEqual(["p1"]);
    expect(s.canSend).toBe(true);
  });
});

// Always-available apps (2026-08-04): Android is version-gated like
// buckets/appHold (ward Kintrinsic 0.6.8, versionCode 39 is the first build that
// reads the clause). Linux is a permanent NEVER, same architectural reason as
// listening directly above it in MIN_VERSION — charterd's lock freezes the
// child's whole user slice indiscriminately, so there is no way to exempt one
// app from it, however current charterd is.
describe("wardenSupport — alwaysAvailable (Android version-gated, Linux NEVER)", () => {
  it("names a laptop as unsupported at any reported version", () => {
    const s = wardenSupport(
      [laptop],
      { [laptop.devicePubkey as string]: status({ appVersionCode: 99999 }) },
      "alwaysAvailable",
    );
    expect(s.unsupported.map((d) => d.id)).toEqual(["l1"]);
    expect(s.tooOld).toEqual([]);
    expect(s.canSend).toBe(false);
  });

  it("names a phone reporting versionCode 38 as too old", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 38 }) },
      "alwaysAvailable",
    );
    expect(s.tooOld.map((d) => d.id)).toEqual(["p1"]);
    expect(s.unsupported).toEqual([]);
    expect(s.canSend).toBe(false);
  });

  it("sends to a phone reporting versionCode 39", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 39 }) },
      "alwaysAvailable",
    );
    expect(s.tooOld).toEqual([]);
    expect(s.canSend).toBe(true);
  });
});

// "Remove from device" (2026-08-27): an Android Device Owner power with no
// Linux counterpart at all — charterd has no launcher to make a package vanish
// from, and a laptop's junk can be uninstalled the ordinary way — so Linux is
// a permanent NEVER rather than a threshold this repo hasn't shipped yet. Ward
// Kintrinsic 0.6.10 (versionCode 41) is the first build that reads the field;
// 0.6.9 (40) is the release that locked USB debugging and so created the need.
describe("wardenSupport — appHide (Android 41+, Linux NEVER)", () => {
  it("names a laptop as unsupported at any reported version", () => {
    const s = wardenSupport(
      [laptop],
      { [laptop.devicePubkey as string]: status({ appVersionCode: 99999 }) },
      "appHide",
    );
    expect(s.unsupported.map((d) => d.id)).toEqual(["l1"]);
    expect(s.tooOld).toEqual([]);
    expect(s.canSend).toBe(false);
  });

  it("names a phone reporting versionCode 40 as too old", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 40 }) },
      "appHide",
    );
    expect(s.tooOld.map((d) => d.id)).toEqual(["p1"]);
    expect(s.unsupported).toEqual([]);
    expect(s.canSend).toBe(false);
  });

  it("sends to a phone reporting versionCode 41", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 41 }) },
      "appHide",
    );
    expect(s.tooOld).toEqual([]);
    expect(s.canSend).toBe(true);
  });

  // Silence is not incapacity — a tablet that is simply switched off still
  // gets the clause, and the guardian still gets the Remove button.
  it("sends to a phone that has never reported", () => {
    expect(wardenSupport([phone], {}, "appHide").canSend).toBe(true);
  });

  // A mixed family: the laptop can never hide an app, but the phone can, so
  // the affordance stays live and the note names the laptop.
  it("still sends when only some devices can honour it", () => {
    const s = wardenSupport(
      [phone, laptop],
      { [phone.devicePubkey as string]: status({ appVersionCode: 41 }) },
      "appHide",
    );
    expect(s.canSend).toBe(true);
    expect(s.unsupported.map((d) => d.id)).toEqual(["l1"]);
  });
});

// Every ward on the branch before this one reports versionCode 38, so on the
// day always-available ships EVERY existing phone is `tooOld` — without this
// note a guardian would name an app, see nothing wrong, and watch nothing
// happen on the phone (the same silent-failure shape `unsupportedNote` exists
// to prevent for the permanent-gap case).
describe("tooOldNote", () => {
  it("names the device reporting versionCode 38 and says what to do", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 38 }) },
      "alwaysAvailable",
    );
    expect(tooOldNote(s, "keep an app open at any hour")).toBe(
      "Phone needs the latest Kintrinsic before it can keep an app open at any hour. Update it, then this starts working by itself.",
    );
  });

  it("is absent once the device reports the shipping versionCode", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 39 }) },
      "alwaysAvailable",
    );
    expect(tooOldNote(s, "keep an app open at any hour")).toBeNull();
  });

  it("says nothing when the device has not reported (silence is not incapacity)", () => {
    const s = wardenSupport([phone], {}, "alwaysAvailable");
    expect(tooOldNote(s, "keep an app open at any hour")).toBeNull();
  });
});

describe("unsupportedNote", () => {
  it("names the devices and says what happens there", () => {
    const s = wardenSupport([phone, laptop], {}, "learning");
    expect(unsupportedNote(s, "A daily allowance", "Their phone is governed by the day's screen time.")).toBe(
      "A daily allowance does nothing on Phone. Their phone is governed by the day's screen time.",
    );
  });

  it("says nothing when every device can honour it", () => {
    expect(unsupportedNote(wardenSupport([laptop], {}, "learning"), "A daily allowance")).toBeNull();
  });
});

// Named times wired three VERSION thresholds (`buckets`, `bucketsWeekly`,
// `appOpenAsk`) to `unsupportedNote`, which reports only the NEVER list — so
// the warning could never render while a ward on 37 dropped every counted cap.
// `supportNote` picks the sentence from the support shape itself.
describe("supportNote", () => {
  it("warns about a too-old ward for a version-threshold feature", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 37 }) },
      "bucketsWeekly",
    );
    expect(unsupportedNote(s, "A weekly allowance")).toBeNull(); // the old wiring
    expect(supportNote(s, "A weekly allowance")).toBe(
      "Phone needs the latest Kintrinsic. A weekly allowance does nothing there until then.",
    );
  });

  it("carries a feature's own consequence when it is worse than 'does nothing'", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 37 }) },
      "bucketsWeekly",
    );
    expect(supportNote(s, "A weekly allowance", undefined, "Until then every cap lifts.")).toBe(
      "Phone needs the latest Kintrinsic. Until then every cap lifts.",
    );
  });

  it("still reports the permanent platform gap", () => {
    const s = wardenSupport([phone], {}, "learning");
    expect(supportNote(s, "A free named time", "There, it costs.")).toBe(
      unsupportedNote(s, "A free named time", "There, it costs."),
    );
  });

  it("is silent when every device can honour it", () => {
    const s = wardenSupport(
      [phone],
      { [phone.devicePubkey as string]: status({ appVersionCode: 38 }) },
      "bucketsWeekly",
    );
    expect(supportNote(s, "A weekly allowance")).toBeNull();
  });
});
