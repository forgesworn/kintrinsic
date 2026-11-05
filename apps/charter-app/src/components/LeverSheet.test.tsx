// The lever sheet: give time, stop now, and the amount on an ask. The store is
// stubbed at `useCharter`, so each test asserts exactly which store calls a tap
// makes and with what.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import type { Child, ChildRequest } from "../domain/types";

const ctx = {
  giveTime: vi.fn(),
  standDown: vi.fn(),
  approveRequest: vi.fn(),
  denyRequest: vi.fn(),
  setRequestChosenMinutes: vi.fn(),
  deviceStatus: {} as Record<string, unknown>,
};

vi.mock("../store/store", async (orig) => ({
  ...(await orig<typeof import("../store/store")>()),
  useCharter: () => ctx,
}));

import { LeverSheet } from "./LeverSheet";

const CHILD: Child = {
  id: "c1",
  name: "Sam",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [
    { id: "d1", label: "Sam's laptop", platform: "linux", pairing: "paired", devicePubkey: "a".repeat(64) },
  ],
  policies: [{ id: "p1", scope: { kind: "device" } }],
};

const ASK: ChildRequest = {
  id: "r1",
  childId: "c1",
  requester: "device",
  deviceId: "d1",
  kind: "time.extend",
  createdAt: Date.now() - 60_000,
  title: "Sam's laptop asks for 30 more minutes",
  minutesRequested: 30,
  status: "pending",
};

describe("LeverSheet", () => {
  let container: HTMLDivElement;
  let root: Root;
  const onClose = vi.fn();

  const mount = (ui: React.ReactElement) => {
    act(() => {
      root.render(ui);
    });
  };
  const button = (label: string): HTMLButtonElement => {
    const b = [...container.querySelectorAll("button")].find((x) => x.textContent?.trim() === label);
    if (!b) throw new Error(`no button "${label}" in: ${container.textContent}`);
    return b;
  };
  const click = async (b: HTMLButtonElement) => {
    await act(async () => {
      b.click();
    });
  };

  beforeEach(() => {
    for (const fn of Object.values(ctx)) if (typeof fn === "function") (fn as ReturnType<typeof vi.fn>).mockReset();
    ctx.giveTime.mockResolvedValue(true);
    ctx.standDown.mockResolvedValue(true);
    ctx.approveRequest.mockResolvedValue(undefined);
    ctx.denyRequest.mockResolvedValue(undefined);
    onClose.mockReset();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  describe("give", () => {
    it.each([
      ["+15 min", 15],
      ["+30 min", 30],
      ["+1 hour", 60],
    ])("%s gives that amount at once, with one store call", async (label, minutes) => {
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      expect(container.textContent).toContain("Give Sam more time");
      await click(button(label));
      expect(ctx.giveTime).toHaveBeenCalledTimes(1);
      expect(ctx.giveTime).toHaveBeenCalledWith("c1", minutes, undefined);
      // Closes after success, handing back the confirmation line.
      expect(onClose).toHaveBeenCalledTimes(1);
      expect(onClose).toHaveBeenCalledWith(`${minutes} minutes sent to Sam.`);
    });

    it("the stepper moves in fives from 30, and its button gives what it shows", async () => {
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      await click(button("+")); // aria-label "5 minutes more"
      await click(button("+"));
      expect(container.textContent).toContain("40 min");
      await click(button("Give 40 min"));
      expect(ctx.giveTime).toHaveBeenCalledTimes(1);
      expect(ctx.giveTime).toHaveBeenCalledWith("c1", 40, undefined);
    });

    it("never goes below 5 or above 240", async () => {
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      for (let i = 0; i < 10; i++) await click(button("−"));
      expect(container.textContent).toContain("5 min");
      expect(button("−").disabled).toBe(true);
      for (let i = 0; i < 60; i++) await click(button("+"));
      expect(container.textContent).toContain("240 min");
      expect(button("+").disabled).toBe(true);
    });

    it("says to turn on parent approval, and stays open, when nothing was signed", async () => {
      ctx.giveTime.mockResolvedValue(false);
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      await click(button("+15 min"));
      expect(container.querySelector('[role="alert"]')?.textContent).toBe(
        "Turn on parent approval first, then try again.",
      );
      expect(onClose).not.toHaveBeenCalled();
    });

    it("shows a send error and stays open", async () => {
      ctx.giveTime.mockRejectedValue(new Error("Couldn't reach any relay"));
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      await click(button("+30 min"));
      expect(container.querySelector('[role="alert"]')?.textContent).toBe("Couldn't reach any relay");
      expect(onClose).not.toHaveBeenCalled();
    });

    it("offers 'Add it to' only when the ward has counted groups, and sends the group", async () => {
      const withGroups: Child = {
        ...CHILD,
        policies: [
          {
            id: "p1",
            scope: { kind: "device" },
            buckets: {
              enabled: true,
              tz: "UTC",
              buckets: [{ id: "play", label: "Play", apps: [], dailyMinutes: 60 }],
            } as never,
          },
        ],
      };
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      expect(container.querySelector("select")).toBeNull();
      mount(<LeverSheet mode="give" child={withGroups} onClose={onClose} />);
      const select = container.querySelector("select")!;
      expect(select).not.toBeNull();
      await act(async () => {
        select.value = "play";
        select.dispatchEvent(new Event("change", { bubbles: true }));
      });
      await click(button("+15 min"));
      expect(ctx.giveTime).toHaveBeenCalledWith("c1", 15, "play");
    });

    it("closes on Escape and on a backdrop tap, but not on a tap inside the sheet", async () => {
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      await click(container.querySelector<HTMLElement>(".sheet")! as HTMLButtonElement);
      expect(onClose).not.toHaveBeenCalled();
      await click(container.querySelector<HTMLElement>(".sheet-backdrop")! as HTMLButtonElement);
      expect(onClose).toHaveBeenCalledTimes(1);
      await act(async () => {
        document.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
      });
      expect(onClose).toHaveBeenCalledTimes(2);
      expect(ctx.giveTime).not.toHaveBeenCalled();
    });
  });

  describe("approve", () => {
    it("opens at what was asked, and 'Approve N min' calls approveRequest with N", async () => {
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      expect(container.textContent).toContain("30 min");
      await click(button("Approve 30 min"));
      expect(ctx.approveRequest).toHaveBeenCalledTimes(1);
      expect(ctx.approveRequest).toHaveBeenCalledWith("r1", 30);
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("steps the persisted chosen amount in fives, below AND above what was asked", async () => {
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      // The ask is an opening bid, not a ceiling: the device enacts any grant
      // up to 1440, so "+" is live at the asked amount.
      expect(button("+").disabled).toBe(false);
      await click(button("−"));
      expect(ctx.setRequestChosenMinutes).toHaveBeenCalledWith("r1", 25);
      await click(button("+"));
      expect(ctx.setRequestChosenMinutes).toHaveBeenCalledWith("r1", 35);
    });

    it("stops at the gift ceiling, or at the ask itself when that is larger", async () => {
      mount(
        <LeverSheet mode="approve" child={CHILD} request={{ ...ASK, chosenMinutes: 240 }} onClose={onClose} />,
      );
      expect(button("+").disabled).toBe(true);
      mount(
        <LeverSheet
          mode="approve"
          child={CHILD}
          request={{ ...ASK, minutesRequested: 300, chosenMinutes: 300 }}
          onClose={onClose}
        />,
      );
      expect(button("Approve 300 min")).toBeDefined();
      expect(button("+").disabled).toBe(true);
    });

    it("a preset sets the amount outright, above the ask included, and says what was asked", async () => {
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      await click(button("1 hour"));
      expect(ctx.setRequestChosenMinutes).toHaveBeenCalledWith("r1", 60);
      await click(button("10 min"));
      expect(ctx.setRequestChosenMinutes).toHaveBeenCalledWith("r1", 10);
      // The note names the ask whenever the amount differs from it, either way.
      mount(
        <LeverSheet mode="approve" child={CHILD} request={{ ...ASK, chosenMinutes: 60 }} onClose={onClose} />,
      );
      expect(container.textContent).toContain("They asked for 30 min.");
      expect(button("Approve 60 min")).toBeDefined();
    });

    it("reads a previously chosen amount, so a retry doesn't snap back to the ask", async () => {
      mount(
        <LeverSheet mode="approve" child={CHILD} request={{ ...ASK, chosenMinutes: 10 }} onClose={onClose} />,
      );
      expect(button("Approve 10 min")).toBeDefined();
      expect(container.textContent).toContain("They asked for 30 min.");
      await click(button("Approve 10 min"));
      expect(ctx.approveRequest).toHaveBeenCalledWith("r1", 10);
    });

    it("'Not now' calls denyRequest", async () => {
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      await click(button("Not now"));
      expect(ctx.denyRequest).toHaveBeenCalledTimes(1);
      expect(ctx.denyRequest).toHaveBeenCalledWith("r1");
      expect(ctx.approveRequest).not.toHaveBeenCalled();
    });

    it("shows a failed send and leaves the sheet open", async () => {
      ctx.approveRequest.mockRejectedValue(new Error("This ask expired on the device"));
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      await click(button("Approve 30 min"));
      expect(container.querySelector('[role="alert"]')?.textContent).toBe("This ask expired on the device");
      expect(onClose).not.toHaveBeenCalled();
    });
  });

  describe("stop", () => {
    it("explains, then 'Warn, then finish' calls standDown without lifting", async () => {
      mount(<LeverSheet mode="stop" child={CHILD} onClose={onClose} />);
      expect(container.textContent).toContain("gets a minute’s warning");
      await click(button("Warn, then finish"));
      expect(ctx.standDown).toHaveBeenCalledTimes(1);
      expect(ctx.standDown).toHaveBeenCalledWith("c1", false);
      expect(onClose).toHaveBeenCalledWith(expect.stringContaining("Sam has a minute"));
    });

    it("when already stood down, 'Allow back on' lifts it", async () => {
      ctx.deviceStatus = {
        ["a".repeat(64)]: { ts: Math.floor(Date.now() / 1000), lockReason: "standdown" },
      };
      mount(<LeverSheet mode="stop" child={CHILD} onClose={onClose} />);
      expect(container.textContent).toContain("Finished for today");
      await click(button("Allow back on"));
      expect(ctx.standDown).toHaveBeenCalledWith("c1", true);
      ctx.deviceStatus = {};
    });

    it("says the real reach of Stop now before it is sent", () => {
      mount(<LeverSheet mode="stop" child={CHILD} onClose={onClose} />);
      const text = container.textContent ?? "";
      expect(text).toContain("every one of their devices");
      expect(text).toContain("Always-on apps stop too, though audio already playing may carry on");
      expect(text).toContain("until midnight, or until you allow them back on");
      expect(text).toContain("the emergency unlock unless you have turned it off");
      // The button is not renamed.
      expect(button("Warn, then finish")).toBeTruthy();
    });

    it("Cancel closes without sending", async () => {
      mount(<LeverSheet mode="stop" child={CHILD} onClose={onClose} />);
      await click(button("Cancel"));
      expect(ctx.standDown).not.toHaveBeenCalled();
      expect(onClose).toHaveBeenCalledTimes(1);
    });
  });
  describe("while the ward is stopped for today", () => {
    const NOTICE = "Sam is stopped for today, so this won't take effect until you allow them back on.";
    const stopped = () => {
      ctx.deviceStatus = {
        ["a".repeat(64)]: { ts: Math.floor(Date.now() / 1000), lockReason: "standdown" },
      };
    };
    afterEach(() => {
      ctx.deviceStatus = {};
    });

    it("the give sheet says so first, and still sends if the guardian insists", async () => {
      stopped();
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      expect(container.textContent).toContain(NOTICE);
      await click(button("+15 min"));
      expect(ctx.giveTime).toHaveBeenCalledTimes(1);
    });

    it("the give sheet offers the existing 'Allow back on' action", async () => {
      stopped();
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      await click(button("Allow back on"));
      expect(ctx.giveTime).not.toHaveBeenCalled();
      expect(container.textContent).toContain("Finished for today");
      await click(button("Allow back on"));
      expect(ctx.standDown).toHaveBeenCalledWith("c1", true);
    });

    it("the approve sheet says so before approving, and does not block it", async () => {
      stopped();
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      expect(container.textContent).toContain(NOTICE);
      await click(button("Approve 30 min"));
      expect(ctx.approveRequest).toHaveBeenCalledWith("r1", 30);
    });

    it("the different-kind-of-time sheet says so too", () => {
      stopped();
      mount(<LeverSheet mode="retype" child={CHILD} request={ASK} onClose={onClose} />);
      expect(container.textContent).toContain(NOTICE);
    });

    it("a lift that lands while the stop sheet is open settles it, never offering 'Warn, then finish'", async () => {
      stopped();
      mount(<LeverSheet mode="stop" child={CHILD} onClose={onClose} />);
      expect(button("Allow back on")).toBeDefined();
      ctx.deviceStatus = {};
      mount(<LeverSheet mode="stop" child={CHILD} onClose={onClose} />);
      expect(container.textContent).toContain("Sam is back on.");
      expect([...container.querySelectorAll("button")].map((b) => b.textContent?.trim())).toEqual([
        "Close",
      ]);
      await click(button("Close"));
      expect(ctx.standDown).not.toHaveBeenCalled();
      expect(onClose).toHaveBeenCalledTimes(1);
    });

    it("says nothing when the ward is not stopped", () => {
      mount(<LeverSheet mode="give" child={CHILD} onClose={onClose} />);
      expect(container.textContent).not.toContain("stopped for today");
      mount(<LeverSheet mode="approve" child={CHILD} request={ASK} onClose={onClose} />);
      expect(container.textContent).not.toContain("stopped for today");
    });
  });
});
