// Home-network delivery, end to end through the real provider and the real
// local signer, with only the relay pool and the Android bridge faked:
//   - a rule save the carrier is holding is a visible WARNING on Limits;
//   - an answer the carrier is holding is an ANSWER (recorded, no second
//     signature possible), and its line follows the ward's ACK;
//   - the route is read by ONE poll, not once per device row;
//   - a multi-device send that reached some devices and queued the rest is
//     logged and shown with that exact split.
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act, type ReactNode } from "react";
import { generateSecretKey, getPublicKey, nip19, type NostrEvent } from "nostr-tools";
import { resetGuardianKeyCacheForTests } from "../signer/guardianKey";

// Every relay refuses, so only the home-network route can hold or carry a wrap.
vi.mock("nostr-tools", async (importOriginal) => {
  const real = await importOriginal<typeof import("nostr-tools")>();
  class FakePool {
    async querySync(): Promise<NostrEvent[]> {
      return [];
    }
    publish(): Promise<string>[] {
      return [Promise.resolve("connection failure: relay down")];
    }
    close(): void {}
  }
  return { ...real, SimplePool: FakePool };
});

import { CharterProvider, STORAGE_KEY, useCharter, type CharterContextValue, type CharterState } from "./store";
import AnsweredDelivery from "../screens/AnsweredDelivery";
import Limits from "../screens/Limits";
import { GiveTime } from "../components/GiveTime";
import { FinishNow } from "../components/FinishNow";
import { useLocalRoute } from "../carrier/useLocalRoute";
import type { Child, ChildRequest } from "../domain/types";

const guardianSk = generateSecretKey();
const LAPTOP = getPublicKey(generateSecretKey());
const PHONE = getPublicKey(generateSecretKey());
const SUBJECT = getPublicKey(generateSecretKey());
const REQ_ID = "9f".repeat(32);
const DAY_MS = 24 * 3600 * 1000;

function child(devices: Child["devices"]): Child {
  return {
    id: "c1",
    name: "Robin",
    color: "#3B6FB2",
    dependantPubkey: null,
    devices,
    policies: [
      { id: "p1", scope: { kind: "device" }, budget: { tz: "UTC", dailyMinutes: 60 } },
      { id: "app1", scope: { kind: "app", appId: "com.mojang.minecraftpe", label: "Minecraft" } },
    ],
  };
}

const laptop = (): Child["devices"][number] => ({
  id: "d1",
  label: "Robin's laptop",
  platform: "linux",
  pairing: "paired",
  devicePubkey: LAPTOP,
});
const phone = (): Child["devices"][number] => ({
  id: "d2",
  label: "Mia's phone",
  platform: "android",
  pairing: "paired",
  devicePubkey: PHONE,
});

function ask(createdAt: number): ChildRequest {
  return {
    id: "r1",
    childId: "c1",
    requester: "device",
    deviceId: "d1",
    kind: "time.extend",
    createdAt,
    title: "Robin's laptop asks for 30 more minutes",
    minutesRequested: 30,
    limitHit: "budget",
    status: "pending",
    reqId: REQ_ID,
    nonce: "cd".repeat(32),
    machine: LAPTOP,
    subject: SUBJECT,
  };
}

describe("queued home-network delivery", () => {
  let container: HTMLDivElement;
  let root: Root;
  let api: CharterContextValue;

  /** What the fake carrier does with a wrap for each ward device. */
  let sendState: (recipient: string) => "queued" | "sent" | "rejected";
  /** What the carrier reports per wrap id (the ward's receipt / ACK). */
  let wrapState: Map<string, string>;
  let sentWraps: { recipient: string; id: string }[];
  let localInfo: Mock<() => string>;
  let sessions: string[];

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    localStorage.setItem("charter.guardian.key.v1", nip19.nsecEncode(guardianSk));
    sendState = () => "queued";
    wrapState = new Map();
    sentWraps = [];
    sessions = [];
    localInfo = vi.fn(() => JSON.stringify({ v: 1, available: true, listening: true, sessions: sessions.map((m) => ({ machine: m, since: 1 })) }));
    window.CharterCarrier = {
      provision: vi.fn(),
      isCarrier: () => true,
      localInfo,
      localSend: (recipient: string, wrapJson: string) => {
        const id = (JSON.parse(wrapJson) as { id: string }).id;
        const state = sendState(recipient);
        sentWraps.push({ recipient, id });
        wrapState.set(id, state === "sent" ? "received" : state);
        return JSON.stringify({ state });
      },
      localDelivery: (idsJson: string) =>
        JSON.stringify(
          Object.fromEntries(
            (JSON.parse(idsJson) as string[])
              .filter((id) => wrapState.has(id))
              .map((id) => [id, { state: wrapState.get(id), at: 1, machine: LAPTOP }]),
          ),
        ),
      localInbox: () => "[]",
      localConsume: vi.fn(),
    };
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    delete window.CharterCarrier;
    localStorage.clear();
    resetGuardianKeyCacheForTests();
    vi.useRealTimers();
    window.location.hash = "";
  });

  function Harness({ children }: { children?: ReactNode }) {
    api = useCharter();
    return <>{children}</>;
  }

  async function mount(state: Partial<CharterState>, ui?: ReactNode) {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [child([laptop()])],
        requests: [],
        activity: [],
        signer: { connected: false, kind: "local", autoSign: true },
        ...state,
      }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(
        <CharterProvider>
          <Harness>{ui}</Harness>
        </CharterProvider>,
      );
    });
    await tick(50); // the persisted local signer reconnects
  }

  const tick = async (ms: number) => {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  };
  const text = () => container.textContent ?? "";
  const button = (label: string): HTMLButtonElement => {
    const b = [...container.querySelectorAll("button")].find(
      (el) => el.textContent?.trim() === label || el.getAttribute("aria-label") === label,
    );
    if (!b) throw new Error(`no button "${label}" in: ${text()}`);
    return b as HTMLButtonElement;
  };
  const click = async (el: HTMLElement) => {
    await act(async () => {
      el.click();
    });
  };
  const request = () => api.state.requests.find((r) => r.id === "r1")!;
  const activity = () => api.state.activity.map((a) => a.summary);

  // ---------------------------------------------------------------- 1 -----
  describe("a rule save the carrier is holding (decision 1)", () => {
    async function saveBlockedApp(): Promise<void> {
      window.location.hash = "/limits/c1";
      await mount({}, <Limits />);
      await click(button("Apps & web"));
      await click(button("Block Minecraft"));
      await click(button("Save limit"));
      await tick(50);
    }

    it("shows a warning banner naming the device, not an error and not silence", async () => {
      await saveBlockedApp();
      const warn = [...container.querySelectorAll(".banner-warn")].map((b) => b.textContent);
      expect(warn).toContain("Saved here. It'll reach Robin's laptop when it's on the home network.");
      expect(text()).not.toMatch(/Couldn’t save/);
      // ...in addition to the activity note.
      expect(activity().some((a) => a.includes("You updated the rules for Minecraft") && a.includes("Not sent yet"))).toBe(true);
    });

    it("says nothing extra when the rule reached the ward", async () => {
      sendState = () => "sent";
      await saveBlockedApp();
      expect(text()).not.toMatch(/Saved here/);
      expect(text()).not.toMatch(/Couldn’t save/);
    });
  });

  // ---------------------------------------------------------------- 2 -----
  describe("a queued answer is an answer (decision 2)", () => {
    const answerNow = async () => {
      await act(async () => {
        await api.approveRequest("r1", 15);
      });
      await tick(50);
    };

    it("records the ask as answered, with 'waiting for the home network' in the answered list and the log", async () => {
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await answerNow();

      expect(request().status).toBe("approved");
      expect(text()).toContain("Waiting for Robin's laptop to join the home network.");
      expect(container.querySelector(".banner-info")).not.toBeNull();
      expect(activity().some((a) => a.startsWith("You approved:") && a.endsWith("waiting for Robin's laptop to join the home network"))).toBe(true);
      expect(sentWraps).toHaveLength(1);
    });

    it("cannot be signed a second time, approved or denied, once answered", async () => {
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await answerNow();
      expect(sentWraps).toHaveLength(1);

      await act(async () => {
        await api.approveRequest("r1", 15);
        await api.denyRequest("r1");
      });
      await tick(50);

      expect(sentWraps).toHaveLength(1); // no second GRANT reached the carrier
      expect(request().status).toBe("approved"); // and the deny did not flip it
    });

    it("flips to 'Applied on {device}' when the ward's ACK says applied", async () => {
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await answerNow();
      expect(text()).not.toContain("Applied on");

      wrapState.set(sentWraps[0].id, "applied");
      await tick(3_100);
      expect(text()).toContain("Applied on Robin's laptop");
      expect(text()).not.toContain("Waiting for");
    });

    it.each(["unchanged"])("treats '%s' as applied too", async (ack) => {
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await answerNow();
      wrapState.set(sentWraps[0].id, ack);
      await tick(3_100);
      expect(text()).toContain("Applied on Robin's laptop");
    });

    it.each([
      ["refused", "Robin's laptop didn't accept this answer."],
      ["failed", "Robin's laptop couldn't carry this out. It may need another try."],
      ["expired", "Robin's laptop didn't act on this: the ask had already expired there."],
    ])("shows a plain warning line when the ward's ACK is '%s'", async (ack, line) => {
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await answerNow();
      wrapState.set(sentWraps[0].id, ack);
      await tick(3_100);
      expect(text()).toContain(line);
      expect(container.querySelector(".banner-warn")).not.toBeNull();
    });

    it("shows 'Expired before it reached {device}' if the 24 h window passes with no delivery", async () => {
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await answerNow();
      expect(text()).toContain("Waiting for Robin's laptop");

      // The ward never came home; the ask's own 24 h is over. Jump the clock
      // (rather than tick through a day of polls) and let the minute ticker fire.
      vi.setSystemTime(Date.now() + DAY_MS);
      await tick(60_000);
      expect(text()).toContain("Expired before it reached Robin's laptop.");
      expect(text()).not.toContain("Waiting for");
      expect(container.querySelector(".banner-warn")).not.toBeNull();
    });

    it("a plain relay failure with no local queue keeps today's behaviour: the ask stays pending", async () => {
      sendState = () => "rejected"; // the carrier did not take it either
      await mount({ children: [child([laptop()])], requests: [ask(Date.now() - 60_000)] }, <AnsweredDelivery />);
      await expect(
        act(async () => {
          await api.approveRequest("r1", 15);
        }),
      ).rejects.toThrow(/Couldn't reach any relay/);
      expect(request().status).toBe("pending");
      expect(text()).not.toContain("Waiting for");
    });
  });

  // ---------------------------------------------------------------- 3 -----
  describe("one poll for the route (decision 3)", () => {
    function Rows() {
      // Ten device rows, each reading the route.
      return (
        <>
          {Array.from({ length: 10 }, (_, i) => (
            <Row key={i} />
          ))}
        </>
      );
    }
    function Row() {
      const { sessions: live, gap } = useLocalRoute();
      return <span data-testid="row">{gap ?? "ok"}:{[...live].length}</span>;
    }

    it("calls localInfo once per poll however many rows there are", async () => {
      await mount({}, <Rows />);
      expect(container.querySelectorAll('[data-testid="row"]')).toHaveLength(10);

      const before = localInfo.mock.calls.length;
      await tick(3_000);
      expect(localInfo.mock.calls.length - before).toBe(1);
      await tick(3_000);
      expect(localInfo.mock.calls.length - before).toBe(2);
    });

    it("rows follow the shared poll: a session appearing shows up on every row", async () => {
      await mount({}, <Rows />);
      const rows = () => [...container.querySelectorAll('[data-testid="row"]')].map((r) => r.textContent);
      expect(new Set(rows())).toEqual(new Set(["ok:0"]));
      sessions = [LAPTOP];
      await tick(3_100);
      expect(new Set(rows())).toEqual(new Set(["ok:1"]));
    });
  });

  // ---------------------------------------------------------------- 4 -----
  describe("a partial batch is logged and shown (decision 4)", () => {
    const TWO = (): Partial<CharterState> => ({ children: [child([laptop(), phone()])] });
    // The phone is home (its receipt arrives at once); the laptop is away.
    const phoneHome = () => {
      sendState = (r) => (r === PHONE ? "sent" : "queued");
    };

    it("GiveTime logs and shows who got it and who is waiting", async () => {
      phoneHome();
      await mount(TWO(), <GiveTime child={child([laptop(), phone()])} />);
      await click(button("Give time"));
      await click(button("Give 30 min"));
      await tick(50);

      const expected = "Reached Mia's phone. Robin's laptop will get it when it's on the home network.";
      expect(text()).toContain(`30 minutes saved for Robin. ${expected}`);
      expect(activity().some((a) => a.startsWith("You gave 30 more minutes") && a.endsWith(` — ${expected}`))).toBe(true);
      // Both devices were handed their wrap: the queued one did not stop the other.
      expect(sentWraps.map((w) => w.recipient).sort()).toEqual([LAPTOP, PHONE].sort());
    });

    it("FinishNow logs and shows the same split", async () => {
      phoneHome();
      await mount(TWO(), <FinishNow child={child([laptop(), phone()])} />);
      await click(button("Finish now"));
      await click(button("Warn, then finish"));
      await tick(50);

      const expected = "Reached Mia's phone. Robin's laptop will get it when it's on the home network.";
      expect(text()).toContain(expected);
      expect(activity().some((a) => a.startsWith("You asked them to finish up now") && a.endsWith(` — ${expected}`))).toBe(true);
    });

    it("when every device is away it still logs the line, naming them all as waiting", async () => {
      await mount(TWO(), <GiveTime child={child([laptop(), phone()])} />);
      await click(button("Give time"));
      await click(button("Give 30 min"));
      await tick(50);

      expect(text()).toContain("Not sent yet — it'll reach Robin's laptop and Mia's phone when they're on the home network.");
      expect(activity().some((a) => a.startsWith("You gave 30 more minutes") && a.includes("saved here"))).toBe(true);
    });

    it("a rule save reports the split too: warning names the waiting device and who already got it", async () => {
      phoneHome();
      window.location.hash = "/limits/c1";
      await mount(TWO(), <Limits />);
      await click(button("Apps & web"));
      await click(button("Block Minecraft"));
      await click(button("Save limit"));
      await tick(50);

      const warn = [...container.querySelectorAll(".banner-warn")].map((b) => b.textContent);
      expect(warn).toContain("Saved here. It'll reach Robin's laptop when it's on the home network. Mia's phone already got it.");
      expect(activity().some((a) => a.includes("Reached Mia's phone. Robin's laptop will get it when it's on the home network."))).toBe(true);
    });
  });
});
