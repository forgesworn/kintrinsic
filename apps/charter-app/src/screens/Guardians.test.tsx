import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import Guardians from "./Guardians";
import {
  LAPTOP,
  ME,
  OTHER,
  PHONE,
  STATEMENT,
  button,
  device,
  family,
  guardians,
  mockStore,
  signetRoot,
  typeInto,
} from "./guardiansTestKit";

const store = vi.hoisted(() => ({ current: null as unknown }));
vi.mock("../store/store", () => ({ useCharter: () => store.current }));

type Store = ReturnType<typeof mockStore>;

describe("Guardians screens", () => {
  let container: HTMLDivElement;
  let root: Root;

  function render(s: Store, sub = "") {
    store.current = s;
    act(() => root.render(<Guardians sub={sub} />));
  }

  beforeEach(() => {
    container = document.createElement("div");
    document.body.append(container);
    root = createRoot(container);
  });
  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    window.location.hash = "";
  });

  it("offers inviting a parent only when every device is ready, saying which needs an update", () => {
    const children = family([device("Laptop", LAPTOP, "linux", guardians())]);
    render(mockStore({ children, keys: { root: signetRoot() }, ready: false }));
    expect(button(container, "Invite a parent")!.disabled).toBe(true);
    expect(container.textContent).toContain("Update Kintrinsic on Laptop first, then invite another parent.");
    expect(container.textContent).toContain("Needs a Kintrinsic update before it can take a second guardian.");

    render(mockStore({ children, keys: { root: signetRoot() } }));
    expect(button(container, "Invite a parent")!.disabled).toBe(false);
  });

  it("blocks leaving when you're the last guardian on a device", () => {
    const alone = guardians({ roots: [{ root: ME, keys: [] }] });
    render(mockStore({ children: family([device("Laptop", LAPTOP, "linux", alone)]), keys: { root: signetRoot() } }));
    expect(button(container, "Leave the family")!.disabled).toBe(true);
    expect(container.textContent).toContain("You're the only guardian on Laptop. Unpair it instead.");
  });

  it("leaves after a confirmation when another guardian remains", async () => {
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } });
    render(s);
    await act(async () => button(container, "Leave the family")!.click());
    expect(s.guardianKeys.leaveFamily).not.toHaveBeenCalled();
    expect(container.textContent).toContain("You stop being a guardian of every device at once");
    const confirm = [...container.querySelectorAll("button")].filter((b) => b.textContent === "Leave the family").pop()!;
    await act(async () => confirm.click());
    expect(s.guardianKeys.leaveFamily).toHaveBeenCalledOnce();
  });

  it("shows an objection banner for a pending removal of you and objects", async () => {
    const d = `kintrinsic/unguardian/${ME}`;
    const g = guardians({ pending: [{ d, kind: "removal", takesEffectAt: 9_999_999_999, expiresAt: 9_999_999_999 }] });
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", g)]), keys: { root: signetRoot() } });
    render(s);
    expect(container.textContent).toContain("The other parent has asked to remove you");
    await act(async () => button(container, "Object")!.click());
    expect(s.guardianKeys.objectToRemoval).toHaveBeenCalledWith(d);
    expect(container.textContent).toContain("Objection sent");
  });

  it("shows no objection banner when the pending removal is of the other parent, and offers cancelling your own", async () => {
    const d = `kintrinsic/unguardian/${OTHER}`;
    const g = guardians({ pending: [{ d, kind: "removal", takesEffectAt: 9_999_999_999, expiresAt: 9_999_999_999 }] });
    const s = mockStore({
      children: family([device("Laptop", LAPTOP, "linux", g)]),
      keys: { root: signetRoot(), sent: { [d]: { id: STATEMENT, type: "removal", issuedAt: 9_999_999_999 - 7 * 86400, key: OTHER } } },
    });
    render(s);
    expect(container.textContent).not.toContain("asked to remove you");
    expect(container.textContent).toContain("Waiting for you to confirm in person here.");
    await act(async () => button(container, "Cancel this")!.click());
    expect(s.guardianKeys.cancelPending).toHaveBeenCalledWith(d);
  });

  it("disables removing the other parent for a key kept only on this phone", () => {
    render(mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]) }));
    const remove = [...container.querySelectorAll("button")].find((b) => b.textContent?.startsWith("Remove the other parent"))!;
    expect(remove.disabled).toBe(true);
    expect(container.textContent).toContain("a key kept only on this phone can't confirm in person");
    expect(button(container, "Change my identity")!.disabled).toBe(true);
  });

  it("asks for a guardian PIN before inviting when an Android ward has none", () => {
    const children = family([device("Robin's phone", PHONE, "android", guardians())]);
    render(mockStore({ children, keys: { root: signetRoot() } }), "invite");
    expect(container.textContent).toContain("Set a guardian PIN");
    render(mockStore({ children, keys: { root: signetRoot(), pinSentAt: { [PHONE]: 1 } } }), "invite");
    expect(container.textContent).toContain("Invite a parent");
    expect(container.textContent).not.toContain("Set a guardian PIN");
  });

  it("invites a pasted npub and then shows the other parent's checklist", async () => {
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } });
    render(s, "invite");
    const { nip19 } = await import("nostr-tools");
    const r2 = "6".repeat(64);
    act(() => typeInto(container.querySelector("input")!, `nostr:${nip19.npubEncode(r2)}`));
    await act(async () => button(container, "Next")!.click());
    await act(async () => button(container, "Send the invitation")!.click());
    expect(s.guardianKeys.inviteCoGuardian).toHaveBeenCalledWith(r2, {});
    expect(container.textContent).toContain("Join a family");
    expect(container.textContent).toContain("Sent to Laptop.");
  });

  it("refuses inviting your own guardian ID", async () => {
    const { nip19 } = await import("nostr-tools");
    render(mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } }), "invite");
    act(() => typeInto(container.querySelector("input")!, nip19.npubEncode(ME)));
    await act(async () => button(container, "Next")!.click());
    expect(container.querySelector('[role="alert"]')!.textContent).toContain("your own guardian ID");
  });

  it("shares the family roster and says when there's no one to share with", async () => {
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } });
    s.guardianKeys.sendRoster.mockResolvedValueOnce({ sent: [], held: [], failed: [] });
    render(s);
    await act(async () => button(container, "Share the family with the other guardian")!.click());
    expect(container.textContent).toContain("Nothing to share with yet");
  });

  it("lists roster-only devices as unconfirmed", () => {
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } });
    s.guardianKeys.unconfirmedRosterDevices.mockReturnValue([
      { subject: "x".repeat(64), machine: PHONE, name: "Sam's tablet", platform: "android" },
    ]);
    render(s);
    expect(container.textContent).toContain("Sam's tablet");
    expect(container.textContent).toContain("unconfirmed");
  });

  it("withdraws a lost phone only after a strong confirmation", async () => {
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } });
    render(s, "lost");
    await act(async () => button(container, "Withdraw")!.click());
    expect(container.textContent).toContain("That phone will stop receiving anything at once");
    expect(s.guardianKeys.withdrawInstall).not.toHaveBeenCalled();
    await act(async () => button(container, "Withdraw that phone for good")!.click());
    expect(s.guardianKeys.withdrawInstall).toHaveBeenCalledWith("5".repeat(64));
  });

  it("explains why a local-only family can't change identity", () => {
    render(mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]) }), "move");
    expect(container.textContent).toContain("Moving it to another identity isn't available yet.");
  });

  it("refuses joining on a phone that keeps its own key", () => {
    render(
      mockStore({ children: family([device("Laptop", LAPTOP, "linux")]), signer: { connected: true, kind: "local", autoSign: false } }),
      "join",
    );
    expect(container.textContent).toContain("can't join another parent's family");
    expect(button(container, "Connect My Signet")).toBeUndefined();
  });

  it("joins: connects My Signet, which approves this phone", async () => {
    const s = mockStore({ children: [], signer: { connected: false, kind: "none", autoSign: false } });
    render(s, "join");
    await act(async () => button(container, "Connect My Signet")!.click());
    const { nip19 } = await import("nostr-tools");
    const [link, invited] = [...container.querySelectorAll("input")];
    act(() => typeInto(link, "bunker://abc?relay=wss://one.example"));
    // The invited identity is required here, so it is checked before anything is signed.
    expect(button(container, "Connect")!.disabled).toBe(true);
    act(() => typeInto(invited, nip19.npubEncode(OTHER)));
    await act(async () => button(container, "Connect")!.click());
    expect(s.pairSignet).toHaveBeenCalledWith("bunker://abc?relay=wss://one.example", { via: "signet", expectRoot: OTHER });
  });

  it("joins on a connected My Signet with the invited identity as the expected root", async () => {
    const { nip19 } = await import("nostr-tools");
    const s = mockStore({ children: [] });
    render(s, "join");
    act(() => typeInto(container.querySelector("input")!, nip19.npubEncode(OTHER)));
    await act(async () => button(container, "Approve this phone")!.click());
    expect(s.guardianKeys.approveInstall).toHaveBeenCalledWith({ expectRoot: OTHER });
  });

  it("after joining, shows the guardian ID and the confirm-in-person checklist", () => {
    render(
      mockStore({ children: [], keys: { root: signetRoot(), anchoredRoots: { [OTHER]: { anchoredAt: 1, how: "scanned" } } } }),
      "join",
    );
    expect(container.textContent).toContain("Now confirm in person");
    expect(container.textContent).toContain("About this device → Guardians → Guardian presence");
    expect(container.textContent).toContain("Family list from: npub1");
  });

  it("after approval, asks for the inviting parent's guardian code before confirming in person", async () => {
    const s = mockStore({ children: [], keys: { root: signetRoot() } });
    render(s, "join");
    expect(container.textContent).toContain("Scan the other parent's guardian code");
    expect(container.textContent).not.toContain("Now confirm in person");
    expect(button(container, "Scan the other parent's guardian code")).toBeDefined();
    // A wrong or garbled code: the reason is shown and nothing moves on.
    s.guardianKeys.anchorRoot = vi.fn(() => {
      throw new Error("That isn't a guardian code. It starts with npub1.");
    });
    const input = container.querySelector<HTMLInputElement>('input[placeholder="npub1…"]')!;
    act(() => typeInto(input, "npub1garbage"));
    await act(async () => button(container, "Use this code")!.click());
    expect(s.guardianKeys.anchorRoot).toHaveBeenCalledWith("npub1garbage");
    expect(container.textContent).toContain("That isn't a guardian code. It starts with npub1.");
    expect(container.textContent).not.toContain("Now confirm in person");
    // A second phone of the parent's own identity skips it.
    await act(async () => button(container, "This is a second phone for my own identity")!.click());
    expect(container.textContent).toContain("Now confirm in person");
  });

  it("shows a ward confirmed in person as waiting for the anchored parent's family list", () => {
    const s = mockStore({
      children: [],
      keys: { root: signetRoot(), anchoredRoots: { [OTHER]: { anchoredAt: 1, how: "scanned" } } },
    });
    s.guardianKeys.waitingPresenceWards = vi.fn(() => [{ machine: PHONE, confirmedAt: 1, ts: 2 }]);
    render(s);
    expect(container.textContent).toContain("Waiting for npub1");
    expect(container.textContent).toContain("'s family list");
    expect(container.textContent).toContain("this phone sends it nothing: no limits, no PIN, no family list.");
    expect(button(container, "Show my guardian code")).toBeDefined();
  });

  it("a past self-removal doesn't hide someone else's removal of you (M2)", async () => {
    const d = `kintrinsic/unguardian/${ME}`;
    const g = guardians({ pending: [{ d, kind: "removal", takesEffectAt: 9_999_999_999, expiresAt: 9_999_999_999 }] });
    // This phone left once before: its own removal of itself is still in `sent`.
    const s = mockStore({
      children: family([device("Laptop", LAPTOP, "linux", g)]),
      keys: { root: signetRoot(), sent: { [d]: { id: STATEMENT, type: "removal", issuedAt: 1, key: ME } } },
    });
    render(s);
    expect(container.textContent).toContain("The other parent has asked to remove you");
    expect(button(container, "Cancel this")).toBeUndefined();
  });

  it("offers no cancel on an expired item", () => {
    const d = `kintrinsic/unguardian/${OTHER}`;
    const g = guardians({ pending: [{ d, kind: "removal", takesEffectAt: 5, expiresAt: 10 }] });
    render(
      mockStore({
        children: family([device("Laptop", LAPTOP, "linux", g)]),
        keys: { root: signetRoot(), sent: { [d]: { id: STATEMENT, type: "removal", issuedAt: 1, key: OTHER } } },
      }),
    );
    expect(container.textContent).toContain("expired");
    expect(button(container, "Cancel this")).toBeUndefined();
  });

  it("offers finishing a move that lapsed after some devices switched (M1)", async () => {
    const s = mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } });
    s.guardianKeys.state.nextRoot = {
      ...signetRoot()!,
      pubkey: OTHER,
      bunkerUri: "bunker://next",
      moveD: `kintrinsic/root/${OTHER}`,
      moveId: STATEMENT,
      cancelledAt: 5,
    };
    s.guardianKeys.moveProgress.mockReturnValue({ to: OTHER, lapsed: true, moved: ["Laptop"] });
    render(s);
    expect(button(container, "Finish my identity move")!.disabled).toBe(false);
    render(s, "move");
    expect(container.textContent).toContain("lapsed before every device switched");
    await act(async () => button(container, "Finish the move")!.click());
    expect(s.guardianKeys.finishMove).toHaveBeenCalledOnce();
  });

  it("disables confirming in person for a key kept only on this phone", () => {
    render(mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]) }));
    expect(button(container, "Confirm in person")!.disabled).toBe(true);
  });

  it("refuses a removal route for a key no device reports", () => {
    render(mockStore({ children: family([device("Laptop", LAPTOP, "linux", guardians())]), keys: { root: signetRoot() } }), `remove/${"9".repeat(64)}`);
    expect(container.textContent).toContain("That guardian isn't on any of your devices.");
  });
});
