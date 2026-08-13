import { describe, expect, it } from "vitest";
import {
  canOpenInstallWindow,
  installWindow,
  installWindowLabel,
  INSTALL_WINDOW_MINUTES,
  MAX_INSTALL_WINDOW_MINUTES,
  pruneWindows,
  putWindow,
  accountSummary,
} from "./installWindow";
import type { Device } from "../domain/types";

const NOW = 1_782_752_400;
const device = (over: Partial<Device> = {}): Device => ({
  id: "d1",
  label: "Robin’s phone",
  platform: "android",
  pairing: "paired",
  ...over,
});

describe("who can be offered an install window", () => {
  it("is any paired phone — not only a phone whose update is broken", () => {
    // The regression this feature IS: the window existed but was gated behind
    // `updateHealth.attempts > 2`, so a perfectly healthy phone could never be
    // let through the install lock by any route at all.
    expect(canOpenInstallWindow(device())).toBe(true);
  });

  it("is not a laptop — the clause has no install lock to stand down there", () => {
    expect(canOpenInstallWindow(device({ platform: "linux" }))).toBe(false);
  });

  it("is not an unpaired device — there is nothing to send a clause to", () => {
    expect(canOpenInstallWindow(device({ pairing: "unpaired" }))).toBe(false);
  });
});

describe("the window a guardian is offered", () => {
  it("is one the ward's own core will honour", () => {
    // The ward refuses anything past MAX_MAINTENANCE_SECS outright. Offering
    // longer would look like it worked and be dropped on arrival.
    expect(INSTALL_WINDOW_MINUTES).toBeLessThanOrEqual(MAX_INSTALL_WINDOW_MINUTES);
    expect(MAX_INSTALL_WINDOW_MINUTES * 60).toBe(3600);
  });

  it("is long enough for a real store download, not just a cabled repair", () => {
    expect(INSTALL_WINDOW_MINUTES).toBeGreaterThanOrEqual(30);
  });
});

describe("whether the window is open", () => {
  it("is read from the expiry, so it closes on its own", () => {
    expect(installWindow(NOW + 600, NOW).open).toBe(true);
    expect(installWindow(NOW + 600, NOW).secondsLeft).toBe(600);
  });

  it("is shut the instant the expiry passes — never a latched flag", () => {
    // THE invariant. The phone re-derives the lock every tick, so at NOW+1 it is
    // locked again; if this screen still said "open" it would be lying about a
    // phone that had already re-locked.
    expect(installWindow(NOW, NOW).open).toBe(false);
    expect(installWindow(NOW - 1, NOW).open).toBe(false);
    expect(installWindow(NOW - 100_000, NOW).secondsLeft).toBe(0);
  });

  it("is shut when none was ever opened", () => {
    expect(installWindow(undefined, NOW).open).toBe(false);
    expect(installWindow(0, NOW).open).toBe(false);
  });
});

describe("how the time left reads", () => {
  it("never rounds down to a number smaller than the phone allows", () => {
    // 50 seconds left must not read "0 minutes" — a guardian would stop trying
    // while the window was still genuinely open.
    expect(installWindowLabel(50)).toBe("50 seconds left");
    expect(installWindowLabel(61)).toBe("2 minutes left");
    expect(installWindowLabel(1800)).toBe("30 minutes left");
  });

  it("says one thing singularly", () => {
    expect(installWindowLabel(1)).toBe("1 second left");
    expect(installWindowLabel(60)).toBe("1 minute left");
  });

  it("is closed at zero and below", () => {
    expect(installWindowLabel(0)).toBe("closed");
    expect(installWindowLabel(-5)).toBe("closed");
  });
});

describe("remembering an open window across a reload", () => {
  // The bug this exists to prevent: the expiry lived in component state, so
  // reloading Kintrinsic during an open window forgot it — which took "Close
  // now" away for exactly as long as the loosening lasted, and showed a phone
  // as shut while it was genuinely open.
  it("keeps a live window and forgets an expired one", () => {
    const windows = { sam: NOW + 600, alex: NOW - 1, jo: NOW };
    expect(pruneWindows(windows, NOW)).toEqual({ sam: NOW + 600 });
  });

  it("survives junk in storage rather than trusting it", () => {
    expect(pruneWindows({ sam: "soon" as unknown as number }, NOW)).toEqual({});
  });

  it("records a window per child, leaving siblings alone", () => {
    const one = putWindow({}, "sam", NOW + 600, NOW);
    const two = putWindow(one, "alex", NOW + 300, NOW);
    expect(two).toEqual({ sam: NOW + 600, alex: NOW + 300 });
  });

  it("forgets a child's window when it is closed early", () => {
    const open = putWindow({}, "sam", NOW + 600, NOW);
    expect(putWindow(open, "sam", null, NOW)).toEqual({});
  });

  it("prunes as it writes, so one child's stale entry can't outlive itself", () => {
    const stale = { alex: NOW - 5 };
    expect(putWindow(stale, "sam", NOW + 600, NOW)).toEqual({ sam: NOW + 600 });
  });
});

describe("the account of what came through a window", () => {
  const change = (kind: "installed" | "updated", pkg: string) => ({
    pkg,
    label: pkg,
    kind,
    at: NOW,
  });

  it("says nothing at all when the phone has never reported one", () => {
    // An absent account and an EMPTY one are different facts: "no window has
    // ever been opened here" vs "one was, and nothing came through". Showing
    // the reassuring sentence for the first would be a claim we can't make.
    expect(accountSummary(undefined)).toBe(null);
  });

  it("says plainly when nothing came through", () => {
    expect(accountSummary({ startedAt: NOW, changes: [] })).toBe(
      "Nothing was installed or updated.",
    );
  });

  it("counts new apps and updates separately", () => {
    const account = {
      startedAt: NOW,
      changes: [change("installed", "a"), change("updated", "b"), change("updated", "c")],
    };
    expect(accountSummary(account)).toBe("1 app installed, 2 updated.");
  });

  it("reads naturally for one of each", () => {
    expect(accountSummary({ startedAt: NOW, changes: [change("installed", "a")] })).toBe(
      "1 app installed.",
    );
    expect(accountSummary({ startedAt: NOW, changes: [change("updated", "b")] })).toBe(
      "1 updated.",
    );
  });
});
