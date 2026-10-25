// What the guardian reads once a home-network answer or rule change is acked:
// "Applied on {device}" under Approvals, "— applied on {device}" on Activity,
// and a plain warning when the ward refused. Mounts the real provider so the
// 3-second delivery poll is what moves the state.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { act } from "react";
import Approvals from "./Approvals";
import Activity from "./Activity";
import { CharterProvider, STORAGE_KEY, type CharterState } from "../store/store";
import type { Child, ChildRequest } from "../domain/types";
import type { LocalWrapRecord } from "../store/localDelivery";

const MACHINE = "e".repeat(64);
const NOW = Date.now();

const CHILD: Child = {
  id: "c1",
  name: "Mia",
  color: "#3B6FB2",
  dependantPubkey: null,
  devices: [{ id: "d1", label: "Pixel", platform: "android", pairing: "paired", devicePubkey: MACHINE }],
  policies: [{ id: "pol1", scope: { kind: "device" } }],
};

function answered(id: string, reqId: string, kind: ChildRequest["kind"], status: ChildRequest["status"]): ChildRequest {
  return {
    id,
    childId: "c1",
    requester: "device",
    deviceId: "d1",
    kind,
    createdAt: NOW - 60_000,
    title: `Pixel asks (${kind})`,
    status,
    reqId,
    nonce: "1".repeat(64),
    machine: MACHINE,
  };
}

const rec = (over: Partial<LocalWrapRecord>): LocalWrapRecord => ({
  wrap: "w",
  machine: MACHINE,
  deviceId: "d1",
  childId: "c1",
  kind: "grant",
  at: NOW - 30_000,
  state: "sent",
  ...over,
});

describe("home-network delivery lines", () => {
  let container: HTMLDivElement;
  let root: Root;
  /** What the (fake) carrier reports for each wrap. */
  let carrier: Record<string, string>;

  beforeEach(() => {
    vi.useFakeTimers();
    localStorage.clear();
    carrier = {};
    window.CharterCarrier = {
      provision: vi.fn(),
      isCarrier: () => true,
      localInfo: () => JSON.stringify({ v: 1, available: true, listening: true, sessions: [] }),
      localSend: () => JSON.stringify({ state: "unavailable" }),
      localDelivery: (idsJson: string) =>
        JSON.stringify(
          Object.fromEntries(
            (JSON.parse(idsJson) as string[])
              .filter((id) => carrier[id])
              .map((id) => [id, { state: carrier[id], at: 1, machine: MACHINE }]),
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
    vi.useRealTimers();
  });

  function mount(state: Partial<CharterState>, ui: React.ReactElement) {
    localStorage.setItem(
      STORAGE_KEY,
      JSON.stringify({
        children: [CHILD],
        requests: [],
        activity: [],
        signer: { connected: false, kind: "none", autoSign: false },
        ...state,
      }),
    );
    container = document.createElement("div");
    document.body.appendChild(container);
    act(() => {
      root = createRoot(container);
      root.render(<CharterProvider>{ui}</CharterProvider>);
    });
  }

  const tick = async (ms: number) => {
    await act(async () => {
      await vi.advanceTimersByTimeAsync(ms);
    });
  };

  it("a time grant reads 'Applied on Pixel' only once the ward's ACK arrives", async () => {
    mount(
      {
        requests: [answered("r1", "a".repeat(64), "time.extend", "approved")],
        localDeliveries: [rec({ wrap: "g1", reqId: "a".repeat(64) })],
      },
      <Approvals />,
    );
    await tick(100);
    expect(container.textContent).toMatch(/Sent to Pixel/);
    expect(container.textContent).not.toMatch(/Applied on/);

    carrier.g1 = "applied";
    await tick(3_100);
    expect(container.textContent).toContain("Applied on Pixel");
  });

  it("an app-open approval is applied only when the grant and the apps clause both are", async () => {
    const reqId = "b".repeat(64);
    mount(
      {
        requests: [answered("r2", reqId, "app.open", "approved")],
        localDeliveries: [rec({ wrap: "g2", reqId }), rec({ wrap: "c2", kind: "clause", reqId })],
      },
      <Approvals />,
    );
    carrier.g2 = "applied";
    await tick(3_100);
    expect(container.textContent).not.toMatch(/Applied on/);

    carrier.c2 = "applied";
    await tick(3_100);
    expect(container.textContent).toContain("Applied on Pixel");
  });

  it("an install approval says 'Approved on', never 'installed'", async () => {
    const reqId = "c".repeat(64);
    mount(
      {
        requests: [answered("r3", reqId, "install.app", "approved")],
        localDeliveries: [rec({ wrap: "g3", reqId, state: "applied" })],
      },
      <Approvals />,
    );
    await tick(100);
    expect(container.textContent).toContain("Approved on Pixel");
    expect(container.textContent).not.toMatch(/installed/i);
  });

  it("shows a refusal as a plain warning", async () => {
    const reqId = "d".repeat(64);
    mount(
      {
        requests: [answered("r4", reqId, "time.extend", "approved")],
        localDeliveries: [rec({ wrap: "g4", reqId })],
      },
      <Approvals />,
    );
    carrier.g4 = "refused";
    await tick(3_100);
    expect(container.textContent).toContain("Pixel didn't accept this answer.");
    expect(container.querySelector(".banner-warn")).not.toBeNull();
  });

  it("appends '— applied on {device}' to a rule-change line once acked, and keeps it off otherwise", async () => {
    mount(
      {
        activity: [
          { id: "a1", childId: "c1", ts: NOW - 5_000, outcome: "rule-changed", summary: "You updated the daily time limit", wraps: ["cl1"] },
          { id: "a2", childId: "c1", ts: NOW - 6_000, outcome: "rule-changed", summary: "You updated the app rules" },
        ],
        localDeliveries: [rec({ wrap: "cl1", kind: "clause" })],
      },
      <Activity />,
    );
    await tick(100);
    expect(container.textContent).toContain("You updated the daily time limit");
    expect(container.textContent).not.toMatch(/applied on/);

    carrier.cl1 = "applied";
    await tick(3_100);
    expect(container.textContent).toContain("You updated the daily time limit — applied on Pixel");
    expect(container.textContent).not.toMatch(/app rules — applied/);
  });

  it("keeps an existing undelivered note and never invents an applied one", async () => {
    mount(
      {
        activity: [
          {
            id: "a3",
            childId: "c1",
            ts: NOW - 5_000,
            outcome: "rule-changed",
            summary: "You updated the daily schedule — saved here, but not sent yet: turn on parent approval to send it",
          },
        ],
      },
      <Activity />,
    );
    await tick(100);
    expect(container.textContent).toContain("not sent yet: turn on parent approval to send it");
    expect(container.textContent).not.toMatch(/applied on/);
  });
});
